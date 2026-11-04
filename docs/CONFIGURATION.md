# Configuration

Settings, memory keys and the config file reference. Back to the [README](../README.md).

## Configure

Settings come from, in order: **command-line flag, then environment variable, then the config file, then the default.** One path per setting, so what `ostler serve` does is what `ostler doctor` tested.

### Easy setup

```sh
ostler init
```

Asks for a Qdrant URL and API key (skip both for no memory), a tenant name, whether to enable `--learn` and `--lean`, and the privacy choices for memory: where embeddings are computed (Qdrant Cloud Inference, local Ollama or the non-semantic `dev-hash`) and how many days to keep learned skills and episodes (`retain_days`, default 180). Sealing (encrypting memory on this machine) is on by default. It writes the config file with owner-only permissions and immediately tests the connection. Non-interactive:

```sh
ostler init --qdrant-url https://<cluster>.cloud.qdrant.io --qdrant-api-key "$KEY" --tenant alice
# privacy flags: --seal auto|on|off, --embeddings cloud|ollama|dev-hash, --retain-days N, --yes (never prompt)
ostler init --embeddings ollama --seal on --retain-days 90 --yes
```

The `:6333` port is added to Qdrant Cloud URLs for you. If the connection test fails the settings are still saved, so you can fix one value instead of retyping everything:

```sh
ostler config show                          # key is masked
ostler config set qdrant_url http://localhost:6333
ostler config unset learn
ostler config path                          # ~/.config/ostler/config.toml
```

### Which keys do I need?

**Only Qdrant ones, and only for memory.** Everything else (browsing, form filling) needs no account and no key.

| you want | you need |
|---|---|
| browsing and form filling only | nothing |
| memory, trying it out | `--memory ephemeral` (in-process, lost on exit), no keys |
| memory, no server, kept across sessions | `ostler init --local` (or `--memory local`): one encrypted file next to the config, no keys, no network |
| memory on **Qdrant Cloud** | cluster URL + API key, with **Inference enabled** on the cluster (embeddings run inside Qdrant; no separate embedding provider or key) |
| memory on a self-hosted/open-source Qdrant | URL (+ key if you set one) and `--embeddings ollama` (a local Ollama server embeds; semantic; only vectors reach Qdrant) or `--embeddings dev-hash` (not semantic: exact-key and `autocomplete` matches work, fuzzy meaning does not) |

Qdrant Cloud Inference follows Qdrant's documentation and is covered by request-shape tests, but has **not** been run against a live Cloud cluster by the maintainers. The open-source path is tested against a real Qdrant server.

<details>
<summary><b>Config file reference</b></summary>

`~/.config/ostler/config.toml` (or `$XDG_CONFIG_HOME`, or the path in `OSTLER_CONFIG`):

| key | environment variable | meaning |
|---|---|---|
| `qdrant_url` | `QDRANT_URL` | Qdrant REST endpoint |
| `qdrant_api_key` | `QDRANT_API_KEY` | Qdrant API key (secret; file is `0600`) |
| `tenant` | `OSTLER_TENANT` | isolates users sharing a cluster (default `default`) |
| `memory` | `OSTLER_MEMORY` | `auto` (on if a URL is set), `qdrant`, `local` (encrypted file, no server), `ephemeral`, `off` |
| `embeddings` | `OSTLER_EMBEDDINGS` | `cloud` (Qdrant Cloud Inference), `ollama` (local) or `dev-hash` |
| `embedding_model` | `OSTLER_EMBEDDING_MODEL` | model name: Cloud Inference model (default `sentence-transformers/all-minilm-l6-v2`) or the Ollama name (e.g. `all-minilm`) |
| `embedding_dim` | `OSTLER_EMBEDDING_DIM` | vector size of the model (default 384) |
| `embedding_url` | `OSTLER_EMBEDDING_URL` | Ollama server (default `http://127.0.0.1:11434`) |
| `seal` | `OSTLER_SEAL` | `auto` (default: seal when memory is in Qdrant), `on`, `off` |
| `isolation` | `OSTLER_ISOLATION` | `shared` (default) or `collection` (one set of collections per tenant) |
| `allow_insecure_qdrant` | `OSTLER_ALLOW_INSECURE_QDRANT` | allow plain `http://` to a non-loopback Qdrant (refused by default) |
| `retain_days` | `OSTLER_RETAIN_DAYS` | on start, forget learned skills and episodes not verified for this many days (default 180; 0 = keep forever) |
| `qdrant_on_disk` | `OSTLER_QDRANT_ON_DISK` | keep vectors on disk: less RAM per user |
| `learn` | `OSTLER_LEARN` | save non-sensitive values typed after an accepted submit (off by default) |
| `lean` | `OSTLER_LEAN` | lower memory use; no images in screenshots |
| `key_store` | `OSTLER_KEY_STORE` | where the sealing key lives: `file` (default) or `keychain` |
| `verbose` | `OSTLER_VERBOSE` | timings, fingerprints and ids in tool output |
| `a2a_bind`, `a2a_token` | `OSTLER_A2A_BIND`, `OSTLER_A2A_TOKEN` | listen address and bearer token for `ostler a2a` |
| `tools` | `OSTLER_TOOLS` | `core`, `forms` or `all` |

The sealing key itself is never in the config file: it is `OSTLER_MEMORY_KEY` (base64, 32 bytes), or in the store chosen by `key_store` (`OSTLER_KEY_STORE`): `file` (default, an owner-only `memory.key` beside the config file) or `keychain` (macOS Keychain / Linux Secret Service). `ostler memory key export --yes-print` and `ostler memory key import` back it up and restore it.

</details>

Anything stored: [what is and is not saved](../SECURITY.md). Inspect or erase it any time: `ostler memory list | remember | forget | skills | prune | wipe`.
