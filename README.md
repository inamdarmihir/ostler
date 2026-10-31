<p align="center">
  <img src="assets/logo.svg" alt="Waymarker" width="96" height="96">
</p>

<h1 align="center">Waymarker</h1>

<p align="center">
  <a href="https://github.com/inamdarmihir/waymarker/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/inamdarmihir/waymarker/actions/workflows/ci.yml/badge.svg"></a>
  <a href="LICENSE"><img alt="License: Apache-2.0" src="https://img.shields.io/badge/license-Apache--2.0-blue.svg"></a>
  <img alt="Rust 1.82+" src="https://img.shields.io/badge/rust-1.82%2B-orange.svg">
  <img alt="MCP" src="https://img.shields.io/badge/protocol-MCP-black.svg">
  <a href="https://inamdarmihir.github.io/waymarker/"><img alt="Site" src="https://img.shields.io/badge/site-github.io-informational.svg"></a>
</p>

**The headless browser for AI coding agents that fills complex forms and remembers how.**

Rust, MCP, Chrome, and [Qdrant](https://qdrant.tech) memory. Works with Claude Code, Codex, Cursor, VS Code and anything else that speaks the [Model Context Protocol](https://modelcontextprotocol.io).

> **Status: v0.2, early.** Tested against real Chrome and a real Qdrant server, but only on local fixtures, not the open web. A [design review](docs/REVIEW-2026-10.md) found real flaws; they are listed openly with a fix order. Read [what that means](docs/ECONOMICS.md#6-what-can-go-wrong) before relying on it.

---

Waymarker is a browser server your coding agent drives over MCP. Instead of clicking one field per tool call, the agent asks for the form's typed fields, sends `{label: value}` for the whole form, and gets back what was actually filled, verified by reading each value back. Dependent dropdowns, multi-step wizards, masks, comboboxes, shadow DOM and same-origin iframes are handled by the engine, and the page's own validation messages come back when a submit is rejected.

What worked is saved in Qdrant: which profile fact fills which field, per site and per form, so the next visit starts informed.

### Why Waymarker?

| | Waymarker | typical snapshot-per-action MCP browser |
|---|---|---|
| Fill a 20-field form | **3 tool calls** (inspect, fill, submit) | ~1 per field plus a page snapshot each time |
| Wrong or missing field | **reported** (`UNMATCHED`, `ambiguous`, read-back mismatch) | usually silent |
| Dependent dropdowns, wizards | handled inside `form_fill` / `form_submit` | agent has to notice and retry |
| Submit result | `Submitted` / `Advanced` / `NOT SUBMITTED` + the page's messages | agent infers from a new snapshot |
| Memory across sessions | **Qdrant**: facts, playbooks, episodes | none, or a browser profile |
| Tool schema cost per turn | ~1.4k tokens (`--tools forms` is smaller) | varies |

The "3 vs ~20" row is measured on our own fixtures with a scripted agent, and the comparison column is **our engine driven that way, not another product**. Tools that already offer a batched fill (Playwright MCP, chrome-devtools-mcp) are closer to Waymarker than this table suggests. See [Benchmarks](#benchmarks) and the [design review](docs/REVIEW-2026-10.md).

---

## Install

You need **Chrome or Chromium** and **Rust 1.82+**. Prebuilt binaries are not published yet.

```sh
cargo install --git https://github.com/inamdarmihir/waymarker waymarker-cli

# or from a checkout
git clone https://github.com/inamdarmihir/waymarker && cd waymarker
cargo build --release -p waymarker-cli      # binary: target/release/waymarker
```

Check it works:

```sh
waymarker doctor
```

## Quick start

```sh
waymarker init                      # optional: save Qdrant URL + API key and other defaults (see Configure)
waymarker setup claude --apply      # register with Claude Code
waymarker fixtures                  # serve demo forms at http://127.0.0.1:8787
```

Then ask your agent: *"Open http://127.0.0.1:8787/dependent.html and fill in the shipping form for Ada Lovelace in Germany, Bavaria."*

### What the agent sees

Real output from the bundled fixtures (`waymarker serve --tools forms`), lightly reformatted. *State* is disabled until *Country* is chosen.

```text
> form_inspect
form [e2] fingerprint=07efd5b3061187a1
  e3 text    "Recipient"      [required]
  e4 select  "Country"        [required] options: Choose… | United States | Germany | India
  e5 select  "State / region" [required, disabled] options: (Choose a country first)
  e6 text    "City"
  e7 submit  "Ship it"

> form_fill {"Country": "Germany", "State": "Bavaria", "Email": "ada@example.com"}
Filled 2/2 fields in 386ms (form e2).
  OK e4 "Country" = "de"
  OK e5 "State / region" = "by"        <- options loaded after Country changed
  UNMATCHED key "Email": no field on the page matches this key
Still empty and required:
  e3 "Recipient"

> form_submit
NOT SUBMITTED: the page reported validation problems.
  e3 "Recipient": "Please fill in this field."
```

A key that matches nothing is reported, not guessed. A rejected submit says why, in the page's words.

---

## Configure

Settings come from, in order: **command-line flag, then environment variable, then the config file, then the default.** One path per setting, so what `waymarker serve` does is what `waymarker doctor` tested.

### Easy setup

```sh
waymarker init
```

Asks for a Qdrant URL and API key (skip both for no memory), a tenant name, and whether to enable `--learn` and `--lean`. It writes the config file with owner-only permissions and immediately tests the connection. Non-interactive:

```sh
waymarker init --qdrant-url https://<cluster>.cloud.qdrant.io --qdrant-api-key "$KEY" --tenant alice
```

The `:6333` port is added to Qdrant Cloud URLs for you. If the connection test fails the settings are still saved, so you can fix one value instead of retyping everything:

```sh
waymarker config show                          # key is masked
waymarker config set qdrant_url http://localhost:6333
waymarker config unset learn
waymarker config path                          # ~/.config/waymarker/config.toml
```

### Which keys do I need?

**Only Qdrant ones, and only for memory.** Everything else (browsing, form filling) needs no account and no key.

| you want | you need |
|---|---|
| browsing and form filling only | nothing |
| memory, trying it out | `--memory ephemeral` (in-process, lost on exit), no keys |
| memory on **Qdrant Cloud** | cluster URL + API key, with **Inference enabled** on the cluster (embeddings run inside Qdrant; no separate embedding provider or key) |
| memory on a self-hosted/open-source Qdrant | URL (+ key if you set one) and `--embeddings dev-hash`. Those vectors are not semantic: exact-key and `autocomplete` matches work, fuzzy meaning does not |

Qdrant Cloud Inference follows Qdrant's documentation and is covered by request-shape tests, but has **not** been run against a live Cloud cluster by the maintainers. The open-source path is tested against a real Qdrant server.

### Config file reference

`~/.config/waymarker/config.toml` (or `$XDG_CONFIG_HOME`, or the path in `WAYMARKER_CONFIG`):

| key | environment variable | meaning |
|---|---|---|
| `qdrant_url` | `QDRANT_URL` | Qdrant REST endpoint |
| `qdrant_api_key` | `QDRANT_API_KEY` | Qdrant API key (secret; file is `0600`) |
| `tenant` | `WAYMARKER_TENANT` | isolates users sharing a cluster (default `default`) |
| `memory` | `WAYMARKER_MEMORY` | `auto` (on if a URL is set), `qdrant`, `ephemeral`, `off` |
| `embeddings` | `WAYMARKER_EMBEDDINGS` | `cloud` (Qdrant Cloud Inference) or `dev-hash` |
| `embedding_model` | `WAYMARKER_EMBEDDING_MODEL` | Cloud Inference model (default `sentence-transformers/all-minilm-l6-v2`, 384 dims) |
| `qdrant_on_disk` | `WAYMARKER_QDRANT_ON_DISK` | keep vectors on disk: less RAM per user |
| `learn` | `WAYMARKER_LEARN` | save non-sensitive values typed after an accepted submit (off by default) |
| `lean` | `WAYMARKER_LEAN` | lower memory use; no images in screenshots |
| `tools` | `WAYMARKER_TOOLS` | `core`, `forms` or `all` |

Anything stored: [what is and is not saved](SECURITY.md). Inspect or erase it any time: `waymarker memory list | remember | forget | wipe`.

---

## Use with your agent

`waymarker setup <client>` prints the exact config, using this binary's **absolute path** (avoids "command not found" when the app's `PATH` differs from your shell's).

| client | command | result |
|---|---|---|
| Claude Code | `waymarker setup claude --apply` | runs `claude mcp add` for you |
| Codex | `waymarker setup codex` | prints the block for `~/.codex/config.toml` |
| Cursor | `waymarker setup cursor` | prints JSON for `~/.cursor/mcp.json` |
| VS Code | `waymarker setup vscode` | prints JSON for `.vscode/mcp.json` |
| anything else | `waymarker setup json` | generic `mcpServers` JSON |

Extra serve flags go after `--`: `waymarker setup claude --apply -- --lean --tools forms`. Only Claude Code is applied automatically; other clients' files are never modified. More in [docs/CLIENTS.md](docs/CLIENTS.md), problems in [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md).

### Tools

| group | tools |
|---|---|
| browse | `navigate` `snapshot` `click` `type` `press` `wait_for` `screenshot` `back` |
| tabs | `tabs` (list, new, switch, close; popups arrive as tabs) · `context` (notes shared by every tab) |
| forms | `form_inspect` `form_fill` `form_submit` `fill_field` |
| memory | `memory_remember` `memory_recall` `memory_list` `memory_forget` |
| other | `stats` |

`--tools core|forms|all` controls how many the agent sees. The full set (19 tools) is about 5.6 KB of schema, roughly 1.4k tokens on every turn; a test keeps it under 7 KB. Tool output is terse by default (verified fields are named once, no timings or fingerprints); `--verbose` brings the debug detail back.

**Tabs share one browser context**, so cookies and logins carry across them, and the `context` tool is a small key/value notepad every tab (and every later call) can read, for things like "applicant = Ada, order id = 4471". It survives a browser restart.

---

## Memory

Three Qdrant collections, multi-tenant through a `tenant` payload index:

| collection | holds | never holds |
|---|---|---|
| `waymarker_profile` | facts you saved (`email`, `company`, …) | passwords, card numbers, anything `looks_sensitive` |
| `waymarker_playbooks` | per site and form: which fact fills which field | the values themselves |
| `waymarker_episodes` | what went wrong on a form, so it is not repeated | secrets |

Retrieval order for a field: a saved playbook mapping, then an exact `autocomplete`-token match, then semantic search. `form_fill` with `auto: "required"` fills what memory can supply and reports the rest. Details: [ARCHITECTURE](docs/ARCHITECTURE.md), [ADRs](docs/adr/).

---

## Benchmarks

Fixtures only, one Apple M2 (8 cores), Chrome 156, localhost. Full tables, methodology and caveats: [docs/BENCHMARKS.md](docs/BENCHMARKS.md).

| metric | kind | result |
|---|---|---|
| browser time per form task | measured | 0.4-1.1 s |
| 8 parallel sessions, 8 cores | measured | 354-393 tasks/min, p95 < 1 s, 100% success |
| memory per session (physical footprint) | measured | **412 MiB** `--lean` / 658 MiB default |
| cold start | measured | ~0.6 s |
| Qdrant retrieval, 200k points / 10k tenants | measured (hash vectors, 20 points per tenant) | 0.4 ms p50 |
| 20-field form, tool calls | measured (scripted agent) | **3** vs 22 one-call-per-field |
| 20-field form, LLM cost / end-to-end time | **modelled** (illustrative price, 3 s per turn, 1.5k base context) | $0.021 vs $0.105 / 13 s vs 70 s |
| browser compute | modelled from measured CPU | ~$0.006 per 1,000 tasks |

**Read these carefully.** No LLM is in the loop: a script drives both columns, so this shows call counts and bytes, not whether a model succeeds. The one-call-per-field column is Waymarker itself driven the way snapshot-per-action tools work. With a realistic 20k-token agent context the 20-field cost ratio falls from ~5x to ~2.7x, and across all fixtures the aggregate is ~1.7x. The next milestone is a real-model evaluation against real tools on real sites.

Reproduce: `cargo build --release -p waymarker-bench && ./target/release/waymarker-bench forms --reps 3`.

---

## CLI reference

```text
waymarker <COMMAND>

  serve      Run the MCP server on stdio (what the agent launches)
  a2a        Serve Agent2Agent (A2A) over HTTP so other agents can delegate forms
  init       Save settings (Qdrant URL/key, tenant, defaults) and test them
  config     show | path | set <key> <value> | unset <key>
  setup      Print or apply the config for claude | codex | cursor | vscode | json
  doctor     Check that Chrome launches and Qdrant is reachable
  inspect    Open a URL and print the typed form model as JSON
  memory     list | remember <key> <value> | forget <key> | wipe --yes
  fixtures   Serve the demo form pages (default port 8787)
```

### `waymarker serve`

| flag | default | description |
|---|---|---|
| `--tools` | `all` | `core`, `forms` or `all` |
| `--lean` | off | lean Chrome flags; no image/font/media downloads |
| `--headed` | off | show the browser window |
| `--profile-dir DIR` | throwaway | persistent Chrome profile (keeps logins) |
| `--chrome-path PATH` | auto | Chrome binary (`WAYMARKER_CHROME`) |
| `--allow-domain D` | any | only this domain and subdomains; repeatable |
| `--allow-file` | off | permit `file://` URLs |
| `--require-confirm-submit` | off | `form_submit` needs `confirm=true` |
| `--learn` | off | save non-sensitive typed values after an accepted submit |
| `--verbose` | off | timings, fingerprints and ids in tool output (costs context) |
| `--memory` | `auto` | `auto`, `qdrant`, `ephemeral`, `off` |
| `--qdrant-url`, `--qdrant-api-key` | none | Qdrant endpoint and key |
| `--embeddings`, `--embedding-model`, `--embedding-dim` | `cloud`, MiniLM, 384 | embedding source |
| `--qdrant-on-disk` | off | vectors on disk |
| `--tenant` | `default` | memory namespace |

Set `WAYMARKER_NO_SANDBOX=1` to run Chrome without its sandbox (needed as root or in some containers). See [Troubleshooting](docs/TROUBLESHOOTING.md).

---

## A2A: let other agents hand it forms

```sh
waymarker a2a --allow-domain example.com
curl http://127.0.0.1:8765/.well-known/agent-card.json
```

Besides MCP, Waymarker speaks [Agent2Agent](https://a2a-protocol.org) v1.0. Another agent sends a task (`{"url": …, "values": {…}, "submit": true}`), Waymarker fills and verifies the form, and answers `input-required` when it needs something (a missing field, a rejected value, or your **confirm** before anything is submitted). Each conversation gets its own browser and tabs. It binds to loopback, requires a bearer token to listen anywhere else, and refuses requests from web pages. Checked against the official A2A Python SDK. Details, security notes and limits: [docs/A2A.md](docs/A2A.md).

## Safety

- `--allow-domain` is enforced **inside the browser** on navigations, form posts, `fetch`/XHR, beacons and event streams, so a request to another host never leaves. It does not cover images, scripts, stylesheets or WebSockets, and it ignores ports.
- `--require-confirm-submit` blocks **every non-GET request** (click, Enter, page script) unless it happens inside a confirmed `form_submit`. A GET with side effects is not covered.
- Page text is wrapped as untrusted (the wrapper cannot be closed from inside); secrets are refused by memory and masked in reports, snapshots and notes; memory never overwrites a fact you told it.
- Popups open as tabs under the same policy. A crashed Chrome is restarted and the agent is told.

**Still open, from our own review:** file fields accept any local path, the DevTools port is a TCP port other local users can reach, an empty allowlist still permits localhost and private addresses (including cloud metadata), `confirm()` dialogs are auto-accepted, and Chrome can be orphaned if the server is killed. For anything sensitive use `--allow-domain` and a throwaway profile. Status of every finding: [review](docs/REVIEW-2026-10.md#status). Policy and threat model: [SECURITY.md](SECURITY.md).

## How it compares

From each project's own README (see [research](docs/research/landscape.md); star counts move; we have **not** run a head-to-head):

| | language | form handling | memory |
|---|---|---|---|
| **Waymarker** | Rust | typed form model, verified fill, wizards, dependent fields, shadow DOM, same-origin iframes | Qdrant: profile, playbooks, episodes |
| agent-browser (Vercel) | Rust CLI | `fill`/`select`/`check`, batching, `--delta` snapshots | cookie/storage state, auth vault |
| Playwright MCP (Microsoft) | Node | accessibility snapshots, `browser_fill_form` batch fill | browser profile |
| chrome-devtools-mcp (Google) | Node | `fill_form` batch fill plus debugging/perf tooling | none |
| Lightpanda | Zig | n/a (engine without rendering; AGPL) | n/a |

Stagehand and Browser Use cache or learn actions too; they are Python/TypeScript frameworks rather than MCP tools for coding agents.

## Roadmap

- [x] CDP client, in-page runtime, form engine, Qdrant memory, MCP server, benchmark harness, `init`/`config`/`setup`
- [ ] **Fix the review findings**, starting with wrong-fill prevention and browser-level enforcement of the safety flags ([order](docs/REVIEW-2026-10.md#recommended-order-of-work))
- [ ] **Real-model evaluation** against real tools on real sites, published including failures
- [ ] Prebuilt binaries and a one-line installer
- [ ] Live validation of Qdrant Cloud Inference; Linux benchmark numbers
- [ ] Tabs/popups, cross-origin iframes, HTTP transport

## Repository

`crates/` cdp · browser · forms · memory · mcp · cli · fixtures · bench. `fixtures/` test pages. Docs: [architecture](docs/ARCHITECTURE.md), [ADRs](docs/adr/), [benchmarks](docs/BENCHMARKS.md), [economics](docs/ECONOMICS.md), [research](docs/research/landscape.md), [design review](docs/REVIEW-2026-10.md), [changelog](CHANGELOG.md), [contributing](CONTRIBUTING.md).

## License

Apache-2.0. See [LICENSE](LICENSE).
