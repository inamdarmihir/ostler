# Ostler: a founder's critique, and what to do about it

Written as a YC partner would read the repo before a first meeting: looking for reasons not to back it. Every
claim below was checked against `main` (8329816, November 2026). Numbers are from the repo, not estimates.
The second half is the plan.

## The verdict in five lines

1. **The core idea is good and rare**: a browser for agents that checks its own work. Quotes are verified against
   pages actually read, form values are read back, and an uncertain outcome is labelled uncertain. Almost no one else
   does this.
2. **That idea is buried.** It sits inside a 12-crate, 51,000-line Rust codebase with 32 tools, 38 environment
   variables, a vector database option, an A2A server and a 4,600-line crate that nothing uses. A new user meets
   the plumbing before the idea.
3. **It competes where it cannot win.** As a local MCP browser it goes head to head with Playwright MCP (Microsoft)
   and Chrome DevTools MCP (Google): free, backed by browser vendors, cross-browser or DevTools-deep, and now shipped
   inside Playwright itself. "Another MCP browser" is not a reason to switch.
4. **It is hard to plug into anything else.** It is MCP over stdio only, with no Python or TypeScript API, no HTTP
   transport, no CLI mode for agents, and it refuses to attach to a remote browser. It cannot sit on top of
   Browserbase, Kernel or Steel, which is where agent swarms actually run.
5. **The open-source value is real but not reusable.** The verification pieces are published as "internal crates
   with no stability promise", so nobody else can build on them.

The fix is not more features. It is to **cut, focus on verification, and make that verification usable from
anywhere**.

---

## 1. Where Ostler sits in the market

The browser-agent space has split into layers. Ostler has been trying to be all of them.

| Layer | Who | What they own | Where Ostler stands |
|---|---|---|---|
| Automation library + local MCP | Playwright MCP and CLI (Microsoft), Chrome DevTools MCP (Google), Vercel `agent-browser` | Driving a browser from an agent. Free, vendor-backed. Playwright covers Chromium, Firefox and WebKit; DevTools MCP covers performance and network debugging | Ostler's 16 core tools (navigate, click, type, snapshot, tabs, screenshot…) are a subset of what these do. No moat here. |
| Hosted browser infrastructure | Browserbase, Kernel (YC S25), Steel, Anchor, Hyperbrowser, Browser Use cloud | Fleets, parallel sessions, profiles, proxies, managed auth, recordings, stealth | Ostler runs one local Chrome per process. Should not compete; should integrate. |
| Agent frameworks | Browser Use (YC, 110k+ GitHub stars), Stagehand (Browserbase), Skyvern | Turning a goal into actions: act / observe / extract, self-healing | Ostler has `run` scripts and planned missions. A small team cannot out-build these. |
| **Verification and evidence** | Almost nobody | Proving what an agent did and read: checked citations, read-back fills, honest outcomes, false-success benchmarks | **This is Ostler's lane.** It is the one thing in the repo nobody else ships. |

Two shifts in 2026 make this sharper:

- **The leaders are moving from MCP tool menus to CLI + skills.** Playwright's own README says coding agents
  increasingly prefer CLI workflows because they cost fewer tokens. Chrome DevTools MCP shipped a CLI and skill.
  Browser Use's CLI 3.0 (July 2026) deleted its fixed action menu in favour of raw CDP and editable Python. Ostler is
  MCP-only with 32 tools, so it is betting the other way without saying why.
- **Swarms run on hosted browsers.** Anyone running many agents in parallel uses a fleet provider. A tool that
  only drives a local Chrome is a laptop tool.

## 2. How clean is it?

### The numbers

| Measure | Value | What it says |
|---|---|---|
| Rust source (excluding `tests/`) | ~51,000 lines in 12 crates, plus a 2,234-line `runtime.js` | Large for a single-binary tool whose core value is a few thousand lines |
| Integration tests | ~10,700 lines; 783 tests pass with real Chrome | Strong. Keep this. |
| Comment density | 3–8% of lines per crate | Not over-commented line by line. The verbosity is elsewhere (below). |
| Functions over 100 lines | 37 | `dispatch` is 427 lines, `main` is 312, `detect_injection` is 285, `attach_inner` is 266, `fill_form` is 226 |
| Crates with no dependents | `ostler-workflow` (4,646 lines) | Pure, tested logic (risk classes, task contracts, traces, receipts) that nothing calls |
| Duplicate implementations | Two source archives: `ostler-research::archive` (durable, unused) and an in-memory one in `ostler-mcp/src/evidence.rs` (used) | Built in parallel by different plans, never reconciled |
| Planning documents | 13 plan, roadmap, scope or review docs in `docs/`, including three separate "level 100" plans (`PLAN-LEVEL-100.md`, `MATURITY-PLAN.md`, `LAUNCH-PLAN.md`) | Nobody can tell which plan is current. Two of these were written in the same week. |
| Third-party crates in `Cargo.lock` | 322 | High for a local tool; every one is attack surface and build time |

### Module by module

| Crate | Lines | Verdict | Why |
|---|---|---|---|
| `ostler-cdp` | 1.2k | **Keep as is** | Small, one job (launch Chrome, talk CDP). |
| `ostler-browser` | 5.7k + 2.2k JS | **Keep, split** | The engine. `page.rs` (1.9k) mixes navigation, input, snapshots and policy; `fill.rs` (1.7k) has three functions over 190 lines; `runtime.js` is one 2,234-line IIFE. Split by concern; nothing needs to be rewritten. |
| `ostler-forms` | 1.3k | **Keep** | Typed form model. Clear. |
| `ostler-extract` | 3.6k | **Keep, promote** | Readability, BM25, quote verification, text fragments, injection detection, PDF. This is the most reusable code in the repo. |
| `ostler-research` | 4.2k | **Keep, prune** | Robots, ledger, budget and search are solid. Pick one archive (this crate's durable one) and delete the other. |
| `ostler-mcp` | 7.0k | **Refactor** | `App` is a god object. `dispatch` is one 427-line `match`. `call_cancellable` (164 lines) wraps every result in a stack of text prefixes (challenge notice, memory notice, popup notes, filters). Errors are mostly `String` with an optional kind, so callers still parse text. |
| `ostler-memory` | 8.5k | **Shrink hard** | The largest crate is for an optional feature: remembering form facts. It has Qdrant, local, sealed (AES-GCM + HMAC), tenant JWTs, embeddings, sparse vectors and rank fusion. That is a security product inside a browser tool. Keep the local encrypted store; move Qdrant, tenants, sparse and fusion behind a feature flag or into a separate repo until a user asks. |
| `ostler-workflow` | 4.6k | **Wire or delete** | Zero dependents. Either connect the 20% that matters (postconditions, risk classes) or remove it from the workspace. Dead code that looks finished is worse than no code. |
| `ostler-a2a` | 1.5k | **Feature-gate** | A2A is a second protocol to maintain for a small audience. Build it only with `--features a2a`. |
| `ostler-cli` | 4.3k | **Split** | `main.rs` is 2,048 lines; `main()` is 312. `install.rs` (writing client configs) is genuinely useful and should stay prominent. |
| `ostler-fixtures`, `ostler-bench` | 2.9k, 3.9k | **Keep, open up** | Real assets: realistic test pages and a false-success benchmark. Currently framed as internal; see section 4. |

### Does it read as AI-written?

Partly, yes, and not where people usually look. The code is not full of filler comments. The tells are in the prose
around the code:

- **Narrating the build session in source.** `ostler-research/src/search/brave.rs` begins "Verification status
  (honest): … confirmed from … third-party examples seen through a search tool. The documentation site itself could not
  be fetched in the build environment". That describes how an AI session went, not what the code does. A maintainer
  writes "Untested against the live API: `age`, `page_age` and the 20-result cap come from third-party examples."
- **Recurring moral vocabulary.** "honest" appears 15 times in code and 35 in docs, including output strings like
  "failed honestly" and "flagged honestly" in the bench reports. "never" appears 272 times in code. Say what happens
  instead: "failed", "flagged".
- **Hedged essays in user-facing text.** Several tool and error messages exceed 250 characters (the longest is 352).
  An agent pays for every byte; a person skims past it. One sentence, then the next step.
- **Sentence-length test names.** `a_page_that_talks_to_agents_is_flagged_in_the_header_but_not_obeyed` is readable,
  but hundreds of them make the test list feel generated. Shorter names with a one-line doc comment where needed.
- **Plans about plans.** 13 planning docs, many restating each other's findings, is the strongest tell of all. Humans
  keep one roadmap and delete what it replaces.
- **Over-explained module headers.** Some `//!` blocks run to 15+ lines with tables (`risk.rs`, `sealed.rs`). Good
  content, wrong place: move design rationale to an ADR and keep the header to what a reader needs before the code.

None of this is fatal. All of it is cheap to fix, and fixing it is what makes the repo feel like a person owns it.

## 3. How easy is it to integrate?

Harder than it looks from the README.

| Problem | Evidence | Why it hurts |
|---|---|---|
| One transport | MCP over stdio. No Streamable HTTP MCP. | Cannot be a shared service; cannot be reached from a hosted agent or another machine without A2A. |
| No library API | The PyPI wheel and npm package only install the binary ("no Python code", per `pyproject.toml`). | A Python developer cannot `import` anything: no `verify_quote()`, no `fill_and_check()`. Frameworks must go through MCP. |
| No agent CLI mode | No `ostler read <url> --json`, no skills file | The direction Playwright, Chrome DevTools and Browser Use have all taken. Shell loops are cheaper than MCP for coding agents. |
| Refuses remote browsers | `LaunchOptions::cdp_url` is loopback only (`ostler-cdp/src/launch.rs`) | Cannot attach to Browserbase, Kernel, Steel or Anchor sessions, so cannot add verification to anyone's swarm. This is the single biggest integration gap. |
| Too much surface | 32 tools; 16 `serve` flags; 38 `OSTLER_*` variables, about 13 of them for memory, Qdrant, embeddings and keys | Each one is a decision a new user has to understand or ignore. |
| One browser, one process | One Chrome per server. Per-page locking is still planned (E03). | Parallel agents mean parallel processes and parallel Chromes. |
| Research needs a key | `search` fails until SearXNG or Brave is configured | The headline feature, cited research, does not work out of the box. |
| Docs sprawl | 39 docs, 4,500 lines, three level-100 plans | A developer evaluating it cannot find the one page that says what to do. |

**A correction to my own earlier plan.** `LAUNCH-PLAN.md` (L10) proposes a keyless search fallback that runs queries on
a public search page. `docs/TROUBLESHOOTING.md` already records that Google, DuckDuckGo and Bing show verification
walls to headless browsers, and Ostler rightly does not get around those. That fallback would fail often and look
broken. Replace it: ship with a clear one-line setup for a free search API, and make `read`, `open_many` and
`cite` work fully on URLs the user or agent already has. Most coding-agent research starts from a known docs URL anyway.

## 4. How much open-source value does it bring?

Separate what is commodity from what is new.

**Commodity (others do it as well or better):** navigate, click, type, press, hover, scroll, tabs, screenshot,
snapshot with refs, wait_for. Playwright MCP has all of these across three engines.

**Genuinely new, and worth open-sourcing properly:**

1. **Checked citations**: a quote verified against the exact text that was read, with a text-fragment link, and
   staleness tracked when the page changes (`ostler-extract::quote`, `fragment`, `ostler-mcp/src/evidence.rs`).
2. **Read-back form filling**: every value verified from the DOM after filling, with drift detection before submit
   (`ostler-browser/src/fill.rs`).
3. **Honest outcome classification**: submitted, advanced, validation failed, unchanged, or server error, plus
   uncertain when a page changed mid-action.
4. **A false-success benchmark**: tasks scored on whether the agent *claimed* success when it had not succeeded
   (`ostler-bench`, `ostler-fixtures` packs). This is something the whole field lacks.
5. **Untrusted-content wrapping and injection flags** on everything a page returns (`ostler-extract::inject`).
6. **Polite research plumbing**: robots.txt per RFC 9309, per-host limits and a ledger of what could not be read.

**Why that value is not reaching anyone yet:** it is locked behind Ostler's own MCP server. The crates are on
crates.io but described as internal with no stability promise. There are no Python or TypeScript bindings, and the
benchmark is not packaged for anyone else to run against Playwright MCP, Browser Use or Stagehand.

## 5. What tasks Ostler should genuinely target

Pick tasks where **being provably right matters more than being fast or covering every site**. Concretely:

| Target | Example | Why Ostler wins |
|---|---|---|
| **Cited research for coding agents** | "Check the current API docs for X and quote the parameter limits" | Quotes are checked against what was read; stale docs are flagged. Coding agents make up API details constantly. |
| **Form and portal work with review** | Fill a vendor form, a support ticket or a sign-up from known facts, show the values, submit on approval | Read-back and the approval gate mean the human can trust what was entered. |
| **QA of your own app** | "Try sign-up on localhost after my change and tell me what actually happened" | Outcome classification says *submitted* vs *unchanged* vs *validation failed*, not "looks done". |
| **Verification layer for other agents' browsers** | Attach to a Browserbase or Kernel session and check what another agent claims | No one else does this; it turns competitors into distribution. |
| **Evaluation of browser agents** | Run the false-success benchmark against any agent | An open benchmark others cite is the best marketing an infra project can have. |

**Do not target:** large-scale scraping, stealth, CAPTCHA solving, proxy rotation, running fleets, or general
"do anything on any site" autonomy. Hosted providers and frameworks own those, with money and teams Ostler does not
have. Say so in the README.

---

## 6. The plan

Four phases. Each ends with something a user can see. Nothing here adds a new subsystem; most of it removes one.

### Phase A: Cut and clean (2 weeks)

| # | Change | Done when |
|---|---|---|
| A1 | **One plan.** Keep `ROADMAP.md` as the only forward-looking doc. Fold the useful parts of `PLAN.md`, `BUILD-PLAN.md`, `AGENTIC-PLAN.md`, `MATURITY-PLAN.md`, `PLAN-LEVEL-100.md`, `LAUNCH-PLAN.md`, `GAP-MATRIX.md`, `COMPARISON-AND-ROADMAP.md` and the `SCOPE-*` docs into it, then move the rest to `docs/archive/`. | `docs/` has one roadmap; every other doc describes how something works today. |
| A2 | **Delete or wire `ostler-workflow`.** Recommended: move `expect.rs` and `risk.rs` into `ostler-mcp` where they will be used (E12, E27), delete the rest from the workspace. | No crate in the workspace has zero dependents (except `ostler-bench`). |
| A3 | **One archive.** Use `ostler-research::archive` from `evidence.rs`; delete the in-memory archive added in #35. | One implementation, one test suite. |
| A4 | **Shrink memory.** Default build has the local encrypted store only. Qdrant, tenant tokens, sparse vectors and fusion go behind `--features qdrant`. Cut the environment variables that only those features read from the default docs. | Default build has fewer than 15 `OSTLER_*` variables; `cargo build` without features does not compile Qdrant code. |
| A5 | **Feature-gate A2A.** `--features a2a`. | Default binary and docs do not mention A2A. |
| A6 | **Split the big functions.** `dispatch` becomes one small handler per tool (`tools/navigate.rs`, …) behind a trait or a table; `main()` delegates to one function per subcommand; `runtime.js` is split into files concatenated at build time. | No function over 150 lines outside tests and fixtures. |
| A7 | **Human style pass.** Add `docs/STYLE.md` (below). Rewrite module headers, user-facing strings and bench output to it. Remove build-session narration (`brave.rs` and any like it). | `grep -ri "honest" crates` returns only places where it is a defined term; no user-facing string over 200 characters. |

### Phase B: Make it easy to plug in (3–4 weeks)

| # | Change | Done when |
|---|---|---|
| B1 | **Agent CLI mode.** `ostler read <url> [--query q] --json`, `ostler cite <url> "<quote>"`, `ostler fill <url> --values file.json`, `ostler check <url>`. Each prints one JSON result envelope (the `ostler.result/1` shape already on `main`). Ship a skills file for Claude Code and Codex. | A coding agent can do the three hero tasks with shell commands only, with fewer tokens than the MCP path, measured. |
| B2 | **Attach to remote browsers.** Allow `--cdp-url wss://…` for named providers, with the policy still enforced, and document Browserbase, Kernel and Steel. Keep loopback-only as the default. | The F1 form test and a cite test pass against one hosted provider's session. |
| B3 | **Streamable HTTP MCP transport.** `ostler serve --http 127.0.0.1:PORT` with a token. | A hosted agent can call it; stdio stays the default. |
| B4 | **Small default tool list.** The default profile lists about 10 tools: `navigate`, `read`, `snapshot`, `act` (click/type/press as one tool with an action field), `fill`, `submit`, `cite`, `evidence`, `extract`, `search_tools`. Everything else is discoverable through `search_tools`, as today. | Initial tool schema under 4 KB. |
| B5 | **Research without a key, honestly.** Drop the public-search-page fallback from the launch plan. `read`, `open_many` and `cite` are the default research path; `search` explains one free provider setup in one line. | Use cases 1 and 2 from the launch plan pass with no key when the agent has URLs. |

### Phase C: Make the open-source value reusable (4 weeks, in parallel with B)

| # | Change | Done when |
|---|---|---|
| C1 | **`ostler-verify` library.** One crate with a stable API: `verify_quote`, `text_fragment`, `classify_submit`, `read_back`, `flag_injection`. Python bindings (PyO3) and a TypeScript package (WASM where possible). | `pip install ostler-verify` and `npm i ostler-verify` work; a Playwright MCP or Browser Use user can check a quote in five lines. |
| C2 | **Open false-success benchmark.** Package the fixture packs and scoring as `ostler-bench` that runs any MCP browser server or any CLI agent, and publish results for Ostler, Playwright MCP and Browser Use. | Results table in the README, reproducible with one command. |
| C3 | **Adapters, not rivals.** Small examples: "add checked citations to Browser Use", "verify Stagehand form fills", "check a Kernel session". | Three examples in `examples/`, each under 50 lines, run in CI. |

### Phase D: Focus the product story (1 week, after A)

| # | Change | Done when |
|---|---|---|
| D1 | **README in 60 seconds.** One sentence: *"Ostler is a browser for AI agents that proves what it did: cited research with checked quotes, forms read back before you submit, and honest outcomes."* Then install, one `ostler try`, and the three hero tasks. Move status, design review and benchmarks below the fold. | A new reader can say what Ostler is for after the first screen. |
| D2 | **Say what it is not.** A short "Use something else for…" section pointing to Playwright MCP (cross-browser testing), Chrome DevTools MCP (performance debugging) and hosted providers (fleets, stealth). | Present in README and site. |
| D3 | **Three demos, one per hero task**, recorded from real tool output. | Replace the current two GIFs. |

### What this replaces in the existing plans

- `LAUNCH-PLAN.md` keeps its levels and gates, but its L10 keyless-search item is replaced by B5, and phases A–D come
  before L20.
- E10 (hard interactions), E29 (replay) and E31 (inspector) move down. They are framework and infra work where
  others are ahead.
- E19–E21 (claims, assessment, source families) move up. They extend the one lane Ostler owns.

## 7. Style guide (to become `docs/STYLE.md`)

Short enough to follow.

- **Comments say why, not what.** If the code says it, delete the comment. Design history goes in an ADR.
- **Module headers: five lines or fewer.** What this module does, what it does not do, and where to look next.
- **User-facing text: one sentence, then the next step.** No more than 200 characters. No "Note:", "please", "honestly".
- **Name things for what they are.** `fill_and_verify`, not `fill_form_with_verification_and_drift_detection`.
  Test names under about 60 characters, with a doc comment if the case needs a story.
- **No session narration.** Never write how something was researched or what the build environment could not
  do. Write what is tested and what is not.
- **One plan.** If you write a new plan doc, delete or archive the one it replaces in the same PR.
- **Prefer deletion.** A PR that removes code and keeps the tests green is the best PR.

## 8. How we will know it worked

| Signal | Today | Target after A–D |
|---|---|---|
| Lines of Rust in the default build | ~51k | under 35k, with no lost tests |
| Default tools listed / initial schema | 16 core / capped at 5.4 KB by `schemas_are_small` | about 10 / under 4 KB |
| `OSTLER_*` variables a default user can meet | 38 | under 15 |
| Planning documents | 13 | 1 roadmap plus ADRs |
| Ways to use the verification code outside Ostler's MCP server | 0 | Python, TypeScript, CLI, remote CDP |
| Others' agents measured on the false-success benchmark | 0 | at least 3, published |
| Time for a new developer to finish a real task (not setup) | not measured | under 10 minutes for 8 of 10 testers (launch plan L60) |
