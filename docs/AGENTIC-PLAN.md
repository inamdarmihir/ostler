# Agentic plan: from verified tools to verified missions

Status: **proposal, draft 1**, 2026-11-08. An addendum to [BUILD-PLAN.md](BUILD-PLAN.md): it keeps that plan's pillars and defaults (section 11: no LLM inside, local first, no evasion) and changes the order and the shape of the workflow layer (P5). The research behind it, with sources, is in [research/agent-platforms-2026-11.md](research/agent-platforms-2026-11.md).

## 0. In one page

- **What "more agentic" means for Ostler.** The agent loop stays in the client (Claude Code, Codex, Cursor, an Agent SDK app). Ostler makes each turn of that loop **goal-aware, self-checking, recoverable, resumable and interruptible**: the agent states an outcome, Ostler checks it, keeps the state outside the model's context, recovers from its own failures, and hands control to a person when needed.
- **The new unit is a mission, not a tool call.** `mission begin {goal, done_when, budget, risk_ceiling}` opens an append-only session log; every action inside it is policy-checked, traced and verified; `mission end` returns **pass, fail or unknown** from Ostler's own check, not from the model's opinion. This is the false-success metric turned into a product feature.
- **Two MCP features give Ostler agency without putting a model inside it**: *sampling* (Ostler asks the client's model one narrow question, for example when a replayed skill diverges) and *elicitation* (Ostler asks the user directly for an approval, a missing fact or a 2FA code). Both degrade to plain text replies on clients that lack them.
- **A third front door: an open executor for Anthropic's browser toolset.** Anthropic's `browser_toolset_20260801` leaves the browser to the application. Ostler becomes the executor any Messages API app can drop in, with read-back, postconditions and policy on every member call.
- **Differentiation in one sentence:** the others ship browsers and agents; Ostler ships **verdicts**: an outcome check, a cited evidence report, a receipt and a replayable record for every mission, locally, for any model and any browser backend, from an agent that says it is one.
- **Order of work:** the review's critical fixes first (they are the verification promise), then click and research quality, then missions and the session log, then handoff and live view, then the executor, then skills with replay, then WebMCP and identity. About 26 weeks for one maintainer, including the fixes; cut line in section 6. Section 9 details the browsing and research work; runnable examples of what works today are in [examples/](../examples/).

## 1. Lessons from Anthropic and Cloudflare, and what Ostler does with each

| lesson | where it comes from | Ostler's answer |
|---|---|---|
| Use the most precise path first: connector, then browser, then screen control | Cowork tool precedence | Prefer **site-declared tools (WebMCP)**, then the form engine and refs, then coordinates. Say plainly in docs: if a connector exists, use it; Ostler is for the long tail without an API. |
| The session is not the context window | Managed Agents | An **append-only mission log** on disk, outside the model's context, with `wake` / resume after a crash of Ostler, Chrome or the client. |
| Stable small interfaces outlive harnesses | Managed Agents (`execute`, `wake`, `getEvents`) | Ostler's mission API mirrors those verbs, so it fits any harness: MCP, A2A, the Anthropic toolset, a CLI. |
| Credentials never enter the agent's reach | Managed Agents vault and proxy | `{{fact:KEY}}` fills (ADR-0019) become the general rule: secrets, cookies and profile facts are referenced by name, never echoed; the session log stores references, not values. |
| Two independent checkpoints: data in, actions out | Opus 5.5 system card, auto mode | Ostler already fences and flags incoming text. Add the outgoing checkpoint **in the browser** (CDP `Fetch` interception, review flaw 2) plus **taint**: after a page with flagged injection text, cross-origin writes in the same mission need approval. |
| Ask before acting on a new site; block high-risk sites; import logins per site, banking and email off by default | Cowork built-in browser | Per-site first-action approval, a shipped high-risk category list, `ostler session import --site` that defaults banking, email and SSO to off. |
| Watch it live, replay it later | Browser Run Live View and Session Recordings | `ostler watch` (local live view: screencast, action trace, verdicts side by side) and a recording per mission (rrweb-compatible DOM events plus the trace). |
| The agent should be able to ask for help, and get control back | Browser Run Human in the Loop (handoff signal "coming next") | `handoff {reason, kind}` as a typed state today: challenge, login, 2FA, approval, ambiguity. Desktop notification, the person acts in the headed window, `resume` continues the same mission. |
| Failure degrades to an empty result, never a dead session | Kitesurf design rule | Browser relaunch and tab restore from the mission log (review flaw 4). A crash becomes an `is_error` result with a reason, and the next call works. |
| One component owns the network | Kitesurf SandboxOutbound | One policy point in the CDP layer for every request: allowlist, private ranges, robots, rate limits, signing. |
| Sites want to know who the agent acts for | Cloudflare personal agents, Trusted Agent Delegation, Web Bot Auth | Optional **signed requests** (Web Bot Auth) with a user-owned key; a fixed, honest agent token in the User-Agent when signing is on. Never stealth. |
| Sites will declare tools for agents | WebMCP in Chrome | Discover page tools, call them through the same gate, then **verify** the page changed as claimed. |
| Code beats chains of tool calls for bulk work | Code execution with MCP, Code Mode | Done (`run`, ADR-0019). Missions can contain `run` steps. |

## 2. Architecture: the mission layer

```
            front doors (all call the same entry point)
  MCP (stdio, HTTP)   Anthropic browser toolset executor   A2A   CLI
            \                 |                     /      /
             +---------------- mission layer -------------+
             |  contract: goal, done_when, budget, risk   |
             |  append-only log (events, refs, verdicts)  |
             |  gate: data-in flags, actions-out policy,  |
             |        taint, per-site approval            |
             |  handoff / elicitation / sampling          |
             |  skills (record, replay, diverge)          |
             |  watchers (schedule, condition, wake)      |
             +--------------------------------------------+
             |  existing engines: browse, read+cite,      |
             |  forms, extract, run, memory (Qdrant)      |
             +--------------------------------------------+
             |  CDP backend (ADR-0001), one network point |
             |  local Chrome | attached Chrome | remote   |
             |  CDP (Browser Run, Kitesurf, others)       |
             +--------------------------------------------+
```

Most of the middle box already exists as libraries in `ostler-workflow` (task contracts, trace, risk gate, receipts, double-submit guard) and is not wired to the tool surface. This plan wires it, and adds the log, handoff, skills and watchers.

### 2.1 Mission contract

```text
> mission begin {"goal": "Ship order for Ada Lovelace, stop before payment",
                 "done_when": [{"url": "/checkout/review"}, {"text": "Ada Lovelace"}],
                 "budget": {"calls": 60, "seconds": 600},
                 "risk_ceiling": "reversible"}
MISSION m3 open. Checks: url ~ /checkout/review, text "Ada Lovelace". Risk ceiling: reversible (submits need approval).

> mission end
MISSION m3 PASS (2/2 checks). 14 calls, 41 s, 0 handoffs. Receipt: order-draft #A-1182. Record: ~/.ostler/missions/m3/
```

- `done_when` uses the `expect` grammar from BUILD-PLAN section 5 (URL pattern, text, element, form state, download present, citation count).
- `mission end` with no check passing is **FAIL** with the closest evidence; with checks that cannot be evaluated (the page is gone) it is **UNKNOWN**. The model's own claim is recorded beside the verdict, which is exactly the false-success measurement.
- `risk_ceiling`: `read`, `reversible`, `irreversible`, `money` (BUILD-PLAN 6.3). Anything above the ceiling becomes a handoff of kind `approval`.

### 2.2 Session log and resume

- One directory per mission: `log.jsonl` (redacted events: tool, target, policy decision, verdict), `refs.json` (stable element refs per tab), `receipts/`, `record/` (DOM events, screenshots at verdicts), `state.json` (tabs, URLs, storage snapshot reference).
- `mission resume {id}` after any crash: relaunch Chrome with the profile, reopen tabs, restore the contract and budget spent so far, report what is stale. `getEvents`-style paging (`mission log {id, after?}`) lets a fresh agent context catch up without replaying everything into its window.

### 2.3 Handoff, elicitation, sampling

| need | MCP elicitation available | not available |
|---|---|---|
| approval above the risk ceiling | form: "Submit order? [approve / deny]" with the receipt preview | `NEEDS APPROVAL: ...` result; the agent asks the user and calls `approve {token}` |
| missing fact (date of birth, address line) | form field, value goes to memory only if the user ticks "remember" | `NEEDS INPUT` naming the field; the agent asks |
| CAPTCHA, login, 2FA | notification plus headed window; `resume` | same, through `handoff` and `wait_for_human` |

Sampling is used for one thing only at first: a replayed skill reaches a step whose target is missing. Ostler sends the client's model the step's intent and the candidate elements (fenced, trimmed) and asks for one ref. No sampling means the replay stops and returns the same question to the agent as a normal result.

To verify in M-A: which clients implement elicitation and sampling today, and what `rmcp` supports. Neither may be required for any feature to work.

### 2.4 Tool surface budget

New tools join the `work` profile from BUILD-PLAN section 5 and appear after `mission begin` or with `--tools work`: `mission` (begin, status, end, resume, log), `handoff`, `approve`, `skill` (save, run, list), `watch` (add, list, remove), `site_tools` (list, call). Target: under 2 KB of added schema, enforced by the existing size test; `auto` grows only by `mission`.

## 3. Use cases as agentic workflows

Each workflow names its goal, the loop the client agent runs, the Ostler pieces it uses, what **done** means (the check Ostler runs), where a person is pulled in, and what memory keeps for next time.

### 3.1 Deep research with a verified answer

- **Goal:** answer a question across many sources, every claim cited.
- **Loop:** plan sub-questions, `search`, `open_many`, `read` with a query, `cite`, `evidence`; repeat until `evidence` shows no open gaps the answer depends on.
- **Agentic upgrade:** parallel sub-missions (one per sub-question, each with its own scratch tab and budget) writing to **one shared source ledger**, the pattern of Anthropic's multi-agent research system with the ledger as the shared memory. `done_when: {citations_verified: ">= 1 per claim", gaps: "reported"}`.
- **Done means:** every quote VERIFIED against a page read in this mission, stale citations re-read or dropped, gaps listed.
- **Person:** only for paywalls or logins the user owns.
- **Memory:** `source` chunks (BUILD-PLAN 3.2) so a follow-up question starts from what was read.

### 3.2 Watch and react (monitoring)

- **Goal:** "tell me when the visa slot, the price or the status page changes, and do X".
- **Loop:** `watch {url, check, every, then}` registers a condition; Ostler polls politely (robots, per-host limits, jitter) and emits an event when it flips. The client is woken by an MCP notification, an A2A push, or a CLI hook (`ostler watch --exec`), and starts a mission with the diff as its first input.
- **Done means:** the check flipped, with before and after evidence and a timestamp.
- **Person:** the follow-up action uses the normal gate.
- **Memory:** the last good value per watch, sealed.
- **Note:** this is Cloudflare's scheduling idea, done locally and without a hosted queue. Ostler must be running; `ostler service install` (launchd, systemd) keeps it up.

### 3.3 Portal operations: record once, replay verified

- **Goal:** repeat a back-office flow on a site with no API: download last month's invoices from a vendor portal, update a record in an admin tool, file a weekly report. This is the "browser becomes the connector" case Anthropic describes for Claude in Chrome.
- **Loop, first time:** the agent does it inside a mission; on PASS, `skill save {name}` stores the trace as steps with intent, stable locators (role, name, nearby label) and the postcondition each step met. Values are stored as `{{fact:KEY}}` or `{{param:NAME}}`, never literally.
- **Loop, later:** `skill run {name, params}` replays without a model turn per step and checks each step's postcondition. On divergence (target missing, postcondition fails) it asks the client model through sampling, or stops and hands the question back.
- **Done means:** the skill's own final check, plus a receipt (downloaded file hashes, confirmation text).
- **Person:** first-run approvals are remembered per skill and risk class; anything new in a replay asks again.
- **Differentiator:** Stagehand caches actions and self-heals; Ostler's replay also **proves each step happened**, and a healed step is recorded as healed, so a skill that drifts is visible instead of silently "working".

### 3.4 Applications and long forms

- **Goal:** complete a government, job or insurance application across a multi-step wizard.
- **Loop:** `form_inspect`, `form_fill` with `{{fact:KEY}}`, `form_submit`; elicitation for missing facts; uploads only from `--upload-dir`.
- **Done means:** submit classified `Submitted` with a receipt; or the mission ends at the review page when `risk_ceiling` is `reversible`.
- **Person:** irreversible submit is always an approval; ambiguous keys are elicited, never guessed (review flaw 1 must be fixed first).
- **Memory:** playbooks per form signature, learned only from values the user or agent supplied.

### 3.5 Shopping and travel comparison, stop at payment

- **Goal:** compare options across sites, build a cart or hold, stop before paying.
- **Loop:** sub-mission per site, `extract` with `where`/`select` (ADR-0019), a comparison table returned as data; then one mission to the cart.
- **Done means:** the cart page shows the chosen items and the total; payment iframes are detected and not entered.
- **Person:** login and payment always; `money` risk class is never auto-approved.

### 3.6 QA after a code change

- **Goal:** the coding agent changed the app; check that the flow still works.
- **Loop:** a mission per flow with `done_when` assertions, `diagnostics` (console errors, failed requests), the recording attached to the pull request.
- **Done means:** all assertions pass and no new console errors or 4xx/5xx compared with the last passing run.
- **Person:** none; failures are the output.
- **Memory:** last passing record per flow, so the diff is "what changed", not "what is there".

### 3.7 Structured extraction at modest scale

- **Goal:** pull a complete list (all 340 job postings, every row of a paginated table).
- **Loop:** `run` script over pages, `extract` with a completeness verdict, filters applied before results reach the model.
- **Done means:** `COMPLETE` verdict, or `PARTIAL: n of m` with the reason.
- **Scale note:** hundreds of pages belong on a crawler. For read-only public pages, a remote CDP backend (Browser Run, Kitesurf or similar) can run sub-missions in parallel while Ostler keeps the checks; logged-in work stays on the local Chrome.

### 3.8 Site-declared tools (WebMCP)

- **Goal:** use a site's own tools when it declares them (`search_flights`, `add_to_cart`).
- **Loop:** `site_tools list` shows tools found on the page, with names and descriptions fenced as untrusted; `site_tools call` runs one through the gate; declarative `<form toolname>` maps onto the form engine, so read-back still applies.
- **Done means:** the tool's result **and** a postcondition on the page (the cart count changed, the results list rendered). A site tool that claims success while the page disagrees is reported as a mismatch.
- **Person:** a site tool inherits the risk class of what it does, judged from its effect on the page, not from its name.

## 4. Differentiation

### 4.1 Where each player sits

| | who runs the loop | where the browser is | what you get back | identity |
|---|---|---|---|---|
| Claude in Chrome, Cowork browser | Anthropic's product | user's Chrome, or a separate built-in profile | the agent's answer | Claude-only, closed |
| Comet, Gemini auto browse, ChatGPT agent | the vendor's agent | their browser or their cloud | the agent's answer | looks like Chrome |
| Cloudflare Browser Run, Kitesurf | your code | Cloudflare's network | raw CDP, recordings, live view | signed for `/crawl` |
| Browserbase, Stagehand, Browser Use, Skyvern | SDK or platform, often with an LLM inside | vendor cloud or local | actions, cached steps, extractions | varies; some solve CAPTCHAs |
| Playwright MCP, chrome-devtools-mcp, agent-browser | your coding agent | local | snapshots and tool output | plain browser |
| **Ostler** | **your agent, any model** | **local Chrome by default, any CDP backend** | **verdicts: outcome check, cited evidence, receipt, replayable record** | **honest, optionally signed; no evasion** |

### 4.2 The five things to be known for

1. **Verdicts, not snapshots.** Every mission ends PASS, FAIL or UNKNOWN from Ostler's own check, with the false-success rate published per release.
2. **Evidence you can check.** Citations verified against pages read, with text-fragment links; gaps reported.
3. **Local accountability.** The log, receipts and recording sit on the user's machine and can be replayed with the verdicts overlaid; nothing goes to a dashboard you do not own.
4. **Works under anyone's agent.** MCP, A2A, the Anthropic browser toolset, a CLI; local Chrome or a remote CDP fleet. Ostler is the verification layer, not another agent competing for the loop.
5. **An honest agent.** Says it is an agent, optionally signs its requests, respects robots and site policy, never solves CAPTCHAs. As sites start to set terms for personal agents, that is an asset, not a limit.

### 4.3 What we will not build

A chat UI or a new browser; an LLM inside the binary; hosted browsers; stealth, fingerprint spoofing or CAPTCHA solving; crawling at scale; connectors that duplicate a real API.

## 5. The Anthropic browser toolset executor

Anthropic's docs say the application must run every member call itself. Ostler fills that role.

- **Shape:** `ostler exec --toolset anthropic-browser` reads one `{id, name, input}` per line on stdin and writes the matching `tool_result` content on stdout; a small Python helper in the `ostler-mcp` package (and later TypeScript) wraps it: pass the assistant message, get back the list of results with the **batch halt rule** applied and `toolset_name` echoed.
- **Mapping:** `read_page` from `snapshot` with refs renamed to `ref_N` and kept stable until navigation (the docs forbid renumbering); `find` as local lexical and role matching over accessible names (no model); `get_page_text` from `read`; `form_input` from `fill_field` **with the read-back appended** (`Set ref_7 = "DE". Verified.`); pointer members through CDP input events; `zoom` as a clipped screenshot; `read_console` and `read_network` from `diagnostics`; `file_upload` limited to `--upload-dir`; `javascript_exec` refused unless `--allow-js`.
- **What the app gains:** policy enforced in the browser, a gate that returns `is_error` with `NEEDS APPROVAL` for risky actions, injection flags as an extra text line, missions and receipts if the app opts in, the same record and live view.
- **Test:** a conformance suite that replays recorded member-call batches from the docs' examples and checks result shapes, the halt text and stale-ref errors.

## 6. Phases

Phase letters run after BUILD-PLAN's M0. Estimates are for one maintainer.

| phase | weeks | contents | exit criteria |
|---|---|---|---|
| **M-0 Keep the promise** | 3 | Review fixes 1 to 3: frozen key resolution, gated memory suggestions, final re-verify; browser relaunch and signal handling; in-browser policy with `Fetch` interception and the submit gate on every path; masking of notes and snapshots | every "C" item in flaws 1 and 2 has a fixture that failed before and passes now |
| **M-Q Click and research quality** | 4 | Section 9: click effects, no silent script clicks, actionability waits, ref repair, `find`, coordinate clicks, `act` batches; claim support with exact numbers, independent sources and conflicts, parallel `open_many`, PDF text, source metadata, coverage; the research benchmark | each item has a fixture that fails today and passes after; research benchmark results committed |
| **M-A Missions** | 4 | `mission` tool over `ostler-workflow`; `done_when`; append-only log; resume after crash; risk ceiling; receipts; false-success scored from mission verdicts in `ostler-bench` | a killed Chrome mid-mission resumes and finishes; pack suites report verdicts |
| **M-B Person in the loop** | 3 | `handoff` and `approve`; MCP elicitation with text fallback; desktop notifications; per-site first-action approval; session import per site with risky categories off; taint rule | approvals and 2FA work in Claude Code and one other client; injection fixtures cannot trigger a cross-origin write without approval |
| **M-C See it** | 2 | `ostler watch` live view (CDP screencast plus trace) and per-mission recordings with verdict overlay; `ostler replay` | a recording opens offline and shows each verdict at its step |
| **M-D Executor** | 3 | `ostler exec` for `browser_toolset_20260801`, Python helper, conformance suite, one example app | the docs' quick-start loop runs end to end against fixtures |
| **M-E Skills and watchers** | 4 | `skill save/run` with postconditions per step, sampling on divergence, healed-step records; `watch` with polite polling and wake paths; `ostler service install` | a recorded skill survives a fixture redesign and reports the healed step; a watch wakes a client |
| **M-F Open web** | 3 | WebMCP discovery and calls with postconditions; optional Web Bot Auth signing; remote CDP backend profile (read-only missions only) | a WebMCP fixture page works with and without the flag; signed requests verify against a test key directory |

Total about 26 weeks including M-0 and M-Q. BUILD-PLAN's M2 (Qdrant-native memory) and M4 (domain packs) run alongside from M-A on; packs gain `done_when` checks instead of bespoke scorers.

**Cut line:** drop the remote backend profile, Web Bot Auth and the TypeScript helper first. Keep M-0, missions, handoff and the executor: the differentiation depends on them.

### ADRs to write

- ADR-0020: missions and the append-only session log (format, retention, redaction).
- ADR-0021: MCP elicitation and sampling, with fallbacks; what sampling may and may not be asked.
- ADR-0022: the Anthropic browser toolset executor (protocol, ref stability, halt rule, versioning by toolset date).
- ADR-0023: WebMCP support and how site tools are trusted.
- ADR-0024: request signing and agent identity (supersedes the "later" row in ADR-0018).

## 7. How we will know it worked

| measure | target before calling it shipped |
|---|---|
| False-success rate with missions on, per pack, real model in the loop | lower than missions off on the same tasks, published either way |
| Mission resume after injected Chrome crash | 95% of fixture missions finish |
| Injection fixtures that cause an outgoing cross-origin write without approval | 0 |
| Skill replay on redesigned fixtures | passes or stops with a named divergence; never a silent wrong step |
| Executor conformance | all documented result shapes and the halt rule |
| Schema added to `auto` | under 600 bytes |
| Clicks that fall back to a script click without saying so | 0 |
| Snapshot calls per click on the dev-qa and commerce packs | at least a third fewer than v0.2 |
| Research benchmark: claims with a number that does not match its quote, accepted by `evidence` | 0 |
| `open_many` wall time for 10 fixture URLs | at least 2x faster than sequential |

## 8. Risks and open questions

| risk or question | what we do |
|---|---|
| Client support for elicitation and sampling is uneven | every feature has a text fallback; nothing requires either |
| The Anthropic toolset changes version | the executor is keyed by toolset date; old versions stay until removed upstream |
| WebMCP is an origin trial and may change shape | behind a flag; discovery handles both `document.modelContext` and the deprecated `navigator.modelContext` |
| Skills drift silently on real sites | healed steps are recorded and counted; a skill past a drift threshold refuses to run without a fresh first run |
| The session log is a new place for secrets | references only, the same masking as snapshots, retention and `ostler mission forget` |
| Scope against one maintainer | the cut line above; packs and executor conformance cases are contributor-friendly |
| Decision for the maintainer | whether `mission` replaces the planned `task` tool name (proposed: yes, one concept) |

## 9. Browsing and research quality

Two places where agents lose the most time and accuracy today: clicking (misclicks, extra snapshots, stale refs) and deep research (claims the quotes do not support). Each item below names where the code is now, the best practice it follows, and the fixture that proves it. Nothing here needs a model inside Ostler.

### 9.1 Best practices this section follows

| practice | source |
|---|---|
| Check that an element is visible, stable, enabled and would receive the event before acting; wait for those, not for a fixed time | Playwright's actionability checks |
| Refs stay valid until navigation; never renumber; a stale ref is an error that tells the agent to re-read | Anthropic browser toolset docs |
| Prefer the accessibility tree and refs; fall back to coordinates for canvas, maps and virtualized lists; read the tree before a screenshot | Anthropic browser toolset docs |
| Several actions in one turn run in order and stop at the first failure | Anthropic browser toolset batch rule |
| A failure degrades to a clear result, never a dead session | Cloudflare Kitesurf design rules |
| Research fans out to sub-questions, then a separate pass checks citations against sources | Anthropic multi-agent research system |
| Verification is outside the model and returns a verdict | Ostler's own principle (ADR-0007, ADR-0011) |

### 9.2 Clicking and browsing

| # | change | today | done when |
|---|---|---|---|
| C1 | **Click effects.** Report what a click did: navigated, new tab, dialog, a region that appeared (with its new refs), or nothing; on nothing, say what usually helps (hover first, scroll, a covering element) | `ActionOutcome` carries only `navigated` and `changed: bool` from a markup signature (`ostler-browser/src/page.rs:328`) | a menu fixture returns its items in the click result; a no-op click says so; snapshot calls per click drop on the packs |
| C2 | **No silent script clicks.** When the target is covered: try other uncovered points in its box, then adjust scroll for sticky headers, then report a known overlay (cookie banner, modal) and offer its dismiss ref; script click only when asked | a covered target falls back to `el.click()` with a warning (`runtime.js:1238`, `page.rs:1318`) | `overlay.html` click under the modal returns `COVERED by dialog` with the dismiss ref instead of clicking through |
| C3 | **Actionability waits.** Before a pointer click, wait (bounded) until the element is visible, its box is unchanged across two animation frames, it is enabled and the hit test lands on it | no stability check; one hit test | a new fixture with a sliding menu: v0.2 misclicks, the fix clicks the right item |
| C4 | **Ref repair.** Store a fingerprint per ref (role, accessible name, label, parent path, position); when a ref goes stale, re-resolve it only if exactly one element matches and say `re-resolved`; otherwise the stale error stands | refs map to element objects; a re-render makes them `STALE_REF` (`runtime.js:139`) | `remount.html`: a click after a framework re-mount succeeds and reports the re-resolution; an ambiguous re-mount still errors |
| C5 | **Cross-origin iframes.** Inject the runtime into auto-attached child targets, prefix their refs (`f2:e5`), translate coordinates; payment frames stay detected and refused | snapshot prints `cross-origin: content not accessible`; child targets are attached only for policy (`page.rs:195`) | a cross-origin fixture widget can be read and clicked; `pay-frame.html` is still refused |
| C6 | **`find {query}`** returns up to 10 refs ranked by role, name and label with the existing BM25 code, instead of a full snapshot | not available | a 500-element fixture: `find "save button"` returns the right ref in under 1 KB |
| C7 | **Coordinate actions** (`click {x, y}`, `zoom`) for canvas and virtualized content, coordinates from `look` | refs only | a canvas fixture is operable; coordinates are policy-checked like refs |
| C8 | **`act` batches**: an ordered list of actions with the halt rule and one observation at the end | one action per call | a three-step search (click, type, press) is one call; a failure in step 1 reports steps 2 and 3 as not executed |
| C9 | **Isolated world** for the runtime (review finding), so page scripts cannot see or tamper with it | page world, wrapped `fetch` and `setTimeout` visible | a fixture that probes for `window.__wm` sees nothing |

### 9.3 Deep research

| # | change | today | done when |
|---|---|---|---|
| R1 | **Claim support.** `claim {text, citations}` checks, without a model, that the claim's numbers, dates and names occur in its quotes, and that each number matches exactly; `evidence` lists claims as supported, weak or `NUMBER MISMATCH` | `cite` verifies that the words are on the page (`ostler-extract/src/quote.rs:215`), not that they back the claim | a planted "19 kr" claim against a "9 kr" quote is flagged; no false flags on the research pack |
| R2 | **Independent sources and conflicts.** Pages sharing syndicated text (shingle similarity) count once; the same quantity with different values across sources is `CONFLICT`; single-source claims are labelled | every page counts as a source | a fixture pair that copies one article counts as one source; a planted contradiction is reported |
| R3 | **Parallel `open_many`.** A pool of 3 to 4 scratch tabs under the existing `HostLimiter` and robots rules | pages are read one after another in one tab (`ostler-mcp/src/app/research.rs:647`) | 10 fixture URLs on 3 hosts at least 2x faster, per-host spacing unchanged |
| R4 | **PDF text.** Extract text from fetched PDF bytes with a permissively licensed Rust crate (licence and maintenance checked in an ADR), chunk by page, cite with `#page=` | no PDF text layer | a PDF fixture can be read and a quote on page 3 cited with its page link |
| R5 | **Source metadata.** Publish date, author, canonical URL and primary or secondary (does the page link the source it reports on) from JSON-LD and meta tags; `links {kind: "references"}` lists outbound citations and DOIs to follow upstream | not recorded | `evidence` shows dates; a news fixture points to the study it cites |
| R6 | **Coverage.** The agent declares sub-questions; `evidence` lists which have no verified citation; becomes a mission check (`done_when: every sub-question cited, no open conflicts`) | not available | an unanswered sub-question is listed as open |
| R7 | **Source memory across sessions.** Expose the `source` memory kind so follow-ups reuse what was read, with freshness re-checked against the stored content hash | library only | a second session cites a page read in the first only after a fresh hash check |
| R8 | **Research benchmark.** Multi-hop fixtures with a planted conflict, a stale page, a number mismatch, a syndicated copy and an injection; scores citation precision, claim support, conflicts found and gaps reported, with a real model and without | none | results committed per release, negative ones included |

### 9.4 Order

C1, C2, C3 and C4 first: together they remove most misclicks and extra snapshot turns, and C2 closes a correctness hole. Then R1, R3, R4 and R8: claim support is the research equivalent of form read-back, and the benchmark is what lets a research accuracy number be published at all. C5 to C9 and R2, R5 to R7 follow. Each item lands with its fixture first, failing, as the design review asks.
