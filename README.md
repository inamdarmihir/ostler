# Waymarker

**A headless browser for AI coding agents, written in Rust, with Qdrant-backed memory and a form engine that fills complex forms in one call.**

Plug it into Claude Code, Codex, Cursor, or anything that speaks [MCP](https://modelcontextprotocol.io). Site and write-up: <https://inamdarmihir.github.io/waymarker/>

> **Status: v0.2, early.** It works end to end and is tested against real Chrome and a real Qdrant server, but it has only been evaluated on local fixtures, not the open web. Read [what that means](docs/ECONOMICS.md#6-what-can-go-wrong) before relying on it.

## Why it exists

Agents fail on forms (dependent dropdowns, wizards, masks, validation, shadow DOM) and burn tokens doing it one field per tool call. They also forget everything between sessions. Waymarker:

1. **Fills a whole form in one call.** `form_inspect` returns typed fields; `form_fill` takes `{label: value}`, resolves, normalises, fills in order, re-inspects after each field (so dependent dropdowns work), and **reads every value back**. `form_submit` tells you whether it was accepted, moved to the next step, or failed validation, with the page's own messages.
2. **Remembers, using Qdrant.** Profile facts, form *playbooks* (structure that worked, never values) and *episodes* (what went wrong) live in Qdrant, tenant-scoped. The next visit knows which fact fills which field.
3. **Is cheap to run and honest about it.** Everything below is measured by a harness in this repo.

## Measured results

20-field form, same engine, two interaction patterns ([full methodology and caveats](docs/BENCHMARKS.md)):

| | Waymarker form engine | one call per field |
|---|---:|---:|
| agent tool calls | **3** | 22 |
| input tokens read across the task | **12.5k** | 166k |
| LLM cost (illustrative price, cached) | **$0.021** | $0.105 |
| end-to-end time (3 s LLM turns) | **13 s** | 70 s |

Small forms gain far less (~1.3-2x). The "per-field" column is our own engine driven the way snapshot-per-action tools are driven, **not a measurement of another product**.

| | |
|---|---|
| browser time per task | 0.4-1.1 s (end-to-end time is dominated by LLM turns) |
| 8 parallel sessions, 8 cores | 354-393 tasks/min, p95 < 1 s, 100% success |
| memory per session (physical footprint) | 412 MiB `--lean` / 658 MiB default |
| compute cost | ~$0.006 per 1,000 tasks (the LLM bill is ~3000x that) |
| Qdrant retrieval, 200k points / 10k tenants | 0.4 ms p50, flat from 20k points |

Measured on an Apple M2, localhost, fixtures. See the [economics write-up](docs/ECONOMICS.md) for what these numbers do and do not imply.

## Quickstart

```sh
cargo install --git https://github.com/inamdarmihir/waymarker waymarker-cli   # needs Rust 1.80+ and Chrome
waymarker doctor
claude mcp add waymarker -- waymarker serve
```

Codex, Cursor and others: [docs/CLIENTS.md](docs/CLIENTS.md). Try it without any site: `waymarker fixtures` then ask your agent to fill `http://127.0.0.1:8787/dependent.html`.

With memory:

```sh
export QDRANT_URL=https://<cluster>.cloud.qdrant.io:6333 QDRANT_API_KEY=…
claude mcp add waymarker -- waymarker serve --learn --lean
```

## Tools

`navigate` `snapshot` `click` `type` `press` `wait_for` `screenshot` `back` · `form_inspect` `form_fill` `form_submit` `fill_field` · `memory_remember` `memory_recall` `memory_list` `memory_forget` `stats`. Profiles (`--tools core|forms|all`) keep the schema small; the full set is ~1.1k tokens of context.

## How it compares

Honest summary from each project's README (see [research](docs/research/landscape.md); star counts move):

| | language | form handling | memory | notes |
|---|---|---|---|---|
| **Waymarker** | Rust | typed form model, verified fill, wizards, dependent fields, shadow DOM, same-origin iframes | Qdrant: profile, playbooks, episodes | v0.2; fixtures-only evaluation; Chrome only |
| agent-browser (Vercel) | Rust CLI | `fill`/`select`/`check` primitives, batching | cookie/storage state, auth vault | larger, more mature; closest competitor |
| Playwright MCP (Microsoft) | Node | accessibility snapshots, `browser_fill_form` batch fill | browser profile | mature, broad |
| chrome-devtools-mcp (Google) | Node | automation + debugging/perf tooling | none | built for debugging |
| Lightpanda | Zig | n/a (engine, no rendering) | n/a | fast engine; AGPL; a possible future backend |

We have **not** yet run a head-to-head on identical real-site tasks. When we do, we will publish it whichever way it goes.

## Safety

Domain allowlist, `--require-confirm-submit`, secrets refused by memory and masked in output, playbooks never store values, page text marked untrusted. Details and limits in [SECURITY.md](SECURITY.md). Not independently audited.

## Roadmap

- [x] CDP client, in-page runtime, form engine, Qdrant memory, MCP server, benchmark harness
- [ ] **Real-site form evaluation with published results (including failures)**: the biggest open question
- [ ] Head-to-head benchmark against other tools on identical tasks
- [ ] Live validation of Qdrant Cloud Inference; Linux CI benchmark numbers
- [ ] Qdrant sizing study (quantization, on-disk) for memory per user
- [ ] Tabs/popups, cross-origin iframes, calendar widgets, HTTP transport
- [ ] Pluggable backend (Lightpanda) once there is evidence it earns its maintenance

## Repository map

`crates/` cdp · browser · forms · memory · mcp · cli · fixtures · bench. `fixtures/` the test pages. `docs/` [architecture](docs/ARCHITECTURE.md), [ADRs](docs/adr/), [benchmarks](docs/BENCHMARKS.md), [economics](docs/ECONOMICS.md), [research](docs/research/landscape.md), [original plan](docs/PLAN.md). `bench-results/` raw numbers.

## License

Apache-2.0. See [LICENSE](LICENSE).
