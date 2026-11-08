# Connect Ostler to your agent

`ostler setup <client>` prints the exact configuration for your machine, with the full path to the binary. The blocks below are that output with the path replaced by `ostler`. Full guide: [docs/CLIENTS.md](https://github.com/inamdarmihir/ostler/blob/main/docs/CLIENTS.md).

| client | command | what it does |
|---|---|---|
| Claude Code | `ostler setup claude --apply` | runs `claude mcp add` for you |
| Codex | `ostler setup codex` | prints a block for `~/.codex/config.toml` |
| Cursor | `ostler setup cursor` | prints JSON for `~/.cursor/mcp.json` |
| VS Code | `ostler setup vscode` | prints JSON for `.vscode/mcp.json` |
| anything else | `ostler setup json` | a generic `mcpServers` block |
| every app found on this computer | `ostler start` | connects each, with a backup of its config |

Only Claude Code is changed automatically; other clients' files are never modified by `setup`. Restart the client afterwards.

## Claude Code

```sh
claude mcp add ostler -- ostler serve
```

## Codex (`~/.codex/config.toml`)

```toml
[mcp_servers.ostler]
command = "ostler"
args = ["serve"]
```

## Cursor (`~/.cursor/mcp.json`) and generic clients

```json
{
  "mcpServers": {
    "ostler": { "command": "ostler", "args": ["serve"] }
  }
}
```

## VS Code (`.vscode/mcp.json`)

```json
{
  "servers": {
    "ostler": { "type": "stdio", "command": "ostler", "args": ["serve"] }
  }
}
```

## Safer defaults for real sites

Add flags to `args` for the job at hand:

| flag | use it when |
|---|---|
| `--allow-domain example.com` | always on real work: the browser can reach only the sites you list |
| `--require-confirm-submit` | the task submits anything with consequences (example 10) |
| `--profile-dir ~/.ostler/profile` | the site needs your login; sign in once with `ostler login <url>` |
| `--headed` | you may need to clear a CAPTCHA or 2FA in the window (`wait_for_human`) |
| `--tools research` / `--tools forms` | you want a smaller tool list for one kind of task |
