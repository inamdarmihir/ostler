# Ostler build plan: v0.3 to public launch

Status: **plan of record, draft 1**, 2026-11-06. Supersedes the roadmap's "next sprint" and folds in [SCOPE-RESEARCH.md](SCOPE-RESEARCH.md) and [SCOPE-TASKS.md](SCOPE-TASKS.md), which stay as background. Section 11 lists the defaults this plan assumes; change any of them and the affected milestone moves.

## 1. What we are building

**Ostler is a browser for AI agents that verifies what it does, cites what it reads, and remembers what worked, with Qdrant as its memory.**

It stays a tool server: one local Rust binary, MCP and A2A front doors, no LLM inside, no cloud browsers, no anti-bot evasion. What changes is its range. Forms were the first task class. This plan adds the others agents actually run (acting on UIs, transactions, research, extraction, monitoring, QA) and the workflow layer that long-running agent jobs need.

### The story we launch with

Browser agents fail quietly: they report "done" when the order did not go through, cite a page that does not say what they claim, or fill the wrong field and move on. Today nobody measures that. Ostler's launch is built around one number:

> **False success rate**: how often an agent says a task succeeded when it did not.

We publish an open benchmark that measures it across browser MCP servers with real models in the loop (open-weight and frontier), release the dataset and trajectories on Hugging Face, and show that Ostler's verification lowers it. If our numbers are bad on some tasks, we publish those too; that is the project's existing habit and its best credibility asset on Hacker News.

## 2. Pillars

| # | pillar | what it means | built on |
|---|---|---|---|
| P1 | **Verified actions** | every action can carry a postcondition (`expect`); fills are read back; submits are classified and leave a receipt | form engine, submit classifier |
| P2 | **Grounded reading** | pages read as clean, chunked text with locators; every citation's quote is checked against the stored page | `snapshot`, untrusted fencing |
| P3 | **Qdrant-native memory** | one memory model on Qdrant everywhere: embedded (Qdrant Edge) on a laptop, server or Cloud for teams; hybrid retrieval, diversity, learning from outcomes | `ostler-memory`, sealing, learning loop |
| P4 | **Domain coverage** | domain packs: fixtures, widget support, eval tasks and playbook seeds per domain, contributed by the community | fixture server, playbooks |
| P5 | **Workflow layer** | task contracts, action trace, resume, human-in-the-loop, risk-classed gates, idempotency | `context`, A2A tasks, confirm gate |
| P6 | **Open evaluation** | false-success benchmark, live smoke set, public dataset and leaderboard | `ostler-bench`, `bench-compare` |

## 3. Qdrant-native design

Today memory talks to a Qdrant server over hand-written REST, with an encrypted local file as the no-server option and a non-semantic hash embedder for development. The plan makes Qdrant the memory everywhere and uses more of what it offers.

### 3.1 Deployment modes, one model

| mode | engine | embeddings | who it is for |
|---|---|---|---|
| **local** (new default) | **Qdrant Edge**, in-process (`qdrant-edge` crate), shard files beside the config | local ONNX models through fastembed-rs | everyone; zero setup, nothing leaves the machine |
| **server** | self-hosted Qdrant | local (vectors only reach Qdrant) or Ollama | teams, shared playbooks |
| **cloud** | Qdrant Cloud | Cloud Inference or local | hosted teams |

The `Store` trait stays. `EdgeStore` replaces the encrypted local file store as the default, so local and server modes run the same query shapes and the docs describe one system. Server-side features Edge lacks degrade the way the pre-1.14 formula fallback already does: detected, ranked client-side, reported in `doctor`.

To verify in M0 before committing: Edge's release status (it launched as a private beta), licence, on-disk format stability, and which Query API features it supports. fastembed-rs's support for sparse and rerank models. If Edge is not ready, local mode ships on a bundled Qdrant binary managed by `ostler` and moves to Edge later; the `Store` trait makes that a swap.

### 3.2 Memory kinds

| kind | holds | new or existing |
|---|---|---|
| `profile` | facts about the user | existing |
| `playbook` | which fact filled which field, per form signature, with win/loss counts | existing |
| `episode` | failures and validation errors, deduplicated | existing |
| `source` | chunks of pages the agent read, with URL, fetch time, content hash and locators | **new** (research) |
| `skill` | successful action traces for a goal on a page signature, replayable | **new** (record and replay) |

### 3.3 Qdrant features used, and why

| feature | use in Ostler | minimum Qdrant |
|---|---|---|
| Named vectors: `dense`, `bm25` (sparse, IDF), optional `colbert` (multivector) | dense for meaning, BM25 for exact names, IDs and part numbers in labels and pages, late-interaction rerank for source chunks | sparse with IDF, multivector: verify |
| Hybrid query with weighted RRF | every recall: `source` passages, form-field to fact matching, skill lookup | 1.17 for weights |
| Server-side BM25 (`Qdrant/bm25`) | sparse vectors without a client tokenizer, unsealed mode only (see 3.4) | 1.15.3 |
| Formula queries | rank playbook mappings by similarity, win rate and recency (shipped) | 1.14 |
| MMR diversity | research recall returns passages from different sources, not five chunks of one page | verify |
| Grouping (`group_by`) | at most N chunks per URL in results | existing |
| Recommend with positive and negative examples | find skills like the ones that won on this host and unlike the ones that failed | existing |
| Tenant payload index, collection-per-tenant, JWT RBAC | multi-tenant isolation (shipped) | existing |
| `create_vector_name` | add `bm25` to existing collections during migration without recreating them | 1.18 |
| Snapshots | `ostler memory export` and backup | existing |

The supported server floor rises to **1.17** for hybrid features, with 1.14 to 1.16 detected and served by client-side fusion. CI keeps testing the newest release and the floor.

### 3.4 Sealing and hybrid search: the hard problem

Sealed memory (the default with a server) encrypts text before it reaches Qdrant. Server-side BM25 needs the text, so the two are in conflict. The plan:

- Sealed mode computes sparse vectors on the client and replaces each token id with a keyed hash (HMAC with the index key, the same idea used for `host` and `key_norm` today). Exact-term matching still works; Qdrant sees opaque term ids, not words.
- What still leaks, written down in THREAT-MODEL.md: term frequency patterns, document lengths and co-occurrence. That is weaker than full encryption, and we say so. Users who need more turn sparse vectors off for sealed memory (dense only).
- Edge mode needs none of this: data never leaves the machine.

This is also the technical centrepiece of the Hugging Face article (section 9).

### 3.5 Ephemeral per-page index

`read` with a `query` chunks the page into a throwaway in-process Edge shard and runs the same hybrid query used for long-term memory. Query-focused reading then needs no LLM and no separate search library, and the code path is the one memory already uses. Pages are persisted to `source` memory only when the agent records a citation or research memory is on.

### 3.6 Client library

M0 spikes the official Rust `qdrant-client` (gRPC) against the current REST code on the memory benchmark. Adopt it if latency and dependency weight are acceptable, so new Query API features arrive with the client rather than being hand-written. Keep the REST path otherwise.

## 4. Use open source, do not rebuild it

Adopt proven, permissively licensed components; every one is checked for licence and maintenance in M0 and recorded in `deny.toml` and an ADR.

| need | adopt | instead of |
|---|---|---|
| Vector memory, local and server | Qdrant Edge, Qdrant, `qdrant-client` | custom local store |
| Local embeddings, sparse, rerank | fastembed-rs (ONNX) | dev-hash in non-dev use |
| MCP protocol, streamable HTTP | `rmcp` (official Rust SDK), evaluated against the current hand-written loop | writing HTTP transport by hand |
| Article extraction | Mozilla Readability, injected through the existing runtime (Apache-2.0) | custom boilerplate removal |
| HTML to markdown | an existing Rust crate, chosen in M0 | custom converter |
| Search | SearXNG (self-hosted, no key) plus one commercial API behind a `SearchProvider` trait | scraping result pages (already shown not to work) |
| Page archiving | WARC records for stored sources | a custom snapshot format |
| Citations | URL text fragments (`#:~:text=`) | custom anchors |
| Crawl etiquette | robots.txt per RFC 9309 | ad hoc rules |
| Agent identity, later | Web Bot Auth if sites adopt it (ADR-0009) | user-agent games |
| Tracing | OpenTelemetry export of the action trace | custom metrics format |
| Releases | cargo-dist (binaries, installers, Homebrew) | hand-rolled scripts |
| Public benchmarks | self-hostable WebArena-style environments and a subset of open task sets (Mind2Web family, BrowseComp-style research questions), licences checked | only our own fixtures |

## 5. Tool surface

Schema size is paid on every turn, so tools are grouped and revealed by context. Postconditions are a parameter, not a tool.

| profile | tools | revealed when |
|---|---|---|
| core | `navigate` `snapshot` `look` `click` `type` `press` `scroll` `hover` `wait_for` `wait_for_human` `back` `tabs` `context` `screenshot` | always |
| forms | `form_inspect` `form_fill` `form_submit` `fill_field` | a page shows form fields (as today) |
| read | `read` `extract` `links` `cite` | a page has article or table content, or after `search` |
| find | `search` `open_many` | always in the `research` preset; on request otherwise |
| memory | `memory_remember` `memory_recall` `memory_list` `memory_forget` (with `kind`) | memory is on |
| work | `task` (begin with contract, check, end), `diagnostics`, `downloads` | a task is begun, or the `qa` preset |

`expect` (on `click`, `type`, `press`, `navigate`, `form_submit`): a URL pattern, text or element that must appear or disappear. Results say `EXPECT OK` or `EXPECT FAILED: ...`.

Presets: `auto` (default), `forms`, `research`, `qa`, `all`. The full set must stay under a stated budget (target 10 KB, enforced by the existing size test), with `auto` near today's 4 KB.

## 6. Workflow layer (P5)

1. **Task contract**: `task begin {goal, expect, budget}`; Ostler checks the stated outcome at `task end` and reports pass, fail or unknown. This is how the false-success rate is measured, and how users get it in production.
2. **Action trace**: append-only, redacted JSON lines per task (tool, URL, target, outcome, expect result); exportable as OpenTelemetry spans; it is the input to skill memory and replay.
3. **Risk-classed gate**, replacing the single `--require-confirm-submit` switch: read / reversible write / irreversible / money and identity, with defaults from SCOPE-TASKS section 4 and a per-domain policy file (`ostler.toml` `[domains."example.com"]`: allow, read-only, risk overrides).
4. **Receipts and idempotency**: after an accepted submit, capture confirmation text, ids and a screenshot; refuse an identical resubmit within a window unless forced.
5. **Human in the loop** as typed states: challenge, approval, 2FA, ambiguity. Same channel in MCP (`wait_for_human`) and A2A (`input-required`).
6. **Session and resume**: `ostler session save|load` (cookies, storage, tabs, context, task state), encrypted with the existing key material.
7. **Concurrency**: per-task browser contexts, per-host rate limits, profile locking.

## 7. Domains (P4)

A **domain pack** is a directory: fixtures that replicate the domain's real widget shapes, eval tasks with outcome checks, optional seed playbooks (labels and aliases, never values), and a README stating what is and is not supported. Packs are the main way contributors extend Ostler.

| pack | tasks | risk default | first-wave widgets |
|---|---|---|---|
| `gov-forms` | multi-step applications, uploads | irreversible gated | wizards, uploads, address fields |
| `jobs` | applications on common applicant-tracking layouts | irreversible gated | resume upload, repeated sections, rich text |
| `research` | find, read, compare, cite | read | articles, PDFs, paginated lists, paywall and bot-wall detection |
| `dev-qa` | verify a local app flow, collect errors | read and reversible | console/network diagnostics, assertions |
| `commerce` | search, filter, compare, cart, stop at payment | hard stop before payment | virtualized lists, variant pickers, cross-origin payment iframes (detected and stopped) |
| `travel` | search and compare, hold where free | hard stop before payment | date-range pickers, calendars, location autocomplete |
| `saas-admin` | table edits, exports, report pulls | reversible; deletes gated | data grids, inline editing, downloads |
| `finance-health` (later) | read statements, appointments | **read-only** | 2FA always handed to a person |

Engine work driven by the packs, in order: cross-origin iframes (CDP out-of-process frame attach), address and location autocomplete, rich-text editors (contenteditable, ProseMirror-style), virtualized lists and infinite scroll, date-range pickers, downloads, drag and drop, closed shadow roots (investigate CDP DOM piercing; may stay a documented limit).

Each widget class goes into `docs/COMPATIBILITY.md` as a row: fixture exists, engine support, eval pass rate.

## 8. Evaluation (P6)

| suite | what | where it runs |
|---|---|---|
| fixture suites | one per pack, local, deterministic, outcome-checked | CI on every PR |
| **false-success benchmark** | the fixture suites plus a self-hosted WebArena-style environment, with a real model driving each MCP server (Ostler, Playwright MCP, Chrome DevTools MCP, others by request); scores task success, **false success**, calls, tokens, wall time | scheduled, results committed |
| research benchmark | multi-hop questions over linked fixture pages with planted injections, a paywall and a stale page; scores answer accuracy, citation precision (quote verified), gaps reported | scheduled |
| live smoke | read-only tasks on real public sites, never submitting | manual, reported separately |
| memory benchmark | recall quality (labelled queries, nDCG and recall@k) for dense vs hybrid vs hybrid+MMR, local Edge vs server; latency at scale | CI subset, full run scheduled |

Models: at least one frontier model and two open-weight models served locally (Ollama or vLLM), so anyone can reproduce without paid keys. The benchmark agent loop lives in `scripts/eval/` (Python, minimal, shared by all servers so it is not tuned for Ostler) and never ships in the product.

Rules: no claims in the README without a committed result file; negative results are published; every number names its model, date and commit.

## 9. Launch plan

**Assets**

- **GitHub**: v0.4 release with binaries via cargo-dist, crates.io and npm; a 60-second demo GIF per pillar (verified fill, cited research answer, memory replay after a site redesign); issue templates; `good first issue` labels on widget fixtures and pack tasks; a "write a domain pack" guide; Discussions for packs and benchmark results; ADRs as the RFC process.
- **Hugging Face**: the benchmark dataset (tasks, fixtures, outcome checks), the trajectories from the published runs, a leaderboard Space that renders committed results (it shows results; it does not host browsers), and an article: *"Measuring false success in browser agents, and the Qdrant memory that reduces it"*, covering the benchmark, the hybrid sealed-memory design (3.4), and results with open-weight models.
- **Hacker News**: a Show HN on launch day pointing to the repo, the benchmark results page and the honest limits section. Lead with the false-success number and the reproducible method, not with feature lists.
- **Qdrant community**: a technical write-up of Edge plus server memory, hybrid retrieval with sealing, and the learning loop; offered for the Qdrant blog or showcase.

**Launch gates** (all must hold):

1. False-success benchmark run on at least three servers and three models, results committed.
2. At least five domain packs with CI-green fixture suites.
3. Live smoke set run within the last two weeks.
4. THREAT-MODEL.md covers research mode, the trace, the gate classes and sealed sparse vectors.
5. Install from a release in under two minutes on clean macOS and Linux, `ostler doctor` green, memory working with no server.
6. Every open "critical" review finding fixed or listed in the README limits.

## 10. Milestones

Estimates assume one maintainer plus occasional contributors. They are planning numbers, not commitments.

| milestone | weeks | contents | exit criteria |
|---|---|---|---|
| **M0 Foundations** | 2 | Tag v0.2.x release with cargo-dist; spikes: Qdrant Edge, fastembed-rs, `qdrant-client`, `rmcp`, Readability; licence checks; ADRs for each adoption; triage open review items that block reading and acting | release installable; spike results recorded as ADRs; go/no-go on Edge |
| **M1 Verified actions and workflow core** | 4 | `expect`; `task` contract; action trace; risk-classed gate and domain policy file; receipts and idempotency; `diagnostics`; false-success scoring in `ostler-bench` | fixture suites score false success; baseline published (including current weaknesses) |
| **M2 Qdrant-native memory** | 4 | `EdgeStore` default; fastembed local embeddings; named `dense` + `bm25` vectors; hybrid weighted RRF, MMR, grouping; sealed sparse (3.4); migration via `create_vector_name`; memory benchmark with nDCG | `ostler memory` works with no server; hybrid beats dense on labelled set or the result is published as negative; THREAT-MODEL updated |
| **M3 Grounded reading and research** | 4 | `read` (Readability, chunks, PDFs), `read` with query on ephemeral shards, `extract`, `links`, `cite` with quote verification and text fragments; `search` (SearXNG + one API); `open_many`, budgets, robots.txt; `source` memory; injection flags | research benchmark green on fixtures; zero unverified citations accepted by `cite` |
| **M4 Domains and replay** | 5 | packs `gov-forms`, `jobs`, `research`, `dev-qa`, `commerce`; widgets: cross-origin iframes, autocomplete, rich text, virtualized lists, date ranges, downloads; `skill` memory and replay; session save/load; streamable HTTP | five packs CI-green; COMPATIBILITY matrix published; replay survives a fixture redesign |
| **M5 Evaluation and launch** | 3 | full benchmark runs; HF dataset, trajectories, Space; article; docs and site refresh; v0.4 release; Show HN | all launch gates in section 9 hold |

Total about 22 weeks. Cut line if time runs short: drop `travel` and `saas-admin` packs, multivector rerank and streamable HTTP; keep the false-success benchmark, Edge memory and `cite`, because the launch story depends on them.

### Code layout changes

```
crates/
  ostler-extract/    new: pure readability/markdown/chunking/table extraction, fuzzed
  ostler-research/   new: search providers, source ledger (WARC), cite verification, budgets
  ostler-workflow/   new: task contracts, trace, risk classes, receipts, sessions
  ostler-memory/     + EdgeStore, fastembed embedder, hybrid/MMR queries, source and skill kinds
  ostler-mcp/        + read/find/work profiles, presets, expect, streamable HTTP (rmcp if adopted)
  ostler-fixtures/   + packs/<name>/ fixtures and task files
  ostler-bench/      + false-success scoring, memory nDCG
scripts/eval/        new: shared model-in-the-loop agent harness, HF export
```

## 11. Defaults this plan assumes

Taken from the recommendations in the two scope docs; each can be overridden.

1. Ostler stays a tool server; no LLM in the product. A packaged research agent over A2A is after launch.
2. Identity: "verified browser for agents"; forms stay the flagship demo.
3. Local Qdrant Edge is the default memory; sealing stays default for server modes.
4. Search: SearXNG plus one commercial API; no key needed for the default install.
5. robots.txt respected by default for `open_many` and crawling, with an override.
6. Transactions stop before payment by default; finance and health are read-only.
7. The audit trace is a local file, exportable on request, with the same masking as snapshots.
8. Domain packs ship as a mechanism with five first-party packs; community packs are reviewed like code.
9. One public launch at M5, with quiet releases at M0, M2 and M4.

## 12. Risks

| risk | effect | mitigation |
|---|---|---|
| Scope against one maintainer | slips, half-finished pillars | cut line in section 10; packs are where contributors help |
| Qdrant Edge not production-ready | default local mode delayed | M0 go/no-go; bundled-server fallback behind the same trait |
| Benchmark seen as self-serving | launch credibility | shared harness, competitors' own batch tools, open-weight models, negative results published, invite maintainers to check their numbers |
| Prompt injection through research | data exfiltration | research mode keeps profile facts off, no memory writes from page text, network gates, injection fixtures in CI |
| Sealed sparse vectors leak term statistics | weaker privacy than claimed | written in the threat model; dense-only option |
| Terms of service and robots | legal and reputational | robots default, no evasion, read-only defaults in sensitive domains, per-domain policy |
| Live-web brittleness | fixture results do not transfer | live smoke set as a launch gate; results reported separately |
