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

## Securing it

An A2A endpoint lets a network client steer a browser that can log in to things. Defaults are conservative:

- Binds **127.0.0.1**. Binding anything else **requires `--a2a-token`** (or `OSTLER_A2A_TOKEN`); the server refuses to start otherwise.
- With a token, the RPC endpoint needs `Authorization: Bearer …` (compared in constant time). The Agent Card stays public, as the protocol expects, and advertises the scheme but never the token.
- Requests carrying an `Origin` header (a web page in a browser) are refused, and the `Host` header must be a loopback name or one you list with `--allowed-host`/`--public-url`. This blocks drive-by and DNS-rebinding attacks from a page open on the same machine.
- **Always set `--allow-domain`.** Without it, any remote agent can send this browser to any website. The server warns at startup. The allowlist is enforced inside the browser for every conversation.
- TLS is not terminated here. Put it behind a reverse proxy for anything beyond localhost, and set `--public-url`.
- All conversations share one Qdrant `tenant`. If different callers must not see each other's saved profile, run one server per tenant.

## Verified against

The official A2A Python SDK (`a2a-sdk` 1.3.0) resolved the card, selected the JSON-RPC interface, streamed a three-turn conversation (input-required, input-required, completed) and received artifacts on every turn against the real binary. That was a manual check; the repository's own tests (`crates/ostler-a2a/tests`) use plain HTTP.

Not implemented: gRPC and REST bindings, push notifications, extended cards, signed cards, and the v0.3 wire format.
