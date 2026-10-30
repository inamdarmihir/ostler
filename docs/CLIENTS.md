# Using Waymarker from an agent

Waymarker is an MCP server over stdio. Any MCP client can launch it. Verify your setup first:

```sh
waymarker doctor
```

## Install

```sh
# from source (needs Rust 1.80+ and Chrome/Chromium)
cargo install --git https://github.com/inamdarmihir/waymarker waymarker-cli
```

## Claude Code

```sh
claude mcp add waymarker -- waymarker serve
# smaller context, forms only:
claude mcp add waymarker -- waymarker serve --tools forms
```

## Codex (`~/.codex/config.toml`)

```toml
[mcp_servers.waymarker]
command = "waymarker"
args = ["serve"]
```

## Cursor / VS Code / others (JSON)

```json
{ "mcpServers": { "waymarker": { "command": "waymarker", "args": ["serve"] } } }
```

> Client config formats change between releases. If one of these does not load, check that client's current MCP documentation; the server itself only needs `waymarker serve` on stdio.

## Memory with Qdrant

```sh
export QDRANT_URL="https://<your-cluster>.cloud.qdrant.io:6333"
export QDRANT_API_KEY="…"
waymarker doctor                        # creates collections, checks connectivity
claude mcp add waymarker -- waymarker serve --learn
```

- Embeddings default to **Qdrant Cloud Inference** (`sentence-transformers/all-minilm-l6-v2`, 384 dims). Enable Inference on the cluster in the Qdrant Cloud console. This path follows Qdrant's documentation and is covered by request-shape tests, but has not been exercised against a live cluster by the maintainers; please report problems.
- Against open-source Qdrant (no inference): `--embeddings dev-hash` uses non-semantic client-side vectors. Exact lookups and `autocomplete` matching work; semantic matching is poor. For development only.
- `--memory ephemeral` keeps memory in-process (lost on exit) for trying things out.
- `--tenant` (or `WAYMARKER_TENANT`) separates users sharing a cluster. `--qdrant-on-disk` lowers RAM per user.
- `--learn` saves non-sensitive values the agent typed after a form is accepted. Off by default: Waymarker records *how* forms were filled automatically, but only stores *your data* when you opt in or call `memory_remember`.

```sh
waymarker memory list
waymarker memory remember email you@example.com
waymarker memory forget email
waymarker memory wipe --yes
```

## Safety flags

| flag | effect |
|---|---|
| `--allow-domain example.com` (repeatable) | only that domain and subdomains; checked after redirects and clicks |
| `--require-confirm-submit` | `form_submit` fails unless the agent passes `confirm=true`, so a human can approve |
| `--tools core\|forms\|all` | which tools the agent sees |
| `--lean` | lower memory; no images/fonts/media |
| `--profile-dir DIR` | persistent Chrome profile (keeps logins) |
| `--headed` | visible browser, for watching it work |

## A good agent workflow

1. `navigate` (returns the page's interactive elements)
2. `form_inspect` (typed fields; shows what memory knows about this form)
3. `form_fill` with `{label: value}` for what you know; add `auto: "required"` to fill the rest from memory
4. `form_submit`, then read the status: *Submitted*, *Advanced* (next step: repeat 2-4), or *NOT SUBMITTED* with the page's validation messages.

`waymarker inspect <url>` prints the form model as JSON for debugging. `waymarker fixtures` serves the test pages for a local demo.
