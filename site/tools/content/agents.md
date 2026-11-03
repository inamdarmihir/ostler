# Use with your agent

Ostler is an MCP server over stdio. Any MCP client can launch it. `ostler setup <client>` prints the exact config, using this binary's **absolute path**. That avoids "command not found" when an app's `PATH` differs from your shell's.

| client | command | result |
|---|---|---|
| Claude Code | `ostler setup claude --apply` | runs `claude mcp add` for you |
| Codex | `ostler setup codex` | prints the block for `~/.codex/config.toml` |
| Cursor | `ostler setup cursor` | prints JSON for `~/.cursor/mcp.json` |
| VS Code | `ostler setup vscode` | prints JSON for `.vscode/mcp.json` |
| anything else | `ostler setup json` | generic `mcpServers` JSON |

Only Claude Code is applied automatically. Other clients' files are never modified: you paste or merge the printed snippet. Extra `serve` flags go after `--`:

```sh
ostler setup claude --apply -- --lean --tools forms
```

> [!NOTE]
> Client config formats change between releases. If one of these does not load, check that client's current MCP documentation. The server itself only needs `ostler serve` on stdio.

## Claude Code

```sh
ostler setup claude --apply
# manual equivalent:
claude mcp add ostler -- ostler serve
```

Reconnect with `/mcp` after installing or rebuilding.

## Codex

```sh
ostler setup codex
```

Prints a block for `~/.codex/config.toml`. Paste or merge it:

```toml
[mcp_servers.ostler]
command = "/absolute/path/to/ostler"
args = ["serve"]
```

## Cursor

```sh
ostler setup cursor
```

Prints JSON for `~/.cursor/mcp.json` (or `.cursor/mcp.json` in a project):

```json
{
  "mcpServers": {
    "ostler": {
      "command": "/absolute/path/to/ostler",
      "args": ["serve"]
    }
  }
}
```

## VS Code

```sh
ostler setup vscode
```

Prints JSON for `.vscode/mcp.json`. VS Code uses a `servers` key and an explicit transport type:

```json
{
  "servers": {
    "ostler": {
      "type": "stdio",
      "command": "/absolute/path/to/ostler",
      "args": ["serve"]
    }
  }
}
```

## Any other MCP client

```sh
ostler setup json
```

prints the common `mcpServers` shape. The only requirement is that the client starts `ostler serve` and speaks MCP over stdio. `ostler serve` should print one `ready` line on stderr and wait; stdout carries only MCP messages.

## A good agent workflow

1. `navigate` returns the page's interactive elements.
2. `form_inspect` returns typed fields and shows what memory knows about this form.
3. `form_fill` with `{label: value}` for what you know; add `auto: "required"` to fill the rest from memory.
4. `form_submit`, then read the status: *Submitted*, *Advanced* (next step: repeat 2 to 4), or *NOT ACCEPTED* with the page's validation messages.

`ostler inspect <url>` prints the form model as JSON for debugging.

## Control what the agent sees and can do

| flag | effect |
|---|---|
| `--tools core\|forms\|all` | which tools the agent sees (12, 16 or 21). Fewer tools means less schema in every turn |
| `--allow-domain example.com` (repeatable) | only that domain and subdomains, every request and WebSockets included; `example.com:8443` pins a port |
| `--require-confirm-submit` | `form_submit` fails unless the agent passes `confirm=true`, so a human can approve |
| `--upload-dir DIR` (repeatable) | file fields may be filled from this directory; without it uploads are refused |
| `--block-private-networks` | with no allowlist, refuse loopback, private, metadata and internal hosts |
| `--accept-dialogs` | answer the page's `confirm()` and `prompt()` with OK; they are dismissed by default |
| `--lean` | lower memory; no images, fonts or media |
| `--profile-dir DIR` | persistent Chrome profile (keeps logins); one profile per server |
| `--headed` | visible browser, for watching it work and for `wait_for_human` |
| `--learn` | save non-sensitive values typed after an accepted submit (off by default) |

The full flag list is on the [Configuration and CLI](configuration.html) page, and the reasoning is in the [Security model](security.html).

## Tabs share one browser context

Cookies and logins carry across tabs. The `context` tool is a small key/value notepad that every tab and every later call can read, for things like "applicant = Ada, order id = 4471". It survives a browser restart. Popups arrive as tabs under the same domain policy.

## Other agents: A2A

If another agent, rather than a coding agent, should hand Ostler a form, use [A2A](a2a.html) instead of MCP.
