# Launch plan: level 1 to level 100

This plan takes Ostler from where it stands today to a public 1.0 launch. It complements
[MATURITY-PLAN.md](MATURITY-PLAN.md), which says *what code changes* each engineering item needs. This document
says *in what order*, *what else a public launch needs*, and *how we prove each step*.

## How levels work

- There are ten levels, **L10 to L100**. Each level is one release, and each has a short list of **gates**.
- A gate is pass or fail, checked by a command, a test, a benchmark number or a named document. "Mostly done" is a fail.
- You are at level N only when **every gate of N and of every level below it passes on the same commit**. There is no
  partial credit and no in-between scores (no "level 37").
- A gate that passed once can fail later. A regression drops the level until it is fixed; a release is never
  tagged at a level it does not currently meet.
- Every level has gates on three tracks:
  - **Engineering**: the E-items from [MATURITY-PLAN.md](MATURITY-PLAN.md).
  - **Ease of use**: how little an agent, or the person behind it, has to know and do to get a job finished. This
    covers the workflow itself, not just installation. See "Ease of use" below.
  - **Launch**: trust, quality evidence, distribution, docs, support and legal. A product with perfect code and no
    install path, security policy or honest docs is not launch ready.

Two levels are public milestones:

| Milestone | Level | Version | Meaning |
|---|---|---|---|
| **Public beta** | L60 | 0.7 | Safe to announce widely with a "beta" label and a narrow, written support envelope. Real users, real feedback, breaking changes still allowed with notice. |
| **1.0 launch** | L100 | 1.0 | The spec's definition of level 100: a dependable browser and research runtime **within a published support envelope**. Stable schemas, semver commitments, independent evidence of correctness. |

We recommend launching publicly at L60 rather than waiting for L100. A beta with a strong, honest support envelope
earns real-world bug reports that the L70–L100 work needs. Launching at L100 without that feedback would rely on
fixtures alone.

## Ease of use: the rules every level follows

Setup is only the first five minutes. Most of a user's time is spent running jobs, so every level is also judged on
how simple the *jobs* are. Six rules, each backed by a gate that can fail:

1. **Say the goal, not the steps.** A person or agent should be able to ask for an outcome ("compare these three
   pages", "fill this form and let me approve", "check my site") and get it in a handful of calls. Each supported
   use case has a **call budget** and an **output budget** (see the use-case table). A use case that needs more
   calls than its budget is a bug, even if it works.
2. **Works with no configuration.** The first useful result must not depend on a key, a server or a flag. Today
   `search` fails until `OSTLER_SEARXNG_URL` or a Brave key is set (`research.rs` `provider()`), and memory tools say
   "memory is not configured" until `--memory` is passed (`app.rs` `NO_MEMORY`). Both need a working default and an
   honest message when a default is weaker than the configured option.
3. **Every result says what to do next.** Success, failure and uncertainty each end with one plain sentence naming the
   next step (this is the `next_action` field in E01, but written for humans too). No error asks the caller to
   "check the page" without saying how.
4. **Fix what can be fixed safely.** If a ref went stale and exactly one element matches the same role and name, use
   it and say so (E08 re-resolve). If a cookie banner covers a button, say which one and offer the dismiss action
   instead of failing. Never guess when two things match.
5. **Docs are organised by job, not by tool.** The cookbook and getting-started pages list tasks ("Compare products",
   "Check my site after a change"), each with a copy-paste prompt, the expected result and the three most common
   problems. The tool reference stays, but no one needs it to finish a job.
6. **Measured, not felt.** Ease is reported as numbers on every release: time to first success, calls per use case,
   output tokens per use case, and first-attempt success with a real model. A change that makes any of them worse
   needs a CHANGELOG note.

## Use cases we must complete

A use case is **supported** only when it passes on a fixture with a real model, inside its call and output budgets, with
a plain-language result a non-expert can read. The set below is the public promise. Each row becomes a ready-made
prompt (MCP `prompts/get`, plus `ostler try <name>` that runs it on the bundled fixtures with no AI client needed).

| # | Use case | Example request | Call budget | Needs | Level it must pass |
|---|---|---|---|---|---|
| 1 | Read and summarise a page | "Summarise this article." | 2 | `read` | L10 |
| 2 | Research with checked citations | "Why did X happen? Cite your sources." | 12 | search default, `open_many`, `cite` | L10 (with default search), L70 (PDFs, archive) |
| 3 | Compare pages or products | "Compare these three plans." | 6 | `open_many`, `extract` | L30 |
| 4 | Fill a form, show me, wait for approval | "Fill this in with my details and let me check it." | 5 | `form_fill`, approvals | L10, L40 (scoped approvals) |
| 5 | Check my web app after a change | "Try the signup flow on localhost." | 10 | local-dev policy profile, `batch` | L40 |
| 6 | Pull a table or list into rows | "Get the pricing table as CSV." | 4 | `extract`, schema extraction | L30, L80 |
| 7 | Log in once, reuse the session | "Sign in to my portal and download the invoice." | 8 | named profile, handoff, downloads | L60 |
| 8 | Do a repeated job again | "Do last week's report export again for this month." | 3 | verified replay (E29) | L60 |
| 9 | Watch a page for a change | "Tell me if this price drops." | 3 per check | archive versions, change report | L70 |
| 10 | Multi-page task with a budget | "Find the three cheapest options, stop after 10 pages." | 15 | mission budget, partial report | L50 |
| 11 | Hand a form to another agent | A2A `SendMessage` | 1 | A2A | L10 (already supported) |

Row 8 matters most for ease: it is what lets a job done once be repeated without re-explaining the steps. The plan
therefore pulls **semantic `find` (from E10) forward to L30** and **verified replay (E29) forward to L60**. Both stay
behind the E27 approval rules, so a replay can never skip a consequential step.

## Where Ostler stands today (checked on `main`, `d037fb6`)

**Level: below L10.** Three of the seven L10 gates pass.

| L10 gate | Status | Evidence |
|---|---|---|
| Full workspace tests green with Chrome required | **pass** | `OSTLER_REQUIRE_CHROME=1 cargo test --workspace`: 752 passed, 0 failed, 0 skipped |
| `cargo clippy --workspace --all-targets -D warnings` and `cargo fmt --check` | **pass** | both exit 0 |
| Supply-chain checks green (cargo-deny, cargo-audit), including the new `lopdf` | **pass, to confirm in CI** | `lopdf` is MIT (allowed in `deny.toml`); its transitive licences are checked only by CI's `security.yml` |
| Review follow-ups from PR #35 closed | fail | see "L10" below |
| Public docs match the code | fail | `DISTRIBUTION-CHECKLIST.md` repeats "Open VSX done" in unrelated rows; `THREAT-MODEL.md` says there is no security workflow, but `security.yml` exists; `ROADMAP.md` lists `extract` as missing, but it ships |
| Dependency ADR for `lopdf` (agentic plan R4) | fail | not written |
| CHANGELOG `Unreleased` lists the #35 changes | fail | to check and add |

What already exists and counts toward later levels: v0.2.0 is released on GitHub, PyPI and Open VSX. CI runs
Linux tests with Chrome and Qdrant, plus a Windows build and tests. There is a security policy with private
reporting, cargo-deny and cargo-audit, Dependabot, SLOs (proposed), benchmarks, a threat model and fuzzing.

---

## The levels

Each level lists its engineering items, its launch items and its gates. E35 (independent correctness tests) and
E36 (performance and compatibility) are not a level of their own. **Every level adds the adversarial fixtures,
fault injection and benchmarks for what it builds**, as MATURITY-PLAN says.

### L10 — Clean baseline (0.2.1)

*Engineering*
- Route A2A submits through the double-submit guard (`app.rs` `run_form_task_inner`, which calls `submit()` today).
- An MCP-level test for `form_submit` + `again` (the existing browser test would have passed before the fix).
- Show `superseded` in `evidence` and `read` output; store archived pages as text and hash only, not full `Doc`s.

*Ease of use*
- **Research works with no key.** When no search provider is configured, `search` falls back to running the query on a
  public search page in a scratch tab and returns the results, labelled `provider: browser (no key)`. Build it as a
  `SearchProvider` in `ostler-mcp` (the trait lives in `ostler-research`, which has no browser dependency) and use it
  from `App::provider` (`research.rs`) when `ProviderConfig::from_env` returns `None`. If the page shows a
  verification wall, stop and say so; Ostler does not get around those. The message for a configured provider
  stays as is, and the label tells the agent this result is the weaker option.
- **`ostler try <use case>`**: runs a bundled walk-through on the local fixtures and prints what the agent would see,
  with no AI client and no key. It reuses `ostler fixtures` (`Cmd::Fixtures` in `ostler-cli/src/main.rs`) and the
  scripted runner in `ostler-bench`. It is the 60-second "does it work, and what does it feel like" check.
- **`ostler doctor --fix`**: each failing check offers its fix as a prompt (for example, download a test Chrome
  when none is found) instead of only printing the command.
- Rewrite the first screen of the README around the use-case table above: one sentence, one install command, one
  `ostler try`, and a link to the cookbook. Move the long status and design-review text further down.

*Launch*
- Fix the three doc inaccuracies above. Add a CHANGELOG entry. Write the `lopdf` ADR (licence, maintenance,
  decompression limits).
- Add a `docs/LAUNCH-PLAN.md` status line to the README "where Ostler stands" section, linking here.

*Gates*: the seven L10 gates in the table above, plus use cases 1, 2, 4 and 11 passing within their call budgets on a
machine with no search key and no memory flag. Tag `v0.2.1`.

### L20 — Contracts and concurrency (0.3)

*Engineering*: E01 typed result envelope; E02 request scheduling, cancellation and progress; E03 per-page locking.

*Ease of use*
- Every result ends with one plain sentence naming the next step (the human-readable form of `next_action`). A test
  walks every `ToolError` and `Status` value and fails if any has no next-step text.
- Trim tool descriptions to what an agent needs to choose the tool; move examples to `prompts/get`. The
  `schemas_are_small` caps in `tools.rs` stay, and the structured listing gets its own cap.
- Memory has a local default. `--memory local` (the encrypted file store) is offered by `ostler start` as a single yes/no
  question; saying no keeps today's behaviour, and the memory tools then say how to turn it on in one line.

*Launch*: write ADR-0020 (outcome envelope) and ADR-0021 (concurrency model). Record a "result shape" page in `docs/TOOLS.md`.

*Gates*
- An MCP test where `ping` answers in under 100 ms while a 10 s `wait_for` runs.
- Cancelling a write returns `WRITE_OUTCOME_UNKNOWN`; cancelling a read returns `CANCELLED`.
- Two `read`s on separate tabs overlap in time; two `click`s on one tab do not.
- Validation rejection, execution error and uncertain completion give three distinct `structuredContent.status` values.
- Text-only clients see unchanged output (existing e2e assertions still pass).

### L30 — Action correctness (0.4)

*Engineering*: E07 actionability; E08 refs tied to their document, and navigation tied to the action that caused it; E09 typed effects and `batch`.

*Ease of use*
- **Semantic `find`** (pulled forward from E10): `find {role?, name?, text?}` returns ranked refs and refuses to pick when
  the top two tie. This is what lets an agent say "the Sign in button" instead of reading a snapshot for `e14`.
- **Safe re-resolve** (E08): a stale ref with exactly one matching element is used, and the result says it was re-matched.
- **Overlay-aware blockers**: when a cookie banner or modal blocks a click, the result names it and gives the one
  action that closes it, so the agent does not hunt for the right ref.
- Use cases 3 and 6 added to the walk-through, with their call budgets.

*Launch*: update `docs/FAILURE-MODES.md` with the new error codes and what an agent should do for each.

*Gates*
- Fixtures: a covered button, a moving button, a stale `e1` after navigation, a late navigation, and a no-op click.
  Each gives the documented code, never a silent wrong click.
- The false-success rate (`ostler-bench` false-success suite) does not rise, and no no-op click is reported as `ok`.
- A failed step 2 in a `batch` marks steps 3 and later `not_executed`.
- `find` returns a ranked list and refuses to choose between two equal matches. A stale ref with one match is re-matched
  and labelled; with two matches it fails and says which two.
- Use cases 3 and 6 pass with a real model inside their call budgets.

### L40 — Security foundation (0.5)

*Engineering*: E25 policy profiles, plus redirect and DNS-rebinding defences; E26 isolated world for the runtime; E27 scoped approvals and handoff states.

*Launch*
- Rewrite `THREAT-MODEL.md` against the new design. Retire "same-world runtime" from the residual risks if E26 lands.
- Book an external security review for L90 (it needs lead time).
- Pin GitHub Actions by commit SHA. `SECURITY.md` currently says they are pinned by tag.

*Gates*
- In `security_e2e`: public research cannot reach loopback; `--allow-local` reaches only the named port; an
  allowed host's redirect to `169.254.169.254` is blocked; a rebinding resolver stub is blocked.
- A hostile fixture that overrides prototypes and `window.__wm` cannot change refs or click targets. A secret
  planted in `localStorage` appears in no tool output.
- An approval for one amount or recipient does not authorise another; grants expire and do not cross missions.
- Soak security pass (SLO 2): 0 canary leaks, and the control run reaches the canary.

### L50 — Reliable work (0.6)

*Engineering*: E04 mission state machine and journal; E05 crash recovery; E06 resource budgets; E11 form consistency; E12 postconditions wired from `ostler-workflow`.

*Ease of use*
- **Missions are one idea to the user**: "start a job with a goal and a budget; ask where it is; finish". The agent does
  not manage journals, checkpoints or ids by hand. `mission begin` returns a short handle, and every later result
  carries the mission status line, so a new agent context can continue from that line alone.
- After a crash or a pause, the first result says in two sentences what finished, what is uncertain and what to do
  next. No agent should have to read a transcript to recover.
- Use case 10 (multi-page task with a budget) added, ending in a partial report when the budget runs out.

*Launch*: ADR-0023 (journal and recovery). A "what Ostler does after a crash" section in `docs/OPERATIONS.md`.

*Gates*
- Crash test (kill Chrome before, during and after a submit): the "before" case resubmits normally, the "during"
  case reports `uncertain` and refuses a blind repeat, and the "after" case is settled by a reopen check.
- A new `App` resumes a mission's goal, progress and pending actions from the journal alone.
- A "Saved"-without-saving fixture fails `RecordReopened`. A field reset by a later field blocks the submit.
- Budget exhaustion returns `BUDGET_EXCEEDED` and keeps every completed result.
- SLO 5: a 20-minute soak with one forced Chrome kill has 0 unexplained failures.
- A fresh agent context, given only the mission handle, continues a half-finished job correctly (use case 10).

### L60 — PUBLIC BETA (0.7)

*Engineering*: E13 downloads; E28 named profiles with locks; E34 health states and degraded operation.

*Ease of use*
- **Do it again** (E29 pulled forward): after a job succeeds, `workflow save <name>` stores it with semantic locators,
  expected results and secret references (never the secrets). `workflow run <name> {params}` repeats it. If the page
  changed so a step no longer matches, replay stops and says which step and what it found instead. A risky step still
  needs a fresh approval, so a replay cannot skip one.
- **Sign-in is one handoff**: when a login or verification is needed, the agent gets a `needs_user` result that says
  "sign in in the browser window, then continue". Cookies persist in a named profile (E28), so the second run needs no
  handoff. Use case 7 is the test.
- **Downloads just work**: a click that starts a download returns the file's name, size and where it was saved, and
  `downloads read` hands a PDF or HTML file to the reader.
- A beginner's cookbook: each use case in the table is a page with the prompt to paste, what the agent does in plain
  words, what you will see, and the three most common problems.

*Launch*: the public beta checklist. Every row is a gate.

| Area | Gate |
|---|---|
| Install | Every path in the README works from a clean machine: PyPI, npm (`npx -y ostler serve`), Homebrew, crates.io, GitHub binaries, `.mcpb`, VS Code Marketplace and Open VSX. Each one is checked on a fresh VM for macOS, Linux and Windows, and the result is recorded in `DISTRIBUTION-CHECKLIST.md`. |
| Onboarding | `ostler start` to the first successful tool call takes under 5 minutes for 10 people who have never used Ostler (at least 3 on Windows), observed and written up. |
| Listings | Official MCP registry, Glama, Smithery, mcp.so, PulseMCP, the Cursor and VS Code directories, and awesome-mcp-servers: submitted. Claude Desktop directory: submitted once the `.mcpb` is verified on a Mac. |
| Support envelope | `docs/SUPPORT.md` says what is supported: OS and client matrix, Chrome stable, the tool list, and the "do not promise" list from the spec (universal site access, CAPTCHA solving, exactly-once writes, guaranteed truth). It also states the beta label and the breaking-change policy. |
| Privacy | A plain statement that Ostler makes no telemetry or update calls, and what is stored locally (memory, journal, archive, downloads) and how to delete it. |
| Security | `SECURITY.md` gives a response target, such as acknowledging within 3 working days. Private vulnerability reporting is enabled and has been tested once. |
| Issues | Issue templates for bug, site-compatibility and feature requests. Labels for P0–P3. A triage rota, even a single named person. |
| Release process | A release can be cut from `docs/RELEASING.md` by someone other than its author, and that dry run has been done once. |
| Quality | The 2-hour soak on macOS and on Linux meets SLO 5 and SLO 6 (no monotonic memory growth). SLO 1 shows 0 false "Verified" claims on the realistic suite. |
| Docs | The website and README are checked against the code: every tool, flag and install command in them exists and works. |
| First workflow | Of the 10 testers, at least 8 finish a real job of their choice (not only setup) from the cookbook in under 10 minutes, with no help. Every point where one asked for help is turned into a fix or a doc line. |
| Use cases | Use cases 1 to 8 and 10 and 11 pass with a real model inside their call and output budgets, on macOS, Linux and Windows. Ease numbers (time to first success, calls and tokens per use case, first-attempt success) are in the release notes. |
| Repeat jobs | A job saved with `workflow save` runs again with new parameters. A deliberately redesigned page stops the replay and reports the step, and no risky step runs without an approval. |

*Gates*: every row above, plus all lower levels. Tag `v0.7.0-beta`. Announce.

### L70 — Research core (0.8)

*Engineering*: E15 PDF ingestion wired into `read` (built on the #35 module, with a parse deadline and an encrypted-PDF test); E16 pooled `open_many`; E17 durable source archive on disk; E18 cross-source `retrieve`; E14 search hardening.

*Ease of use*
- **Watch a page** (use case 9): `watch <url>` reads it now, stores a version, and on the next check reports in plain words
  what changed since the last version. It uses the archive from E17, so it needs no new storage.
- PDFs read like web pages: the agent calls `read` on a PDF link and gets page-numbered text, with no new tool to
  learn. Scanned PDFs say so and say what to do.
- Research results end with a short "what is still unknown" list, so the agent can give a straight answer.

*Launch*: ADR-0024 (archive versioning). Update `docs/WEB-SEARCH.md` with the error states and fallback config.

*Gates*
- A quote on page 7 of a fixture PDF cites `page: 7`. A scanned PDF gives a `scanned` gap, not empty "evidence".
- Ten fixture URLs with a 1 s delay finish in under 4 s. One blocked URL does not cancel the other nine or mix up their content.
- After a restart, an old citation still verifies against its stored version, and a changed page creates a new version.
- Provider outage, exhausted quota, no results and unread sources are four distinct results.
- Recall@5 on the held-out cross-document question set is recorded and becomes the baseline.
- Use case 9: a fixture page that changes between two checks produces a one-paragraph change report. An unchanged page
  says "no change" and costs under 500 output tokens.
- `read` on a PDF link works with no extra arguments, and the agent never has to know it was a PDF.

### L80 — Research quality (0.9)

*Engineering*: E19 claims; E20 deterministic guards and model assessment; E21 source families and conflict handling; E22 research runner and skill; E23 stopping rules and export; E24 schema extraction.

*Launch*: ADR-0025 (claims and assessments). A worked research example in `docs/COOKBOOK.md` with its exported report.

*Gates*
- On a human-labelled set covering negation, wrong date, wrong unit, population shift and causal overreach,
  agreement is reported and **no claim with only a quote match is shown as "verified"**.
- Copies of the same article count as one evidence family.
- Budget exhaustion produces a useful partial report with an open-questions list, not a "complete" answer.
- Missing schema fields are `unknown`, and a truncated table is `partial`.

### L90 — Release candidate (1.0-rc)

*Engineering*: E10 hard interactions, as fixtures demand (cross-origin frames first); E29 verified replay; E30 memory integrity; E31 local inspector; E32 diagnostics; E33 versioned schemas, Python and TypeScript SDKs, and CLI JSON output.

*Launch*
- **External security review completed**, with every finding fixed or documented with its risk.
- The compatibility matrix is **tested in CI**, not just built: macOS arm64 and Windows join Linux; three MCP protocol versions are covered.
- State migration test: a workspace written by 0.9 opens in 1.0-rc.
- `outcome` schema v1 frozen and published in `docs/schemas/`.

*Gates*: all of the above, plus 2 weeks with no open P0 or P1 issues from beta users.

### L100 — 1.0 LAUNCH

*Engineering*: E35 and E36 reach their final form as release gates.
- **Held-out real-model evaluation** (`scripts/eval/`): tasks the team did not tune on, with outcomes checked by
  the fixture server's own records. It reports **useful completion and false completion together**. A
  runtime that refuses everything scores 0. Unsupported, flaky and regressed tasks are counted separately.
- Performance budgets hold: p95 latency per tool (SLO 3), memory over a 2-hour session (SLO 6), cancellation
  delay, and archive and journal growth. Each is measured, not assumed.

*Launch*
- Semver 1.0 commitments written into `COMPATIBILITY.md`: what is public surface and the deprecation window.
- 30 days of beta with no open P0, and P1 issues triaged within the stated target.
- **Ease numbers are part of the evaluation.** All 11 use cases pass on the held-out tasks with a real model, and the
  release notes show, for each, calls used against budget, output tokens, and first-attempt success. A use case that
  only passes with a retry is reported as a retry, not a pass.
- A first-time user can go from install to a finished real job in under 10 minutes on each supported OS (repeat of the
  L60 test with fresh testers, at least 8 of 10 succeeding).
- Launch materials (post, demo, docs landing page) claim nothing that the gates did not measure.

*Gates*: every gate of every level, green on the release commit. Tag `v1.0.0`.

---

## Stop-ship rules (any level)

A release is blocked, whatever its level, by any of:

1. A false "Verified" or "Submitted" claim on any fixture (SLO 1).
2. Any canary leak in the security pass (SLO 2).
3. A known way to submit twice without the caller asking for it.
4. A cited quote whose source text can no longer be shown.
5. A doc or website claim that the code does not support, or a use case in the public table that exceeds its call
   budget or needs setup the docs do not mention.
6. A failing `cargo test --workspace` with Chrome required, or a red security workflow.

## Next two weeks

1. Finish L10, one small PR: the three engineering follow-ups, the doc fixes, the CHANGELOG entry and the `lopdf` ADR.
   In the same release, the ease-of-use items: keyless `search` fallback, `ostler try`, `doctor --fix`, and the
   README first screen built around the use cases. Tag `v0.2.1`.
2. Write the 11 use-case fixtures and their call budgets before building more features, so each later level has a
   target to hit. Start L20 with E01; every later item reports through the outcome envelope, so it goes first.
3. Book the external security review for the L90 window, and pick the 10 onboarding testers needed for L60.
4. Fix `DISTRIBUTION-CHECKLIST.md` so its status column is true, then work through the accounts and secrets that the
   install and listing gates need (`VSCE_PAT`, `HOMEBREW_TAP_TOKEN`, npm and crates.io). These need a person with the accounts, not code.

## Owners

Every level needs a named owner before it starts. Owners are not assigned in this document; add them here.

| Level | Owner |
|---|---|
| L10–L30 | TBD |
| L40 | TBD (plus external reviewer for L90) |
| L50–L60 | TBD |
| L70–L80 | TBD |
| L90–L100 | TBD |
