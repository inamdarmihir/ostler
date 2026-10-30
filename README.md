# Waymarker

A headless browser for AI coding agents, written in Rust. It reaches agents over MCP, so Claude Code, Codex, Cursor and anything else that speaks MCP can use it. It aims to fill complex forms reliably and to remember what worked, using [Qdrant](https://qdrant.tech) for memory.

> **Status: pre-alpha, design phase.** There is no browser functionality yet. The CLI prints its version and nothing else. The design, the competitive research and the roadmap are in [`docs/`](docs/). Nothing here is benchmarked, and no performance claims are made until the benchmark harness exists.

## Why

Browsers are the part of the stack where coding agents fail most often, and forms are where they fail most visibly: multi-step wizards, dependent dropdowns, date pickers, validation errors, uploads, iframes and shadow DOM. Agents also start every session from zero, re-learning the same site quirks and re-deriving the same user data.

The existing tools are good at driving a browser (see [the landscape](docs/research/landscape.md)). We think two things are still missing:

1. **A form model, not just form primitives.** Turn a page into a typed intermediate representation (fields, constraints, dependencies, steps), fill it in one call, and verify the result.
2. **Memory that learns.** Successful fill sequences, user profile facts and past failures are stored in Qdrant and retrieved by similarity, so the second run is cheaper than the first.

## Plan

See [`docs/PLAN.md`](docs/PLAN.md) for the full plan, phases and risks. Engineering decisions are recorded as ADRs in [`docs/adr/`](docs/adr/).

| Phase | Scope |
|---|---|
| 0 | Research hardening, ADRs, speed spike |
| 1 | CDP client, Chrome backend, snapshots, MCP server |
| 2 | Form IR, fill planner, verifier, wizards |
| 3 | Qdrant memory (procedural, semantic, episodic) |
| 4 | Lightpanda fast path, pipelining, public benchmarks |
| 5 | Site, docs, launch |

## Development

```sh
cargo run      # prints the version
cargo clippy -- -D warnings
```

## License

Apache-2.0. See [LICENSE](LICENSE).
