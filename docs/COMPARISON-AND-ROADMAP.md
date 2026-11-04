# Comparison and roadmap

How Ostler differs from other browser MCP servers, and the roadmap checklist. Back to the [README](../README.md). See also the longer [ROADMAP](ROADMAP.md).

### Why Ostler?

| | Ostler | typical snapshot-per-action MCP browser |
|---|---|---|
| Fill a 20-field form | **3 tool calls** (inspect, fill, submit) | ~1 per field plus a page snapshot each time |
| Wrong or missing field | **reported** (`UNMATCHED`, `ambiguous`, read-back mismatch) | usually silent |
| Dependent dropdowns, wizards | handled inside `form_fill` / `form_submit` | agent has to notice and retry |
| Submit result | `Submitted` / `Advanced` / `NOT ACCEPTED` + the page's messages | agent infers from a new snapshot |
| Memory across sessions | **Qdrant**: facts, playbooks, episodes | none, or a browser profile |
| Tool schema cost per turn | ~1.5k tokens (`--tools forms` is smaller) | varies |

The "3 vs ~20" row is measured on our own fixtures with a scripted agent, and the comparison column is **our engine driven that way, not another product**. Tools that already offer a batched fill (Playwright MCP, chrome-devtools-mcp) are closer to Ostler than this table suggests. See [the benchmark summary](BENCHMARK-SUMMARY.md) and [BENCHMARKS.md](BENCHMARKS.md) and the [design review](REVIEW-2026-10.md).

## How it compares

From each project's own README (see [research](research/landscape.md); star counts move; we have **not** run a head-to-head):

| | language | form handling | memory |
|---|---|---|---|
| **Ostler** | Rust | typed form model, verified fill, wizards, dependent fields, shadow DOM, same-origin iframes | Qdrant: profile, playbooks, episodes |
| agent-browser (Vercel) | Rust CLI | `fill`/`select`/`check`, batching, `--delta` snapshots | cookie/storage state, auth vault |
| Playwright MCP (Microsoft) | Node | accessibility snapshots, `browser_fill_form` batch fill | browser profile |
| chrome-devtools-mcp (Google) | Node | `fill_form` batch fill plus debugging/perf tooling | none |
| Lightpanda | Zig | n/a (engine without rendering; AGPL) | n/a |

Stagehand and Browser Use cache or learn actions too; they are Python/TypeScript frameworks rather than MCP tools for coding agents.

## Roadmap

- [x] CDP client, in-page runtime, form engine, Qdrant memory, MCP server, benchmark harness, `init`/`config`/`setup`
- [ ] **Fix the review findings**, starting with wrong-fill prevention and browser-level enforcement of the safety flags ([order](REVIEW-2026-10.md#recommended-order-of-work))
- [ ] **Real-model evaluation** against real tools, on a wider set of real sites than the [7-page check](BENCHMARKS.md#7-real-site-evaluation-fill-only-no-llm), published including failures
- [ ] Prebuilt binaries and a one-line installer
- [ ] Live validation of Qdrant Cloud Inference; Linux benchmark numbers
- [ ] Cross-origin iframes, HTTP transport for MCP (tabs and popups-as-tabs exist)
