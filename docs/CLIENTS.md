# Using Ostler from an agent

Ostler is an MCP server over stdio. Any MCP client can launch it. Verify your setup first:

```sh
ostler doctor
```

## Install

```sh
# from source (needs Rust 1.82+ and Chrome/Chromium)
cargo install --git https://github.com/inamdarmihir/ostler ostler-cli
```

## Easiest: `ostler setup`

```sh
ostler setup claude --apply     # runs `claude mcp add` for you
ostler setup codex              # prints the ~/.codex/config.toml block
ostler setup cursor             # prints ~/.cursor/mcp.json
ostler setup vscode             # prints .vscode/mcp.json
ostler setup json               # generic mcpServers JSON
ostler setup claude --apply -- --lean --tools forms   # extra serve flags after --
```

It uses this binary's absolute path, so it works even when the client's `PATH` differs from your shell's. Only `claude` is applied automatically; other config files are never modified. Manual equivalents:

- Claude Code: `claude mcp add ostler -- ostler serve`
- Codex: `[mcp_servers.ostler]` with `command = "ostler"`, `args = ["serve"]`
- Cursor / others: `{ "mcpServers": { "ostler": { "command": "ostler", "args": ["serve"] } } }`

> Client config formats change between releases. If one of these does not load, check that client's current MCP documentation; the server itself only needs `ostler serve` on stdio.

## Memory with Qdrant

```sh
ostler init          # prompts for URL + API key, saves them privately, tests the connection
ostler doctor
ostler setup claude --apply -- --learn
```

Or non-interactively: `export QDRANT_URL=… QDRANT_API_KEY=…` (these override the saved config). See the README's [Configure](../README.md#configure) section for every setting and where it comes from.

- Embeddings default to **Qdrant Cloud Inference** (`sentence-transformers/all-minilm-l6-v2`, 384 dims). Enable Inference on the cluster in the Qdrant Cloud console. This path follows Qdrant's documentation and is covered by request-shape tests, but has not been exercised against a live cluster by the maintainers; please report problems.
- Against open-source Qdrant (no inference): `--embeddings dev-hash` uses non-semantic client-side vectors. Exact lookups and `autocomplete` matching work; semantic matching is poor. For development only.
- `--memory ephemeral` keeps memory in-process (lost on exit) for trying things out.
- `--tenant` (or `OSTLER_TENANT`) separates users sharing a cluster. `--qdrant-on-disk` lowers RAM per user.
- `--learn` saves non-sensitive values the agent typed after a form is accepted. Off by default: Ostler records *how* forms were filled automatically, but only stores *your data* when you opt in or call `memory_remember`.

```sh
ostler memory list
ostler memory remember email you@example.com
ostler memory forget email
ostler memory wipe --yes
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

`ostler inspect <url>` prints the form model as JSON for debugging. `ostler fixtures` serves the test pages for a local demo.
