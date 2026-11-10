# Threat model

STRIDE per trust boundary (see [SYSTEM-DESIGN.md](SYSTEM-DESIGN.md) s4). [SECURITY.md](../SECURITY.md) is the per-threat table with the exact flags and tests; this page organises the same facts by boundary and states residual risk. Not audited by a third party.

Assets: the user's saved facts (PII), logged-in browser sessions (`--profile-dir`), the sealing key, the Qdrant admin key, the A2A bearer token, the host network (LAN, cloud metadata), local files reachable by upload.

Actors: a hostile web page (assumed capable of arbitrary JS and DOM), a manipulated or buggy agent, a remote A2A caller, another local OS user, whoever can read Qdrant (operator, leaked key, backup).

## B1: agent (MCP client) to Ostler

The agent is semi-trusted: it acts for the user but can be steered by page text.

| STRIDE | threat | mitigation that exists | residual risk |
|---|---|---|---|
| S | Another process speaks to the stdio server | stdio is a private pipe to the launching client | any process running as the same user can start its own instance |
| T | Agent submits something irreversible | `--require-confirm-submit` blocks every non-GET request outside a confirmed `form_submit`; the gate closes on cancel/timeout (`Disarm` drop guard) | off by default; GET with side effects and WebSockets not covered |
| R | No record of what the agent did | per-tool counters via the `stats` tool (calls, ms, errors, estimated tokens) | no durable audit log (**planned**, enterprise item in [ECONOMICS.md](ECONOMICS.md) s4) |
| I | Agent reads secrets back | memory API refuses password/token/card-like keys and card-like values; sensitive fields never auto-filled; values masked in reports, snapshots, notes | heuristic detection: a secret under an innocuous key and value is not caught |
| D | Agent floods the browser (huge pages, popups, dialogs) | max 8 adopted popups; dialogs dismissed or acknowledged; soak stress pages (8 MB page, 5,000 fields, 200,000 px screenshot, alert storm, hanging server) | per-tool wall-clock deadline and typed retry-aware errors are in progress and not claimed here (see [ADR-0011](adr/0011-reliability-principles.md)) |
| E | Agent navigates outside its remit | allowlist in the browser (`--allow-domain`, optional `host:port`); upload only under `--upload-dir`; `file:` needs `--allow-file`; metadata addresses always refused | with no allowlist `serve` permits loopback/LAN (deliberate); DNS rebinding can outwit the name check |

## B2: Ostler to the web page (untrusted)

| STRIDE | threat | mitigation that exists | residual risk |
|---|---|---|---|
| S | Page impersonates a trusted UI or forges Ostler's notices | all page-derived text (content, titles, URLs, dialogs, `look` legend) is wrapped in `<page_content untrusted="true">`, wrapper defanged against escape; server instructions tell the model not to follow it | a model can still be persuaded by text a person could see |
| T | Page tampers with the runtime (`window.__wm`, prototypes) to mislead field values or visibility | runtime keeps its own references to `fetch`/XHR/`setTimeout`, avoids `eval`, executes no page-supplied strings; `form_submit` re-reads verified fields before clicking and reports `CHANGED BEFORE SUBMIT` | same JS world; isolated world not done (ADR-0004). Never rely on the runtime against adversarial pages |
| R | Page denies/alters what was submitted | `ServerError` and `Unchanged` are reported as not submitted; submit evidence checked, not just URL | a server that lies about success is out of scope |
| I | Page exfiltrates data via sub-requests, redirects, popups | request interception for every type plus a browser-wide resolver rule; popups and cross-site iframes held at creation; canary-server tests across 12 channels (`security_e2e.rs`, soak) | port pins are defence in depth for WebSockets; the always-on metadata check covers navigations and literals only |
| I | Hidden text carries an injected instruction | invisible text (opacity 0, zero font, off-screen, same colour on solid background) is dropped and counted | gradients, images, `clip`-style text are not detected |
| D | Page hangs, mutates endlessly, spams popups/dialogs | settle has a bounded wait; popup cap; navigate wait bounded (30 s in `page.rs`) | per-tool deadline of 120 s (`OSTLER_TOOL_TIMEOUT_SECS`), reported as ACTION_UNCERTAIN |
| E | Page reaches the user's files or host network | uploads gated and path-resolved (`..`, symlinks); `confirm()`/`prompt()` dismissed; private networks blockable | anything inside `--upload-dir` can be attached to any visited page |

## B3: Ostler to Qdrant (and the embedder)

Qdrant is treated as honest-but-curious: it may be read by its operator, leaked, or backed up.

| STRIDE | threat | mitigation that exists | residual risk |
|---|---|---|---|
| S | Wrong or malicious server answers | `https` required for non-loopback (`--allow-insecure-qdrant` to override); a point that fails AES-GCM open is skipped | no certificate pinning |
| T | Server moves or edits points | sealed format `v2` authenticates clear fields (`tenant`, `wins`, `losses`, `verified_ts`); point id bound as AEAD associated data; lookup hashes re-checked | server can delete points or withhold results |
| R | Who wrote what | none (single user per tenant) | not applicable until multi-user |
| I | Operator or backup reads PII | fact values, playbooks, episodes AES-256-GCM; site names, labels, keys, point ids are HMAC hashes; episodes redacted of emails, 4+ digit runs, URL queries | vector, tenant name, win/loss counters, timing, point count are visible; with `--embeddings cloud` field text goes to Qdrant Cloud Inference (use `ollama` to keep it local) |
| I | Key lost or replaced | start refuses rather than silently replace a key that cannot open existing memory; key created only after Qdrant seen to hold no sealed memory | lost key means sealed memory is unreadable by design |
| D | Qdrant slow or down | 4 s request timeout; after two consecutive failures lookups are skipped for 30 s, tool results carry a note | `forget` cascades to playbook mappings derived from a fact; reads and upserts retry at most twice with jittered backoff; deletes never retry |
| E | One tenant reads another | `tenant` filter on every query (tested on real Qdrant) | all tenants share collections: a leaked Qdrant key exposes every tenant |

## B4: local user and OS

| STRIDE | threat | mitigation that exists | residual risk |
|---|---|---|---|
| S/E | Another local user attaches to Chrome | `--remote-debugging-pipe`: no listening socket (tested in `no_debug_port.rs`); Chrome exits with the server | `OSTLER_CDP_TRANSPORT=ws` (debugging) reopens an unauthenticated localhost port |
| I | Key or session data read from disk | `memory.key` owner-only (checked by `doctor`, fixed by `doctor --fix`); optional OS keychain | `--profile-dir` sessions are readable by any process running as you |
| I | Secrets in logs | logs go to stderr; sensitive values are masked in tool output | stderr destination is the MCP client's choice |
| D | Leftover temp profile after SIGKILL | Chrome itself exits when the pipe closes | the temp profile directory of a killed server remains |

## B5: A2A callers (network)

| STRIDE | threat | mitigation that exists | residual risk |
|---|---|---|---|
| S | Unauthenticated remote caller | loopback bind by default; non-loopback bind requires `--a2a-token`; constant-time bearer compare | the Agent Card is public by protocol design; no TLS (use a reverse proxy) |
| T | Browser-based drive-by or DNS rebinding | requests with an `Origin` header refused; `Host` must be loopback or listed (`--allowed-host`, `--public-url`) | depends on the operator setting `--public-url` correctly |
| R | Attribution of a submission | submit needs explicit `confirm: true` unless `--a2a-auto-submit` | no per-caller identity: one token, one memory tenant |
| I | Caller reads another conversation | one `contextId` is one Chrome, tabs and notes; contexts never see each other's pages | all conversations share one Qdrant tenant: run one server per tenant |
| D | Caller exhausts the host | `--max-contexts` (default 4), idle close (300 s), task timeout (120 s default); request body limit (1 MiB), header/body timeouts, a concurrent-task limit answering 429 with `Retry-After`, `/healthz` and `/readyz`, and graceful drain on SIGTERM exist in `ostler-a2a` | no rate limiting per caller (**planned**) |
| E | Remote caller probes host network | private, loopback, metadata blocked unless `--allow-private-networks`; `--allow-domain` strongly advised | without `--allow-domain` a caller can send the browser to any public site |

## Cross-cutting residual risks

1. Prompt injection that a human could also read cannot be filtered; the controls are the allowlist and the confirm gate, both opt-in on `serve`.
2. Same-world runtime: treat page-reported state as advisory on hostile sites.
3. No third-party audit; a `deny.toml` for cargo-deny is in the tree; dependabot and a security workflow are not, and are not claimed here (see [ADR-0011](adr/0011-reliability-principles.md) and the CHANGELOG).
4. Windows is experimental (CI only, no day-to-day use); Linux secret-service key store is tested against a scripted fake `secret-tool`, not a real keyring.

## Verifying

The commands in [SECURITY.md](../SECURITY.md#verify-it-yourself) and the soak harness (`scripts/soak/`, see [OPERATIONS.md](OPERATIONS.md)) exercise B2 end to end, including a control run that must reach the canary.

## Research mode (planned, partly built as libraries)

Reading many untrusted pages makes prompt injection the main threat, and memory raises the stakes. Built as libraries and not yet reachable over MCP: injection-pattern flags on chunks (reported, never relied on), a risk-classed action gate (read / reversible write / irreversible / money-identity, stricter class when unsure), a double-submit guard, a redacted action trace. Fixture packs include an exfiltration endpoint (`/exfil`) and a payment endpoint (`/pay`) to test that nothing is sent. Open: gate policy per domain, memory writes sourced from page text, and navigations containing recalled values are designed in [BUILD-PLAN.md](BUILD-PLAN.md) but not enforced. Do not treat research mode as safe against a hostile page until they are.
