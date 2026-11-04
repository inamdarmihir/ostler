# Agent2Agent (A2A)

Ostler can be driven by another agent over [A2A](https://a2a-protocol.org) (v1.0, JSON-RPC over HTTP, SSE streaming), as well as by a coding agent over MCP. Use MCP when one agent owns the browser; use A2A when another agent (or service) should be able to hand Ostler a form as a task.

```sh
ostler a2a --allow-domain example.com          # http://127.0.0.1:8765
curl http://127.0.0.1:8765/.well-known/agent-card.json
```

Ostler is a **tool, not a language model**. It understands structured requests, not free text. It tells a client so (it answers chatter with a message that shows the expected shape).

## Sending work

Send `SendMessage` (or `SendStreamingMessage`) with a `data` part:

```json
{"url": "https://example.com/signup",
 "values": {"Email": "ada@example.com", "Country": "Germany"},
 "submit": true}
```

| field | meaning |
|---|---|
| `url` | page to open first. Omit to continue on the conversation's current page |
| `values` | `{label \| name \| ref: value}`, filled and verified by reading each field back |
| `auto` | `required` or `all`: fill the rest from the owner's saved profile (needs Qdrant memory) |
| `submit` | submit once everything is filled and verified |
| `confirm` | the caller agrees to the submission |

No `values`, no `auto`, no `submit`: it just **inspects** the form. A bare URL in a text part does the same.

## The task lifecycle

| state | when | what the client does |
|---|---|---|
| `TASK_STATE_COMPLETED` | inspected, filled, or submitted and accepted | read the artifacts |
| `TASK_STATE_INPUT_REQUIRED` | required fields missing, a key matched nothing or was ambiguous, the page rejected a value, a wizard advanced, or it is **waiting for `{"confirm": true}`** | reply on the same `taskId` with more `values` or `confirm` |
| `TASK_STATE_FAILED` | blocked by policy, timed out, or the page never accepted the submit | read the `result` artifact |
| `TASK_STATE_CANCELED` | `CancelTask` | |

**Nothing is submitted until the caller says `confirm: true`**, unless the server runs with `--a2a-auto-submit`. A remote agent can of course just send it; the point is that a human-in-the-loop client has a place to stop.

Each task has two artifacts: `result` (compact text) and `data` (the same information as JSON: `missing`, `unmatched`, `fieldErrors`, …). Both are marked `metadata.untrusted: true` because they contain text from the website. Treat them as data, never as instructions.

## Conversations share state

One A2A `contextId` is one Ostler session: its own Chrome, its own **tabs**, and its own shared **context** notes. Messages with the same `contextId` (or the same `taskId`) continue where the last one stopped, e.g. the form that was filled is still open when `{"confirm": true}` arrives. Different `contextId`s can never see each other's pages.

Each conversation is a Chrome process (about 400-650 MiB), so it is capped: `--max-contexts` (default 4), idle ones are closed after `--idle-secs` (default 300).

## Methods

`SendMessage`, `SendStreamingMessage`, `GetTask`, `ListTasks`, `CancelTask`, `SubscribeToTask`. `GetExtendedAgentCard` and the push-notification methods return the A2A errors for "not supported" (`-32007`, `-32003`). Errors use the A2A codes (`-32001` task not found, `-32002` not cancelable, `-32004` unsupported operation, `-32009` version not supported) with a `google.rpc.ErrorInfo` detail.

`A2A-Version` must be `1.x` if sent. A missing header is accepted (the spec says to assume 0.3, which this server does not implement, so being lenient here is a deliberate choice).

## Limits, timeouts and shutdown

The HTTP front door is bounded so a slow, greedy or abusive client cannot exhaust the machine. These are fields of `A2aConfig` (defaults shown). Only those with a flag shown have one today; the rest are library defaults until `ostler a2a` exposes them.

| Setting (`A2aConfig`) | Default | Behaviour |
| --- | --- | --- |
| `max_body_bytes` | 1 MiB | Larger request bodies get HTTP **413** (checked against `Content-Length`, and while reading chunked bodies). |
| `header_read_timeout` | 10 s | Time to send a request's headers; also the idle limit on a kept-alive connection. The connection is closed on expiry (slowloris). |
| `body_timeout` | 15 s | Time to send the body once headers are in. HTTP **408** on expiry. |
| `request_timeout` | 150 s | Ceiling on handling a request until its response starts (SSE streams continue afterwards). Raised to `task_timeout + 15 s` if lower, so a blocking `SendMessage` is never cut short. HTTP **408**. |
| `max_concurrent_tasks` | 8 | Tasks running, or queued behind their conversation's turn. Over the limit: HTTP **429**, `Retry-After: 5`, error reason `TOO_MANY_TASKS_IN_FLIGHT`. Nothing is queued without bound. |
| `max_contexts` (`--max-contexts`) | 4 | Concurrent conversations (one Chrome each). Over the limit: HTTP **503**, `Retry-After: 15`, reason `TOO_MANY_CONTEXTS`. |
| `task_timeout` (`--task-timeout-secs`) | 120 s | One task; it then fails with "The task timed out." |
| `idle` (`--idle-secs`) | 300 s | Idle conversations are closed. |
| `drain_timeout` | 30 s | See shutdown below. |

429/503 responses are JSON-RPC errors (code `-32603`) with `error.data[0].metadata.retryAfterSeconds`, plus the `Retry-After` header. Clients should back off and retry.

**Probes** (no auth, no `Host` check, body is only a status word; nothing about the host, paths or versions):

- `GET /healthz`: `200 {"status":"ok"}` while the process is serving.
- `GET /readyz`: `200 {"status":"ready"}` when the server is not shutting down and a Chrome binary can be found (`chrome_path`, else the usual discovery order); otherwise `503 {"status":"not_ready"}`. It checks that a browser can be located, not that a launch succeeds.

**Request ids.** Every response has an `X-Request-Id` header. A client-supplied `X-Request-Id` (up to 64 characters of `A-Za-z0-9-_.`) is echoed; otherwise a UUID is generated. Error bodies repeat it: `requestId` in the plain-JSON auth/host/timeout errors and `error.data[0].metadata.requestId` in JSON-RPC errors. Quote it when reporting a problem.

**Graceful shutdown.** On SIGTERM or SIGINT the server stops accepting connections, `/readyz` turns 503, new tasks are refused with 503 (`SHUTTING_DOWN`), and in-flight requests and tasks get up to `drain_timeout` to finish. Anything still running is then aborted, every browser is closed, and the process exits. Set your orchestrator's termination grace period above `drain_timeout`.

Not limited here: the number of open connections. Put a reverse proxy or firewall in front for connection-level limits.

## Securing it

An A2A endpoint lets a network client steer a browser that can log in to things. Defaults are conservative:

- Binds **127.0.0.1**. Binding anything else **requires `--a2a-token`** (or `OSTLER_A2A_TOKEN`); the server refuses to start otherwise.
- With a token, the RPC endpoint needs `Authorization: Bearer …` (compared in constant time). The Agent Card stays public, as the protocol expects, and advertises the scheme but never the token.
- Requests carrying an `Origin` header (a web page in a browser) are refused, and the `Host` header must be a loopback name or one you list with `--allowed-host`/`--public-url`. This blocks drive-by and DNS-rebinding attacks from a page open on the same machine.
- **Always set `--allow-domain`.** Without it, any remote agent can send this browser to any public website. The allowlist is enforced inside the browser for every conversation (every request type, WebSockets included). Without an allowlist the server blocks loopback, private and cloud-metadata addresses so a remote agent cannot probe this machine's network (`--allow-private-networks` turns that off); the server warns at startup. Uploads are refused unless you pass `--upload-dir`.
- TLS is not terminated here. Put it behind a reverse proxy for anything beyond localhost, and set `--public-url`.
- All conversations share one Qdrant `tenant`. If different callers must not see each other's saved profile, run one server per tenant.

## Verified against

The official A2A Python SDK (`a2a-sdk` 1.3.0) resolved the card, selected the JSON-RPC interface, streamed a three-turn conversation (input-required, input-required, completed) and received artifacts on every turn against the real binary. That was a manual check; the repository's own tests (`crates/ostler-a2a/tests`) use plain HTTP.

Not implemented: gRPC and REST bindings, push notifications, extended cards, signed cards, and the v0.3 wire format.
