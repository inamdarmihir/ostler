<h1 align="center">Waymarker</h1>

<p align="center">
  <b>A headless browser for AI coding agents.</b><br>
  Fills complex forms in one call, verifies what it typed, and remembers what worked, using Qdrant.<br>
  Written in Rust. Speaks <a href="https://modelcontextprotocol.io">MCP</a>, so it works with Claude Code, Codex, Cursor and anything else that does.
</p>

<p align="center">
  <a href="https://github.com/inamdarmihir/waymarker/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/inamdarmihir/waymarker/actions/workflows/ci.yml/badge.svg"></a>
  <a href="LICENSE"><img alt="License: Apache-2.0" src="https://img.shields.io/badge/license-Apache--2.0-blue.svg"></a>
  <img alt="Rust 1.80+" src="https://img.shields.io/badge/rust-1.80%2B-orange.svg">
  <a href="https://inamdarmihir.github.io/waymarker/"><img alt="Site" src="https://img.shields.io/badge/site-github.io-informational.svg"></a>
</p>

> **Status: v0.2, early.** Works end to end and is tested against real Chrome and a real Qdrant server, but only on local fixtures, not the open web. A [design review](docs/REVIEW-2026-10.md) found real flaws (listed openly, with a fix order). Read [what that means](docs/ECONOMICS.md#6-what-can-go-wrong) before relying on it.

## Quickstart (2 minutes)

You need **Chrome or Chromium** and **Rust 1.80+** (prebuilt binaries are not published yet).

```sh
cargo install --git https://github.com/inamdarmihir/waymarker waymarker-cli
waymarker doctor                 # checks Chrome (and Qdrant, if configured)
waymarker setup claude --apply   # registers it with Claude Code
```

Then ask your agent, for example: *"Open http://127.0.0.1:8787/dependent.html and fill in the shipping form for Ada in Germany, Bavaria."* (Run `waymarker fixtures` first to serve the demo pages.)

### Other clients

`waymarker setup <client>` prints the exact config, using this binary's **absolute path** (avoids the classic "command not found because the app has a different PATH").

| client | command | what it does |
|---|---|---|
| Claude Code | `waymarker setup claude --apply` | runs `claude mcp add` for you |
| Codex | `waymarker setup codex` | prints the block for `~/.codex/config.toml` |
| Cursor | `waymarker setup cursor` | prints JSON for `~/.cursor/mcp.json` |
| VS Code | `waymarker setup vscode` | prints JSON for `.vscode/mcp.json` |
| anything else | `waymarker setup json` | generic `mcpServers` JSON |

Add serve flags after `--`, for example `waymarker setup claude --apply -- --lean --tools forms`. Only Claude Code is applied automatically; other clients' files are never modified. Details: [docs/CLIENTS.md](docs/CLIENTS.md). Trouble? [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md).

## What the agent sees

Real output from the bundled fixtures (`waymarker serve --tools forms`), lightly reformatted for width. The page is a shipping form where *State* is disabled until *Country* is chosen.

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

Three things to notice: dependent dropdowns work in a single call; a key that matches nothing is **reported, not guessed**; and submission tells the agent exactly why it failed, in the page's own words.

## Why it exists

Agents fail on forms (dependent dropdowns, wizards, masks, validation, shadow DOM) and burn tokens doing it one field per tool call. They also forget everything between sessions. Waymarker:

1. **Fills a form per call.** `form_inspect` returns typed fields; `form_fill` takes `{label: value}`, resolves, normalises, fills in order, re-inspects after each field (so dependent dropdowns work) and **reads every value back**. `form_submit` reports *Submitted*, *Advanced* (next wizard step) or *failed validation*.
2. **Remembers, using Qdrant.** Profile facts, form *playbooks* (structure that worked, never values) and *episodes* (what went wrong) live in Qdrant, tenant-scoped, so the next visit knows which fact fills which field.
3. **Is honest about cost.** Numbers below come from a harness in this repo, and each says whether it was measured or modelled.

## Measured and modelled results

Fixtures only, one Apple M2, localhost. [Full methodology and caveats](docs/BENCHMARKS.md).

| | measured or modelled | result |
|---|---|---|
| browser time per form task | measured | 0.4-1.1 s |
| 8 parallel sessions on 8 cores | measured | 354-393 tasks/min, p95 < 1 s, 100% success |
| memory per session (physical footprint) | measured | 412 MiB `--lean` / 658 MiB default |
| browser compute cost | modelled from measured CPU | ~$0.006 per 1,000 tasks |
| Qdrant retrieval at 200k points / 10k tenants | measured (hash vectors, 20 points/tenant) | 0.4 ms p50 |
| 20-field form: tool calls | measured (scripted agent) | 3 vs 22 for one-call-per-field |
| 20-field form: LLM cost, end-to-end time | **modelled** (illustrative price, 3 s per turn, 1.5k-token base context) | $0.021 vs $0.105, 13 s vs 70 s |

**Read these carefully.** There is no LLM in the benchmark loop: a script drives both columns, so it shows call counts and bytes, not whether a model succeeds. The "one call per field" column is Waymarker itself driven the way snapshot-per-action tools work, **not a measurement of another product**; tools with a batched fill (Playwright MCP, chrome-devtools-mcp) would sit much closer to Waymarker. With a realistic 20k-token agent context the 20-field cost ratio falls from ~5x to ~2.7x, and across all fixtures the aggregate is ~1.7x. The honest claim is "fewer turns per completed form", and the evaluation that proves or kills it (a real model against real tools on real sites) is the next milestone.

## Tools

`navigate` `snapshot` `click` `type` `press` `wait_for` `screenshot` `back` · `form_inspect` `form_fill` `form_submit` `fill_field` · `memory_remember` `memory_recall` `memory_list` `memory_forget` `stats`.

`--tools core|forms|all` controls how many the agent sees; the full set costs ~1.1k tokens of context on every turn (enforced by a test).

## Memory with Qdrant (optional)

```sh
export QDRANT_URL=https://<cluster>.cloud.qdrant.io:6333 QDRANT_API_KEY=…
waymarker doctor
waymarker setup claude --apply -- --learn --lean
```

Embeddings use **Qdrant Cloud Inference**; that path follows Qdrant's docs and is pinned by tests but has **not** been run against a live cluster. `--learn` is opt-in: without it Waymarker records how forms were filled, never your data. Inspect or erase anytime: `waymarker memory list|remember|forget|wipe`. More in [docs/CLIENTS.md](docs/CLIENTS.md#memory-with-qdrant).

## Safety

Domain allowlist (`--allow-domain`), `--require-confirm-submit`, secrets refused by memory, page text marked untrusted. **Known gaps, found by our own review and not yet fixed:** the allowlist is checked after navigation rather than at the network layer, the submit confirmation only guards `form_submit`, file fields accept any local path, and the DevTools port is open to local users. Use `--allow-domain` and a throwaway profile for anything sensitive. See [SECURITY.md](SECURITY.md) and the [review](docs/REVIEW-2026-10.md).

## How it compares

From each project's README (see [research](docs/research/landscape.md); star counts move; we have **not** run a head-to-head):

| | language | form handling | memory |
|---|---|---|---|
| **Waymarker** | Rust | typed form model, verified fill, wizards, dependent fields, shadow DOM, same-origin iframes | Qdrant: profile, playbooks, episodes |
| agent-browser (Vercel) | Rust CLI | `fill`/`select`/`check`, batching, `--delta` snapshots | cookie/storage state, auth vault |
| Playwright MCP (Microsoft) | Node | accessibility snapshots, `browser_fill_form` batch fill | browser profile |
| chrome-devtools-mcp (Google) | Node | `fill_form` batch fill plus debugging/perf tooling | none |
| Lightpanda | Zig | n/a (engine, no rendering; AGPL) | n/a |

Stagehand and Browser Use cache or learn actions too; they are Python/TS frameworks rather than MCP tools for coding agents.

## Roadmap

- [x] CDP client, in-page runtime, form engine, Qdrant memory, MCP server, benchmark harness
- [ ] **Fix the review findings**, starting with wrong-fill prevention and browser-level enforcement of the safety flags ([order](docs/REVIEW-2026-10.md#recommended-order-of-work))
- [ ] **LLM-in-the-loop evaluation** against real tools on real sites, results published including failures
- [ ] Prebuilt binaries and one-line install
- [ ] Live validation of Qdrant Cloud Inference; Linux benchmark numbers
- [ ] Tabs/popups, cross-origin iframes, HTTP transport

## Repository map

`crates/` cdp · browser · forms · memory · mcp · cli · fixtures · bench. `fixtures/` test pages. `docs/` [architecture](docs/ARCHITECTURE.md), [ADRs](docs/adr/), [benchmarks](docs/BENCHMARKS.md), [economics](docs/ECONOMICS.md), [research](docs/research/landscape.md), [design review](docs/REVIEW-2026-10.md). [CHANGELOG](CHANGELOG.md) · [CONTRIBUTING](CONTRIBUTING.md).

## License

Apache-2.0. See [LICENSE](LICENSE).
