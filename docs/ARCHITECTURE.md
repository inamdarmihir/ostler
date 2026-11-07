# Architecture

```
  agent (Claude Code, Codex, Cursor, …)
        │  MCP / JSON-RPC over stdio
        ▼
  ostler-mcp ── tool catalogue, profiles, safety gates, metrics, memory glue
        │                                  │
        ▼                                  ▼
  ostler-browser                  ostler-memory ──► Qdrant (REST)
   Page / fill engine / snapshots      profile · playbooks · episodes
                                       (sealed on this machine by default)
        │         │
        │         └─ ostler-forms (form IR, key→field matching, value normalisation)
        ▼
  ostler-cdp ── async CDP over a pipe to Chrome (WebSocket optional), Chrome launcher
        ▼
  Chrome (headless by default, --headed to show the window)  ◄── runtime.js injected into every document
```

| crate | responsibility |
|---|---|
| `ostler-cdp` | One connection to the browser (a `--remote-debugging-pipe` by default, so there is no listening port; `OSTLER_CDP_TRANSPORT=ws` for debugging), id-matched requests, broadcast events, flattened target sessions, Chrome discovery/launch, process-tree memory measurement. No browser-automation dependency. |
| `ostler-browser` | `Page` (navigate, snapshot, click, type, key, upload, screenshot, wait), the fill/verify/submit engine, domain policy, the in-page runtime. |
| `ostler-forms` | Pure logic, no I/O: form IR types, matching agent keys to fields, option/date/time normalisation, sensitive-field detection, form fingerprints. |
| `ostler-memory` | `Store` trait with `QdrantStore` (Cloud Inference or client vectors from a local embedder: Ollama or the non-semantic hash), `InMemoryStore` and `SealedStore` (client-side encryption wrapper); `Memory` API for one tenant; per-tenant Qdrant access tokens (`token.rs`). |
| `ostler-mcp` | JSON-RPC loop, 30 tools in 5 profiles (15 core, 19 forms, 21 research, 30 all; `auto` grows from core), lazy browser, per-tool metrics, background memory writes. `evidence.rs` holds the research session: pages read (markdown for passages, plain words for verification), verified citations, re-verification when a page changes; `app/research.rs` wires `read`, `cite`, `evidence`, `search`, `open_many`, `extract` and `links` to `ostler-browser`, `ostler-extract` and `ostler-research`. |
| `ostler-a2a` | The Agent2Agent HTTP surface (`ostler a2a`); see [A2A.md](A2A.md). |
| `ostler` (`crates/ostler-cli`) | `serve`, `a2a`, `init`, `config`, `setup`, `doctor`, `inspect`, `memory`, `fixtures`. |
| `ostler-fixtures` | The 49 form test pages (`PAGES` in `crates/ostler-fixtures/src/lib.rs`, plus an internal 404 page), the domain packs under `packs/` (research, gov-forms, jobs, dev-qa, commerce: 34 tasks, 29 pages), and a recording HTTP server (`/__records`, `/__reset`, `/exfil`, `/pay`, `robots.txt`, a second origin port). |
| `ostler-bench` | Forms / footprint / concurrency / memory-scale benchmarks and the LLM cost model. |

## The decisions that matter, and why

Each has an ADR in [`docs/adr/`](adr/) where the alternatives are written down.

**An injected runtime instead of CDP's accessibility tree.** A single script (`runtime.js`) runs inside every document and does traversal, snapshotting and field manipulation. It sees open shadow roots and same-origin iframes through one code path, can run React-safe native setters, and answers a question in one round-trip that would take dozens of CDP calls. The cost: it is our code running in a hostile page's world, so it is written defensively (no `eval`, no page-supplied selectors executed as code) and it cannot see closed shadow roots or cross-origin frames.

**A form IR, not form primitives.** `form_inspect` returns typed fields (kind, label, required, options, pattern, current value, error) and submit controls. `form_fill` takes `{label | name | ref: value}`, resolves each key to a field (exact ref → name/id → label → fuzzy; **ambiguity is reported, never guessed**), normalises the value for the field (option text → option index, `"March 5, 2001"` → `2001-03-05`, `"3:30 PM"` → `15:30`; ambiguous `03/04/2024` is refused), fills in document order, **re-inspects after every field** so dependent controls appear, and **reads each value back**. This is what removes round-trips: the model states intent once.

**Settle by counting work, not by sleeping.** Page scripts are wrapped so the runtime knows how many `fetch`/XHR requests and short timers are in flight. After any action it waits until the DOM is quiet *and* nothing is pending. Fixed sleeps are either too short (flaky) or too long (slow). The wrapper must not count its own polling; getting that wrong cost us 3 s per action until the benchmark exposed it.

**Verification with a fallback strategy.** Typed input (`Input.insertText`) is tried first because it fires the same events a user would. If the page rejects it (masked or framework-controlled inputs), the engine retries with the native value setter plus events. Values the page reformats (`(555) 123-4567`) are reported as *reformatted*, not as errors.

**Submit outcomes.** `form_submit` classifies the result as `ValidationFailed`, `Advanced` (next wizard step), `Submitted`, `Unchanged` or `ServerError`. `ServerError` is an HTTP 4xx/5xx answer to the submit with no validation messages on the page: it is reported as failed, never as submitted, and the agent is told the data may or may not have been stored. With no `form` argument, `submit` targets the form that `form_fill` last filled (`last_form`, cleared after every submit); if the page has several forms and none was filled, it refuses and lists them rather than guessing the first.

**Observation is part of the action.** `navigate` returns an interactive snapshot; `click`/`type`/`press` return a delta snapshot. Saves a tool call per step and keeps the context small. `observe: none` opts out.

**Memory as three kinds, one tenant filter.** *Profile* (facts about the user), *playbooks* (structure of forms that worked: field labels and which fact fed them, **never values**), *episodes* (validation errors and failures, deduplicated with a count). Retrieval prefers the most reliable signal first: the form's own playbook mapping → the field's `autocomplete` token matched against facts (payload filter, no vectors) → a trusted mapping from a similar form on the same host, ranked in Qdrant by a score-boosting formula over similarity, win rate and recency → semantic vector search. Every fill outcome patches the playbook: wins reinforce a mapping, failures demote it, and `prune` forgets stale, unproven skills ([ADR-0008](adr/0008-learning-loop.md)). Writes happen in the background, never on the response path. Every point carries `tenant`, indexed with `is_tenant`; `--isolation collection` instead gives each tenant its own three collections (the usual names prefixed with `ostler_<tenant>_`), see "Tenant tokens" below. On start, `serve` forgets learned skills and episodes not verified for `--retain-days` (default 180; 0 keeps them); facts you saved never expire.

**One Qdrant API, three vector sources (`--embeddings`).** `cloud`: with Qdrant Cloud Inference the client sends `{text, model}` and Qdrant embeds server-side, so the field text reaches Qdrant. `ollama`: a local Ollama server embeds on this machine (`--embedding-url`, default `http://127.0.0.1:11434`) and only vectors reach Qdrant; plain http to a non-loopback embedder is refused. `dev-hash`: a deterministic client-side hash embedder for tests and open-source Qdrant development; explicitly **not** semantic.

**Sealing (`SealedStore`, `--seal auto|on|off`).** With memory in Qdrant, `auto` wraps the store so Qdrant never sees plaintext payloads. Per point: `tenant`, `wins`, `losses` and `verified_ts` stay in clear (Qdrant filters, ranks and expires on them); `key_norm`, `fingerprint`, `autocomplete`, `id_hint`, `host` and `labels_norm` become keyed hashes (HMAC-SHA-256), so equality filters still work without the server learning site names, labels or keys; everything else (fact values, descriptions, playbooks, episodes) is AES-256-GCM encrypted with the point id bound in as associated data, so a blob moved to another point does not open and a point that fails to open is skipped. **Point ids are keyed**: unsealed ids are a UUIDv5 of tenant, kind and key (guessable: anyone could test `uuid5(tenant, "profile", "passport number")`), sealed ids are derived with the index key. Both sub-keys come from one 32-byte master key (`OSTLER_MEMORY_KEY`, or an owner-only `memory.key` beside the config file, created on first use). Not hidden: the vector, the tenant name, win/loss counters, timing and point counts. Lose the key and sealed memory is unreadable (`ostler memory wipe --yes` starts over). `ostler memory seal` migrates facts saved before sealing; learned skills and episodes from then are dropped and relearned. It also upgrades `v1` blobs (not read by this build: `v2` authenticates the clear fields) and re-files facts and playbooks whose lookup key predates the current `norm()`; `probe_key` reports both as `NeedsUpgrade`, and every start refuses until `memory seal` has run. The key is created only on a first start in which Qdrant was seen to hold no sealed memory for the tenant (`--allow-new-key` covers a first-time setup with Qdrant down), under a lock file for the keychain store. Episodes are additionally redacted of emails, runs of 4+ digits and URL query strings before they are embedded or stored. Plain `http://` to a non-loopback Qdrant is refused unless `--allow-insecure-qdrant`.

**Tenant tokens.** With `--isolation collection`, each tenant gets its own collections and `ostler memory token` mints an HS256 JWT, signed with the Qdrant admin key, whose `access` claim limits the bearer to those collections (optionally read-only, 30 days by default). It needs Qdrant with `service.jwt_rbac: true`. Isolation is per collection because Qdrant 1.20 rejects the older per-point payload-filter claim.

**Safety is structural.** With an allowlist, every request the browser makes (documents, sub-resources, WebSockets, popups) is held and checked, and Chrome is started with a host-resolver rule that makes every other host unresolvable. Without one, navigations are checked and cloud-metadata addresses are always refused; loopback and LAN stay open so local dev servers work (`--block-private-networks` closes them). Non-http(s) schemes are blocked; `--require-confirm-submit` blocks every non-GET request outside a confirmed submit (`submit` arms the gate for its own duration through a `Disarm` drop guard, so a cancelled or timed-out submit, e.g. an A2A deadline, cannot leave the page able to send data); secrets are refused by the memory API and never auto-filled; sensitive values are masked in reports, snapshots and notes; page-derived text (including titles and URLs) is wrapped in `<page_content untrusted="true">` and the server instructions tell the model not to follow it. If Qdrant fails twice in a row, lookups are skipped for 30 s and tool results say so.

## Known limits (deliberately not hidden)

- Chrome/Chromium only (Lightpanda/other CDP engines are a planned pluggable backend, see ADR-0001; not implemented).
- Cross-origin iframes and closed shadow roots are invisible to the runtime.
- No CAPTCHA solving, 2FA, anti-bot evasion or login-state management beyond `--profile-dir`. Verification walls are *detected and handed to a person* (`CHALLENGE:` notice, `wait_for_human`, A2A `input-required`); this is deliberate, see [ADR-0009](adr/0009-challenges-are-handed-off.md).
- Calendar pickers on read-only inputs are handled heuristically (month header, prev/next, day cells); dropdown-only and year-only pickers are not.
- MCP runs over stdio only; `ostler a2a` is the HTTP surface.
- Linux and Windows are untested locally; CI covers Linux and Windows (Windows is experimental).

## Related

[SYSTEM-DESIGN.md](SYSTEM-DESIGN.md) (context, constraints, capacity, trade-offs) · [THREAT-MODEL.md](THREAT-MODEL.md) · [FAILURE-MODES.md](FAILURE-MODES.md) · [SLOS.md](SLOS.md) · [COMPATIBILITY.md](COMPATIBILITY.md) · [OPERATIONS.md](OPERATIONS.md) · [ADR-0011](adr/0011-reliability-principles.md)
