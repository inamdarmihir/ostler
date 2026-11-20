# Comparison and roadmap

How Ostler differs from other browser MCP servers, and the roadmap checklist. Back to the [README](../README.md). See also the longer [ROADMAP](ROADMAP.md).

### Why Ostler?

| | Ostler | typical snapshot-per-action MCP browser |
|---|---|---|
| Fill a 20-field form | **3 tool calls** (inspect, fill, submit) | ~1 per field plus a page snapshot each time |
| Wrong or missing field | **reported** (`UNMATCHED`, `ambiguous`, read-back mismatch) | usually silent |
| Dependent dropdowns, wizards | handled inside `form_fill` / `form_submit` | agent has to notice and retry |
| Submit result | `Submitted` / `Advanced` / `NOT ACCEPTED` + the page's messages | agent infers from a new snapshot |
| Memory across sessions | **Qdrant**: facts, playbooks, episodes | none, or a browser profile |
| Tool schema cost per turn | ~1.5k tokens (`--tools forms` is smaller) | varies |

The "3 vs ~20" row is measured on our own fixtures with a scripted agent, and the comparison column is **our engine driven that way, not another product**. Tools that already offer a batched fill (Playwright MCP, chrome-devtools-mcp) are closer to Ostler than this table suggests. See [the benchmark summary](BENCHMARK-SUMMARY.md) and [BENCHMARKS.md](BENCHMARKS.md) and the [design review](REVIEW-2026-10.md).

## How it compares

From each project's own README (see [research](research/landscape.md); star counts move; we have **not** run a head-to-head):

| | language | form handling | memory |
|---|---|---|---|
| **Ostler** | Rust | typed form model, verified fill, wizards, dependent fields, shadow DOM, same-origin iframes | Qdrant: profile, playbooks, episodes |
| agent-browser (Vercel) | Rust CLI | `fill`/`select`/`check`, batching, `--delta` snapshots | cookie/storage state, auth vault |
| Playwright MCP (Microsoft) | Node | accessibility snapshots, `browser_fill_form` batch fill | browser profile |
| chrome-devtools-mcp (Google) | Node | `fill_form` batch fill plus debugging/perf tooling | none |
| Lightpanda | Zig | n/a (engine without rendering; AGPL) | n/a |

Stagehand and Browser Use cache or learn actions too; they are Python/TypeScript frameworks rather than MCP tools for coding agents.

## Competitive landscape and moat (2026-11)

Method: vendor pages and third-party roundups found by web search on 2026-11-20. Funding figures are as reported by those pages, not verified. A company is marked **YC** only where a ycombinator.com company or launch page was found. Vendor benchmarks (stealth, speed, cost) are the vendors' own and favour them; none is reproduced here. We have **not** run a head-to-head against any of these.

### Primary user: the coding agent (Claude Code, Codex, Cursor and similar)

Ostler is for the agent that is *building and checking software*, not for a business automating web workflows. That reframes the field above. The hosted-infrastructure layer (stealth, scale, per-hour browsers) is not the competition: a coding agent runs one local browser against `localhost` or a docs site. The real alternatives are the other **local MCP browsers**: Playwright MCP, Chrome DevTools MCP and agent-browser, plus doing without a browser at all.

What a coding agent needs from a browser, in the order it needs it:

1. **Did my change work?** Look at the running app, click through it, and read the console and network (`diagnostics`, `look`, `snapshot`). A page that looks fine while its API call returned 500 is the failure that matters.
2. **Read the docs and cite them.** Look up an API in the vendor's docs and be able to show where the claim came from (`read`, `cite`, `evidence`), including docs behind a login in the user's own browser.
3. **Remember what was learned about this project's sites** across sessions: which doc pages answered what, how the dev server behaves (source memory in Qdrant, `--keep-sources`).
4. **Stay cheap and safe in the agent's context:** a small tool schema, terse output, page text fenced as untrusted, a domain allowlist, no surprise submits.

Form filling, A2A, and the commerce/jobs/gov packs serve a different customer. They stay, but they are not where the next work goes. The packs that matter for this goal are `dev-qa` and `research`.

### Who is in the market, by layer

| layer | players | what they sell | YC (per ycombinator.com) |
|---|---|---|---|
| Hosted browser infrastructure | Browserbase, Kernel, Hyperbrowser, Steel, Anchor, Browserless, Cloudflare Browser Run | cloud browsers by the browser-hour, stealth, proxies, CAPTCHA, live view and replays, persistent logins | Kernel, Hyperbrowser; others not confirmed |
| Agent frameworks and clouds | Browser Use, Stagehand (Browserbase), Skyvern | an agent loop over a browser, often with an action cache, plus a hosted cloud | Browser Use, Skyvern |
| MCP browsers for coding agents | Playwright MCP, Chrome DevTools MCP, agent-browser, **Ostler** | local tools a client model drives | none confirmed |
| Page-to-data APIs | Firecrawl and similar | scrape, crawl, search at scale, anti-bot handled server-side | not confirmed |
| Benchmarks | WebArena (and "Verified"), Online-Mind2Web, Mind2Web 2 | task suites and success judges | n/a |

Where the money and effort go: the hosted layer is converging on a bundled platform (browser, search, fetch, identity, a managed agent behind one API call), competes on stealth and scale, and bills per browser-hour or per task. Anchor now advertises "cost per verified completed workflow" and a deterministic action cache with AI fallback. Stagehand and Skyvern cache actions too. Evaluation of overstated agent success is published work (Online-Mind2Web; WebArena Verified).

### What is **not** a moat

- *"Nobody measures false success."* Others do, in papers and in vendor copy. Ours is a number from one deterministic script (1 of 30), which is not a measurement.
- Form filling in few calls. Playwright MCP and Chrome DevTools MCP have batch fills; the scripted comparison could not tell the three apart on correctness.
- Playbook memory. Action caches ship in Stagehand, Skyvern and Anchor.
- Being open source or free. Browser Use, Stagehand and Skyvern are open source.
- Stealth, CAPTCHA, scale. Ostler declines these by design (ADR-0009); the hosted layer wins there.

### What could be a moat, and the measurement that decides it

Ostler is a local, model-free verifier with a Qdrant evidence store. The competitors above are infrastructure or agents; none was found shipping a *client-side verdict layer that runs in the user's own Chrome and is independent of whichever model drives it*. That is a hypothesis. Each item below is a claim we may only make once its measurement exists.

| claim | why a competitor would not simply copy it | measurement that proves it | status |
|---|---|---|---|
| **Verdicts from live page state, no model inside.** Outcome checks (read-back, `expect`, receipts) are computed from the DOM and network, so they do not share the agent's blind spots. | Agent vendors grade with a model; infra vendors do not see intent. | False-success rate with the verdict layer on vs off, same model and tasks, on a held-out set, with confidence intervals. | **Not measured.** The verdict layer is not wired to MCP. |
| **Evidence store: hash-pinned, versioned sources in Qdrant that citations are checked against.** A citation is `VERIFIED` live, `VERIFIED AGAINST A STORED COPY` with fetch time and hash, or refused. | Scrape APIs return text, not a checkable ledger; retention and sealing are product decisions they have not made. | Citation precision (quote present *and* supports the claim) and stale-citation catch rate over two-session tasks; recall quality (nDCG, recall@k) dense vs hybrid vs hybrid+MMR on labelled queries. | Mechanism tested ([ADR-0020](adr/0020-source-memory.md)); precision, recall quality and the model run **not measured**. |
| **Runs in the user's own signed-in Chrome, with the safety policy enforced in the browser** (allowlist, private-network block, confirm gate), no stealth. | Cloud vendors cannot reach the user's session; they sell the opposite posture. | Zero canary leaks across channels (in CI today) plus unsafe-action rate under injected instructions, per server, with a model in the loop. | Enforcement tested; model-in-the-loop rate **not measured**. |
| **Cost to a verified outcome.** Fewer tokens and calls *per verified completion*, not per run. | Anchor's pitch is similar; it is unverified and hosted. | Tokens and calls per verified completed task, completed tasks only, three servers, three models. | **Not measured** (scripted numbers only). |

The order of work follows from this table: run the model-in-the-loop baseline, run the verification on/off ablation, wire the verdict layer only as far as the numbers justify, and publish the result whichever way it points. If the ablation shows no drop in false success, the first row is not a moat and the project is a well-built tool, not a defensible product.

## Roadmap

- [x] CDP client, in-page runtime, form engine, Qdrant memory, MCP server, benchmark harness, `init`/`config`/`setup`
- [ ] **Fix the review findings**, starting with wrong-fill prevention and browser-level enforcement of the safety flags ([order](REVIEW-2026-10.md#recommended-order-of-work))
- [ ] **Real-model evaluation** against real tools, on a wider set of real sites than the [7-page check](BENCHMARKS.md#7-real-site-evaluation-fill-only-no-llm), published including failures
- [ ] Prebuilt binaries and a one-line installer
- [ ] Live validation of Qdrant Cloud Inference; Linux benchmark numbers
- [ ] Cross-origin iframes, HTTP transport for MCP (tabs and popups-as-tabs exist)
