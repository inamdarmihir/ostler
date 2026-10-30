# Architecture

```
  agent (Claude Code, Codex, Cursor, …)
        │  MCP / JSON-RPC over stdio
        ▼
  waymarker-mcp ── tool catalogue, profiles, safety gates, metrics, memory glue
        │                                  │
        ▼                                  ▼
  waymarker-browser                  waymarker-memory ──► Qdrant (REST)
   Page / fill engine / snapshots      profile · playbooks · episodes
        │         │
        │         └─ waymarker-forms (form IR, key→field matching, value normalisation)
        ▼
  waymarker-cdp ── async CDP over one WebSocket, Chrome launcher
        ▼
  Chrome (headless)  ◄── runtime.js injected into every document
```

| crate | responsibility |
|---|---|
| `waymarker-cdp` | One WebSocket to the browser, id-matched requests, broadcast events, flattened target sessions, Chrome discovery/launch, process-tree memory measurement. ~650 lines, no browser-automation dependency. |
| `waymarker-browser` | `Page` (navigate, snapshot, click, type, key, upload, screenshot, wait), the fill/verify/submit engine, domain policy, the in-page runtime. |
| `waymarker-forms` | Pure logic, no I/O: form IR types, matching agent keys to fields, option/date/time normalisation, sensitive-field detection, form fingerprints. |
| `waymarker-memory` | `Store` trait with `QdrantStore` (Cloud Inference or client vectors) and `InMemoryStore`; `Memory` API for one tenant. |
| `waymarker-mcp` | JSON-RPC loop, 17 tools in 3 profiles, lazy browser, per-tool metrics, background memory writes. |
| `waymarker-cli` | `serve`, `doctor`, `inspect`, `memory`, `fixtures`. |
| `waymarker-fixtures` | The 11 test pages and a recording HTTP server. |
| `waymarker-bench` | Forms / footprint / concurrency / memory-scale benchmarks and the LLM cost model. |

## The decisions that matter, and why

Each has an ADR in [`docs/adr/`](adr/) where the alternatives are written down.

**An injected runtime instead of CDP's accessibility tree.** A single script (`runtime.js`) runs inside every document and does traversal, snapshotting and field manipulation. It sees open shadow roots and same-origin iframes through one code path, can run React-safe native setters, and answers a question in one round-trip that would take dozens of CDP calls. The cost: it is our code running in a hostile page's world, so it is written defensively (no `eval`, no page-supplied selectors executed as code) and it cannot see closed shadow roots or cross-origin frames.

**A form IR, not form primitives.** `form_inspect` returns typed fields (kind, label, required, options, pattern, current value, error) and submit controls. `form_fill` takes `{label | name | ref: value}`, resolves each key to a field (exact ref → name/id → label → fuzzy; **ambiguity is reported, never guessed**), normalises the value for the field (option text → option index, `"March 5, 2001"` → `2001-03-05`, `"3:30 PM"` → `15:30`; ambiguous `03/04/2024` is refused), fills in document order, **re-inspects after every field** so dependent controls appear, and **reads each value back**. This is what removes round-trips: the model states intent once.

**Settle by counting work, not by sleeping.** Page scripts are wrapped so the runtime knows how many `fetch`/XHR requests and short timers are in flight. After any action it waits until the DOM is quiet *and* nothing is pending. Fixed sleeps are either too short (flaky) or too long (slow). The wrapper must not count its own polling; getting that wrong cost us 3 s per action until the benchmark exposed it.

**Verification with a fallback strategy.** Typed input (`Input.insertText`) is tried first because it fires the same events a user would. If the page rejects it (masked or framework-controlled inputs), the engine retries with the native value setter plus events. Values the page reformats (`(555) 123-4567`) are reported as *reformatted*, not as errors.

**Observation is part of the action.** `navigate` returns an interactive snapshot; `click`/`type`/`press` return a delta snapshot. Saves a tool call per step and keeps the context small. `observe: none` opts out.

**Memory as three kinds, one tenant filter.** *Profile* (facts about the user), *playbooks* (structure of forms that worked: field labels and which fact fed them, **never values**), *episodes* (validation errors and failures, deduplicated with a count). Retrieval prefers the most reliable signal first: playbook mapping → the field's `autocomplete` token matched against facts (payload filter, no vectors) → semantic vector search. Writes happen in the background, never on the response path. Every point carries `tenant`, indexed with `is_tenant`.

**One Qdrant API, two vector sources.** With Qdrant Cloud Inference the client sends `{text, model}` and Qdrant embeds server-side. A client-side embedder exists for tests and for development against open-source Qdrant. The deterministic hash embedder used there is explicitly **not** semantic.

**Safety is structural.** Domain allowlist enforced on navigation *and* after every action (redirects, link clicks); non-http(s) schemes blocked; optional `--require-confirm-submit`; secrets refused by the memory API and never auto-filled; sensitive values masked in reports; page-derived text wrapped in `<page_content untrusted="true">` and the server instructions tell the model not to follow it.

## Known limits (deliberately not hidden)

- Chrome/Chromium only (Lightpanda/other CDP engines are a planned pluggable backend, see ADR-0001; not implemented).
- One page per server. No tabs or popups.
- Cross-origin iframes and closed shadow roots are invisible to the runtime.
- No CAPTCHA, 2FA, anti-bot or login-state management beyond `--profile-dir`.
- Custom date-picker widgets are filled by typing; calendar-click widgets are not special-cased.
- stdio transport only (no HTTP transport yet).
- Linux and Windows are untested locally; CI covers Linux.
