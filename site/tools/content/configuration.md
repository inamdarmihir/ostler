# Configuration and CLI

Settings come from, in order: **command-line flag, then environment variable, then the config file, then the default.** One path per setting, so what `ostler serve` does is what `ostler doctor` tested.

## Easy setup

```sh
ostler init
```

Asks for a Qdrant URL and API key (skip both for no memory), a tenant name, and whether to enable `--learn` and `--lean`. It writes the config file with owner-only permissions and immediately tests the connection. Non-interactively:

```sh
ostler init --qdrant-url https://<cluster>.cloud.qdrant.io --qdrant-api-key "$KEY" --tenant alice
```

The `:6333` port is added to Qdrant Cloud URLs for you. If the connection test fails the settings are still saved, so you can fix one value instead of retyping everything:

```sh
ostler config show                          # key is masked
ostler config set qdrant_url http://localhost:6333
ostler config unset learn
ostler config path                          # ~/.config/ostler/config.toml
```

## Config file reference

`~/.config/ostler/config.toml` (or `$XDG_CONFIG_HOME`, or the path in `OSTLER_CONFIG`):

| key | environment variable | meaning |
|---|---|---|
| `qdrant_url` | `QDRANT_URL` | Qdrant REST endpoint |
| `qdrant_api_key` | `QDRANT_API_KEY` | Qdrant API key (secret; file is `0600`) |
| `tenant` | `OSTLER_TENANT` | isolates users sharing a cluster (default `default`) |
| `memory` | `OSTLER_MEMORY` | `auto` (on if a URL is set), `qdrant`, `ephemeral`, `off` |
| `embeddings` | `OSTLER_EMBEDDINGS` | `cloud` (Qdrant Cloud Inference), `ollama` (local) or `dev-hash` |
| `embedding_model` | `OSTLER_EMBEDDING_MODEL` | Cloud Inference model (default `sentence-transformers/all-minilm-l6-v2`, 384 dims) |
| `qdrant_on_disk` | `OSTLER_QDRANT_ON_DISK` | keep vectors on disk: less RAM per user |
| `learn` | `OSTLER_LEARN` | save non-sensitive values typed after an accepted submit (off by default) |
| `lean` | `OSTLER_LEAN` | lower memory use; no images in screenshots |
| `tools` | `OSTLER_TOOLS` | `core`, `forms` or `all` |

Other environment variables used by the code: `OSTLER_SEAL`, `OSTLER_ISOLATION`, `OSTLER_MEMORY_KEY`, `OSTLER_EMBEDDING_URL`, `OSTLER_EMBEDDING_DIM`, `OSTLER_ALLOW_INSECURE_QDRANT`, `OSTLER_RETAIN_DAYS`, `OSTLER_KEY_STORE`, `OSTLER_ALLOW_NEW_KEY` (same as `--allow-new-key`: let a first start create the sealing key while Qdrant cannot be checked), `OSTLER_VERBOSE`, `OSTLER_CHROME`, `OSTLER_NO_SANDBOX`, `OSTLER_A2A_BIND` and `OSTLER_A2A_TOKEN`. The seal, isolation, embedding-URL, insecure-Qdrant and retention settings are :unreleased:.

## Commands

```text
ostler <COMMAND>

  serve      Run the MCP server on stdio (what the agent launches)
  a2a        Serve Agent2Agent (A2A) over HTTP so other agents can delegate forms
  init       Guided setup: Qdrant URL/key, tenant, privacy choices (sealing, local embeddings, retention); then verify
  config     show | path | set <key> <value> | unset <key>
  setup      Print or apply the config for claude | codex | cursor | vscode | json
  doctor     Check that Chrome launches, and report memory: Qdrant reachable, sealing, key, embeddings, isolation, transport
  inspect    Open a URL and print the typed form model as JSON
  memory     list | skills | prune --days N | remember <key> <value> | forget <key> | seal | key export/import | token | wipe --yes
  fixtures   Serve the demo form pages (default port 8787)
```

`ostler memory` also has `seal` and `token` :unreleased:, described in [Memory and privacy](memory.html). Memory flags go before the subcommand: `ostler memory --memory ephemeral list`.

## `ostler serve` flags

| flag | default | description |
|---|---|---|
| `--tools` | `auto` | `auto` lists the core tools and adds forms and memory when a page has form fields; or `core`, `forms`, `all` |
| `--lean` | off | lean Chrome flags; no image, font or media downloads |
| `--headed` | off | show the browser window |
| `--profile-dir DIR` | throwaway | persistent Chrome profile (keeps logins); one profile per server |
| `--chrome-path PATH` | auto | Chrome binary (`OSTLER_CHROME`) |
| `--allow-domain D` | any | only this domain and subdomains; repeatable; `D:8443` pins a port |
| `--allow-file` | off | permit `file://` URLs |
| `--upload-dir DIR` | none (uploads refused) | directory file fields may be filled from; repeatable |
| `--block-private-networks` | off (`a2a`: on) | with no allowlist, refuse loopback, private, metadata and internal hosts (`a2a --allow-private-networks` opts out) |
| `--accept-dialogs` | off | answer the page's `confirm()` and `prompt()` with OK (dismissed by default) |
| `--require-confirm-submit` | off | `form_submit` needs `confirm=true` |
| `--learn` | off | save non-sensitive typed values after an accepted submit |
| `--verbose` | off | timings, fingerprints and ids in tool output (costs context) |
| `--memory` | `auto` | `auto`, `qdrant`, `ephemeral`, `off` |
| `--qdrant-url`, `--qdrant-api-key` | none | Qdrant endpoint and key |
| `--embeddings` | `cloud` | `cloud`, `ollama` (embed locally, only vectors leave) or `dev-hash` |
| `--embedding-model`, `--embedding-dim` | MiniLM, 384 | embedding model and its dimension |
| `--embedding-url` | `http://127.0.0.1:11434` | where the Ollama embedding server lives :unreleased: |
| `--qdrant-on-disk` | off | vectors on disk |
| `--seal` | `auto` | encrypt memory on this machine before it reaches Qdrant (`on`, `off`) :unreleased: |
| `--isolation` | `shared` | `collection`: one set of collections per tenant, pairs with `ostler memory token` :unreleased: |
| `--allow-insecure-qdrant` | off | allow plain http to a non-loopback Qdrant :unreleased: |
| `--retain-days` | 180 | forget unverified skills and episodes on start (0 = keep) :unreleased: |
| `--tenant` | `default` | memory namespace |

Set `OSTLER_NO_SANDBOX=1` to run Chrome without its sandbox (needed as root or in some containers). See [Troubleshooting](troubleshooting.html).

## `ostler a2a` flags

`ostler a2a` takes the same flags as `serve`, plus:

| flag | default | description |
|---|---|---|
| `--bind` | `127.0.0.1:8765` | address to listen on. Anything other than loopback requires `--a2a-token` |
| `--a2a-token` | none | bearer token clients must send (`OSTLER_A2A_TOKEN`) |
| `--public-url` | none | externally visible base URL for the Agent Card |
| `--allowed-host` | none | additional accepted `Host` header values; repeatable |
| `--max-contexts` | 4 | concurrent remote conversations; each one is a Chrome process |
| `--idle-secs` | 300 | close a conversation's browser after this many idle seconds |
| `--a2a-auto-submit` | off | treat `submit: true` as already confirmed |

See [A2A](a2a.html) for the protocol and its security notes.
