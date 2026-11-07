# Roadmap: where Ostler stands and what is left

> The plan of record from v0.3 to launch is [BUILD-PLAN.md](BUILD-PLAN.md). This page tracks the current state.

Status as of 2026-11-02. "Have" is covered by tests in this repo; "Gap" is what developers get from other
browser MCPs (Playwright MCP, chrome-devtools-mcp, Browser Use) that Ostler does not yet do.

## Have

| Area | What works |
|---|---|
| Navigation and page reading | navigate, snapshot (full/delta/interactive, subtree by ref, `max_chars`), back, tabs with shared cookies, popups become tabs, honest HTTP/PDF/sign-in-redirect notes |
| Actions | click (with overlay detection), type, press, hover, scroll, wait_for, screenshot, `look` (numbered click targets) |
| Forms | typed form model, one-call fill with per-field verification, selects, radios, checkbox groups, dates, files, comboboxes, dependent dropdowns, wizards, submit outcome (accepted / next step / validation failed) |
| Memory | facts remembered and recalled, sealed Qdrant storage, learning from validation failures, survives site redesigns |
| Safety | untrusted page text fencing, domain allow-list, private-network block, submit confirmation gate, CAPTCHA hand-off, secrets refused |
| Token economy | `auto` tool profile (15 core tools, research tools revealed after the first `read`, forms on form pages), `read` with a `query` returns only the best passages, delta observations, terse output |
| Research | `read`, `cite` (quote verified against pages read this session), `evidence` (citations and gaps), `search` (SearXNG, Brave), `open_many` (robots.txt, per-host spacing, scratch tab), `extract` (tables, lists with a completeness verdict), `links` |
| Operations | `doctor --fix`, `install --client`, A2A mode, bench suite and comparison, own or attached Chrome |

## Gaps, ranked by how often developers hit them

1. **Debugging signal**: console messages and failed network requests. This is the main reason developers keep chrome-devtools-mcp installed. Proposed tool: `diagnostics` (console errors/warnings and 4xx/5xx or failed requests since the last call, capped).
2. **Session persistence for logins**: `--profile-dir` keeps cookies, but there is no export/import of a portable session and no `auth` helper. Proposed: `ostler session save|load`.
3. **Cross-origin iframes and real-world widgets** (Stripe, Google Places, reCAPTCHA-adjacent flows): fixtures cover same-origin only.
4. **Structured extraction**: pull a table or list as rows instead of reading a snapshot. Proposed: `extract` (tables, lists, links, JSON-LD).
5. **Downloads and uploads from the agent side**: file upload works inside forms; downloads are not surfaced.
6. **Streamable HTTP transport** for remote agents (stdio only today, A2A covers one remote case).
7. **Record and replay** of a successful flow as a playbook the agent can re-run in one call.
8. **Windows** build and CI. Done (experimental): built and tested in CI; needs real-world use before it is called supported.
9. **Published binaries and packages**: nothing is on crates.io or npm until the first tagged release.

## Next sprint (in order)

1. `diagnostics` tool, with an e2e test on a page that logs an error and fails a fetch.
2. `extract` tool for tables and lists.
3. `session save|load`.
4. Real-world widget suite (cross-origin iframe first).
5. Refresh `docs/BENCHMARKS.md` and `bench-results/compare.md` for the `auto` profile; first CI run; tag a release.

Done so far this sprint: `hover`, `scroll`.

## Product documentation

Constraints, risks and operating policy live in: [SYSTEM-DESIGN.md](SYSTEM-DESIGN.md) (context, data flow, constraints, capacity, trade-offs), [THREAT-MODEL.md](THREAT-MODEL.md), [FAILURE-MODES.md](FAILURE-MODES.md), [SLOS.md](SLOS.md) (proposed), [COMPATIBILITY.md](COMPATIBILITY.md), [OPERATIONS.md](OPERATIONS.md) and [ADR-0011](adr/0011-reliability-principles.md).
