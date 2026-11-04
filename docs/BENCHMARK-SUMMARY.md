# Benchmark summary (earlier README table)

The previous README summary, kept for reference. Full methodology: [BENCHMARKS.md](BENCHMARKS.md). Back to the [README](../README.md).

Fixtures only, one Apple M2 (8 cores), Chrome 156, localhost. Full tables, methodology and caveats: [docs/BENCHMARKS.md](BENCHMARKS.md).

| metric | kind | result |
|---|---|---|
| browser time per form task | measured | 0.4-1.1 s |
| 8 parallel sessions, 8 cores | measured | 354-393 tasks/min, p95 < 1 s, 100% success |
| memory per session (physical footprint) | measured | **412 MiB** `--lean` / 658 MiB default |
| cold start | measured | ~0.6 s |
| Qdrant retrieval, 200k points / 10k tenants | measured (hash vectors, 20 points per tenant) | 0.4 ms p50 |
| 20-field form, tool calls | measured (scripted agent) | **3** vs 22 one-call-per-field |
| 20-field form, LLM cost / end-to-end time | **modelled** (illustrative price, 3 s per turn, 1.5k base context) | $0.021 vs $0.105 / 13 s vs 70 s |
| browser compute | modelled from measured CPU | ~$0.006 per 1,000 tasks |

**Read these carefully.** No LLM is in the loop: a script drives both columns, so this shows call counts and bytes, not whether a model succeeds. The one-call-per-field column is Ostler itself driven the way snapshot-per-action tools work. With a realistic 20k-token agent context the 20-field cost ratio falls from ~5x to ~2.7x, and across all fixtures the aggregate is ~1.7x. A separate [real-site check](BENCHMARKS.md#7-real-site-evaluation-fill-only-no-llm) fills 7 live practice pages and reads every field back from the page itself: 44 of 45 correct, 1 honestly reported, 0 false claims after fixing the 2 it found. That is a small friendly corpus, not a success rate. The next milestone is a real-model evaluation against real tools.

Reproduce: `cargo build --release -p ostler-bench && ./target/release/ostler-bench forms --reps 3`.
