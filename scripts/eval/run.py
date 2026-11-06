#!/usr/bin/env python3
"""Run a model-in-the-loop evaluation: servers x models x tasks x reps, scored against the fixture server.

    python3 run.py --servers-file examples/servers.example.json --server ostler \\
        --model scripted-oracle --pack gov-forms --out results/selftest

See README.md for the full command list. Standard library only.
"""
import argparse
import datetime
import json
import os
import platform
import re
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import agent as agent_mod  # noqa: E402
import models as models_mod  # noqa: E402
import report  # noqa: E402
import scoring  # noqa: E402
import tasks as tasks_mod  # noqa: E402
from fixture import DEFAULT_BASE, Fixture, FixtureError  # noqa: E402
from mcp_stdio import McpError, StdioMcp, load_servers  # noqa: E402

DEFAULT_PACKS = os.path.normpath(os.path.join(HERE, "..", "..", "crates", "ostler-fixtures", "packs"))
INFRA_REASONS = ("model_error", "server_error", "server_start_error", "fixture_error", "harness_error")


class NullMcp:
    """Built-in server id 'none': no tools. For testing the harness with scripted models."""
    info = {"name": "none", "version": "0"}
    notifications = []

    def list_tools(self):
        return []

    def call_tool(self, name, args, timeout=None):
        return f"[harness] no such tool: {name}", True, 0

    def close(self):
        pass


def slug(s):
    return re.sub(r"[^A-Za-z0-9._-]+", "-", s).strip("-")[:80]


def git_commit():
    try:
        r = subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=HERE, capture_output=True, text=True, timeout=5)
        return r.stdout.strip() or None
    except Exception:
        return None


def start_server(sid, spec, outdir, mcp_timeout):
    if sid == "none":
        return NullMcp()
    log = os.path.join(outdir, "server-logs", slug(sid) + ".stderr.log")
    os.makedirs(os.path.dirname(log), exist_ok=True)
    m = StdioMcp(spec["command"], env=spec.get("env"), cwd=spec.get("cwd"), stderr_path=log, timeout=mcp_timeout)
    try:
        m.start()
    except Exception:
        m.close()
        raise
    return m


def run_one(task, sid, server_spec, model_label, backend, rep, a, fixture, outdir, shared):
    key = tasks_mod.task_key(task)
    base = a.fixture
    rec = {"server": sid, "model": model_label, "pack": task["pack"], "task": task["id"], "rep": rep,
           "difficulty": task.get("difficulty", ""), "risk": task.get("risk", ""), "tags": task.get("tags", []),
           "status": "ok", "end_reason": "", "error": None}
    events, mcp = [], None
    t0 = time.time()
    try:
        fixture.reset()
        mcp = shared.get(sid) if a.reuse_server else None
        if mcp is None:
            mcp = start_server(sid, server_spec, outdir, a.mcp_timeout)
            if a.reuse_server:
                shared[sid] = mcp
        rec["server_info"] = getattr(mcp, "info", {})
        backend.begin_task(task, {"base": base, "fixture": fixture, "rep": rep})
        res = agent_mod.run_agent(tasks_mod.render(task["instruction"], base), tasks_mod.start_url(task, base),
                                  mcp, backend, a.max_steps, a.max_result_chars)
        events = res["events"]
        records = fixture.records()
    except FixtureError as e:
        return _error(rec, "fixture_error", str(e), events, t0, outdir, a, key)
    except agent_mod.HarnessError as e:
        return _error(rec, "harness_error", str(e), events, t0, outdir, a, key)
    except (McpError, OSError) as e:
        return _error(rec, "server_start_error", str(e), events, t0, outdir, a, key)
    finally:
        if mcp is not None and not a.reuse_server:
            mcp.close()

    chk = scoring.check_outcome(task["outcome"], records, res["final"]["final_answer"], res["final"]["cited_urls"], base)
    cls = scoring.classify(chk["passed"], res["final"]["claim"], res["finished"])
    rec.update({
        "status": "error" if res["end_reason"] in INFRA_REASONS else "ok",
        "end_reason": res["end_reason"], "error": res["error"], "claim": res["final"]["claim"],
        "check_passed": chk["passed"], "check_details": chk["details"], "class": cls,
        "steps": res["steps"], "tool_calls": res["tool_calls"],
        "tokens_in": res["tokens_in"], "tokens_out": res["tokens_out"], "tokens_estimated": res["tokens_estimated"],
        "tool_schema_tokens_est": res["tool_schema_tokens_est"], "wall_s": round(time.time() - t0, 3),
        "records_seen": len(records), "final_answer": res["final"]["final_answer"][:4000],
        "cited_urls": res["final"]["cited_urls"],
    })
    _write_trajectory(rec, events, records, outdir, key)
    return rec


def _error(rec, reason, msg, events, t0, outdir, a, key):
    rec.update({"status": "error", "end_reason": reason, "error": msg[:500], "check_passed": False,
                "check_details": [], "class": "no_finish", "claim": None, "steps": 0, "tool_calls": 0,
                "tokens_in": 0, "tokens_out": 0, "tokens_estimated": True, "tool_schema_tokens_est": None,
                "wall_s": round(time.time() - t0, 3), "records_seen": 0, "final_answer": "", "cited_urls": []})
    _write_trajectory(rec, events, [], outdir, key)
    return rec


def _write_trajectory(rec, events, records, outdir, key):
    d = os.path.join(outdir, "trajectories")
    os.makedirs(d, exist_ok=True)
    name = f"{slug(rec['server'])}__{slug(rec['model'])}__{slug(key)}__r{rec['rep']}.jsonl"
    rec["trajectory"] = os.path.join("trajectories", name)
    with open(os.path.join(outdir, rec["trajectory"]), "w") as f:
        f.write(json.dumps({"type": "meta", **{k: rec[k] for k in ("server", "model", "pack", "task", "rep")}}) + "\n")
        for e in events:
            f.write(json.dumps(e, default=str) + "\n")
        f.write(json.dumps({"type": "result", "status": rec["status"], "end_reason": rec["end_reason"],
                            "error": rec["error"], "claim": rec.get("claim"), "check_passed": rec.get("check_passed"),
                            "check_details": rec.get("check_details"), "class": rec.get("class"),
                            "records": [_short_record(r) for r in records[:50]]}, default=str) + "\n")


def _short_record(r):
    r = dict(r)
    b = r.get("body")
    if isinstance(b, str) and len(b) > 1000:
        r["body"] = b[:1000] + "...[truncated]"
    return r


def parse_args(argv=None):
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--servers-file", default=os.path.join(HERE, "examples", "servers.example.json"))
    p.add_argument("--server", action="append", default=[], help="server id (repeat or comma-separate); 'none' = no tools")
    p.add_argument("--model", action="append", default=[], help="model spec or name from --models-file (repeat or comma-separate)")
    p.add_argument("--models-file")
    p.add_argument("--base-url", help="OpenAI-compatible base URL, e.g. http://127.0.0.1:11434/v1")
    p.add_argument("--api-key-env", help="name of the environment variable holding the API key (never the key itself)")
    p.add_argument("--anthropic-version")
    p.add_argument("--temperature", type=float)
    p.add_argument("--max-tokens", type=int)
    p.add_argument("--packs-dir", default=DEFAULT_PACKS)
    p.add_argument("--pack", action="append", default=[])
    p.add_argument("--task", action="append", default=[], help="task id")
    p.add_argument("--tag", action="append", default=[])
    p.add_argument("--fixture", default=DEFAULT_BASE, help="fixture server base URL (ostler fixtures)")
    p.add_argument("--reps", type=int, default=1)
    p.add_argument("--max-steps", type=int, default=30)
    p.add_argument("--max-result-chars", type=int, default=8000)
    p.add_argument("--mcp-timeout", type=float, default=120.0)
    p.add_argument("--reuse-server", action="store_true", help="keep one server process per server id for all runs (faster, less isolated)")
    p.add_argument("--seed", type=int, default=0)
    p.add_argument("--out")
    p.add_argument("--list-tasks", action="store_true")
    return p.parse_args(argv)


def _split(vals):
    return [x for v in vals for x in v.split(",") if x]


def main(argv=None):
    a = parse_args(argv)
    try:
        tasks = tasks_mod.load_packs(a.packs_dir, _split(a.pack) or None, _split(a.task) or None, _split(a.tag) or None)
    except (tasks_mod.TaskError, OSError, ValueError) as e:
        print(f"error loading tasks: {e}", file=sys.stderr)
        return 2
    if a.list_tasks:
        for t in tasks:
            print(f"{tasks_mod.task_key(t):40s} {t['difficulty']:8s} {t['title']}")
        print(f"{len(tasks)} tasks")
        return 0
    if not tasks:
        print(f"no tasks found under {a.packs_dir} (filters: packs={a.pack} tasks={a.task} tags={a.tag})", file=sys.stderr)
        return 2
    server_ids = _split(a.server)
    model_specs = _split(a.model)
    if not server_ids or not model_specs:
        print("need at least one --server and one --model", file=sys.stderr)
        return 2
    servers = {} if server_ids == ["none"] else load_servers(a.servers_file)
    servers["none"] = {}
    for s in server_ids:
        if s not in servers:
            print(f"unknown server {s!r}; defined: {sorted(servers)}", file=sys.stderr)
            return 2
    backends = {}
    for m in model_specs:
        try:
            backends[m] = models_mod.build(m, a.models_file, a.base_url, a.api_key_env, a.temperature, a.max_tokens,
                                           a.anthropic_version, a.seed)
        except ValueError as e:
            print(f"error: {e}", file=sys.stderr)
            return 2

    started = datetime.datetime.now(datetime.timezone.utc)
    run_id = started.strftime("%Y%m%dT%H%M%SZ")
    outdir = a.out or os.path.join(HERE, "results", run_id)
    os.makedirs(outdir, exist_ok=True)
    fixture = Fixture(a.fixture)
    try:
        fixture.records()
    except FixtureError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2

    config = {
        "run_id": run_id, "started": started.isoformat(), "harness_commit": git_commit(),
        "prompt_version": agent_mod.PROMPT_VERSION, "system_prompt": agent_mod.SYSTEM_PROMPT,
        "max_steps": a.max_steps, "max_result_chars": a.max_result_chars, "reps": a.reps, "seed": a.seed,
        "fixture": a.fixture, "python": platform.python_version(), "platform": platform.platform(),
        "servers": {s: {"command": servers[s].get("command"), "env_keys": sorted((servers[s].get("env") or {}).keys())} for s in server_ids},
        "models": {label: pub for (_b, label, pub) in backends.values()},
        "tasks": [tasks_mod.task_key(t) for t in tasks], "reuse_server": a.reuse_server,
        "temperature": a.temperature, "tool_results": "text only; images replaced by a placeholder",
    }
    with open(os.path.join(outdir, "config.json"), "w") as f:
        json.dump(config, f, indent=1)
    with open(os.path.join(outdir, "tasks.json"), "w") as f:
        json.dump(tasks, f, indent=1)

    runs, shared = [], {}
    runs_path = os.path.join(outdir, "runs.jsonl")
    total = len(server_ids) * len(model_specs) * len(tasks) * a.reps
    done = 0
    try:
        with open(runs_path, "w") as rf:
            for sid in server_ids:
                for m in model_specs:
                    backend, label, _pub = backends[m]
                    for t in tasks:
                        for rep in range(a.reps):
                            rec = run_one(t, sid, servers[sid], label, backend, rep, a, fixture, outdir, shared)
                            runs.append(rec)
                            rf.write(json.dumps(rec, default=str) + "\n")
                            rf.flush()
                            done += 1
                            print(f"[{done}/{total}] {sid} / {label} / {tasks_mod.task_key(t)} r{rep}: "
                                  f"{rec['class']} (check {'pass' if rec.get('check_passed') else 'fail'}, "
                                  f"claim {rec.get('claim')}, {rec['status']})", flush=True)
                    for s in list(shared.values()):
                        pass
    finally:
        for s in shared.values():
            s.close()
    config["finished"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
    config["server_info"] = {}
    for r in runs:
        if r.get("server_info"):
            config["server_info"].setdefault(r["server"], r["server_info"])
    with open(os.path.join(outdir, "config.json"), "w") as f:
        json.dump(config, f, indent=1)
    report.write_results(outdir, config, runs)
    print(f"\nresults: {os.path.join(outdir, 'results.md')}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
