# ADR-0015: `rmcp` for streamable HTTP only, behind an adapter; keep the stdio loop until a transcript suite passes on both

Status: proposed (M0 spike, 2026-11-06). Answers the `rmcp` row of BUILD-PLAN section 4; relates to [ADR-0011](0011-reliability-principles.md).

## Context
`crates/ostler-mcp/src/lib.rs` is a 305-line hand-written JSON-RPC loop over stdio. It negotiates among three protocol versions (`2025-06-18`, `2025-03-26`, `2024-11-05`), serves a per-profile tool list that **changes during a session** (forms and memory tools are revealed when a page shows form fields), and sends `notifications/tools/list_changed` right after the response that caused the change. The plan wants streamable HTTP (M4) and proposes the official SDK `rmcp` rather than writing the HTTP transport by hand.

## Decision
1. **Adopt `rmcp` for the streamable HTTP server, at M4**, as a thin adapter: an `rmcp::ServerHandler` whose `list_tools` and `call_tool` delegate to the same `App` the stdio loop uses. No `#[tool]` macros (they add `schemars` and proc-macro weight; our tool definitions already exist as JSON).
2. **Keep the hand-written stdio loop** for now. Replacing it buys nothing for the product and risks the `list_changed` behaviour and the tested edge cases (hostile arguments, size budget, clean shutdown on SIGTERM).
3. **Gate a later stdio migration** on a transcript suite: recorded JSON-RPC sessions (initialize, list, call, list_changed, errors, hostile input) that pass byte-for-byte after normalising ids on both implementations.
4. **Per-session state first.** `App` today keeps one global "revealed" flag, right for one stdio client. HTTP serves many sessions: reveal state must move into a session object before the adapter is useful.

## Rejected alternatives
- *Replace the stdio loop with `rmcp` now.* Behaviour differences found below; no user benefit yet.
- *Write streamable HTTP by hand.* Session ids, SSE streams, `Host` checks and resumption are exactly what the SDK is for.
- *Another community MCP crate.* Not evaluated.

## Consequences
- The SDK moves fast: 3.5.0 (2026-09-28) to 3.9.0 (2026-11-05) is five minor releases in six weeks, and it already knows a protocol revision (`2026-07-28`) without an `initialize` handshake. Pin exactly; budget a small bump each release.
- Notification ordering differs (below): with `rmcp`, a notification sent from inside the handler arrives **before** the tool response; ours arrives after. Both are valid MCP; a client that re-reads the tool list on notification works either way. The transcript suite must normalise this or we must send from the response path.
- Host header validation comes on by default (below), which suits a local server.
- Compile weight of the HTTP server feature: 86 crate versions and about 48 s cold build here.

## Verification
Run 2026-11-06, Linux x86_64, rustc 1.97.0.

| claim | how verified | result |
|---|---|---|
| version, licence, MSRV | crates.io API | `rmcp` 3.9.0 (2026-11-05), Apache-2.0, `rust_version` 1.88; ~45M downloads |
| protocol versions | read `src/model.rs` | knows `2024-11-05`, `2025-03-26`, `2025-06-18`, `2025-11-25`, `2026-07-28`; our three are a subset |
| hand-written server, no macros | spike: `--no-default-features --features server,transport-io`, `ServerHandler` with `get_info`, `list_tools`, `call_tool`, ~50 lines | compiled first try; 48 s cold build; 4.3 MB unstripped binary |
| `tools/list_changed` works | drove the spike over stdio with a Python client: initialize, `tools/list` (1 tool), `tools/call navigate` (handler calls `ctx.peer.notify_tool_list_changed()` and flips a flag), `tools/list` | `initialize` result had `capabilities.tools.listChanged: true`; messages after the call arrived as **`notifications/tools/list_changed` first, then the response**; second `tools/list` returned 2 tools |
| streamable HTTP server exists | added feature `transport-streamable-http-server`; read `transport/streamable_http_server/tower.rs` | compiles; `StreamableHttpService` is a `tower_service::Service` (`ostler-a2a` already uses hyper-util); config has `allowed_hosts` defaulting to `localhost, 127.0.0.1, ::1`, `json_response`, `cancellation_token`; 86 crate versions |
| licence tree | script over `cargo metadata` vs `deny.toml` | all 73 resolved packages (stdio spike) in the allow list; the HTTP feature set was not re-checked |

**Not verified (UNVERIFIED):** serving real HTTP and interop with a real client (Claude Desktop, Inspector, Playwright-style clients); `list_changed` over a stateful HTTP session's SSE stream; stateless mode; graceful shutdown; how `rmcp` maps our `isError` conventions and size limits; behaviour under our hostile-argument tests; the other protocol versions (only `2025-06-18` was negotiated in the spike); authentication features (`auth`) were not looked at.

**Risks to the dynamic tool list, in order of concern:** (1) per-session reveal state (Decision 4); (2) notification ordering above; (3) `rmcp` caches nothing for us, but clients on `2026-07-28` get per-request metadata instead of a handshake, which changes when "session start" is observable; (4) a tool whose visibility depends on page state needs the page to be the session's page, so HTTP sessions need their own browser context.
