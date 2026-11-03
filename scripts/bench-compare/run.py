#!/usr/bin/env python3
"""Ostler vs Playwright MCP vs Chrome DevTools MCP on the same local fixture tasks. No LLM in the loop.

Usage: run.py --fixtures http://127.0.0.1:8765 --ostler PATH --npm-dir DIR --out out.json [--reps 3]
The fixture server (`ostler-bench serve-fixtures`) must already be running; ground truth is its
record of what the page actually submitted (GET /__submissions), not what any tool reports.
"""
import argparse, json, os, platform, shutil, statistics, subprocess, sys, tempfile, time, urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from mcpclient import MCP
from tasks import TASKS
import servers

CHROME = {"darwin": "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"}.get(
    platform.system().lower(), shutil.which("google-chrome") or shutil.which("chromium") or "")


def http(base, path, method="GET"):
    req = urllib.request.Request(base + path, method=method, data=b"" if method == "POST" else None)
    return json.loads(urllib.request.urlopen(req, timeout=5).read())


def pkg_version(npm_dir, pkg):
    return json.load(open(os.path.join(npm_dir, "node_modules", pkg, "package.json")))["version"]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--fixtures", default="http://127.0.0.1:8765")
    ap.add_argument("--ostler", required=True)
    ap.add_argument("--npm-dir", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--reps", type=int, default=3)
    ap.add_argument("--only", default="", help="comma list of server ids")
    ap.add_argument("--chrome", default=CHROME)
    a = ap.parse_args()
    a.ostler = os.path.abspath(a.ostler)
    a.npm_dir = os.path.abspath(a.npm_dir)
    bin_ = os.path.join(a.npm_dir, "node_modules", ".bin")
    cfg = tempfile.NamedTemporaryFile(suffix=".toml", delete=False).name
    env = dict(os.environ, OSTLER_CONFIG=cfg)

    chrome_ver = subprocess.run([a.chrome, "--version"], capture_output=True, text=True).stdout.strip()
    node_ver = subprocess.run(["node", "-v"], capture_output=True, text=True).stdout.strip()
    ostler_ver = subprocess.run([a.ostler, "--version"], capture_output=True, text=True, env=env).stdout.strip()

    defs = {
        "ostler": (lambda: [a.ostler, "serve", "--memory", "ephemeral"], servers.ostler,
                   "navigate(interactive) -> form_fill -> form_submit (per wizard step)"),
        "ostler-inspect": (lambda: [a.ostler, "serve", "--memory", "ephemeral"], servers.ostler_inspect,
                           "navigate(none) -> form_inspect -> form_fill -> form_submit"),
        "playwright": (lambda: [os.path.join(bin_, "playwright-mcp"), "--headless", "--isolated",
                                "--browser", "chrome"], servers.playwright,
                       "browser_navigate -> browser_snapshot -> browser_fill_form -> browser_click"),
        "devtools": (lambda: [os.path.join(bin_, "chrome-devtools-mcp"), "--headless", "--isolated",
                              "--executablePath", a.chrome], servers.devtools,
                     "navigate_page -> take_snapshot -> fill_form -> click"),
    }
    only = [s for s in a.only.split(",") if s] or list(defs)
    results = {"meta": {
        "date": time.strftime("%Y-%m-%d"), "machine": f"{platform.machine()} {platform.system()} {platform.release()}",
        "chrome": chrome_ver, "node": node_ver, "ostler": ostler_ver,
        "playwright_mcp": pkg_version(a.npm_dir, "@playwright/mcp"),
        "chrome_devtools_mcp": pkg_version(a.npm_dir, "chrome-devtools-mcp"),
        "reps": a.reps, "token_estimate": "bytes/4 of tool output text (same as ostler-bench)",
    }, "servers": {}, "runs": []}

    for sid in only:
        argv_f, driver, seq = defs[sid]
        cwd = tempfile.mkdtemp(prefix="bench-cwd-")  # Playwright writes .playwright-mcp/ in cwd
        t0 = time.time()
        try:
            m = MCP(argv_f(), env=env, cwd=cwd).start()
        except Exception as e:
            results["servers"][sid] = {"error": f"failed to start: {e}"}
            continue
        # Warm-up: launch the browser outside the measured tasks, record it as cold start.
        try:
            if sid.startswith("ostler"):
                w = m.call("navigate", url="about:blank", observe="none")
            elif sid == "playwright":
                w = m.call("browser_navigate", url="about:blank")
            else:
                w = m.call("navigate_page", pageId=1, type="url", url="about:blank")
        except Exception as e:
            results["servers"][sid] = {"error": f"warm-up failed: {e}"}
            m.close(); continue
        results["servers"][sid] = {
            "info": m.info, "sequence": seq, "tool_count": len(m.tools), "schema_bytes": m.schema_bytes,
            "cold_start_ms": round((time.time() - t0) * 1000), "warmup_call_ms": round(w.ms)}
        for rep in range(a.reps):
            for task in TASKS:
                http(a.fixtures, "/__reset", "POST")
                n0 = len(m.calls)
                err = None
                try:
                    driver(m, a.fixtures + task["page"], task)
                except servers.Fail as e:
                    err = str(e)
                except Exception as e:  # driver/protocol failure is a result too
                    err = f"{type(e).__name__}: {e}"
                time.sleep(0.4)  # let async fetches land before reading ground truth
                subs = http(a.fixtures, "/__submissions")
                calls = m.calls[n0:]
                exact = len(subs) == 1 and subs[0] == task["expect"]
                results["runs"].append({
                    "server": sid, "task": task["id"], "rep": rep, "steps": len(calls),
                    "out_bytes": sum(c.out_bytes for c in calls), "args_bytes": sum(c.args_bytes for c in calls),
                    "image_bytes": sum(c.image_bytes for c in calls),
                    "wall_ms": round(sum(c.ms for c in calls), 1), "ok": exact,
                    "submissions": len(subs), "got": None if exact else (subs[-1] if subs else None),
                    "error": err, "calls": [c.tool for c in calls],
                })
        m.close()
        shutil.rmtree(cwd, ignore_errors=True)
    os.unlink(cfg)

    # Aggregate: median over reps; correct only if every rep was exactly right.
    agg = []
    for sid in only:
        for t in TASKS:
            rs = [r for r in results["runs"] if r["server"] == sid and r["task"] == t["id"]]
            if not rs:
                continue
            med = lambda k: statistics.median(r[k] for r in rs)
            agg.append({"server": sid, "task": t["id"], "steps": med("steps"),
                        "out_bytes": med("out_bytes"), "out_tokens": int(-(-med("out_bytes") // 4)),
                        "args_tokens": int(-(-med("args_bytes") // 4)),
                        "wall_ms": round(med("wall_ms")), "correct": f"{sum(r['ok'] for r in rs)}/{len(rs)}",
                        "error": next((r["error"] for r in rs if r["error"]), None),
                        "got": next((r["got"] for r in rs if not r["ok"]), None)})
    results["summary"] = agg
    json.dump(results, open(a.out, "w"), indent=2)
    print(f"wrote {a.out}")


if __name__ == "__main__":
    main()
