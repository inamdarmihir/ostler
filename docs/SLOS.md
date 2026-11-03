# Service level indicators and objectives (proposed)

**Everything here is proposed.** Ostler is a local tool with no hosted service, so there is nothing to alert on and no contractual SLA. These are the targets a release should be held to, each with a way to measure it using what already exists. Numbers marked "baseline" come from [BENCHMARKS.md](BENCHMARKS.md) on one laptop and fixtures; they are not Linux or real-site guarantees. Where no measurement exists, the SLO is stated as a target to be established, not a claim.

| # | SLI (what is measured) | proposed SLO | how it is measured today | baseline / status |
|---|---|---|---|---|
| 1 | **Fill correctness**: share of fields whose value, read back from the DOM by independent JS, equals what was requested, on the realistic suite | 100 percent of fields reported `Verified` are correct (0 false claims); at least 95 percent of fields end `ok` | `ostler-bench suite` (30 fixtures) and `ostler-bench real` (7 live pages, 45 fields) | real: 44/45 correct, 0 false claims; suite in [BENCHMARKS s6](BENCHMARKS.md#6-realistic-forms-suite-correctness-not-cost) |
| 2 | **Safety: leak rate**: requests reaching the canary on a non-allowlisted origin | 0 across all channels; the control run must reach the canary | `scripts/soak/soak.js` security pass; `cargo test -p ostler-browser --test security_e2e` | enforced in CI by e2e tests; soak is manual |
| 3 | **Action latency**: per-tool wall time, p50 and p95, on fixtures | p95 under 1 s for click/type/fill on fixtures; no tool avg above its previous release by more than 20 percent | `stats` tool (avg and max ms per tool), `ostler-bench concurrency` p95 | concurrency bench: p95 under 1 s at 8 sessions; per-action about 160 ms after the settle fix |
| 4 | **Cold start**: server start to first usable page snapshot | target to be established | `ostler-bench footprint` | measured in BENCHMARKS s2 (not restated here) |
| 5 | **Session stability**: tool failures over a timed soak, including one forced Chrome kill | 0 unexplained failures; recovery within one call of the kill | soak step 3 (about 20 workflows, random order; `kill -9` in cycle 4) | manual, local |
| 6 | **Memory footprint**: process-tree RSS/footprint per session, drift across the soak | at most 600 MiB per session; no monotonic growth across cycles | soak samples process-tree memory; `ostler-bench footprint` | 450-550 MiB measured (macOS) |
| 7 | **Token cost**: tool schema bytes, output tokens per completed form | schema under 7 KB (enforced); form task within 10 percent of previous release | unit test on schema size; `ostler-bench forms`; `stats` (estimated tokens) | enforced / measured |
| 8 | **Memory availability impact**: added latency when Qdrant is down | a fill completes with no more than one 4 s timeout of delay, then lookups are skipped for 30 s | circuit breaker behaviour (CHANGELOG, `ostler-memory`); `ostler-bench memory` | implemented; tail not yet benchmarked |
| 9 | **Memory retrieval**: Qdrant query latency p50 at 200k points | p50 under 5 ms (local node) | `ostler-bench memory` | about 0.4 ms measured, hash vectors |
| 10 | **A2A task success**: share of tasks ending `COMPLETED` or a deliberate `INPUT_REQUIRED`, not `FAILED` by timeout | target to be established | `crates/ostler-a2a/tests`; `/healthz` for liveness (no success-rate counters yet) | no baseline |
| 11 | **Regression gate**: `cargo test --workspace` against Chrome and Qdrant | green on every merge to `main` | `ci.yml` (Linux); `weekly.yml` runs the Chrome suites against stable Chrome | enforced |

## How to use these

- A release candidate runs: `cargo test --workspace`, `ostler-bench suite`, the soak (`SOAK_MIN=20`), and `scripts/bench-compare.sh`. Failing SLO 1 or 2 blocks the release. Others block only if regressed without an explanation in the CHANGELOG.
- An SLO is never reported as met from a run that did not produce the number. If a measurement is missing, say "not measured".
- Revisit after the first real-model evaluation ([ROADMAP.md](ROADMAP.md)); SLO 1 should then be defined on end-to-end task success, not only on field read-back.
