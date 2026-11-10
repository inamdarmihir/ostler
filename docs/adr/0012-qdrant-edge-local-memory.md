# ADR-0012: Qdrant Edge for local memory: adopt behind a feature flag, decide the default at M2

Status: rejected (2026-11-10). Qdrant Edge is not used: local memory is the encrypted `LocalStore` file, and the Qdrant backend is optional. The spike below stays as the record of what was checked. Originally: proposed (M0 spike, 2026-11-06). Extends [ADR-0006](0006-memory-design.md) and [ADR-0010](0010-memory-lifecycle.md); answers BUILD-PLAN section 3.1.

## Context
BUILD-PLAN 3.1 wants Qdrant Edge (in-process, shard files beside the config) as the default local memory, so local and server modes run the same query shapes, with a bundled Qdrant server as the fallback if Edge is not ready. Today the no-server option is an encrypted local file store with a non-semantic dev embedder. The question for M0 is a go/no-go on Edge **as the default**.

## Decision
1. **Adopt, opt-in.** Build `EdgeStore` behind a cargo feature `edge`, off by default, selected with `--memory edge`, with the dependency pinned to an exact version (`qdrant-edge = "=0.9.0"` today). The spike shows the API covers every query shape the plan needs.
2. **No-go for making it the default in v0.4 as things stand.** Qdrant itself says Edge is **beta** and that "the API and functionality may change in future releases" (qdrant.tech/edge, 2026-11-06). The crate is 0.x with 8 releases since 2026-03, and there is no documented on-disk format stability promise. A default store must not need a data migration on a dependency bump.
3. **Re-decide at the start of M2** against explicit gates: Edge leaves beta or the maintainer accepts the risk in writing; the release binary growth below is acceptable; a round-trip test (write with version N, open with N+1) exists in CI.
4. **Fallback if the gate fails: bundled Qdrant server**, managed by `ostler` behind the same `Store` trait: a pinned release binary (the CI already downloads one by version and sha256, `scripts/ci-install-qdrant.sh`), started on a loopback port with an API key, data dir beside the config, stopped on exit. It gives hybrid retrieval at the server floor (1.17) at the cost of a second process and a download. Not built or measured in this spike.
5. Until the gate passes, the default local store stays what it is.

## Rejected alternatives
- *Make Edge the default now.* Beta status plus no format promise; see Decision 2.
- *Bundled server only, no Edge.* Works, but a second process, a port and a download for every user. Keep as the fallback, not the first choice.
- *Our own vector index.* Duplicates what Edge already provides (HNSW, sparse, payload indexes, fusion).
- *Other embedded stores (for example SQLite-based vector extensions).* Not evaluated in this spike.

## Consequences
- One feature flag and one `Store` implementation to maintain, even if the default never moves.
- Release binaries get materially larger **if** the feature is on by default: see measurements. Feature-off builds are unaffected.
- A C compiler is needed at build time (`cc` builds quantization and f16 kernels).
- Test assertions must not depend on exact fused scores or the order of tied candidates (observed, below).

## Verification
All run 2026-11-06 on Linux x86_64, rustc 1.97.0, 4 cores shared with other jobs, so times are upper bounds. Spike crates live outside the repo and are not committed.

| claim | how verified | result |
|---|---|---|
| crate exists, licence, versions | `GET https://crates.io/api/v1/crates/qdrant-edge` | 0.9.0 (2026-10-22), Apache-2.0, 8 versions since 2026-03-03, `rust_version` unset, edition 2024 in its `Cargo.toml` (so needs Rust 1.85+; the workspace's 1.89 is fine) |
| status | qdrant.tech/edge, FAQ "ready for production?" | "in beta as of August 2026, and the API and functionality may change" |
| builds here | `cargo add qdrant-edge && cargo build --release` | yes; cold build 4m33s (default release profile) |
| build with the workspace profile | thin LTO, `codegen-units=1`, `strip` via `CARGO_PROFILE_RELEASE_*` | 15m20s; **27.7 MB** stripped binary containing only the spike (29.8 MB stripped with the default profile). Compare with the current release asset before deciding; that size was not measured here |
| dependency weight | `cargo tree -e normal --prefix none \| sort -u \| wc -l` | 344 crate versions |
| named dense + sparse vectors with IDF | spike: `EdgeConfig` with `dense` (Cosine, 4) and `bm25` sparse with `Modifier::Idf` | upserted 5 points with both; sparse score for the one point holding term 99 was 1.3863 = ln(1 + (5-1+0.5)/(1+0.5)), the BM25-style IDF for N=5, n=1 |
| Query API: prefetch + weighted RRF | spike: two `Prefetch` (dense, sparse) fused by `Fusion::Rrf{k:60, weights:[1,2]}` | the sparse-only match ranked first |
| MMR | spike: `ScoringQuery::Mmr{lambda 0.5, candidates_limit 10}` | returned 3 points (ids 1,5,4); near-duplicate id 2 excluded |
| payload index + filter | spike: keyword index on `host`, `must` match filter | returned only the two `b.example` points |
| persistence and reopen | spike: flush, drop, `EdgeShard::load(dir, None)` | count 5; sparse query still returned id 4 with the same score |
| other query kinds present | read `src/shard/query/mod.rs`, `src/edge/reexports.rs` | `Formula`, `Dbsf`, `Sample`, `OrderBy`, `Text` (BM25 over a text index), grouping builder, `CreateVectorName`/`VectorNameOperations`, snapshot manifest. **Present in the API; not exercised** |
| on-disk layout | listed the shard dir | `edge_config.json`, `wal/`, `segments/<uuid>/{segment.json,version.info,...}`; `version.info` contained `0.6.0`. No stability guarantee found in the crate README or the docs page |
| licences of the dependency tree | script over `cargo metadata` against the `deny.toml` allow list (cargo-deny itself not installed) | all 307 resolved packages fit the allow list; no exception needed |
| tied candidates | ran the same spike under two build profiles | fused scores and the order of zero-score candidates differed (ids 3 and 5 swapped; 0.015873 vs 0.016129 for id 5) |

Not verified: macOS or Windows builds (the crate has Linux-only dependencies, `io-uring` and `procfs`, so non-Linux builds exist as a code path but were not tried); memory use; query latency at scale; grouping, `create_vector_name` and snapshot behaviour; the effect of a version bump on an existing shard; fastembed-to-Edge end to end (see [ADR-0013](0013-fastembed-local-embeddings.md)).
