<p align="center">
  <img src="assets/logo.svg" alt="Ostler" width="96" height="96">
</p>

<h1 align="center">Ostler</h1>

<p align="center">
  <a href="https://github.com/inamdarmihir/ostler/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/inamdarmihir/ostler/actions/workflows/ci.yml/badge.svg"></a>
  <a href="LICENSE"><img alt="License: Apache-2.0" src="https://img.shields.io/badge/license-Apache--2.0-blue.svg"></a>
  <img alt="Rust 1.89+" src="https://img.shields.io/badge/rust-1.89%2B-orange.svg">
  <img alt="MCP" src="https://img.shields.io/badge/protocol-MCP-black.svg">
  <a href="https://inamdarmihir.github.io/ostler/"><img alt="Site" src="https://img.shields.io/badge/site-github.io-informational.svg"></a>
</p>

<p align="center">
  <b>The headless browser for AI coding agents that fills complex forms, checks its work, and remembers how.</b><br>
  Rust · MCP · A2A · Chrome · <a href="https://qdrant.tech">Qdrant</a> memory
</p>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="#quick-start">Quick start</a> ·
  <a href="#use-with-your-agent">Use with your agent</a> ·
  <a href="#memory">Memory</a> ·
  <a href="#benchmarks">Benchmarks</a> ·
  <a href="#safety">Safety</a> ·
  <a href="https://inamdarmihir.github.io/ostler/">Website</a>
</p>

Works with Claude Code, Codex, Cursor, VS Code and anything else that speaks the [Model Context Protocol](https://modelcontextprotocol.io).

### At a glance

| | |
|---|---|
| **One call per form** | `form_inspect`, `form_fill`, `form_submit`: typed fields in, intent out, instead of a click and a snapshot per field |
| **Verified, not assumed** | every value is read back from the live page and compared by field type; a page that rewrites a date or picks a different combobox option is reported as a failure |
| **Honest about gaps** | keys that match nothing are `UNMATCHED`, ties are `ambiguous`, a rejected submit returns the page's own messages |
| **Sees like a user** | `look` returns a screenshot with numbered click targets; red borders and toasts count as errors |
| **Remembers** | profile facts, per-site playbooks and episodes in Qdrant, scoped per tenant |
| **Two front doors** | MCP over stdio for coding agents, [A2A](docs/A2A.md) over HTTP for other agents |

> **Status: v0.2, early.** Tested against real Chrome and a real Qdrant server, but only on local fixtures, not the open web. A [design review](docs/REVIEW-2026-10.md) found real flaws; they are listed openly with a fix order. Read [what that means](docs/ECONOMICS.md#6-what-can-go-wrong) before relying on it.

---

Ostler is a browser server your coding agent drives over MCP. Instead of clicking one field per tool call, the agent asks for the form's typed fields, sends `{label: value}` for the whole form, and gets back what was actually filled, verified by reading each value back. Dependent dropdowns, multi-step wizards, masks, comboboxes, shadow DOM and same-origin iframes are handled by the engine, and the page's own validation messages come back when a submit is rejected.

What worked is saved in Qdrant: which profile fact fills which field, per site and per form, so the next visit starts informed.

### Why Ostler?

| | Ostler | typical snapshot-per-action MCP browser |
|---|---|---|
| Fill a 20-field form | **3 tool calls** (inspect, fill, submit) | ~1 per field plus a page snapshot each time |
| Wrong or missing field | **reported** (`UNMATCHED`, `ambiguous`, read-back mismatch) | usually silent |
| Dependent dropdowns, wizards | handled inside `form_fill` / `form_submit` | agent has to notice and retry |
| Submit result | `Submitted` / `Advanced` / `NOT SUBMITTED` + the page's messages | agent infers from a new snapshot |
| Memory across sessions | **Qdrant**: facts, playbooks, episodes | none, or a browser profile |
| Tool schema cost per turn | ~1.5k tokens (`--tools forms` is smaller) | varies |

The "3 vs ~20" row is measured on our own fixtures with a scripted agent, and the comparison column is **our engine driven that way, not another product**. Tools that already offer a batched fill (Playwright MCP, chrome-devtools-mcp) are closer to Ostler than this table suggests. See [Benchmarks](#benchmarks) and the [design review](docs/REVIEW-2026-10.md).

<details>
<summary><b>Contents</b></summary>

[Install](#install) · [Quick start](#quick-start) · [Configure](#configure) · [Use with your agent](#use-with-your-agent) · [Memory](#memory) · [Benchmarks](#benchmarks) · [CLI reference](#cli-reference) · [A2A](#a2a-let-other-agents-hand-it-forms) · [Safety](#safety) · [How it compares](#how-it-compares) · [Roadmap](#roadmap)

</details>

---

## Install

You need **Chrome or Chromium** (Ostler drives a real browser). Pick one way to get the binary:

| how | command | needs |
|---|---|---|
| **From source** (works today) | `cargo install --git https://github.com/inamdarmihir/ostler ostler` | Rust 1.89+, a minute or two to compile |
| Prebuilt binary | `curl -fsSL https://inamdarmihir.github.io/ostler/install.sh \| sh` | nothing else; macOS and Linux |
| npm / `npx` | `npx -y ostler doctor` | Node 18+; downloads the binary on first run |
| crates.io | `cargo install ostler` or `cargo binstall ostler` | Rust (or cargo-binstall for the prebuilt) |

> **Status:** nothing is published to crates.io or npm yet, and the prebuilt binaries are created by the first tagged release (the release process is in [docs/RELEASING.md](docs/RELEASING.md); every install path is tested against a fake release in CI). Until then use the first row. Windows is not built or tested.

Then check it:

```sh
ostler doctor        # Chrome launches? Qdrant reachable (if you set one)? prints the fix when not
```

**Update:** rerun the same install command (`install.sh` replaces the binary in place). **Uninstall:** delete the `ostler` binary (`~/.local/bin/ostler`, or `~/.cargo/bin/ostler`), and `~/.config/ostler` and `~/.cache/ostler` if you want the settings and the npx download gone. Memory lives in your Qdrant; `ostler memory wipe --yes` erases it first.

## Quick start

Three steps: install, verify, connect.

```sh
# 1. install (Rust 1.89+ and Chrome/Chromium required)
cargo install --git https://github.com/inamdarmihir/ostler ostler

# 2. check it works
ostler doctor

# 3. connect your agent (Claude Code shown; others below)
ostler setup claude --apply
```

Using Codex, Cursor or VS Code instead? Replace step 3 with the matching line from
[Use with your agent](#use-with-your-agent): `ostler setup codex`, `ostler setup cursor`
or `ostler setup vscode` — each prints the config for you to place.

Then try the demo: run `ostler fixtures` (serves synthetic test forms at http://127.0.0.1:8787), then ask your agent:

> *Open http://127.0.0.1:8787/dependent.html and fill in the shipping form for Ada Lovelace in Germany, Bavaria.*

Ostler fills Country, watches State load, reads every value back, and reports what the page said. Nothing real is submitted.

Memory (Qdrant) is optional and separate: `ostler init` saves your cluster URL and key,
then re-register with `--learn` when you connect. Details in [Configure](#configure).


### Things to ask your agent

- "Fill in the contact form on this page with my details." (after `memory_remember` of your name and email, or with `--learn`)
- "Sign up for the newsletter on example.com, but ask me before you submit."  (`--require-confirm-submit`)
- "This form has a date picker and a country dropdown: fill it and tell me anything you couldn't verify."
- "What's blocking this page?"  (`look` shows numbered click targets; a CAPTCHA is reported, not bypassed)

### What the agent sees

Real output from the bundled fixtures (`ostler serve --tools forms`), lightly reformatted. *State* is disabled until *Country* is chosen.

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

Verification is strict by design: a combobox must show the option you asked for (not a longer one that merely contains it), a `datetime-local` must read back as the requested instant, and `form_fill` reports `unsettled` when the page was still busy at the end of the fill so the verdicts can be treated with care.

---

## Configure

Settings come from, in order: **command-line flag, then environment variable, then the config file, then the default.** One path per setting, so what `ostler serve` does is what `ostler doctor` tested.

### Easy setup

```sh
ostler init
```

Asks for a Qdrant URL and API key (skip both for no memory), a tenant name, and whether to enable `--learn` and `--lean`. It writes the config file with owner-only permissions and immediately tests the connection. Non-interactive:

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

### Which keys do I need?

**Only Qdrant ones, and only for memory.** Everything else (browsing, form filling) needs no account and no key.

| you want | you need |
|---|---|
| browsing and form filling only | nothing |
| memory, trying it out | `--memory ephemeral` (in-process, lost on exit), no keys |
| memory on **Qdrant Cloud** | cluster URL + API key, with **Inference enabled** on the cluster (embeddings run inside Qdrant; no separate embedding provider or key) |
| memory on a self-hosted/open-source Qdrant | URL (+ key if you set one) and `--embeddings dev-hash`. Those vectors are not semantic: exact-key and `autocomplete` matches work, fuzzy meaning does not |

Qdrant Cloud Inference follows Qdrant's documentation and is covered by request-shape tests, but has **not** been run against a live Cloud cluster by the maintainers. The open-source path is tested against a real Qdrant server.

<details>
<summary><b>Config file reference</b></summary>

`~/.config/ostler/config.toml` (or `$XDG_CONFIG_HOME`, or the path in `OSTLER_CONFIG`):

| key | environment variable | meaning |
|---|---|---|
| `qdrant_url` | `QDRANT_URL` | Qdrant REST endpoint |
| `qdrant_api_key` | `QDRANT_API_KEY` | Qdrant API key (secret; file is `0600`) |
| `tenant` | `OSTLER_TENANT` | isolates users sharing a cluster (default `default`) |
| `memory` | `OSTLER_MEMORY` | `auto` (on if a URL is set), `qdrant`, `ephemeral`, `off` |
| `embeddings` | `OSTLER_EMBEDDINGS` | `cloud` (Qdrant Cloud Inference) or `dev-hash` |
| `embedding_model` | `OSTLER_EMBEDDING_MODEL` | Cloud Inference model (default `sentence-transformers/all-minilm-l6-v2`, 384 dims) |
| `qdrant_on_disk` | `OSTLER_QDRANT_ON_DISK` | keep vectors on disk: less RAM per user |
| `learn` | `OSTLER_LEARN` | save non-sensitive values typed after an accepted submit (off by default) |
| `lean` | `OSTLER_LEAN` | lower memory use; no images in screenshots |
| `tools` | `OSTLER_TOOLS` | `core`, `forms` or `all` |

</details>

Anything stored: [what is and is not saved](SECURITY.md). Inspect or erase it any time: `ostler memory list | remember | forget | wipe`.

---

## Use with your agent

`ostler setup <client>` prints the exact config, using this binary's **absolute path** (avoids "command not found" when the app's `PATH` differs from your shell's).

| client | command | result |
|---|---|---|
| Claude Code | `ostler setup claude --apply` | runs `claude mcp add` for you |
| Codex | `ostler setup codex` | prints the block for `~/.codex/config.toml` |
| Cursor | `ostler setup cursor` | prints JSON for `~/.cursor/mcp.json` |
| VS Code | `ostler setup vscode` | prints JSON for `.vscode/mcp.json` |
| anything else | `ostler setup json` | generic `mcpServers` JSON |

Extra serve flags go after `--`: `ostler setup claude --apply -- --lean --tools forms`. Only Claude Code is applied automatically; other clients' files are never modified. More in [docs/CLIENTS.md](docs/CLIENTS.md), problems in [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md).

### Tools

| group | tools |
|---|---|
| browse | `navigate` `snapshot` `click` `type` `press` `wait_for` `wait_for_human` `look` `screenshot` `back` |
| tabs | `tabs` (list, new, switch, close; popups arrive as tabs) · `context` (notes shared by every tab) |
| forms | `form_inspect` `form_fill` `form_submit` `fill_field` |
| memory | `memory_remember` `memory_recall` `memory_list` `memory_forget` |
| other | `stats` |

`--tools core|forms|all` controls how many the agent sees. The full set (21 tools) is about 5.9 KB of schema, roughly 1.5k tokens on every turn; a test keeps it under 7 KB. Tool output is terse by default (verified fields are named once, no timings or fingerprints); `--verbose` brings the debug detail back.

**Tabs share one browser context**, so cookies and logins carry across them, and the `context` tool is a small key/value notepad every tab (and every later call) can read, for things like "applicant = Ada, order id = 4471". It survives a browser restart.

---

## Memory

Three Qdrant collections, multi-tenant through a `tenant` payload index:

| collection | holds | never holds |
|---|---|---|
| `ostler_profile` | facts you saved (`email`, `company`, …) | passwords, card numbers, anything `looks_sensitive` |
| `ostler_playbooks` | per site and form: which fact fills which field, with wins, losses and last verified time | the values themselves |
| `ostler_episodes` | what went wrong on a form, so it is not repeated | secrets |

Retrieval order for a field: the form's own playbook, then an exact `autocomplete`-token match, then a **playbook from a similar form on the same site** (a redesign or a sibling form), then semantic search. `form_fill` with `auto: "required"` fills what memory can supply and reports the rest.

### It learns from every fill

Playbooks work like skills that improve with use. Ostler doesn't need a model to judge whether a fill worked: the browser reads every value back and the page accepts or rejects the submit, so every outcome is ground truth.

| step | what happens | Qdrant feature |
|---|---|---|
| recall | exact form first; after a redesign, mappings transfer from a similar form on the same site with the same label | keyword-array filter on `labels_norm` + `host`, tenant index |
| rank | proven, recently verified skills beat stale or failing ones | Query API prefetch + **score-boosting formula** (similarity + smoothed win rate + `exp_decay` on `verified_ts`) |
| reinforce | verified fields on accepted submits add wins per mapping | integer payload indexes |
| demote | a mapping that loses more than it wins is dropped | |
| forget | `ostler memory prune --days 90` removes stale, unproven skills | range filter on `verified_ts` |

`ostler memory skills` shows what has been learned: wins, losses, trusted mappings, and which form a skill was learned from. On a scripted site with redesigned forms, correct suggestions went from 1 to 8 of 10 fillable fields, with zero wrong fills in both conditions. That's a synthetic fixture, not real sites ([ADR-0008](docs/adr/0008-learning-loop.md)).

Details: [ARCHITECTURE](docs/ARCHITECTURE.md), [ADRs](docs/adr/).

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

**Read these carefully.** No LLM is in the loop: a script drives both columns, so this shows call counts and bytes, not whether a model succeeds. The one-call-per-field column is Ostler itself driven the way snapshot-per-action tools work. With a realistic 20k-token agent context the 20-field cost ratio falls from ~5x to ~2.7x, and across all fixtures the aggregate is ~1.7x. The next milestone is a real-model evaluation against real tools on real sites.

Reproduce: `cargo build --release -p ostler-bench && ./target/release/ostler-bench forms --reps 3`.

---

## CLI reference

```text
ostler <COMMAND>

  serve      Run the MCP server on stdio (what the agent launches)
  a2a        Serve Agent2Agent (A2A) over HTTP so other agents can delegate forms
  init       Save settings (Qdrant URL/key, tenant, defaults) and test them
  config     show | path | set <key> <value> | unset <key>
  setup      Print or apply the config for claude | codex | cursor | vscode | json
  doctor     Check that Chrome launches and Qdrant is reachable
  inspect    Open a URL and print the typed form model as JSON
  memory     list | skills | prune --days N | remember <key> <value> | forget <key> | wipe --yes
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

</details>

Set `OSTLER_NO_SANDBOX=1` to run Chrome without its sandbox (needed as root or in some containers). See [Troubleshooting](docs/TROUBLESHOOTING.md).

---

## A2A: let other agents hand it forms

```sh
ostler a2a --allow-domain example.com
curl http://127.0.0.1:8765/.well-known/agent-card.json
```

Besides MCP, Ostler speaks [Agent2Agent](https://a2a-protocol.org) v1.0. Another agent sends a task (`{"url": …, "values": {…}, "submit": true}`), Ostler fills and verifies the form, and answers `input-required` when it needs something (a missing field, a rejected value, or your **confirm** before anything is submitted). Each conversation gets its own browser and tabs. It binds to loopback, requires a bearer token to listen anywhere else, and refuses requests from web pages. Checked against the official A2A Python SDK. Details, security notes and limits: [docs/A2A.md](docs/A2A.md).

## Safety

- `--allow-domain` is enforced **inside the browser**. Every request the page makes is checked, whatever its type (documents, images, stylesheets, scripts, `fetch`/XHR, beacons, event streams), and Chrome is launched so that every other host does not resolve, which also covers WebSockets and the first request of a popup. A request to another host never leaves. Ports are ignored.
- `--require-confirm-submit` blocks **every non-GET request** (click, Enter, page script) unless it happens inside a confirmed `form_submit`. A GET with side effects is not covered.
- Page text is wrapped as untrusted (the wrapper cannot be closed from inside), including titles, URLs and the `look` legend. Text a person cannot see (opacity 0, zero font size, far off-screen, same colour as the background) is left out of snapshots and counted, so a hidden instruction never reaches the model. Secrets are refused by memory and masked in reports, snapshots and notes; memory never overwrites a fact you told it.
- **CAPTCHAs and verification walls are reported, never solved or bypassed.** reCAPTCHA, hCaptcha and Turnstile widgets and "verify you are human" interstitials put a `CHALLENGE:` line at the top of the tool result, so the agent stops retrying and asks the user. With `--headed`, `wait_for_human` waits while the person completes it in the window. A2A tasks come back `input-required`. Invisible badges and ordinary pages that mention verification are not flagged.
- Popups open as tabs under the same policy. A crashed Chrome is restarted and the agent is told.

**Still open, from our own review:** file fields accept any local path, the DevTools port is a TCP port other local users can reach, an empty allowlist still permits localhost and private addresses (including cloud metadata), `confirm()` dialogs are auto-accepted, and Chrome can be orphaned if the server is killed. For anything sensitive use `--allow-domain` and a throwaway profile. Status of every finding: [review](docs/REVIEW-2026-10.md#status). Policy and threat model: [SECURITY.md](SECURITY.md).

## How it compares

From each project's own README (see [research](docs/research/landscape.md); star counts move; we have **not** run a head-to-head):

| | language | form handling | memory |
|---|---|---|---|
| **Ostler** | Rust | typed form model, verified fill, wizards, dependent fields, shadow DOM, same-origin iframes | Qdrant: profile, playbooks, episodes |
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

`crates/` cdp · browser · forms · memory · mcp · cli · fixtures · bench. `crates/ostler-fixtures/fixtures/` test pages. Docs: [architecture](docs/ARCHITECTURE.md), [ADRs](docs/adr/), [benchmarks](docs/BENCHMARKS.md), [economics](docs/ECONOMICS.md), [research](docs/research/landscape.md), [design review](docs/REVIEW-2026-10.md), [changelog](CHANGELOG.md), [contributing](CONTRIBUTING.md).

## License

Apache-2.0. See [LICENSE](LICENSE).
