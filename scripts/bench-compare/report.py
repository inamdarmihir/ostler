#!/usr/bin/env python3
"""Render the comparison JSON as markdown tables."""
import json, sys

d = json.load(open(sys.argv[1]))
m = d["meta"]
PKG = {"playwright": m["playwright_mcp"], "devtools": m["chrome_devtools_mcp"], "ostler": m["ostler"], "ostler-inspect": m["ostler"]}
LABEL = {"ostler": "Ostler", "ostler-inspect": "Ostler (+form_inspect)",
         "playwright": "Playwright MCP", "devtools": "Chrome DevTools MCP"}
tasks, order = [], []
for r in d["summary"]:
    if r["task"] not in tasks: tasks.append(r["task"])
    if r["server"] not in order: order.append(r["server"])
for s_ in d["servers"]:
    if s_ not in order: order.append(s_)
S = {(r["server"], r["task"]): r for r in d["summary"]}
out = []
out.append("| task | server | steps | out tokens (est) | args tokens (est) | wall ms | correct (server-side) |")
out.append("|---|---|---:|---:|---:|---:|---:|")
for t in tasks:
    for s in order:
        r = S.get((s, t))
        if r: out.append(f"| {t} | {LABEL[s]} | {r['steps']:g} | {r['out_tokens']:,} | {r['args_tokens']:,} | {r['wall_ms']:,} | {r['correct']} |")
out.append("")
out.append("| server | version | tools | tool schema (bytes) | schema tokens (est) | cold start ms |")
out.append("|---|---|---:|---:|---:|---:|")
for s in order:
    i = d["servers"][s]
    if "error" in i: out.append(f"| {LABEL[s]} | could not run: {i['error']} | | | | |"); continue
    out.append(f"| {LABEL[s]} | {i['info'].get('version')} (npm {PKG.get(s, '')}) | {i['tool_count']} | {i['schema_bytes']:,} | {-(-i['schema_bytes']//4):,} | {i['cold_start_ms']:,} |")
out.append("")
out.append("| server | steps (sum of 5 tasks) | out tokens (sum) | wall ms (sum) | tasks correct |")
out.append("|---|---:|---:|---:|---:|")
for s in order:
    rs = [r for r in d["summary"] if r["server"] == s]
    ok = sum(1 for r in rs if r["correct"].split("/")[0] == r["correct"].split("/")[1])
    out.append(f"| {LABEL[s]} | {sum(r['steps'] for r in rs):g} | {sum(r['out_tokens'] for r in rs):,} | {sum(r['wall_ms'] for r in rs):,} | {ok}/{len(rs)} |")
out.append("")
out.append("Sequences:")
for s in order:
    if "sequence" in d["servers"][s]: out.append(f"- {LABEL[s]}: {d['servers'][s]['sequence']}")
print("\n".join(out))
