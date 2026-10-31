# Ostler: a Rust headless browser for coding agents, with Qdrant-native memory

Name: **Ostler** (an ostler tends travellers' horses, which is what this does for agents; crates.io, npm and GitHub availability verified 2026-10-30; bare `quill` was taken). License: Apache-2.0.
Directory `/Users/sonic3022/Downloads/Browser` is empty, so this is a greenfield build.

## 1. Context

Coding agents (Claude Code, Codex, Cursor) drive browsers through MCP. Most of the time they fail on **forms**: multi-step wizards, dependent dropdowns, date pickers, validation errors, file uploads, iframes and shadow DOM. They also **forget everything between runs**: they re-discover every site's quirks and re-derive the same user data each session.

Goal: a research-grade, production-quality open-source project that is the reference showcase for Qdrant as agent memory, with a GitHub Pages portfolio site that explains the engineering choices.

Decisions already made with the user:
- **Engine**: whichever is fast and reliable, so a pluggable CDP backend. Chrome is the reliable default. Lightpanda is an opt-in fast path with automatic fallback to Chrome. No custom engine.
- **Embeddings**: Qdrant Cloud Inference only in v1. Memory needs a Qdrant Cloud cluster and key. Risk: this adds setup friction for new users and weakens the "zero config" pitch. Mitigation: an `Embedder` trait, so a local backend can be added in v1.1 without touching callers. Offline, memory-free mode still works for all non-memory tools.
- **Name/license**: Ostler, Apache-2.0. The repo is public from day one, by the user's choice.

## 2. Competitive research (what exists, what it lacks)

Star counts come from the repo pages fetched during planning. Treat them as snapshots. Several comparison blog posts I found are vendor-written SEO pieces, so I rely on READMEs only.

| Project | Facts (from README) | Gap relative to our goals |
|---|---|---|
| **agent-browser** (Vercel Labs) | Native Rust CLI plus daemon, CDP to Chrome, ~44.9k stars, Apache-2.0. Accessibility snapshots with `@eN` refs, `--delta`, MCP with tool profiles, encrypted auth vault, session save/restore. | **Closest competitor.** Rust alone is not a differentiator. Persistence is cookies and localStorage state files. It has no semantic or episodic memory, no form model (just `fill`/`select`/`check` primitives), and no learning across runs. |
| **Playwright MCP** (Microsoft) | ~38.9k stars. Accessibility-tree snapshots, `browser_fill_form` (batch fill), persistent profile. README says CLI+skills are more token-efficient than MCP for coding agents. States it is "not a security boundary". | Node-based. Full snapshots are token-heavy. No memory beyond profile. `fill_form` has no knowledge of validation, dependent fields or wizard steps. |
| **chrome-devtools-mcp** (Google) | ~54.5k stars. Puppeteer-based, strong debugging and perf tracing. | Built for debugging, not task automation. Chrome only. No memory. |
| **Lightpanda** | Zig, AGPL-3.0, ~38.1k stars. Claims 9x faster and 16x less memory than headless Chrome (own benchmark, 933 pages, m5.large). Native MCP. No rendering. | A fast engine, not an agent layer. No memory or form intelligence. AGPL matters for embedding. Treat as a backend we plug in, not a competitor. |
| **Stagehand / Browser Use / Skyvern** | LLM-in-the-loop frameworks. Stagehand caches repeated actions. | Python/TS frameworks, not MCP-first tools for coding agents. Memory is ad hoc (action caches). Not verified in depth. See Phase 0. |
| **Rust newcomers** (Obscura, OxiBrowser) | Early projects with partial CDP. | Low maturity. Not verified in depth. |
| **Qdrant MCP** (`mcp-server-qdrant`) | Generic `store`/`find` tools. | No browser integration. A generic notebook, not structured procedural memory. |

Evidence that forms are the unsolved problem: Online-Mind2Web reports frontier agents at roughly 30–61% (vs ~90% claimed on WebVoyager), with most failures in filters/sorting and navigation. ClawBench reports omitted required fields and mis-clicks on similar labels, with a best score of 33.3%. These are secondary summaries. Verify against the papers in Phase 0 before citing.

**Honest finding**: "Rust headless browser with MCP" is already taken by agent-browser. Our defensible differentiators are the next two sections, not the language.

## 3. Differentiators

1. **Memory as a first-class subsystem on Qdrant** (nobody ships this for browsers):
   - *Procedural memory*: successful form-fill "playbooks" per site/form signature, retrieved by vector similarity plus payload filters (domain, form fingerprint).
   - *Semantic memory*: user/profile facts (name, address, company, preferences) with provenance and per-field sensitivity tags. Retrieval answers "what value goes in this field?".
   - *Episodic memory*: past runs, failures, and their fixes, so the agent does not repeat mistakes.
   - Showcases Qdrant features: named vectors (label/DOM-context/visual), hybrid dense+sparse search, payload filtering, multi-tenancy by collection or payload, Cloud Inference. Confirm exact feature names and availability against current Qdrant docs in Phase 0.
2. **Form compiler, not form primitives**: turn a page into a typed **Form IR** (fields, types, constraints, dependencies, steps, validation rules, required flags). The agent submits values for the IR in one call, and the engine handles ordering, waits for dependent fields, retries on validation errors, and verifies by re-reading the DOM. This directly targets the failure modes above.
3. **Token-frugal observations**: delta snapshots, form-scoped snapshots, and memory-retrieved context instead of re-sending the page. Target a measurable reduction versus Playwright MCP full snapshots on the same tasks (to be measured, not assumed).
4. **Speed through architecture** (claims need benchmarks to back them): single Rust process, async Tokio, direct CDP with command pipelining/batching, no Node hop, connection reuse, Lightpanda fast path for DOM-only pages with automatic Chrome fallback, and memory lookups overlapped with page load.
5. **Reproducible research**: an open benchmark harness (form task suite) plus published numbers, including negative results.

## 4. Architecture

Cargo workspace:

```
ostler/
  crates/
    ostler-cdp/        # thin async CDP client (evaluate chromiumoxide vs own, see Decisions)
    ostler-engine/     # Backend trait: Chrome, Lightpanda; routing + fallback
    ostler-dom/        # accessibility tree + DOM extraction, delta snapshots, shadow DOM, iframes
    ostler-forms/      # Form IR extractor, field typing, fill planner, verifier, wizard state machine
    ostler-memory/     # Embedder trait (CloudInference impl), Qdrant collections, schemas, retrieval, write policy
    ostler-mcp/        # MCP server (stdio + streamable HTTP), tool definitions, profiles
    ostler-cli/        # `ostler` binary: serve, doctor, bench, memory inspect/export/wipe
    ostler-bench/      # benchmark + eval harness
  site/               # portfolio site (GitHub Pages)
  docs/               # ADRs, design docs, research notes
  fixtures/           # local form test pages (wizards, date pickers, shadow DOM, iframes, validation)
```

Key flows:
- **Observe → plan → act → verify**: `form_inspect` returns Form IR; `form_fill` takes `{field_id: value}` or `auto: true`, which asks memory for values and returns only the unresolved fields to the agent.
- **Memory write policy**: write a playbook only after post-submit verification succeeds. Store failures with the error text. Never store secrets, only references, because passwords and payment data go through a separate local vault/redaction path and never into Qdrant. Every memory item has TTL/decay and is user-inspectable and deletable (`ostler memory ...`).
- **Safety**: a domain allowlist, a per-session confirmation gate for irreversible submits (configurable), prompt-injection hygiene (page text is marked as untrusted in tool outputs), and no arbitrary JS tool by default.

## 5. MCP integration

- Tools (small, typed, namespaced): `navigate`, `snapshot` (modes: full/delta/form), `form_inspect`, `form_fill`, `form_submit`, `click`, `type`, `select`, `upload`, `wait`, `screenshot`, `memory_recall`, `memory_save`, `memory_forget`.
- Tool **profiles** (`core`, `forms`, `memory`, `all`) to keep the schema small, since tool-schema size is itself a token cost.
- Transports: stdio for local use, streamable HTTP for remote. Ship ready-to-paste config for Claude Code (`claude mcp add`), Codex (`config.toml`), Cursor and VS Code. Verify each client's current config format at implementation time.
- Rust MCP SDK: evaluate the official `rmcp` crate first.

## 6. Phases and milestones

**Phase 0: Research hardening (before any code, ~1 week)**
- Read the primary sources for Stagehand, Browser Use, Skyvern, Obscura, OxiBrowser and the Online-Mind2Web/ClawBench papers. Write `docs/research/landscape.md` with dated, linked claims.
- Confirm: `rmcp` maturity, state of `chromiumoxide`/`spider_chrome` maintenance (the search found 2025 releases only), Qdrant Cloud Inference model list/limits and the Rust client API, Lightpanda's CDP coverage for forms.
- Spike: 1-day prototype timing CDP round-trips and Lightpanda vs Chrome on 20 fixture forms. This decides whether the speed claims are worth making.
- Exit criteria: ADR-001 to ADR-005 written (engine, CDP crate, embedding, memory schema, MCP SDK).

**Phase 1: Core (weeks 2–4)**: CDP client, Chrome backend, snapshots with refs and delta, MCP server with the core profile, and a working Claude Code demo.

**Phase 2: Forms (weeks 4–7)**: Form IR, fill planner, verifier, wizards, dependent dropdowns, date pickers, uploads, iframes, shadow DOM. Fixture suite as the gate.

**Phase 3: Memory (weeks 6–9)**: Qdrant collections, Cloud Inference embedder, playbook/profile/episode schemas, write policy, `memory_*` tools, inspect/wipe CLI.

**Phase 4: Speed and benchmarks (weeks 8–10)**: Lightpanda backend with fallback, pipelining, benchmarks vs Playwright MCP and agent-browser on identical tasks. Publish methodology and raw data.

**Phase 5: Launch (weeks 10–12)**: Site, docs, demo video/GIFs, technical write-up (the research angle), prebuilt binaries, Homebrew/cargo-binstall/npx shim. Coordinate with Qdrant (blog post, listing).

## 7. Repo and portfolio site

- GitHub repo `ostler` with: README (one GIF, a 3-line install, a comparison table that is honest about competitors), `ARCHITECTURE.md`, `docs/adr/`, CONTRIBUTING, SECURITY, CODE_OF_CONDUCT, issue/PR templates, good-first-issue labels, CI badges.
- CI (GitHub Actions): `cargo fmt`/`clippy -D warnings`/`test`, `cargo deny`, fixture e2e against Chrome, release builds for macOS arm64/x64 and Linux, benchmark job that tracks regressions.
- Site: static (Astro or plain HTML/CSS) deployed through Actions to `<user-or-org>.github.io/ostler`. Sections: Why, Architecture, **Engineering decisions (ADR-driven: choice, alternatives rejected, why)**, Benchmarks (interactive charts with downloadable raw data), Memory deep dive, Install per client, Research notes. Needs the GitHub account or org name that will own the repo.
- Creating the repo and pushing is outward-facing, so I will confirm owner/org and visibility before doing it.

## 8. Risks

- **agent-browser is large and well-funded** and could add memory. Mitigation: ship depth in forms and memory fast, and stay honest in comparisons.
- **Speed claims**: Lightpanda lacks rendering and some web APIs, and it is AGPL. Keep it an optional external binary, not linked in.
- **Cloud-only embeddings** limit try-out. Mitigated by the trait and a planned local backend.
- **Benchmark credibility**: publish the harness, versions and hardware; avoid cherry-picking.
- **Security**: memory could leak PII, and pages can inject prompts. Covered by the policy above plus threat-model doc.
- Star count cannot be planned. What we control is quality, a crisp demo and a credible write-up.

## 9. Verification (how we know it works)

- Unit tests per crate. Fixture suite: ≥30 local form pages covering the failure modes listed in section 2, all run in CI.
- E2E: start `ostler serve`, connect real Claude Code and Codex, complete a multi-step form; run it twice and show the second run uses memory (fewer tool calls/tokens).
- Benchmarks: same tasks across Ostler, Playwright MCP, agent-browser; report success rate, latency, tokens, and tool calls with variance.
- Memory: recall precision on a labelled field-matching set; confirm secrets never appear in Qdrant payloads (automated test).
- `ostler doctor` validates the Chrome install, Qdrant connectivity and MCP handshake.

## 10. Open items needing the user later

GitHub owner/org and Qdrant Cloud credentials, whether to coordinate with the Qdrant team on branding, and final name after the collision check.
