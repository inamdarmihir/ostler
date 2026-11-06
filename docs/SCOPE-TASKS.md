# Scope proposal: a holistic task and domain model

Status: **background**, 2026-11-06; the plan of record is [BUILD-PLAN.md](BUILD-PLAN.md). Companion to [SCOPE-RESEARCH.md](SCOPE-RESEARCH.md), which covers the research half in detail. This page widens the frame: which tasks and domains Ostler should support, what they share, and what a workflow (as opposed to a single call) needs. Nothing here is built.

## 1. Approach: few primitives, one verification idea

Supporting "every task" site by site does not scale. The proposal is to support a small set of **task classes**, each built from shared primitives, and for each class to define what **"verified"** means. That is the same bet form filling made (read every value back), generalised.

| task class | agent's intent | what Ostler returns as proof | today |
|---|---|---|---|
| **Fill** | enter structured data | each value read back, by field type | built |
| **Act** | click, navigate, operate a UI | a checked **postcondition** (URL, text or element now present/absent), not just "click succeeded" | partial: delta snapshots, no explicit assertion |
| **Transact** | submit something with consequences | outcome class, captured **receipt** (confirmation number, order id, screenshot), double-submit guard | partial: submit outcome classes, confirm gate |
| **Read** | learn what a page says | content with locators; quotes verifiable | partial: `snapshot` |
| **Find** | discover sources | structured results, gaps reported | missing |
| **Extract** | turn pages into rows | rows plus **completeness** evidence (page count, row count vs page's stated total) | missing |
| **Monitor** | notice change | content hash and diff since last look | missing |
| **Test/Debug** | check a web app | console and network errors, assertions, screenshots | missing |
| **Authenticate** | get and keep access | session state saved, SSO/2FA handed to a person | `--profile-dir` only |

The new general primitive is **`expect`**: any action can carry a postcondition, and the result says whether it held. It turns "I clicked Next" into "the page now shows step 2", and it is what Act, Transact and Test all need. Everything else in the table is a variation on a primitive Ostler already has or on the research plan.

## 2. Domains

Domain support should be driven by what is common in the wild and by risk, not by logo. Proposed tiers; "risk" sets the default safety gate (section 4).

| tier | domains | typical tasks | risk | notes |
|---|---|---|---|---|
| 1 | Forms-heavy public sites: government, legal, insurance, HR/job applications, event sign-ups | multi-step wizards, uploads, dependent fields | medium (data submitted) | strongest today; needs real-site widgets (cross-origin iframes, address autocomplete) |
| 1 | Web research and documentation | find, read, compare, cite | low | see SCOPE-RESEARCH |
| 1 | Developer and QA workflows: local apps, staging, CI dashboards, cloud consoles | verify a flow, read errors, file an issue | low to medium | needs `diagnostics`, assertions |
| 2 | E-commerce and travel | search, compare, cart, checkout, booking | high at checkout | virtualized lists, calendars, price drift; hard stop before payment by default |
| 2 | Enterprise SaaS: CRM, ticketing, admin consoles, internal tools | bulk edits, exports, report pulls | medium to high | logins, SSO, tables with inline editing, rich text editors |
| 3 | Finance, banking, healthcare portals | statements, claims, appointments | high | read-only by default; explicit opt-in to act; 2FA always human |
| 3 | Social and content platforms | posting, scheduling, inbox triage | medium | heavy anti-automation; terms of service vary; support only where permitted |

Out of scope by principle: CAPTCHA/anti-bot evasion, paywall bypass, credential stuffing, mass scraping, anything needing stealth ([ADR-0009](adr/0009-challenges-are-handed-off.md)).

**Widget classes are the real unit of coverage.** Sites differ, widgets repeat. A coverage matrix tracked in `docs/COMPATIBILITY.md` should list each class with fixture status: date and range pickers, rich-text editors, virtualized tables and infinite scroll, drag and drop, cross-origin payment/address iframes, file download flows, PDF viewers, canvas and maps, chat-style UIs, web-component libraries with closed shadow roots (a known blind spot), and in-browser spreadsheets. Each gap becomes a fixture first, then an engine change, matching the existing practice.

## 3. What a workflow needs beyond single tool calls

Agentic workflows run long, get interrupted, run in parallel and need accountability. Proposed workflow-level capabilities, in rough priority:

1. **Task contract.** A task is a goal plus an **outcome check** the caller states up front ("order confirmation page shows an order id", "15 rows extracted"). Ostler reports pass/fail against it rather than the agent inferring success. Uses `expect`.
2. **Action trace and audit log.** An append-only, redacted record of what was done, on which URL, with which outcome. Gives replay (roadmap item), debugging, and compliance. Values masked as in snapshots today.
3. **Checkpoint and resume.** Persist tab URLs, shared `context`, and ledger so a crashed or paused workflow continues. Builds on `context` (already survives restart) and session save.
4. **Human-in-the-loop as a first-class state.** Generalise `wait_for_human` and A2A `input-required` beyond CAPTCHAs: approvals before irreversible steps, 2FA, ambiguous choices. Same mechanism, typed reasons.
5. **Idempotency and double-submit protection.** Fingerprint a submitted form plus values; refuse an identical resubmit within a window unless forced. Prevents duplicate orders when an agent retries after a timeout.
6. **Concurrency.** Several workflows or agents against one machine: per-workflow tab and cookie scoping, profile locking, per-host rate limits. Concurrency benchmarks exist; the isolation model does not yet.
7. **Budgets and timeouts per task** (pages, bytes, wall time, spend-adjacent actions), reported when hit.
8. **Observability.** Per-tool metrics exist; add task-level success/failure counts and an export (OpenTelemetry is the obvious candidate; not evaluated).
9. **Integration surfaces.** MCP over stdio and A2A exist. Add streamable HTTP for remote agents, and consider embedding Ostler as a Rust library for frameworks that do not speak MCP. A scripting CLI for non-agent callers is a cheap by-product of the trace format.

## 4. Safety model scaled to action risk

The current gate is one switch (`--require-confirm-submit`). Broader tasks need classes of action with default policy:

| class | examples | default |
|---|---|---|
| read | navigate, snapshot, extract | allowed within allowlist |
| reversible write | add to cart, save draft, filter | allowed |
| irreversible or external | submit order, send message, post, delete, change settings | blocked until confirmed; confirmation names the action and the values |
| money or identity | payment, transfer, password or 2FA change | blocked unless explicitly enabled per domain; never auto-filled |

Classification is heuristic (button text, form action, HTTP method) and will be wrong sometimes, so the rule stays "when unsure, treat as the stricter class and say so". A per-domain policy file (allowlist, risk overrides, read-only) is the natural home, replacing scattered flags. Prompt-injection and exfiltration controls from SCOPE-RESEARCH section 6 apply to all classes, and matter more as tasks gain consequences.

## 5. Knowledge that transfers: domain packs

Qdrant playbooks already record which fact fills which field per site. Generalise to **domain packs**: shareable, reviewable bundles of site-class knowledge (field aliases for government forms, known widget quirks, typical wizard shapes), distributed as files, never containing user values. Open question: whether to ship any curated packs, or only the mechanism and let users export their own playbooks. Curated packs create a maintenance and accuracy burden the project has no capacity for yet.

## 6. How to measure it

A broad claim needs a broad, honest benchmark ([ADR-0007](adr/0007-measurement-first.md)):

- One **task suite per class**, on local fixtures replicating widget classes and domain shapes (the fixture server already hosts 49 pages), each with a stated outcome check. Scored on success, wrong-but-claimed-success (the metric that matters most), calls, tokens and gaps reported.
- A **small live smoke set** that never submits: read-only tasks on real sites, run manually and reported separately, because fixture results do not transfer automatically (the README already says Ostler is mostly fixture-tested).
- Baselines: the same scripted-agent method as `scripts/bench-compare`, adding a real model in the loop once; the review flagged its absence as an open flaw.

## 7. Suggested sequencing

This slots around the research phases rather than replacing them.

| step | content |
|---|---|
| A | `expect` postconditions on actions; task contract; action trace. Small, and unlocks Act, Transact and Test |
| B | Research phases 0 to 2 (see SCOPE-RESEARCH), including `diagnostics` and `extract` |
| C | Risk-classed action gate and per-domain policy file; idempotency guard; receipt capture |
| D | Session save/load and auth handoff; checkpoint and resume |
| E | Widget coverage push, tier 1 domains first: cross-origin iframes, address autocomplete, downloads, rich-text editors |
| F | Streamable HTTP, concurrency isolation, observability export; decide on library embedding and domain packs |

## 8. Decisions needed

1. **Is "holistic" the identity?** Ostler becoming a general verified-action browser changes the README, name story and comparison set. Or does it stay forms-first with research and tasks as optional profiles? Recommend the former, with forms remaining the flagship demo.
2. **Which tiers are in for the next release?** Recommend tier 1 only, with tier 2 as fixture-backed experiments and tier 3 explicitly read-only.
3. **Transactions.** Allow checkout and booking at all (with the hard stop), or stop at "ready to pay"? Recommend stopping at the payment step by default.
4. **Scope of the audit log.** Local file only, or exportable? It stores URLs and outcomes, so it has its own retention and privacy decisions.
5. **Library embedding and domain packs.** Worth the maintenance, or defer until there is demand?
6. **Capacity.** The plan above is large against a project with one maintainer and no tagged release. Where should it be cut if only A and part of B fit in the next quarter?
