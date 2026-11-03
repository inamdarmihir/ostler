# Using Ostler from an agent

Ostler is an MCP server over stdio. Any MCP client can launch it. Verify your setup first:

```sh
ostler doctor        # every failing check prints the command that fixes it
ostler doctor --fix  # apply the safe ones: create the config directory, chmod 600 an open key file
```

## Install

You need Chrome or Chromium, plus one of:

```sh
cargo install --git https://github.com/inamdarmihir/ostler ostler   # from source (Rust 1.89+); works today
curl -fsSL https://inamdarmihir.github.io/ostler/install.sh | sh     # prebuilt binary, from the first release
claude mcp add ostler -- npx -y ostler serve                         # no install step: npx downloads it on first run (first release)
```

See the [README](../README.md#install) for the status of each and [RELEASING](RELEASING.md) for how they get published.

## Easiest: `ostler install`

```sh
ostler install --list                      # every client, its config path, whether Ostler is registered
ostler install --client claude-code        # claude-code | claude-desktop | cursor | vscode | windsurf | codex
ostler install --client cursor --dry-run   # show the change, write nothing
ostler install --client cursor --uninstall # remove only the ostler entry
ostler install --client codex -- --lean    # extra `serve` flags after --
```

It edits the client's own config file: the file is parsed first and **refused if it is not valid JSON/TOML** (nothing is written), a timestamped backup (`<file>.ostler-backup-<unix time>`) is made before any change, your other servers are kept (Codex's TOML is edited as text, so comments survive), and it prints the before/after of the `ostler` entry. Running it again changes nothing. It uses this binary's absolute path. Files: `~/.claude.json`, `~/.cursor/mcp.json`, `~/.codeium/windsurf/mcp_config.json`, `~/.codex/config.toml`, Claude Desktop's `claude_desktop_config.json` (macOS and Windows only; Linux has no official location, so it says so instead of guessing) and VS Code's user `mcp.json`. `OSTLER_INSTALL_HOME` replaces the home directory (for tests and sandboxes). It never touches the network, and VS Code files with comments are refused rather than rewritten.

## Zero config

With nothing configured, `ostler serve` needs no Qdrant, key or flags: memory is in-process for the session (nothing is written to disk) and Chrome is launched headless with a throwaway profile. On start it prints what is on and off:

```text
ostler 0.2.0 ready (MCP over stdio)
  browser: own headless Chrome, throwaway profile (--profile-dir keeps logins, --cdp-url attaches to yours)
  memory:  in-process, this session only (nothing written to disk)
  off: Qdrant, sealed storage, learning from forms. Opt in with `ostler init`.
```

Qdrant, sealing and `--learn` stay explicit opt-ins (`ostler init`). `--memory off` turns memory off entirely.

## Use the Chrome you already have

Ostler can drive a Chrome you started yourself, so you keep your own window and sessions:

1. Start Chrome with a debugging port on loopback and a dedicated profile (Chrome ignores the flag on its default profile; you start this, Ostler never opens the port for you):
   - macOS: `"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --remote-debugging-port=9222 --remote-debugging-address=127.0.0.1 --user-data-dir=$HOME/.ostler/chrome-profile`
   - Linux: `google-chrome --remote-debugging-port=9222 --remote-debugging-address=127.0.0.1 --user-data-dir=$HOME/.ostler/chrome-profile`
2. Check it: `ostler doctor --cdp-url http://127.0.0.1:9222` (opens one blank tab, closes it).
3. Register it: `ostler install --client claude-code -- --cdp-url http://127.0.0.1:9222`.

The one-prompt flow: tell your agent "start Chrome with the command above, then run `ostler doctor --cdp-url http://127.0.0.1:9222`", then install as in step 3.

Safety: only loopback URLs are accepted (a DevTools port has no authentication: anything that can reach it controls the browser, including every logged-in site), the URL is checked when `serve` starts, Ostler works in a tab of its own and closes only that tab on exit, and your browser keeps running. While the port is open, any local program can drive that Chrome, so use a dedicated profile and close it when done. `--cdp-url` conflicts with `--profile-dir`, is refused in A2A mode, and `--allow-domain` then relies on request interception without Chrome's DNS pinning.

## Manual: `ostler setup`

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

After installing or rebuilding `ostler`, reconnect the MCP client (Claude Code: `/mcp`; others: restart it) so it starts the new binary.

> Client config formats change between releases. If one of these does not load, check that client's current MCP documentation; the server itself only needs `ostler serve` on stdio.

## Memory with Qdrant

```sh
ostler init          # prompts for URL + API key, saves them privately, tests the connection
ostler doctor
ostler setup claude --apply -- --learn
```

Or non-interactively: `export QDRANT_URL=… QDRANT_API_KEY=…` (these override the saved config). See the README's [Configure](../README.md#configure) section for every setting and where it comes from.

- Embeddings default to **Qdrant Cloud Inference** (`sentence-transformers/all-minilm-l6-v2`, 384 dims). Enable Inference on the cluster in the Qdrant Cloud console. This path follows Qdrant's documentation and is covered by request-shape tests, but has not been exercised against a live cluster by the maintainers; please report problems.
- Against open-source Qdrant (no inference): `--embeddings ollama` embeds on your machine with a local Ollama server (semantic; only vectors reach Qdrant; `--embedding-model`/`--embedding-dim` must match the model, e.g. `all-minilm`, 384), or `--embeddings dev-hash` uses non-semantic client-side vectors (exact lookups and `autocomplete` matching work; semantic matching is poor; development only). With the default `cloud`, field text is sent to Qdrant Cloud Inference.
- Memory in Qdrant is encrypted on your machine by default (`--seal auto`); back up the sealing key, because lost keys cannot be recovered. See the README's [privacy and security model](../README.md#privacy-and-security-model-of-memory) and [SECURITY.md](../SECURITY.md). `--isolation collection` and `ostler memory token` give per-tenant Qdrant access.
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
| `--allow-domain example.com` (repeatable) | only that domain and subdomains, every request and WebSockets included; `example.com:8443` pins a port |
| `--upload-dir DIR` (repeatable) | file fields may be filled from this directory; without it uploads are refused |
| `--block-private-networks` | with no allowlist, refuse loopback, private, metadata and internal hosts (on by default for `a2a`) |
| `--accept-dialogs` | answer the page's `confirm()` and `prompt()` with OK; they are dismissed by default |
| `--require-confirm-submit` | `form_submit` fails unless the agent passes `confirm=true`, so a human can approve |
| `--tools auto\|core\|forms\|all` | which tools are listed; `auto` (default) lists the 14 core tools and adds forms and memory once a page has form fields |
| `--lean` | lower memory; no images/fonts/media |
| `--profile-dir DIR` | persistent Chrome profile (keeps logins) |
| `--headed` | visible browser, for watching it work |

## A good agent workflow

1. `navigate` (returns the page's interactive elements)
2. `form_inspect` (typed fields; shows what memory knows about this form)
3. `form_fill` with `{label: value}` for what you know; add `auto: "required"` to fill the rest from memory
4. `form_submit`, then read the status: *Submitted*, *Advanced* (next step: repeat 2-4), or *NOT ACCEPTED* with the page's validation messages.

`ostler inspect <url>` prints the form model as JSON for debugging. `ostler fixtures` serves the test pages for a local demo.
