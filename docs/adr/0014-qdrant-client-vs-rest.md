# ADR-0014: Keep the hand-written Qdrant REST client; do not adopt `qdrant-client` (gRPC) now

Status: proposed (M0 spike, 2026-11-06). Answers BUILD-PLAN section 3.6. Relates to [ADR-0006](0006-memory-design.md), [ADR-0010](0010-memory-lifecycle.md).

## Context
`crates/ostler-memory/src/qdrant.rs` (1,476 lines) talks to Qdrant over REST with `reqwest` + rustls and carries the behaviour the lifecycle ADR depends on: retries with jitter for safe requests only, a circuit breaker, version probing with a client-side fallback below 1.14, tenant filters, sealing, and scoped JWT tokens. The plan proposes the official Rust `qdrant-client` so new Query API features arrive with the client, "if latency and dependency weight are acceptable".

## Decision
**Reject for now; keep REST.** Add the Query API shapes the plan needs (prefetch, weighted RRF, MMR, `group_by`, `create_vector_name`) as small typed `serde` structs next to the existing formula code. Re-open if (a) the hand-written request code for these grows past what a reviewer can hold in their head, or (b) a feature appears that REST cannot express, or (c) the M2 memory benchmark shows a latency win for gRPC that matters.

## Why
- **Coverage is not the reason to switch.** The client's generated types include everything we need (below), but REST accepts the same Query API as JSON, and we already build one of those requests (formula) by hand.
- **gRPC needs port 6334** (tonic over HTTP/2). Users with a proxy or gateway that only forwards 6333, and the bundled-server fallback in [ADR-0012](0012-qdrant-edge-local-memory.md), would need to expose a second port.
- **No build-time `protoc`.** The crate has no `build.rs` and ships generated code, so this is not a cost. (Checked: no `build.rs` in 1.20.0; `protoc` is not installed here and the crate compiled.)
- **A second HTTP stack.** `tonic` brings hyper/h2 alongside the `reqwest` we already use in memory, a2a and bench.
- **Our resilience layer would be rebuilt on top of it** (retry classes, breaker, version probe) with no measured gain.
- **No benefit measured**, see below.

## Rejected alternatives
- *Adopt `qdrant-client` wholesale.* See above.
- *Adopt it only for new Query API calls.* Two clients in one store; two error models.
- *Generate our own client from Qdrant's OpenAPI.* Heavier than the few structs we need.

## Consequences
- We own the JSON for new query shapes and must test them against the supported server floor (1.14 to 1.17) in CI, which already installs Qdrant by pinned version and sha256.
- A new Query API field is a few lines of Rust when the server grows it, not a dependency bump.
- The decision is cheap to reverse: the `Store` trait hides the transport.

## Verification
Run 2026-11-06, Linux x86_64, rustc 1.97.0.

| claim | how verified | result |
|---|---|---|
| version, licence | crates.io API | `qdrant-client` 1.20.0 (2026-10-22), Apache-2.0, `rust_version` unset, edition 2021; 1.19.0 on 2026-08-04, 1.18.0 on 2026-05-11 |
| gRPC only | read crate features and source | features `serde`, `download_snapshots` (reqwest), `generate-snippets`, `uuid`; transport is `tonic` 0.14 with `tls-ring`, `tls-native-roots`, `gzip` |
| no `protoc` at build | package listing has no `build.rs`; `which protoc` is empty; build succeeded | confirmed |
| compile cost | `cargo build --release`, `--no-default-features --features serde`, compile-only spike | 2m19s while sharing CPU; 127 crate versions. The default-feature build was not timed |
| Query API coverage | grep of generated `src/qdrant.rs` | `Mmr`, `Rrf { k, weights }`, `Formula`/`Expression`, `query`, `query_batch`, `query_groups`, `create_vector_name` all present (types and methods; **not called against a server**) |
| licence tree | script over `cargo metadata` vs `deny.toml` | all 107 resolved packages in the allow list |

**Not measured (UNVERIFIED): the latency comparison BUILD-PLAN 3.6 asks for.** It needs a running Qdrant. An attempt to download a Qdrant release binary in this sandbox was refused by the session's permission policy, so no server was available and no benchmark was run. Run it in CI, which already has a pinned server: same collection, 10k points, 1,000 queries each by REST and by gRPC, report p50, p95 and throughput. Until that exists, nothing here says gRPC is faster or slower.
