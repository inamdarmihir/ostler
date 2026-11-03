# Memory and privacy

Memory is optional. Browsing and form filling need no account and no key. When you turn it on, Ostler saves facts you told it, which fact fills which field on which site, and what went wrong on a form, in your own [Qdrant](https://qdrant.tech).

This page is exact about what is stored, what is encrypted, and what is **not** hidden. The sealing, local-embedding and per-tenant-token features below are described in the changelog's *Unreleased* section: they are on `main` and not in a tagged release yet. :unreleased:

## What is stored

Three Qdrant collections, multi-tenant through a `tenant` payload index:

| collection | holds | never holds |
|---|---|---|
| `ostler_profile` | facts you saved (`email`, `company`, …) | passwords, card numbers, anything that looks sensitive |
| `ostler_playbooks` | per site and form: which fact fills which field, with wins, losses and last verified time | the values themselves |
| `ostler_episodes` | what went wrong on a form, so it is not repeated | secrets |

Ostler sends no telemetry. Disk holds nothing by default, apart from a Chrome profile if you pass `--profile-dir`, and the config and key files described below.

Secrets are refused by the memory API (password, token and card-like keys, and card-number-like values), sensitive fields are never auto-filled from memory nor stored by `--learn`, and sensitive values are masked in tool output. Detection is heuristic (names, `autocomplete` tokens, digit patterns): a secret stored under an innocuous key with an innocuous value will not be caught. Learned values never overwrite facts you saved.

`--learn` is off by default. Ostler records *how* forms were filled automatically (structure only), but stores *your data* only when you opt in with `--learn` or call `memory_remember`.

## Sealing: encrypt before it reaches Qdrant

With memory in Qdrant, payloads are **sealed on your machine by default** (`--seal auto`). :unreleased:

- Fact values, descriptions, playbooks and episodes are encrypted with **AES-256-GCM**, with the point id bound in as associated data.
- Site names, field labels, profile keys and fingerprints are replaced by **keyed hashes (HMAC-SHA-256)**, and point ids are keyed too, so filters still work but the server cannot test guesses ("passport number") against what it stores.
- Episodes are **redacted before** they are embedded or stored: emails, runs of four or more digits (phones, PIN codes, card numbers) and URL query strings are masked, in addition to the values the agent typed.
- The key lives in `OSTLER_MEMORY_KEY` or an owner-only `memory.key` next to the config file, created atomically on the first start. That start only creates a key after Qdrant was reached and holds no sealed memory for your tenant; with Qdrant down it refuses (start Qdrant, restore a backup with `ostler memory key import`, or pass `--allow-new-key` for a genuine first-time setup). **Back it up: there is no recovery.** If you lose it, sealed memory is unreadable.
- `--seal on` forces it; `--seal off` opts out (and `ostler doctor` and `serve` warn that anyone who can read your Qdrant can then read your saved facts and the sites you use).

### What is hidden, and what is not

| | someone who can read your Qdrant (operator, leaked key, backup) sees |
|---|---|
| Fact values and descriptions | encrypted |
| Playbooks and episodes (contents) | encrypted; episodes are also redacted of emails and numbers |
| Site names, field labels, profile keys, fingerprints | keyed hashes only |
| **The vector** | **visible.** With `--embeddings cloud`, the field text is sent to Qdrant Cloud Inference to be embedded, and vectors of labels reveal roughly what they say. Use `--embeddings ollama` to keep text local |
| **Tenant name** | visible |
| **Win and loss counters, timing, point counts** | visible |

The key is, in order of precedence, the `OSTLER_MEMORY_KEY` variable, or the store chosen by `key_store` (`OSTLER_KEY_STORE`): `file` (default, owner-only `memory.key`) or `keychain` (macOS Keychain / Linux Secret Service). :unreleased: `ostler memory key export --yes-print` and `ostler memory key import` back it up and restore it.

### Memory written before sealing

Memory saved before sealing was on is invisible until you run:

```sh
ostler config set seal on
ostler memory seal
```

Saved facts are copied into sealed form and read back to check them before the old plaintext points are deleted, so it is safe to interrupt and run again. **Learned skills and episodes from that time are dropped** and relearned.

## Local embeddings

By default Ostler embeds text with Qdrant Cloud Inference (`sentence-transformers/all-minilm-l6-v2`, 384 dimensions). To keep the text on your machine, embed locally with Ollama: :unreleased:

```sh
ostler serve --embeddings ollama --embedding-model <model> --embedding-dim <dims>
```

| `--embeddings` | where text is embedded | notes |
|---|---|---|
| `cloud` (default) | inside Qdrant Cloud Inference | needs Inference enabled on the cluster. Field text is sent to Qdrant. Covered by request-shape tests; **not run against a live Cloud cluster** by the maintainers |
| `ollama` | on your machine, through an Ollama server | only vectors reach Qdrant. `--embedding-url` defaults to `http://127.0.0.1:11434` (`OSTLER_EMBEDDING_URL`); set `--embedding-model` and `--embedding-dim` to match the model. Remote plain-`http` embedders are refused |
| `dev-hash` | on your machine, deterministic hash | **not semantic.** Exact-key and `autocomplete` lookups work; fuzzy meaning does not. For open-source Qdrant development |

Exact signals need no vectors: lookups by playbook mapping and `autocomplete` token use payload filters.

## Transport

Plain `http://` to a non-loopback Qdrant is **refused**, because the API key and facts would cross the network unencrypted. `--allow-insecure-qdrant` (`OSTLER_ALLOW_INSECURE_QDRANT`) overrides it for links already protected by a VPN or tunnel. :unreleased:

If Qdrant fails twice in a row (transport error or 5xx), lookups are skipped for 30 seconds and tool results start with `Note: memory is unavailable (...); continuing without it.` Requests time out after 4 seconds.

## Per-tenant access tokens

By default all tenants share three collections, separated by a `tenant` filter in every query. A leaked **admin key** exposes every tenant in that mode. For stronger separation: :unreleased:

- `--isolation collection` gives each tenant its own collections.
- `ostler memory token` mints a **JWT that Qdrant itself limits** to those collections, read-write or `--read-only`, and expiring. A leaked client token then exposes one tenant, not the cluster.
- Qdrant must run with `service.jwt_rbac: true`, and an admin must create the collections first.

```sh
# 1. as admin: create alice's collections (`ostler memory list` does it)
export QDRANT_URL=https://<cluster>.cloud.qdrant.io:6333 QDRANT_API_KEY=<admin key>
export OSTLER_ISOLATION=collection OSTLER_TENANT=alice
ostler memory list

# 2. mint a token for alice (30 days by default; add --read-only for search-only)
ostler memory token --days 30

# 3. give alice the token instead of the admin key
#    QDRANT_API_KEY=<token>  with  --isolation collection --tenant alice
```

Tested on a real Qdrant 1.20: another tenant's collections answer 403, and expired, forged and read-only tokens are refused. Qdrant 1.20 rejects the older per-point `payload` claim as deprecated, so isolation is **per collection**.

Not covered: the default `shared` isolation shares collections, so a leaked admin key exposes every tenant, and collection isolation needs `jwt_rbac` and an admin who creates the collections. Over [A2A](a2a.html), all conversations share one tenant; run one server per tenant if callers must not see each other's profile.

## How retrieval works

For a field, in order of reliability:

1. the form's own **playbook** mapping;
2. an exact `autocomplete`-token match against your facts (payload filter, no vectors);
3. a **playbook from a similar form on the same site** (a redesign or sibling form);
4. semantic vector search.

`form_fill` with `auto: "required"` fills what memory can supply and reports the rest. Semantic matches need a margin over the runner-up and never apply to labels about other people. Playbooks persist only exact (playbook or `autocomplete`) matches.

## It learns from every fill

Ostler needs no model to judge whether a fill worked: the browser reads every value back and the page accepts or rejects the submit, so every outcome is ground truth.

| step | what happens | Qdrant feature |
|---|---|---|
| recall | exact form first; after a redesign, mappings transfer from a similar form on the same site with the same label | keyword-array filter on `labels_norm` and `host`, tenant index |
| rank | proven, recently verified skills beat stale or failing ones | Query API prefetch and a **score-boosting formula** (similarity, smoothed win rate and `exp_decay` on `verified_ts`) |
| reinforce | verified fields on accepted submits add wins per mapping | integer payload indexes |
| demote | a mapping that loses more than it wins is dropped | |
| forget | `ostler memory prune --days 90` removes stale, unproven skills; `serve` also forgets those not verified for 180 days on start (`--retain-days`, 0 keeps them). Saved facts never expire | range filter on `verified_ts` |

On a scripted site with redesigned forms, correct suggestions went from 1 to 8 of 10 fillable fields, with zero wrong fills in both conditions. That is a synthetic fixture, not real sites ([ADR-0008](repo:docs/adr/0008-learning-loop.md)).

## Commands

```sh
ostler memory list                      # saved facts
ostler memory remember email you@example.com
ostler memory forget email
ostler memory skills                    # what was learned: wins, losses, trusted mappings
ostler memory prune --days 90
ostler memory seal                      # re-save pre-sealing memory sealed
ostler memory key export --yes-print    # back up the sealing key
ostler memory token                     # per-tenant JWT (see above)
ostler memory wipe --yes                # delete everything for this tenant
```

No Qdrant? `ostler init --local` (or `ostler config set memory local`) keeps memory in one file, `memory.local`, next to your config. It survives restarts, needs no server, and the whole file is encrypted with the same memory key as sealed Qdrant memory (AES-256-GCM, written atomically, safe to use from `serve` and `ostler memory` at once). Embeddings stay on your machine: local word hashing by default (finds shared words, not meaning), or `--embeddings ollama` for semantic search. Back up the key; lost, the file cannot be read. `--seal off` stores plain JSON instead.

Just trying things out: `ostler memory --memory ephemeral list` keeps memory in-process and loses it on exit.

## Which keys do I need?

| you want | you need |
|---|---|
| browsing and form filling only | nothing |
| memory, no server, kept across sessions | `ostler init --local` (encrypted file on this machine), no keys |
| memory, trying it out | `--memory ephemeral` (in-process, lost on exit), no keys |
| memory on **Qdrant Cloud** | cluster URL and API key, with **Inference enabled** on the cluster |
| memory on self-hosted Qdrant | URL (plus key if you set one) and `--embeddings dev-hash`, or `--embeddings ollama` for semantic search |

See [Security model](security.html) for the threat-model rows on secrets, cross-user exposure and anyone who can read your Qdrant.

### Upgrading older memory

Two kinds of older sealed points need one command, `ostler memory seal`, which is safe to repeat and copies before it deletes:

- **`v1` blobs** (written by early builds) are no longer read: they did not authenticate the clear fields. `serve`, `memory list` and the other commands stop with the command to run; the upgrade opens them once with your key and saves them as `v2`.
- **Keys saved under an older normalisation** (emoji-only keys, decomposed accents) sit at a different point than today's lookup asks for. The same command re-files them under the current key and removes the stray copy, without overwriting a newer fact.

On Linux, `key_store = keychain` needs a running Secret Service (gnome-keyring, KWallet). Without one Ostler stops with the reason and `ostler config set key_store file`; it never falls back by itself, so the key is never half in a keyring and half on disk.

