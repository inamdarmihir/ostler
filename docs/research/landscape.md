# Landscape (snapshot 2026-10-30)

Facts below come from each project's README as fetched on the date above. Star counts move; re-check before quoting. Items marked **unverified** come from secondary sources and need primary-source confirmation in Phase 0.

| Project | What it is | Verified facts | Gap for our goals |
|---|---|---|---|
| [agent-browser](https://github.com/vercel-labs/agent-browser) | Native Rust CLI + daemon driving Chrome over CDP | ~44.9k stars, Apache-2.0; accessibility snapshots with `@eN` refs, `--delta`, MCP with tool profiles, encrypted auth vault, cookie/localStorage session restore | Closest competitor. No semantic/episodic memory, no form model beyond `fill`/`select`/`check` primitives |
| [Playwright MCP](https://github.com/microsoft/playwright-mcp) | Microsoft's MCP server on Playwright | ~38.9k stars; accessibility snapshots, `browser_fill_form`, persistent profile; README says CLI+skills use fewer tokens; "not a security boundary" | Node; full snapshots are token-heavy; no memory beyond the profile |
| [chrome-devtools-mcp](https://github.com/ChromeDevTools/chrome-devtools-mcp) | Google's DevTools MCP | ~54.5k stars, Apache-2.0; Puppeteer-based; debugging and performance tracing | Debugging-oriented, Chrome only, no memory |
| [Lightpanda](https://github.com/lightpanda-io/browser) | Zig browser built from scratch, no rendering | ~38.1k stars, AGPL-3.0; CDP + MCP; claims 9x faster / 16x less memory than headless Chrome on its own benchmark (933 pages, m5.large) | An engine, not an agent layer. We treat it as an optional backend. AGPL: keep it an external process |
| Stagehand, Browser Use, Skyvern | LLM-in-the-loop automation frameworks | **unverified**: Stagehand caches repeated actions | Python/TS frameworks rather than MCP-first tools; memory is ad hoc |
| Obscura, OxiBrowser | Early Rust browsers with partial CDP | **unverified** | Low maturity |
| [mcp-server-qdrant](https://github.com/qdrant/mcp-server-qdrant) | Generic `store`/`find` over Qdrant | Official; stdio/SSE | No browser integration; unstructured memory |

## Evidence that forms are unsolved (secondary, verify against the papers)

- Online-Mind2Web: frontier agents roughly 30–61% versus ~90% claimed on WebVoyager; most failures in filters/sorting and navigation.
- ClawBench: omitted required form fields and mis-clicks on similarly labelled elements; best score 33.3%.
- WAREX: agent success drops under injected network faults, and agents tend to quit early. The two sources found disagree on the numbers.

## Open questions for Phase 0

- Maintenance state of `chromiumoxide` / `spider_chrome` (only 2025 releases found).
- Maturity of the `rmcp` MCP SDK.
- Qdrant Cloud Inference models, limits and Rust client support.
- How much of the web-form surface Lightpanda handles over CDP.
