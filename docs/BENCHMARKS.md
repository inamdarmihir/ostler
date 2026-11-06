# Benchmarks

Everything below was measured by `ostler-bench` on one machine and is reproducible with the commands at the end. The harness prints its own assumptions with every table. If a number here disagrees with what you measure, trust yours and open an issue.

**Machine:** Apple M2, 8 cores, 8 GB RAM, macOS 26.6, Chrome 156, Qdrant (latest release binary at the time), single machine, everything over localhost.
**Date:** 2026-10-30 (section 5: 2026-11-02). **Revision:** not recorded. The raw files in `bench-results/` record the version string `ostler 0.2.0` and the date, not a commit, and `git log` shows the history of this document, not the revision that was measured. Re-runs should note `git rev-parse HEAD`.

## What these benchmarks do and do not show

- They run against the **local fixtures** in `crates/ostler-fixtures/fixtures/` (the form benchmark drives 10 of the 49 form fixture pages, built to reproduce documented form-filling failure modes: basic, validation, dependent, wizard, controlled, combobox, shadow, iframe, longform, types). They show the engine handles those failure modes and what it costs. They do **not** show success rates on the open web. Section 7 checks seven live practice pages (correctness of what the tool claims, no model, small corpus); a real-model evaluation on a wider set of real sites is the next milestone ([roadmap](../README.md#roadmap)).
- The "per-field" baseline is **our own engine driven the way snapshot-per-action tools are driven** (one call per field, each returning a full page snapshot). It isolates the effect of the interaction pattern. It is **not** a measurement of Playwright MCP, agent-browser or any other product (section 5 runs Playwright MCP and Chrome DevTools MCP for real). Tools that offer batched form fills would sit between the two columns.
- LLM prices, LLM turn latency and cloud prices are **illustrative parameters** (flags on the harness). Token counts are `bytes / 4`, an estimate.

## 1. Forms: tool calls, tokens, cost, latency

Median of 3 runs. `calls` = agent tool calls. `in tok` = total input tokens the model reads across all turns (every turn re-reads the conversation). `$ cached` = LLM cost with prompt caching (cache reads at 0.1x, writes at 1.25x) at the illustrative $3 / $15 per Mtok. `e2e` = tool time + turns x 3 s.

**Baseline and mode:** `engine` is Ostler's form engine. `per-field` is the same engine driven one call per field, not another product. Scripted, no LLM in the loop, local fixtures, one machine.

| scenario | mode | calls | out tok | in tok | tool ms | e2e s | $ cached |
|---|---|---:|---:|---:|---:|---:|---:|
| basic contact form | engine | 3 | 201 | 11,300 | 473 | 12.5 | 0.0169 |
| basic contact form | per-field | 6 | 695 | 21,866 | 499 | 21.5 | 0.0236 |
| validation (3 rules) | engine | 3 | 177 | 11,218 | 491 | 12.5 | 0.0166 |
| validation (3 rules) | per-field | 5 | 498 | 18,021 | 490 | 18.5 | 0.0211 |
| dependent dropdowns | engine | 3 | 229 | 11,400 | 845 | 12.8 | 0.0170 |
| dependent dropdowns | per-field | 6 | 879 | 22,679 | 852 | 21.9 | 0.0245 |
| 3-step wizard | engine | 7 | 385 | 23,917 | 1,014 | 25.0 | 0.0238 |
| 3-step wizard | per-field | 9 | 987 | 33,244 | 1,022 | 31.0 | 0.0302 |
| controlled inputs | engine | 3 | 164 | 11,199 | 472 | 12.5 | 0.0166 |
| controlled inputs | per-field | 5 | 457 | 17,884 | 488 | 18.5 | 0.0210 |
| custom combobox | engine | 3 | 128 | 11,079 | 423 | 12.4 | 0.0162 |
| custom combobox | per-field | 3 | 184 | 11,193 | 421 | 12.4 | 0.0165 |
| shadow DOM | engine | 3 | 158 | 11,160 | 467 | 12.5 | 0.0165 |
| shadow DOM | per-field | 4 | 331 | 14,470 | 469 | 15.5 | 0.0188 |
| same-origin iframe | engine | 3 | 169 | 11,172 | 450 | 12.4 | 0.0165 |
| same-origin iframe | per-field | 4 | 377 | 14,586 | 461 | 15.5 | 0.0190 |
| **long form (20 fields)** | **engine** | **3** | **585** | **12,544** | **1,055** | **13.1** | **0.0210** |
| **long form (20 fields)** | **per-field** | **22** | **8,337** | **166,358** | **1,113** | **70.1** | **0.1050** |
| rich inputs (9 fields) | engine | 3 | 339 | 11,732 | 658 | 12.7 | 0.0179 |

All 19 rows completed successfully (the server received the expected submission). The per-field baseline cannot drive the `rich inputs` page (radio and checkbox groups have no single control to address), so it is omitted there.

**Totals over the 9 scenarios both modes support:** engine 31 calls / 117k tokens / $0.161; per-field 64 calls / 324k tokens / $0.280. Ratio per-field/engine: **2.1x calls, 2.8x tokens, 1.8x latency, 1.7x cost.**

### What this says, honestly

1. **The advantage scales with form size.** Small forms (2-4 fields): ~1.3-2x. A 20-field form: **7.3x fewer calls, 13x fewer input tokens, 5.0x cheaper, 5.4x faster end to end.** Per-field cost grows with fields x conversation length (quadratic-ish); the engine path stays at 3 calls.
2. **End-to-end time is dominated by LLM turns, not the browser.** Tool time is 0.4-1.1 s of a 12-25 s task. Making the browser faster barely moves the total; **removing round-trips does.** This is why the design optimises call count and output size before raw browser speed.
3. **A fixed floor exists.** Tool schemas (~1,138 tokens at the `all` profile when this was measured; the 21-tool set is now about 5.9 KB, roughly 1.5k tokens, see the README) plus the base prompt are re-read on every turn. That is why 3-field forms look similar across modes; `--tools core|forms` shrinks it.

## 2. Browser footprint and startup

One real task (navigate + fill + submit on the 20-field form), median of 3. Memory is **physical footprint** on macOS (`footprint`), PSS on Linux. Summed RSS would read ~3.3x higher on macOS because shared pages are counted per process, so it is not used.

| profile | start ms | processes | memory MiB | task ms | success |
|---|---:|---:|---:|---:|---:|
| default | 631 | 10 | 658 | 585 | 100% |
| lean (`--lean`) | 565 | 10 | 412 | 587 | 100% |

`--lean` (lean Chrome flags + no image/font/media downloads) cuts memory by 37% with no change in task time. Site isolation stays on. Screenshots omit images in this mode.

A latency bug found by these benchmarks and fixed: the page runtime's own settle-polling reset its own quiet-timer, so every action waited the 3 s cap. After the fix, `about:blank` round-trips went from 3,031 ms to 160 ms and the e2e suite from 40 s to 7 s.

## 3. Concurrency and cost to serve

Each session is its own Chrome + MCP app. Task mix: 6 form scenarios (navigate + fill + submit), the dependent-dropdown API adds 250 ms of simulated latency. 6 tasks per session.

| sessions | tasks/min | p50 ms | p95 ms | mem/session MiB | CPU-s/task | compute $/1k tasks* | success |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 79 | 507 | 891 | 549 | 0.52 | 0.0064 | 100% |
| 2 | 150 | 486 | 861 | 508 | 0.48 | 0.0059 | 100% |
| 4 | 262 | 495 | 894 | 505 | 0.48 | 0.0060 | 100% |
| 8 | 354 | 605 | 960 | 483 | 0.50 | 0.0065 | 100% |
| 8 (lean) | 393 | 527 | 918 | 454 | 0.48 | 0.0061 | 100% |

\* Compute only: CPU-seconds and RAM-time at illustrative cloud prices ($0.04/vCPU-h, $0.005/GiB-h). Excludes LLM tokens, Qdrant, bandwidth, proxies, idle capacity. CPU includes Chrome startup amortised over 6 tasks.

Reading: throughput scales near-linearly to 8 sessions on 8 cores with flat latency and no failures. **Compute is ~$0.006 per 1,000 tasks; the LLM bill for the same tasks is ~$17-24 per 1,000 (section 1). Compute is roughly 0.03% of the cost of a task.**

## 4. Memory (Qdrant) at scale

Real Qdrant server, multi-tenant (`tenant` keyword payload index with `is_tenant`), 20 facts per tenant, 256-dim client-side vectors, no sealing (the harness does not wrap the store in `SealedStore`) (a 384-dim model is ~1.5x the vector bytes). Sequential queries, localhost HTTP. `sem` = `Memory::suggest` semantic path (2 requests). `key` = exact fact lookup (1 filtered scroll, no vector math).

| tenants x facts | points | load s | sem p50 | sem p95 | sem p99 | key p50 | key p95 | QPS (16 clients) | Qdrant RSS MiB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1,000 x 20 | 20,000 | 0.6 | 0.4 ms | 0.4 ms | 0.5 ms | 0.2 ms | 0.2 ms | 2,898 | 293 |
| 10,000 x 20 | 200,000 | 8.9 | 0.4 ms | 0.4 ms | 0.5 ms | 0.2 ms | 0.3 ms | 2,792 | 1,634 |
| 10,000 x 20, on-disk vectors | 200,000 | 8.4 | 0.4 ms | 0.7 ms | 0.8 ms | 0.2 ms | 0.5 ms | 2,432 | 1,298 |

- **Latency is flat from 20k to 200k points**: filtered retrieval cost follows the tenant's own 20 points, not the collection size. That is the point of the tenant index.
- **Memory is the thing to watch.** Qdrant's RSS after load-and-query was ~8 KB per point untuned at 200k (about 6.5 KB with on-disk vectors). That is a loaded-and-queried figure that includes write-path allocations, not an isolated steady-state number. Do not extrapolate it to millions of users; measure on your own restarted node. Levers: on-disk vectors (`--qdrant-on-disk`), quantization, fewer vectors per user (exact-key and `autocomplete` lookups use payload filters and need no vector).
- Over a real network add the round-trip to the cluster; Cloud Inference adds model time on embedding calls. **Neither is measured here.**

## 5. Ostler vs Playwright MCP vs Chrome DevTools MCP (same fixtures, scripted agent)

Section 1 compares Ostler with a *pattern*. This section runs the **real competitor servers** on the same local fixture pages and checks the result on the fixture server, not in the tool's reply.

**What was run (2026-11-02, Apple M2, macOS 26.6 / Darwin 25.6.0, one machine, localhost, all headless against the same Google Chrome 156.0.8078.74):**

| server | version | tools | tool definitions (est. tokens, sent every turn) |
|---|---|---:|---:|
| Ostler | 0.2.0 (this commit) | 21 | 1,513 |
| Playwright MCP | npm `@playwright/mcp` 0.0.84 (reports Playwright 1.65.0-alpha-2026-10-30), `--headless --isolated --browser chrome` | 25 | 5,265 |
| Chrome DevTools MCP | npm `chrome-devtools-mcp` 1.12.0, `--headless --isolated --executablePath <Chrome>` | 32 | 7,416 |

Node v26.10.0. Median of 3 runs per task in one session per server (browser pre-launched by a warm-up call; cold start is reported separately in the JSON). Raw data: [`bench-results/compare.json`](../bench-results/compare.json), table: [`bench-results/compare.md`](../bench-results/compare.md).

**Method.** There is no LLM. A deterministic script plays an "ideal agent" per server: the shortest sensible sequence, with element refs parsed from the server's own output, never guessed. Each task is judged only by what the fixture server received (`GET /__submissions`): exactly one submission, equal field for field to the expected record. `steps` = tool calls. `out tokens` = bytes of tool output text / 4 (an estimate, as elsewhere in this file; no images were returned). `wall ms` = sum of tool-call times (no LLM time, no post-submit wait).

Sequences (the best idiomatic one found for each, using each server's batch fill tool):

- **Ostler:** `navigate` (observe `interactive`, which lists the controls) -> `form_fill` (label -> value map) -> `form_submit`; repeated per step on the wizard. Variant **+form_inspect**: `navigate` (observe `none`) -> `form_inspect` -> `form_fill` -> `form_submit`.
- **Playwright MCP:** `browser_navigate` -> `browser_snapshot` (in this version navigate returns the snapshot as a file link, so an MCP-only agent calls `browser_snapshot`) -> `browser_fill_form` -> `browser_click` on the submit button. Wizard: snapshot + fill + click per step. Dependent dropdowns: two `browser_fill_form` calls (country, then state and city); Playwright's select waits for the options by itself.
- **Chrome DevTools MCP:** `navigate_page` -> `take_snapshot` -> `fill_form` (`includeSnapshot=false`) -> `click`. Wizard: snapshot + fill + click per step. Dependent dropdowns: fill country, `wait_for` the text "Bavaria", fill state and city, click (a single batch failed because the option list loads after a fetch).

| task | server | steps | out tokens | wall ms | correct |
|---|---|---:|---:|---:|---:|
| basic form | **Ostler** | **3** | 227 | **526** | 3/3 |
| basic form | Playwright MCP | 4 | 355 | 710 | 3/3 |
| basic form | Chrome DevTools MCP | 4 | **168** | 840 | 3/3 |
| validation (3 rules) | **Ostler** | **3** | 202 | **497** | 3/3 |
| validation (3 rules) | Playwright MCP | 4 | 300 | 1,116 | 3/3 |
| validation (3 rules) | Chrome DevTools MCP | 4 | **156** | 1,287 | 3/3 |
| dependent dropdowns | **Ostler** | **3** | **238** | **844** | 3/3 |
| dependent dropdowns | Playwright MCP | 5 | 389 | 1,921 | 3/3 |
| dependent dropdowns | Chrome DevTools MCP | 6 | 476 | 1,390 | 3/3 |
| 3-step wizard | **Ostler** | **7** | 396 | **1,036** | 3/3 |
| 3-step wizard | Playwright MCP | 10 | 784 | 2,740 | 3/3 |
| 3-step wizard | Chrome DevTools MCP | 10 | **372** | 2,008 | 3/3 |
| long form (20 fields) | **Ostler** | **3** | **487** | **1,174** | 3/3 |
| long form (20 fields) | Playwright MCP | 4 | 1,132 | 2,268 | 3/3 |
| long form (20 fields) | Chrome DevTools MCP | 4 | 694 | 2,394 | 3/3 |

Totals over the 5 tasks: **Ostler 19 steps / 1,550 tokens / 4.1 s; Playwright MCP 27 / 2,960 / 8.8 s; Chrome DevTools MCP 28 / 1,866 / 7.9 s.** All three completed all 5 tasks correctly in every run. The +form_inspect variant: 24 steps, 1,496 tokens (one more call per task for about the same bytes).

### Where Ostler loses or does not differ

- **On small forms Chrome DevTools MCP returns fewer tokens** than Ostler (basic 168 vs 227, validation 156 vs 202) and about the same on the wizard (372 vs 396). Ostler's replies carry more (field read-back and verification text), which this byte count charges as cost. Whether that read-back is worth it (it is what lets an agent detect a silent failure) is **not measured here**; every tool succeeded on every run, so these fixtures never exercised it.
- **All three finish every task.** With a batch fill tool (both competitors have one) the gap is steps (about 1.4x fewer) and bytes on the largest form, not correctness. The large gaps in section 1 are against a one-field-per-call pattern, which neither competitor requires.
- **Wall time is not a browser-speed win.** Tool time here includes each server's own settling; differences of a few hundred ms are well under one LLM turn (3 s assumed in section 1). Do not read the wall column as end-to-end latency.
- **Tool definitions are the larger per-turn cost.** Ostler's 21 tool definitions are about 1.5k estimated tokens versus 5.3k and 7.4k; on a 4-call task that is repeated every turn and can exceed all of the tool output. It is a property of the tool lists, not of form handling.

### Caveats

- **No LLM.** A real model may take more or fewer steps than the scripted ideal (wrong ref, retries, extra snapshots). The script gives every server its best case; real agents on primitive tools usually do worse, which this does not show.
- **Fixtures only, 5 tasks, one machine, one Chrome, headless.** None of the 49 form fixture pages' hostile behaviours (CAPTCHA, shadow DOM, comboboxes, rich inputs) are run here; the competitors may or may not handle those, and that is **not measured**. No real sites.
- **Token counts are bytes / 4**, not a tokenizer. Playwright snapshots and Ostler observations differ in format, so the ratio is approximate.
- **Versions move.** Playwright MCP changes its snapshot-on-navigate behaviour between releases (this version links a file); re-run with `PLAYWRIGHT_MCP_VERSION=latest`. The sequences are my best attempt per server, not blessed by their authors. If you find a shorter one, open an issue.
- Cold start (server + Chrome launch) is in the JSON but not tabled: it varied 0.7-7.7 s between runs, mostly Node start-up and package loading, and is too noisy to rank.

## 6. Realistic-forms suite (correctness, not cost)

`ostler-bench suite` runs 30 form tasks through `form_inspect` / `form_fill` / `form_submit` (the MCP tool layer) against local fixtures that copy how real sites structure these forms: multi-step checkout with a billing-same-as toggle, an ATS application with resume upload and EEO selects, tax-portal style split date-of-birth selects with PAN/Aadhaar masks and state-to-district dependent selects, address autocomplete, phone with a country-code dropdown, password rules with confirmation, consent boxes, radio "other" text, custom date pickers, contenteditable, nested shadow DOM, an iframe, fields revealed by a selection, async username checks, CSRF/hidden fields, chip inputs, button-and-listbox dropdowns, and seven tasks that must be refused (CAPTCHA, wrong format, server rejection, disabled control, unknown field, terms that stay disabled, username taken). Nothing is scraped; the pages are written from knowledge of those patterns.

Every task has a ground-truth final state, and success is judged by **what the fixture server received**, never by what the tool said. A wrong value, or a "Submitted" that the server did not see, is a *wrong-fill*; the suite exits non-zero on any.

| run | pass | correctly refused | failed honestly | wrong-fills |
|---|---|---|---|---|
| first run, 27 tasks, before fixes | 17 | 6 | 3 | **1** |
| after fixes, 30 tasks | 23 | 7 | 0 | 0 |

Read that honestly: the suite was written by the people who fixed what it found, so 30/30 is a regression floor, not a claim about the open web. The first run is the informative one: a bare day number (`5`) matched the options 05, 15 and 25 and was refused as ambiguous; an address suggestion list that shows the full address could not be picked by its street; inputs inside nested shadow roots were split into separate forms with no submit button; and an array sent to a chip-style combobox was typed into the box as text and reported *Verified* while nothing had been chosen (the wrong-fill). The tests that guard these are in `crates/ostler-browser/tests/suite_regressions_e2e.rs`.

```sh
./target/release/ostler-bench suite                # whole suite; --only <text> for one task; --json out/suite.json
OSTLER_SUITE_TRANSCRIPT=1 ./target/release/ostler-bench suite --only chip   # print the tool output for non-passing tasks
```

## 7. Real-site evaluation (fill only, no LLM)

`ostler-bench real` opens seven live public form-practice pages (demoqa's practice form, formy, practice-automation, selenium.dev's `web-form.html`, testpages.eviltester, expandtesting's register page, ultimateqa's contact page), calls `form_inspect` then `form_fill`, and then reads **45 fields back out of the page with our own JavaScript**, never from the tool's report. It does **not** submit: these are other people's servers.

Every field ends in one of three states: **ok** (the page holds the expected value), **flagged** (it does not, and the fill report said so: `UNMATCHED`, `FAILED`, a warning naming the key), or **false claim** (it does not, and the report did not say so). False claims are the number that has to be zero. The run exits non-zero on any.

| run | fields ok | flagged | false claims | slowest `navigate` |
|---|---:|---:|---:|---:|
| first run | 42 / 44 | 0 | **2** | 38.7 s |
| after the two fixes below | 44 / 45 | 1 | **0** | 14.0 s |

The one flagged field is a deliberate probe: a key (`Favourite colour of the moon`) that matches nothing on the page. The report says `UNMATCHED key … no field on the page matches this key`. The same page's disabled and read-only inputs are asked to change; both keep their value and both are reported (`is disabled on this page`, `is read-only`).

What the first run found, and what changed:

1. **A silent tie between forms.** `ultimateqa.com/filling-out-forms/` has two forms that both have a `Name` and a `Message`. Ostler filled the *later* one (the one with a CAPTCHA question) and said `Filled 2/2`. Nothing on the page was wrong, but the agent was never told there was a choice. A tie now goes to the first form in the document, and the report says `WARNING: 1 other form(s) on this page matched these keys equally well (e15); this filled form e11. Pass form=<ref> to choose another.` (`fill.rs`, regression test `two_forms_that_match_equally_fill_the_first_and_name_the_other`).
2. **`navigate` waited for the `load` event, up to 30 s.** `load` waits for every image, ad and tracker. `practice.expandtesting.com` took 31.7 s to open and `practice-automation.com` 38.7 s, while the form was usable in under a second (the latter's server itself needs 6.5 s to answer). Now, once the new document is interactive, `load` gets 5 more seconds and Ostler carries on, telling the agent so (`page was still loading images or scripts 5s after it became usable; continuing`). Those two pages now open in 7.5 s and 14.0 s. The same cap applies to clicks that navigate. A fixture with a stalled image guards it (`a_stalled_subresource_does_not_hold_navigation_for_the_whole_timeout`).

Per-call time on these pages: `form_inspect` 4 to 25 ms, `form_fill` 68 to 385 ms. Opening the page is the cost, as on every browser tool.

**What this does not show.** The corpus is small, and the pages are *practice* pages built to be automated; they are friendlier than a bank, a government portal or a checkout behind a bot wall. The fill values and the DOM checks were written by us after looking at each form, so this is a check that the engine's claims match the page, not a held-out test of success on the open web. There is no model in the loop and no comparison with another tool. Sites change or disappear: a page that cannot be loaded is reported `unreachable` and left out of the rates. Do not quote 98% as a success rate on real websites.

```sh
./target/release/ostler-bench real                 # needs network; --only <text> for one site; --json out/real.json
OSTLER_REAL_TRANSCRIPT=1 ./target/release/ostler-bench real --only selenium   # print the tool output
```

## 8. Reproduce

```sh
cargo build --release -p ostler-bench -p ostler
./target/release/ostler-bench --json out/forms.json forms --reps 3
./target/release/ostler-bench --json out/footprint.json footprint
./target/release/ostler-bench --json out/concurrency.json concurrency --sessions 1,2,4,8 --lean
OSTLER_QDRANT_BIN=/path/to/qdrant ./target/release/ostler-bench --json out/memory.json memory --sizes 1000x20,10000x20
scripts/bench-compare.sh          # section 5: builds, npm-installs the pinned competitors in a temp dir, runs, writes bench-results/compare.{json,md}
# Cost knobs: --price-in --price-out --llm-turn-secs --base-context   (forms)
#             --vcpu-hour-usd --gib-hour-usd                          (concurrency)
```

Raw results from the run documented here are in [`bench-results/`](../bench-results/). The `bench` GitHub workflow re-runs these on Linux runners and uploads artifacts, which also gives PSS-based memory numbers.
