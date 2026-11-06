#!/usr/bin/env python3
"""Convert a results directory into a Hugging Face dataset layout. Writes files only; uploads nothing.

    python3 export_hf.py <results-dir> <out-dir> [--license apache-2.0] [--name ostler-false-success-bench]

Output:
    <out>/README.md                  dataset card (YAML header + method, fields, caveats)
    <out>/data/runs.jsonl            one row per run: scores, counts, claim, final answer
    <out>/data/trajectories.jsonl    one row per run: the full event list as a JSON string (`events_json`)
    <out>/data/tasks.jsonl           the task definitions used (instruction, outcome checks)
    <out>/data/*.parquet             only if pyarrow is already installed (never installed by this script)
    <out>/config.json, results.json, results.md   copied verbatim for provenance

Publishing is a manual step you do yourself after reading the card (see README.md in scripts/eval).
No network code lives in this file.
"""
import argparse
import json
import os
import shutil
import sys

RUN_FIELDS = ["server", "model", "pack", "task", "rep", "difficulty", "risk", "status", "end_reason", "claim",
              "class", "check_passed", "steps", "tool_calls", "tokens_in", "tokens_out", "tokens_estimated",
              "tool_schema_tokens_est", "wall_s", "records_seen", "final_answer", "cited_urls", "check_details"]


def read_jsonl(path):
    with open(path) as f:
        return [json.loads(line) for line in f if line.strip()]


def write_jsonl(path, rows):
    with open(path, "w") as f:
        for r in rows:
            f.write(json.dumps(r, ensure_ascii=False) + "\n")


def build_rows(results_dir):
    runs = read_jsonl(os.path.join(results_dir, "runs.jsonl"))
    run_rows, traj_rows = [], []
    for i, r in enumerate(runs):
        row = {"run_id": f"{i:05d}"}
        for k in RUN_FIELDS:
            v = r.get(k)
            if k in ("cited_urls", "check_details"):
                v = json.dumps(v if v is not None else [])
            row[k] = v
        run_rows.append(row)
        events = []
        tp = os.path.join(results_dir, r.get("trajectory", ""))
        if r.get("trajectory") and os.path.exists(tp):
            events = read_jsonl(tp)
        traj_rows.append({"run_id": row["run_id"], "server": r["server"], "model": r["model"], "pack": r["pack"],
                          "task": r["task"], "rep": r["rep"], "events_json": json.dumps(events, ensure_ascii=False)})
    return run_rows, traj_rows


def maybe_parquet(rows, path):
    try:
        import pyarrow as pa
        import pyarrow.parquet as pq
    except ImportError:
        return False
    pq.write_table(pa.Table.from_pylist(rows), path)
    return True


def card(config, summary, n_runs, license_id, name, parquet):
    models = ", ".join(sorted(config.get("models", {}))) or "-"
    servers = ", ".join(sorted(config.get("servers", {}))) or "-"
    scripted = any(str(m).startswith("scripted") for m in config.get("models", {}))
    lines = [
        "---",
        f"license: {license_id}",
        "language: [en]",
        "pretty_name: " + name,
        "task_categories: [other]",
        "tags: [browser-agents, mcp, evaluation, false-success, agents]",
        "size_categories: [n<1K]" if n_runs < 1000 else "size_categories: [1K<n<10K]",
        "configs:",
        "- config_name: runs",
        "  data_files: data/runs.jsonl",
        "- config_name: trajectories",
        "  data_files: data/trajectories.jsonl",
        "- config_name: tasks",
        "  data_files: data/tasks.jsonl",
        "---", "",
        f"# {name}", "",
        "Runs of language-model agents driving browser MCP servers on local fixture tasks, scored against what the "
        "fixture server actually received. The headline measure is **false success**: the agent claimed the task "
        "succeeded, but the outcome check failed.", "",
    ]
    if scripted:
        lines += ["> **This export contains scripted (non-model) runs.** They test the harness and must not be read as "
                  "results for any model or server.", ""]
    lines += [
        "## Method", "",
        f"- Run `{config.get('run_id')}`, started {config.get('started')}, harness commit `{config.get('harness_commit')}`, "
        f"system prompt version `{config.get('prompt_version')}`.",
        f"- Servers: {servers}. Models: {models}. Repetitions per task: {config.get('reps')}.",
        f"- One agent loop for every server and model: the same system prompt, a step limit of {config.get('max_steps')}, "
        f"tool results truncated to {config.get('max_result_chars')} characters, text-only tool results "
        "(images replaced by a placeholder), and a built-in `finish(final_answer, cited_urls, claim)` tool that ends the run.",
        "- After each run the harness reads the fixture server's records and the agent's final answer and evaluates the "
        "task's outcome check. A tool's own claim of success is never used.",
        "- Run classes: `true_success`, `false_success` (claimed success, check failed), `true_failure`, `false_failure`, "
        "`unsure`, `no_finish`. `status: error` marks infrastructure failures, excluded from rates.",
        "- Tokens are as reported by the model API when available; `tokens_estimated: true` means characters/4 was used.", "",
        "## Files and fields", "",
        "`data/runs.jsonl` (also parquet: " + ("yes" if parquet else "no, pyarrow was not installed") + "): " +
        ", ".join(f"`{k}`" for k in ["run_id"] + RUN_FIELDS) + ". `cited_urls` and `check_details` are JSON-encoded strings.", "",
        "`data/trajectories.jsonl`: `run_id`, `server`, `model`, `pack`, `task`, `rep`, `events_json` (JSON list of "
        "tools offered, model turns, tool calls with truncated results, the finish call, and the final check).", "",
        "`data/tasks.jsonl`: the task definitions (instruction, start, outcome checks, risk, tags).", "",
        "## Caveats", "",
        "- Small, fixed task sets on local fixture pages; not a sample of the open web.",
        "- Outcome checks were written by the task authors and may be too strict or too lax for a given task.",
        "- Results depend on the server and model versions recorded in `config.json`; npm-distributed servers resolved "
        "to whatever version was current when the run started.",
        "- Trajectories contain model output and truncated page text from fixtures; they contain no API keys "
        "(keys are read from the environment and never written).",
        "- Final answers are free text produced by models and may be wrong or contain unsafe content.", "",
        "## Licence", "",
        f"Declared licence: `{license_id}`. Confirm that it covers every task and fixture in `data/tasks.jsonl` and the "
        "model outputs under the terms of the model providers before publishing.", "",
        "## Summary table", "", "See `results.md`.", "",
    ]
    return "\n".join(lines)


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("results_dir")
    ap.add_argument("out_dir")
    ap.add_argument("--license", default="apache-2.0")
    ap.add_argument("--name", default="ostler-false-success-bench")
    a = ap.parse_args(argv)
    if not os.path.exists(os.path.join(a.results_dir, "runs.jsonl")):
        print(f"{a.results_dir} has no runs.jsonl; is it a results directory?", file=sys.stderr)
        return 2
    with open(os.path.join(a.results_dir, "config.json")) as f:
        config = json.load(f)
    run_rows, traj_rows = build_rows(a.results_dir)
    data = os.path.join(a.out_dir, "data")
    os.makedirs(data, exist_ok=True)
    write_jsonl(os.path.join(data, "runs.jsonl"), run_rows)
    write_jsonl(os.path.join(data, "trajectories.jsonl"), traj_rows)
    tasks_path = os.path.join(a.results_dir, "tasks.json")
    tasks = []
    if os.path.exists(tasks_path):
        with open(tasks_path) as f:
            tasks = json.load(f)
    write_jsonl(os.path.join(data, "tasks.jsonl"), [{**t, "outcome": t["outcome"]} for t in tasks])
    parquet = maybe_parquet(run_rows, os.path.join(data, "runs.parquet"))
    if parquet:
        maybe_parquet(traj_rows, os.path.join(data, "trajectories.parquet"))
    for fn in ("config.json", "results.json", "results.md"):
        src = os.path.join(a.results_dir, fn)
        if os.path.exists(src):
            shutil.copy(src, os.path.join(a.out_dir, fn))
    with open(os.path.join(a.out_dir, "README.md"), "w") as f:
        f.write(card(config, None, len(run_rows), a.license, a.name, parquet))
    print(f"wrote {len(run_rows)} runs to {a.out_dir} (parquet: {'yes' if parquet else 'no'}). Nothing was uploaded.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
