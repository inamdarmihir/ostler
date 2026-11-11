# CLI reference

Commands, `ostler serve` flags, and the A2A server. Back to the [README](../README.md).

## CLI reference

```text
ostler <COMMAND>

  serve      Run the MCP server on stdio (what the agent launches)
  a2a        Serve Agent2Agent (A2A) over HTTP so other agents can delegate forms
  init       Guided setup: Qdrant URL/key, tenant, privacy choices (sealing, local embeddings, retention); then verify
  config     show | path | set <key> <value> | unset <key>
  setup      Print or apply the config for claude | codex | cursor | vscode | json
  agents     Add Ostler subagents (researcher, browser, form-filler) to Claude Code or Codex
  doctor     Check that Chrome launches, and report memory: Qdrant reachable, sealing, key, embeddings, transport
  inspect    Open a URL and print the typed form model as JSON
  memory     list | skills | prune --days N | remember <key> <value> | forget <key> | seal | key export/import | wipe --yes
  fixtures   Serve the demo form pages (default port 8787)
```

<details>
<summary><b><code>ostler serve</code> flags</b></summary>

| flag | default | description |
|---|---|---|
| `--tools` | `all` | `core`, `forms` or `all` |
| `--lean` | off | lean Chrome flags; no image/font/media downloads |
| `--headed` | off | show the browser window |
| `--profile-dir DIR` | throwaway | persistent Chrome profile (keeps logins) |
| `--chrome-path PATH` | auto | Chrome binary (`OSTLER_CHROME`) |
| `--allow-domain D` | any | only this domain and subdomains; repeatable; `D:8443` pins a port |
| `--allow-file` | off | permit `file://` URLs |
| `--upload-dir DIR` | none (uploads refused) | directory file fields may be filled from; repeatable |
| `--block-private-networks` | off (`a2a`: on) | with no allowlist, refuse loopback, private, metadata and internal hosts (`a2a --allow-private-networks` opts out) |
| `--accept-dialogs` | off | answer the page's `confirm()` and `prompt()` with OK (dismissed by default) |
| `--require-confirm-submit` | off | `form_submit` needs `confirm=true` |
| `--learn` | off | save non-sensitive typed values after an accepted submit |
| `--verbose` | off | timings, fingerprints and ids in tool output (costs context) |
| `--memory` | `auto` | `auto`, `qdrant`, `local`, `ephemeral`, `off` |
| `--qdrant-url`, `--qdrant-api-key` | none | Qdrant endpoint and key |
| `--embedding-model`, `--embedding-dim` | MiniLM, 384 | embedding model and its dimension |
| `--embedding-url` | `http://127.0.0.1:11434` | where the Ollama server lives (`--embeddings ollama`) |
| `--seal` | `auto` | encrypt memory on this machine before it reaches Qdrant (`on`, `off`) |
| `--embeddings` | `cloud` | `cloud` (field text goes to Qdrant Cloud Inference), `ollama` (embed locally, only vectors leave), `dev-hash` (local, not semantic) |
| `--allow-insecure-qdrant` | off | allow plain http to a non-loopback Qdrant |
| `--retain-days` | 180 | forget learned skills and episodes not verified for this many days on start (0 = keep; saved facts never expire) |
| `--tenant` | `default` | memory namespace |

</details>

Set `OSTLER_NO_SANDBOX=1` to run Chrome without its sandbox (needed as root or in some containers). See [Troubleshooting](TROUBLESHOOTING.md).

## A2A: let other agents hand it forms

```sh
ostler a2a --allow-domain example.com
curl http://127.0.0.1:8765/.well-known/agent-card.json
```

Besides MCP, Ostler speaks [Agent2Agent](https://a2a-protocol.org) v1.0. Another agent sends a task (`{"url": …, "values": {…}, "submit": true}`), Ostler fills and verifies the form, and answers `input-required` when it needs something (a missing field, a rejected value, or your **confirm** before anything is submitted). Each conversation gets its own browser and tabs. It binds to loopback, requires a bearer token to listen anywhere else, and refuses requests from web pages. Checked against the official A2A Python SDK. Details, security notes and limits: [docs/A2A.md](A2A.md).
