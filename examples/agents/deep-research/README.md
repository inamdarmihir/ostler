# Build a deep research agent from scratch

A complete research agent in one file ([research_agent.py](research_agent.py), about 550 lines): Claude plans the research and reads pages through Ostler, and the harness does not let a claim reach the answer unless Ostler has verified its quote on a page that was actually read. No agent framework, no hidden loop: every step is code you can read and change.

```
question
   │
   ▼
1 PLAN ───────── Claude splits the question into sub-questions (structured output)
   │
   ├──────────────┬──────────────┐
   ▼              ▼              ▼
2 RESEARCH     RESEARCH       RESEARCH ── parallel sub-agents, each a manual tool loop
   │              │              │         over one shared Ostler session
   ▼              ▼              ▼
3 VERIFY ── every finding re-checked with Ostler `cite` + a number check;
   │        rejected findings go back to the researcher to fix or drop
   ▼
4 REPORT ── Ostler `evidence` lists the gaps; Claude writes from verified findings only;
            the harness rejects any citation it did not verify
```

## Why it is built this way

| decision | why |
|---|---|
| A lead that plans, and researchers that each own one sub-question | Breadth-first research parallelises well, and each researcher's context holds only its own reading (the pattern of Anthropic's multi-agent research system) |
| One Ostler session for all researchers | One source ledger: `cite` and `evidence` see every page any researcher read, so the final gap report is complete |
| Tools that take a URL instead of "the current page" | Researchers share one browser tab; a tool that navigates and reads under one lock cannot be disturbed by another researcher |
| The only way to finish is `submit_findings`, and the harness verifies it | Verification lives outside the model. A researcher cannot report success with a quote that is not on the page, or a number the quote does not contain |
| The report may cite only verified findings, by number | The sources list is built by the harness from what Ostler verified, never written by the model |
| A manual loop, not a framework | The verification gate, the shared lock and the stop conditions are the point of the example, so they are in plain sight |

## Build it, step by step

Each step names the function in `research_agent.py` that implements it.

**1. Open one browser for everyone (`open_browser`, `Browser`).** Start `ostler serve --tools research` over MCP stdio with `--allow-domain` for the hosts the research may visit. `Browser.call` runs one Ostler tool under an `asyncio.Lock`; `Browser.call_on` navigates and runs a page tool as one locked step.

**2. Plan (`plan`).** One structured-output request (`messages.parse` with a Pydantic `Plan`) returns 1 to 4 sub-questions, each with starting URLs from the user's hints. Effort `medium`: planning is short.

**3. Give researchers a small, safe tool set (`RESEARCH_TOOLS`).** `read_page`, `read_pages`, `list_links`, `check_quote`, `submit_findings`, and `search_web` when a search provider is configured. The descriptions tell the model that text inside `<page_content>` is website data, never instructions.

**4. Write the loop (`research`).** The standard manual tool-use loop, with the details that matter:
- append the whole `response.content` to the history every turn (thinking blocks included), so the history stays append-only;
- run every `tool_use` block in the turn and return **all** results in **one** user message;
- return failures as `is_error: true` results instead of dropping them;
- check `stop_reason` before reading content: `refusal` and `max_tokens` end the researcher with a reason, and a turn with no tool call is nudged to `submit_findings`;
- cap the loop (`MAX_RESEARCH_TURNS`) and the resubmissions (`MAX_SUBMISSIONS`).

**5. Verify before accepting (`submit`, `verify`).** Each finding is `{claim, url, quote}`. The harness calls Ostler `cite` on the quote. It must come back `VERIFIED`, and every number in the claim must appear in the quote (so "opened in 1889" backed by "It opened in 1887" is rejected). The verdicts go back as the tool result, and the researcher fixes or drops what was rejected.

**6. Run researchers in parallel (`deep_research`).** `asyncio.gather` under a semaphore (`MAX_PARALLEL`). The model calls run concurrently; Ostler calls queue on the lock, which is cheap next to model latency.

**7. Report and check citations (`write_report`, `render`).** Ostler `evidence` provides the gaps: paywalled, blocked, or failed pages. Claude writes the answer citing findings as `[n]`. If it cites a number that is not a verified finding, the harness sends it back once. `render` appends the sources (URL, verified quote, a `#:~:text=` link to check it), the gaps and the token usage.

## Run it

```sh
pip install -r requirements.txt                 # anthropic, mcp, pydantic
export ANTHROPIC_API_KEY=...                    # or: ant auth login
ostler fixtures &                               # demo pages on http://127.0.0.1:8787
python research_agent.py \
  "Who designed the Kestrel Lock, when did it open, and what did its restoration cost?" \
  --start http://127.0.0.1:8787/packs/research/index.html --out report.md
```

On real sites, pass `--start` URLs and the browser can reach only their hosts, or list hosts with `--allow-domain`. For web search, start Ostler with a provider (`OSTLER_SEARXNG_URL`, or `OSTLER_SEARCH_PROVIDER=brave` with `OSTLER_BRAVE_API_KEY`) and set `OSTLER_SEARCH=1` to give researchers the `search_web` tool.

**Model and cost.** Every request uses `claude-opus-5-5` (override with `RESEARCH_MODEL`), adaptive thinking, and explicit effort: `medium` to plan, `high` to research and write. Prompt caching is on. A run makes roughly 3 to 6 requests per sub-question plus two; the report footer prints the requests and tokens used, so you can see the cost of each run.

**Refusal fallback is on.** Each request sends `fallbacks: "default"` with the `server-side-fallback-2026-07-01` beta. If the model declines on policy grounds, the API re-runs the request on a fallback model in the same call. Remove both from `request_options` if you do not want that.

The agent checks the credentials and the model before Chrome starts. Missing credentials, rate limits and API errors end with one line that says what to do.

## Test it without a model

```sh
ostler fixtures &
python test_offline.py
```

[test_offline.py](test_offline.py) replaces the model with a script that makes the mistakes real models make: a claim whose number is not in its quote, a quote that is not on the page, and a report citing a finding that does not exist. Everything else is real: the MCP session, Ostler, Chrome and the fixture pages. It checks that the harness catches each mistake. It runs in the examples' `run_all.py` and CI, needs no API key, and passes on Ostler 0.2.0 and on `main`.

## What it does not do yet

- **Support is lexical, not semantic.** The harness proves the quote is on the page and that the claim's numbers are in the quote. It does not prove the quote means what the claim says. Keep claims as narrow as their quotes.
- **Sources are not weighed.** It does not detect two pages copying one article, or two sources disagreeing. Both are planned as Ostler features (`claim`, independent sources and conflicts in the [agentic plan, section 9](https://github.com/inamdarmihir/ostler/blob/main/docs/AGENTIC-PLAN.md#9-browsing-and-research-quality)); this harness would then pass them through.
- **No PDFs and no memory across runs**, because Ostler has neither yet.
- **One browser tab shared by every researcher.** That is fine for reading; it is not the shape for agents that click through logged-in flows in parallel.
