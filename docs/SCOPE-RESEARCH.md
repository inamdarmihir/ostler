# Scope proposal: from form filling to research and general browser tasks

Status: **draft for discussion**, 2026-11-06. Nothing here is built. The wider task and domain frame is in [SCOPE-TASKS.md](SCOPE-TASKS.md). Section 7 lists the decisions that need an owner's call before work starts.

## 1. Why expand

Ostler began as "a headless browser that fills a whole form in one call and reads every value back". Agents use browsers for more than forms: deep research (find, read, compare, cite), data extraction, monitoring, QA and debugging, and transactions (booking, checkout). Today a research agent on Ostler can `navigate` and `snapshot`, but gets a page-sized accessibility dump per page, no search, no structured extraction, no record of what it has read, and no way to check that a claim is actually on the page it cites.

## 2. The thesis that carries over

Form filling earned its place with three rules. Each has a direct research analogue, and these analogues are the product, not the feature list:

| Forms (today) | Research (proposed) |
|---|---|
| Read every value back from the live page | **Every claim carries a verbatim quote, URL, fetch time and content hash; the quote is checked against the page text.** |
| Unmatched or ambiguous keys are reported, never guessed | **Pages that failed, were blocked, paywalled or truncated are reported as gaps, never silently skipped.** |
| Terse, delta output; schema size is a budget | **Query-focused reading: return the passages relevant to a question, not the page.** |

Differentiator to test, not assume: "the browser tool that will not let an agent cite something it did not read". Whether this matters to users must be measured (see ADR-0007), and the competitive claims in `docs/research/landscape.md` have not been re-verified for research tooling.

## 3. Boundary: tool server, not agent

Ostler has no LLM in the loop and should keep it that way for this scope. It provides primitives and bookkeeping; the client agent (Claude Code, Codex, Cursor) plans the research. This keeps cost, determinism and the security story simple. A packaged research agent can come later over A2A (phase 4), with a user-supplied model.

## 4. Proposed capabilities

Existing pieces to build on are in brackets.

### 4.1 Read (phase 1)
- **`read`**: main content of a page as markdown (article extraction, boilerplate and cookie walls removed), with title, author, date, canonical URL, JSON-LD and outbound links. Chunked with a cursor so long pages and PDFs are read in parts. [`snapshot` with `max_chars`, PDF notes, `<page_content untrusted>` fencing]
- **`read` with `query`**: rank chunks locally (BM25, optionally embeddings through the existing embedder trait) and return the top passages with locators. No LLM needed.
- **`extract`**: tables, lists, definition blocks as rows; pagination and "next page" detection. [roadmap item 4]
- **`links`**: outbound links with anchor text and surrounding context, optionally ranked against a goal string.
- New pure-logic crate `ostler-extract` (no I/O, like `ostler-forms`) so it can be property-tested and fuzzed.

### 4.2 Find and fan out (phase 2)
- **`search`** through a provider trait. WEB-SEARCH.md already concludes search APIs beat scraping results pages, and engines wall headless Chrome. Providers: SearXNG (self-hosted, no key) and one commercial API first; keys via env or the existing config.
- **`open_many`**: fetch N URLs concurrently in tabs, return a one-line status plus `read` summary per URL. Per-host concurrency and delay limits. [tabs, shared context, concurrency bench]
- **Budgets** on every research session: max pages, bytes, wall time, depth. Exhausting one is reported, not hidden.
- Politeness: robots.txt stance is an open decision (section 7).

### 4.3 Remember what was read (phase 3)
- **Source ledger**: URL, fetch time, status, content hash, chunks, and quotes the agent has recorded. Dedupes re-reads, flags pages that changed, survives restarts. [`context` notepad is key/value only]
- **Document memory in Qdrant**: a fourth memory kind alongside profile, playbooks and episodes. Chunks embedded and sealed with the existing `SealedStore`; `research_recall` answers "what did I already read about X". Opt-in, short default retention. [ostler-memory]
- **`cite`**: take (claim, URL, quote), verify the quote appears in the stored page text (whitespace and case normalised), return a deep link using a text fragment (`#:~:text=`) and a verdict: `verified`, `not found`, `page changed`. A report tool lists claims with their verification state.

### 4.4 Other browser tasks, in priority order (phase 2 to 4)
1. **`diagnostics`**: console errors and failed requests since last call. [roadmap 1; cheap; main reason people keep chrome-devtools-mcp]
2. **Session save/load** and downloads surfaced to the agent. [roadmap 2 and 5; needed for research behind logins, and for the reading list]
3. **Record and replay** of a flow as a playbook. [roadmap 7; reuses playbook memory]
4. **Streamable HTTP transport** for remote agents. [roadmap 6]
5. Monitoring ("tell me when this page changes") falls out of the ledger's content hash; no new subsystem.

Transactions (checkout, booking) reuse the form engine plus the existing submit gate; no new scope beyond tests on real flows.

## 5. Constraints that do not change

- **No CAPTCHA solving, stealth or paywall bypass** ([ADR-0009](adr/0009-challenges-are-handed-off.md)). Research hits walls far more than forms do, so the hand-off path (`CHALLENGE:` line, `wait_for_human`, A2A `input-required`) and a clear "blocked sources" list become central. Fewer walls come from `--profile-dir` and saved sessions, not disguise.
- **Local only**: no hosted browsers, nothing runs in a cloud.
- **Token budget**: the schema is paid on every turn (core is ~4.1 KB, a test caps the full set at 7 KB). New tools go in a `research` profile that `auto` reveals when a page is not a form; each tool description stays terse.

## 6. New risks, and what research changes

Research reads far more untrusted text than form filling does, so **prompt injection becomes the main threat**, and memory makes it worse.

- The dangerous combination is private data (memory facts) plus untrusted content plus an exfiltration route (form submit, navigation to an attacker URL, query strings). Research mode should: make profile facts unavailable unless explicitly enabled; refuse memory writes sourced from page text; keep the existing allowlist and non-GET gate; treat navigations whose URL contains recalled values as suspicious.
- Keep and extend the fencing: everything from `read`, `extract` and `search` results is wrapped as untrusted; add an injection-pattern flag on chunks ("ignore previous instructions") that is reported, not relied on.
- Search results and fetched pages are third-party content stored in Qdrant: sealing covers confidentiality, retention covers growth, and `ostler memory wipe` must cover the new kind.
- Update THREAT-MODEL.md before shipping phase 1, not after.

Prerequisite from the [2026-10 review](REVIEW-2026-10.md): "settle on busy pages", "refs reused across documents" and cross-origin iframes are still open, and all hurt reading long, script-heavy pages. Triage these in phase 0.

## 7. Decisions needed

1. **Tool server vs agent.** Recommend tool server now, research agent over A2A later. Confirm.
2. **Search providers.** Recommend a trait with SearXNG plus one paid API. Which API, and is a key requirement acceptable for the default install?
3. **Politeness.** Respect robots.txt by default for `open_many` and crawling, with an override? Recommend yes.
4. **Document memory.** Opt-in Qdrant kind with 30-day default retention, or ledger-only on local disk first? Recommend ledger first, Qdrant documents in the second step of phase 3.
5. **Identity.** README leads with forms. Recommend keeping "verified" as the headline and widening the tagline (for example, "a browser for agents that verifies what it does and what it reads"). Needs a name for the research half of the story.
6. **Priority among other tasks.** Is debugging/QA (`diagnostics`) a first-class goal, or does research come strictly first?
7. **Eval.** Agree the success measure before building: a local research fixture suite (known facts across linked pages, a paywall, a bot wall, pagination, a stale page, planted injections) scoring citation precision, gap reporting, tokens and calls, run against Ostler and the baselines the way `scripts/bench-compare` does for forms.

## 8. Phasing

| phase | content | exit criteria |
|---|---|---|
| 0. Groundwork | Research fixture suite and scoring; triage open review items that affect reading; THREAT-MODEL update; first tagged release of the current build | Suite runs in CI against current Ostler (baseline numbers, including bad ones) |
| 1. Read | `read`, `read query`, `extract`, `links`, `ostler-extract` crate, `research` profile | Fixture suite: reading tasks pass; schema under budget; fuzz target for the extractor |
| 2. Find | `search` providers, `open_many`, budgets, `diagnostics`, politeness | Multi-source fixture tasks pass with gaps reported; per-host limits tested |
| 3. Remember | Source ledger, `cite` with quote verification, then Qdrant document memory and `research_recall` | Zero unverified citations pass `cite` on the suite; wipe/forget cover the new kind |
| 4. Delegate | Session save/load, downloads, A2A research task, record/replay, streamable HTTP | Decided after phase 3 results |

Each phase ships behind the `research` profile and publishes its benchmark numbers, including negative results, as the forms work did.
