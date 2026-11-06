#!/usr/bin/env python3
"""Aggregate run records into results.json / results.md.

    python3 report.py <results-dir>          # regenerate results.json and results.md from runs.jsonl

Rates are over runs with status "ok". Runs that ended in an infrastructure error (server would not
start, model endpoint failed, fixture server unreachable) have status "error": they are counted and
listed but excluded from every rate, so a flaky endpoint is not scored as an agent failure.
A run where the agent hit the step limit or never called finish is status "ok" and class no_finish.
"""
import json
import math
import os
import statistics
import sys
from collections import OrderedDict


def wilson(k, n, z=1.96):
    """95% Wilson score interval for k successes out of n. Returns (lo, hi) in [0,1]."""
    if n == 0:
        return (0.0, 0.0)
    p = k / n
    d = 1 + z * z / n
    c = p + z * z / (2 * n)
    m = z * math.sqrt(p * (1 - p) / n + z * z / (4 * n * n))
    return (max(0.0, (c - m) / d), min(1.0, (c + m) / d))


def _mean(xs):
    return sum(xs) / len(xs) if xs else None


def summarise_group(runs):
    ok = [r for r in runs if r["status"] == "ok"]
    n = len(ok)
    cls = lambda c: sum(1 for r in ok if r["class"] == c)
    passed = sum(1 for r in ok if r["check_passed"])
    fs = cls("false_success")
    claimed = cls("true_success") + fs
    est = sum(1 for r in ok if r["tokens_estimated"])
    return OrderedDict([
        ("runs", len(runs)), ("errors", len(runs) - n), ("n", n),
        ("success", passed), ("success_rate", passed / n if n else None), ("success_ci95", wilson(passed, n) if n else None),
        ("false_success", fs), ("false_success_rate", fs / n if n else None), ("false_success_ci95", wilson(fs, n) if n else None),
        ("claimed_success", claimed), ("false_success_of_claims", fs / claimed if claimed else None),
        ("true_success", cls("true_success")), ("true_failure", cls("true_failure")),
        ("false_failure", cls("false_failure")), ("unsure", cls("unsure")), ("no_finish", cls("no_finish")),
        ("mean_tool_calls", _mean([r["tool_calls"] for r in ok])),
        ("mean_tokens_in", _mean([r["tokens_in"] for r in ok])),
        ("mean_tokens_out", _mean([r["tokens_out"] for r in ok])),
        ("runs_with_estimated_tokens", est),
        ("median_wall_s", statistics.median([r["wall_s"] for r in ok]) if ok else None),
        ("mean_tool_schema_tokens_est", _mean([r["tool_schema_tokens_est"] for r in ok if r.get("tool_schema_tokens_est")])),
    ])


def summarise(runs):
    """Group by server x model x pack, plus an 'all' row per server x model."""
    groups = OrderedDict()
    for r in runs:
        groups.setdefault((r["server"], r["model"], r["pack"]), []).append(r)
        groups.setdefault((r["server"], r["model"], "(all)"), []).append(r)
    rows = []
    for (server, model, pack), rs in groups.items():
        row = OrderedDict([("server", server), ("model", model), ("pack", pack)])
        row.update(summarise_group(rs))
        rows.append(row)
    rows.sort(key=lambda x: (x["server"], x["model"], x["pack"] != "(all)", x["pack"]))
    return rows


def _pct(x):
    return "-" if x is None else f"{100 * x:.0f}%"


def _pct_ci(x, ci):
    return "-" if x is None else f"{100 * x:.0f}% ({100 * ci[0]:.0f}-{100 * ci[1]:.0f})"


def _num(x, nd=1):
    return "-" if x is None else f"{x:.{nd}f}"


def render_md(results):
    cfg = results["config"]
    out = ["# Evaluation results", ""]
    out.append(f"Run `{cfg.get('run_id')}` on {cfg.get('started')} | harness prompt `{cfg.get('prompt_version')}` | "
               f"max steps {cfg.get('max_steps')} | tool-result truncation {cfg.get('max_result_chars')} chars | "
               f"reps {cfg.get('reps')} | {len(results['runs'])} runs")
    out.append("")
    scripted = [m for m in cfg.get("models", {}) if str(m).startswith("scripted")]
    if scripted:
        out += ["> **Scripted backends were used** (" + ", ".join(scripted) + "). Scripted results test the harness; "
                "they say nothing about any browser server or model.", ""]
    out += ["**false success** = the agent claimed success but the outcome check failed (rate over all counted runs; "
            "95% Wilson interval in brackets). **success** = the outcome check passed, whatever the agent claimed. "
            "Tokens are as reported by the API; `est` marks runs where the API did not report usage and "
            "characters/4 was used.", ""]
    out.append("| server | model | pack | n | success | false success | claimed ok -> wrong | true fail | unsure | no finish | errors | tool calls | tokens in | tokens out | wall s (median) |")
    out.append("|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|")
    for r in results["summary"]:
        est = " est" if r["runs_with_estimated_tokens"] else ""
        out.append("| {server} | {model} | {pack} | {n} | {sr} | {fs} | {fc} | {tf} | {un} | {nf} | {er} | {tc} | {ti}{est} | {to}{est} | {w} |".format(
            server=r["server"], model=r["model"], pack=r["pack"], n=r["n"],
            sr=_pct_ci(r["success_rate"], r["success_ci95"]) if r["n"] else "-",
            fs=_pct_ci(r["false_success_rate"], r["false_success_ci95"]) if r["n"] else "-",
            fc=f"{r['false_success']}/{r['claimed_success']}", tf=r["true_failure"], un=r["unsure"],
            nf=r["no_finish"], er=r["errors"], tc=_num(r["mean_tool_calls"]),
            ti=_num(r["mean_tokens_in"], 0), to=_num(r["mean_tokens_out"], 0), est=est, w=_num(r["median_wall_s"])))
    errs = [r for r in results["runs"] if r["status"] == "error"]
    if errs:
        out += ["", "## Infrastructure errors (excluded from rates)", ""]
        for r in errs[:50]:
            out.append(f"- {r['server']} / {r['model']} / {r['task']} rep {r['rep']}: {r['end_reason']}: {str(r.get('error'))[:200]}")
    out += ["", "## What this does not show", "",
            "See the 'What this does NOT prove' section of scripts/eval/README.md. In short: small fixed task sets on "
            "local fixtures, one run configuration, models and server versions as recorded in results.json, "
            "text-only tool results, and an outcome check written by the task authors.", ""]
    return "\n".join(out)


def load_runs(d):
    runs = []
    with open(os.path.join(d, "runs.jsonl")) as f:
        for line in f:
            if line.strip():
                runs.append(json.loads(line))
    return runs


def write_results(d, config, runs):
    results = {"config": config, "summary": summarise(runs), "runs": runs}
    with open(os.path.join(d, "results.json"), "w") as f:
        json.dump(results, f, indent=1)
    with open(os.path.join(d, "results.md"), "w") as f:
        f.write(render_md(results))
    return results


def main(argv=None):
    argv = argv if argv is not None else sys.argv[1:]
    if len(argv) != 1:
        print(__doc__)
        return 2
    d = argv[0]
    with open(os.path.join(d, "config.json")) as f:
        config = json.load(f)
    write_results(d, config, load_runs(d))
    print(os.path.join(d, "results.md"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
