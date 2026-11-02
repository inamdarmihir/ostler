# Benchmarks

Everything below was measured by `ostler-bench` on one machine and is reproducible with the commands at the end. The harness prints its own assumptions with every table. If a number here disagrees with what you measure, trust yours and open an issue.

**Machine:** Apple M2, 8 cores, 8 GB RAM, macOS 26.6, Chrome 156, Qdrant (latest release binary at the time), single machine, everything over localhost.
**Date:** 2026-10-30. **Commit:** see `git log`.

## What these benchmarks do and do not show

- They run against the **local fixtures** in `crates/ostler-fixtures/fixtures/` (10 pages built to reproduce documented form-filling failure modes). They show the engine handles those failure modes and what it costs. They do **not** show success rates on the open web. A real-site evaluation is the next milestone ([roadmap](../README.md#roadmap)).
- The "per-field" baseline is **our own engine driven the way snapshot-per-action tools are driven** (one call per field, each returning a full page snapshot). It isolates the effect of the interaction pattern. It is **not** a measurement of Playwright MCP, agent-browser or any other product. Tools that offer batched form fills would sit between the two columns.
- LLM prices, LLM turn latency and cloud prices are **illustrative parameters** (flags on the harness). Token counts are `bytes / 4`, an estimate.

## 1. Forms: tool calls, tokens, cost, latency

Median of 3 runs. `calls` = agent tool calls. `in tok` = total input tokens the model reads across all turns (every turn re-reads the conversation). `$ cached` = LLM cost with prompt caching (cache reads at 0.1x, writes at 1.25x) at the illustrative $3 / $15 per Mtok. `e2e` = tool time + turns x 3 s.

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
3. **A fixed floor exists.** Tool schemas (~1,138 tokens at the `all` profile) plus the base prompt are re-read on every turn. That is why 3-field forms look similar across modes; `--tools core|forms` shrinks it.

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

Real Qdrant server, multi-tenant (`tenant` keyword payload index with `is_tenant`), 20 facts per tenant, 256-dim client-side vectors (a 384-dim model is ~1.5x the vector bytes). Sequential queries, localhost HTTP. `sem` = `Memory::suggest` semantic path (2 requests). `key` = exact fact lookup (1 filtered scroll, no vector math).

| tenants x facts | points | load s | sem p50 | sem p95 | sem p99 | key p50 | key p95 | QPS (16 clients) | Qdrant RSS MiB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1,000 x 20 | 20,000 | 0.6 | 0.4 ms | 0.4 ms | 0.5 ms | 0.2 ms | 0.2 ms | 2,898 | 293 |
| 10,000 x 20 | 200,000 | 8.9 | 0.4 ms | 0.4 ms | 0.5 ms | 0.2 ms | 0.3 ms | 2,792 | 1,634 |
| 10,000 x 20, on-disk vectors | 200,000 | 8.4 | 0.4 ms | 0.7 ms | 0.8 ms | 0.2 ms | 0.5 ms | 2,432 | 1,298 |

- **Latency is flat from 20k to 200k points**: filtered retrieval cost follows the tenant's own 20 points, not the collection size. That is the point of the tenant index.
- **Memory is the thing to watch.** Qdrant's RSS after load-and-query was ~8 KB per point untuned at 200k (about 6.5 KB with on-disk vectors). That is a loaded-and-queried figure that includes write-path allocations, not an isolated steady-state number. Do not extrapolate it to millions of users; measure on your own restarted node. Levers: on-disk vectors (`--qdrant-on-disk`), quantization, fewer vectors per user (exact-key and `autocomplete` lookups use payload filters and need no vector).
- Over a real network add the round-trip to the cluster; Cloud Inference adds model time on embedding calls. **Neither is measured here.**

## 5. Reproduce

```sh
cargo build --release -p ostler-bench -p ostler
./target/release/ostler-bench --json out/forms.json forms --reps 3
./target/release/ostler-bench --json out/footprint.json footprint
./target/release/ostler-bench --json out/concurrency.json concurrency --sessions 1,2,4,8 --lean
OSTLER_QDRANT_BIN=/path/to/qdrant ./target/release/ostler-bench --json out/memory.json memory --sizes 1000x20,10000x20
# Cost knobs: --price-in --price-out --llm-turn-secs --base-context   (forms)
#             --vcpu-hour-usd --gib-hour-usd                          (concurrency)
```

Raw results from the run documented here are in [`bench-results/`](../bench-results/). The `bench` GitHub workflow re-runs these on Linux runners and uploads artifacts, which also gives PSS-based memory numbers.
