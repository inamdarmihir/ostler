# Soak, security and stress harness

A long-running check for a release build, complementary to the CI tests. It drives the real `ostler serve`
binary over MCP stdio, the way an agent would, and fails loudly.

What it does, in order:

1. **Canary control.** Runs the hostile pages with *no* allowlist and requires a canary server on a
   non-allowlisted origin to be reached. This proves the canary can detect a leak.
2. **Security pass** with `--allow-domain`: URL-policy attacks (external host, other host form, suffix and
   userinfo tricks, IPv6, `file:`, `javascript:`, `data:`, `view-source:`, `chrome:`), a prompt-injection page
   (balanced wrapper tags, no attacker text outside the wrapper, hidden text not shown), 12 exfiltration
   channels to the canary, and stress pages (endless DOM mutation, an 8 MB page, 5,000 fields, a 200,000 px
   screenshot, an alert/confirm storm, a hanging server, popup spam).
3. **Timed soak** over about 20 form workflows on the demo fixtures, random order, fresh values each cycle,
   with a `kill -9` of Chrome in cycle 4 to prove recovery. Per-tool latency and process-tree memory are
   sampled throughout.

It exits early with `ABORT` if the first functional cycle fails wholesale (usually: the fixtures are not
running), so a broken environment does not burn 20 minutes.

## Run it

```sh
cargo build --release -p ostler
./target/release/ostler fixtures &                    # demo forms on http://127.0.0.1:8787
SOAK_MIN=20 bun scripts/soak/soak.js
```

Environment: `SOAK_MIN` (total minutes, default 20), `SEC_ONLY=1` (security and stress pass only),
`PAGEHOST` / `LEAKHOST` (default `127.0.0.1` / `localhost`; swap them to test the allowlist against an IP
literal), `NO_CONTROL=1` (skip the control run), `TAG`, `SOAK_OUT` (report directory, default `/tmp`).

The report is printed and written to `$SOAK_OUT/ostler-soak-report-<TAG>.json`. Read "failures" together
with the slow-call lines (`SLOW/ERR call ...`) in the log: a latency outlier is a finding even when no check
failed.

## What it is not

It runs against local fixtures and local hostile pages, so it says nothing about the open web. The
assertions are deliberately about what Ostler guarantees (nothing reaches the canary, page text stays inside
the wrapper, calls return within a bound), not about page-specific outcomes.
