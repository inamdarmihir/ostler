# Failure modes (FMEA)

What breaks, how we notice, what the caller sees, what limits the damage. "Status" is one of: **done** (code and test in tree at the time of writing), **partial**, **open** (known gap, see [REVIEW-2026-10.md](REVIEW-2026-10.md) or [ROADMAP.md](ROADMAP.md)), **in progress** (being added in the current release cycle, not yet confirmed in the tree; check the [CHANGELOG](../CHANGELOG.md) before relying on it), **planned**.

| component | failure | detection | effect | mitigation | status |
|---|---|---|---|---|---|
| Chrome (launch) | binary missing, sandbox refused (root/container), profile in use, missing libs | `ostler doctor` prints cause and a `fix:` line | no tools work | `OSTLER_CHROME`, `OSTLER_NO_SANDBOX=1` on isolated hosts, `doctor` explains | done |
| Chrome (runtime) | Chrome crashes or is killed mid-session | CDP connection closes; next call sees it | tool call fails | browser is relaunched on the next call; soak kills Chrome (`kill -9`) in cycle 4 and requires recovery | done |
| Chrome (orphaning) | server SIGKILLed | n/a | Chrome would linger | pipe transport: Chrome exits when the pipe closes | done; the temp profile directory of a killed server remains (partial) |
| CDP | oversized frame (tall screenshot) kills reader | request hangs or errors | tool call fails | WebSocket frame limits raised; 200,000 px screenshot in the soak | done |
| CDP | request never answered | caller waits | stalled tool | per-request timeouts in `ostler-cdp`; navigate wait capped at 30 s; a 120 s per-tool deadline (`OSTLER_TOOL_TIMEOUT_SECS`; `wait_for` exempt) that reports ACTION_UNCERTAIN | done (helper-level test; not exercised on a live wedged page) |
| Page | never settles (polling, streaming, animation) | settle wait expires | action returns before page is quiet | bounded wait; result reports it was not settled | partial (settle result handling listed open in the review) |
| Page | framework re-mounts the form mid-fill | read-back by stale ref fails | key could be dropped | keys resolved once against all fields; stale refs followed by label; unmatched keys make the result not ok | done |
| Page | value reformatted / reverted / cleared before submit | read-back; re-read before click | false "verified" | `Reformatted` is distinct from `Mismatch`; `form_submit` re-reads and reports `CHANGED BEFORE SUBMIT` | done |
| Page | ambiguous key (two plausible fields) | matcher | would fill the wrong field | refuse and list candidates, never guess | done |
| Page | widget not supported (cross-origin iframe, closed shadow root, some select widgets) | field absent or `NoMatch` | cannot fill | reported as not found, not as filled | open (documented limits) |
| Page | verification wall (CAPTCHA, 2FA) | challenge detector | cannot proceed | `CHALLENGE:` notice, `wait_for_human`, A2A `input-required` ([ADR-0009](adr/0009-challenges-are-handed-off.md)) | done |
| Page | hostile content / hidden text / prompt injection | fencing, hidden-text filter | model could be steered | `<page_content untrusted>` wrapper; hidden text dropped and counted; allowlist and confirm gate | partial (see [THREAT-MODEL.md](THREAT-MODEL.md)) |
| Submit | server returns 4xx/5xx with no on-page error | HTTP status of the submit | would look like "nothing happened" | classified `ServerError`, reported as failed, "data may or may not have been stored" | done |
| Submit | agent retries a submit that actually succeeded (duplicate order) | `ServerError` text; double-submit guard (`page.rs`) | duplicate submission | agent is told `ServerError` is ambiguous (data may or may not be stored); guard against a second submit of the same form | done (guard present in tree; confirm exact semantics in the CHANGELOG) |
| Submit | submit cancelled or timed out mid-flight | `Disarm` drop guard | confirm gate left open | gate closes when the submit future is dropped | done |
| Policy | request to a non-allowlisted host | interception + resolver rule | blocked | canary tests across 12 channels | done |
| Policy | DNS rebinding | none | name check outwitted | use an allowlist; resolver rule covers browser lookups | partial (documented) |
| Memory (Qdrant) | slow or unreachable | 4 s timeout; two failures in a row | lookups would stall a fill | lookups skipped for 30 s, tool results start with a note; fill continues without memory | done |
| Memory (Qdrant) | transient error on write | background write fails | a lesson is lost | writes are off the response path; the circuit breaker limits stalls; bounded retries (at most 2, jittered backoff) for reads and upserts only | done (tested against a mock Qdrant, not a real one) |
| Memory | key lost or different from the one that sealed the data | start-up check opens sealed points | memory unreadable | refuse to start with recovery steps; `memory key import`; `memory wipe --yes` as last resort | done |
| Memory | key created blind, shadowing existing memory | first-start probe | silent data loss | key created only if Qdrant seen to hold no sealed memory; `--allow-new-key` explicit | done |
| Memory | learned skills grow stale or wrong | win/loss counters, `verified_ts` | wrong mapping replayed | wins reinforce, failures demote; `prune`; start-up expiry (`--retain-days`, default 180); delete cascade for forgotten facts | partial; `Memory::forget` now cascades to playbook mappings derived from the fact; start-up expiry applies to skills and episodes, facts never expire |
| Memory | embedding model changed (same dimension) | none | silently worse recall | dimension mismatch is detected; model-name check | open |
| Memory | tenant isolation breach | tenant filter tests on real Qdrant | cross-user exposure | per-tenant collections + JWT | done (optional) |
| Embedder (Ollama) | not running | request error | no vectors for semantic lookups | exact and `autocomplete` lookups still work without vectors; circuit breaker as above | partial |
| MCP | client sends batch, cancel, concurrent requests | protocol tests | requests are strictly sequential, so a long `wait_for` blocks `ping` | documented | open |
| MCP | tool error | error text returned | agent must decide whether to retry | typed errors (`STALE_REF`, `ACTION_UNCERTAIN`, `BLOCKED`, `NOT_ACTIONABLE`, `NAVIGATION_FAILED`) each with a retry rule; double-submit guard on `form_submit` (override `again=true`) | done ([ADR-0011](adr/0011-reliability-principles.md)) |
| A2A | too many conversations | `--max-contexts` | new context refused | cap and idle close | done |
| A2A | task runs too long | `--task-timeout-secs` (default 120) | task `failed` | timeout with a small stream grace | done |
| A2A | oversized or hostile request, liveness probing | body-size limit (1 MiB default, `max_body_bytes`); `GET /healthz` | resource use; orchestration blind | body limit and `/healthz` in `ostler-a2a/src/server.rs` | done |
| A2A | caller uses wrong `Host`/`Origin` or no token | header checks | request refused | by design | done |
| Host | out of memory (many Chromes) | OS | killed processes | session caps; planning rule in [SYSTEM-DESIGN.md](SYSTEM-DESIGN.md) s7 | partial |
| Release | bad upgrade | installers test, `--locked` MSRV job, CI | broken binary | [OPERATIONS.md](OPERATIONS.md) rollback | done |
