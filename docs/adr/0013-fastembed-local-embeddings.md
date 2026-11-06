# ADR-0013: fastembed-rs for local embeddings: candidate, final adoption blocked on a networked spike

Status: proposed (M0 spike, partly blocked by the sandbox network, 2026-11-06). Relates to [ADR-0002](0002-qdrant-cloud-inference-embeddings.md) and BUILD-PLAN sections 3.1 and 4.

## Context
Local mode needs real embeddings without a server or a paid API: dense for meaning, sparse for learned lexical matching, optionally a reranker. The plan names fastembed-rs (ONNX Runtime). The only non-semantic option today is the dev hash embedder, which must not be the answer outside tests. This spike checks licence, MSRV, what is downloaded when and from where, the offline story, model options and weight.

## Decision
1. **Candidate: adopt fastembed-rs behind a cargo feature `local-embed`, off by default**, selected with an `Embedder` implementation next to the existing ones. Final go is **blocked**: inference was never run here because `huggingface.co` and `cdn.pyke.io` are blocked in this sandbox. Re-run the spike on a networked machine before M2 (steps below).
2. **Release builds link ONNX Runtime statically** (the crate's default `ort-download-binaries`; release CI has network) rather than asking users to install `libonnxruntime` (`ort-load-dynamic`). Source builds with `cargo install` then need network at build time; document it.
3. **Never use fastembed's default cache location.** It defaults to `.fastembed_cache` relative to the current directory (`FASTEMBED_CACHE_DIR` overrides). A server started from any directory would scatter model copies. Always pass the cache dir under the Ostler data dir.
4. **Models are downloaded at first use, from Hugging Face, only after the user opts in**, and only models on an Ostler-maintained list with a pinned revision and sha256 checked by us (fastembed pulls by repo name via `hf-hub`; pinning is our job). Offline: pre-seed the cache, or use the crate's `try_new_from_user_defined` with files from disk.
5. **BM25 is not fastembed's job.** Its sparse models are learned sparse (SPLADE++, BGE-M3 sparse, OpenSearch neural sparse), not BM25. Keyword matching for IDs and part numbers comes from Edge's or the server's BM25 ([ADR-0012](0012-qdrant-edge-local-memory.md)).

## Rejected alternatives
- *Ollama or Qdrant Cloud Inference only.* Both stay supported (ADR-0002); neither gives zero-setup local use.
- *Calling `ort` directly.* Gives up the model catalogue, tokenisation and pooling code that fastembed already contains.
- *`candle` backend* (an optional fastembed feature for Qwen3 and Nomic v2 MoE). Not evaluated.
- *Shipping model files in the binary or the npm package.* Size and per-model licences; not evaluated beyond that.

## Consequences
- A pre-release dependency: fastembed 7.6.1 pins `ort = "=2.0.0-rc.15"`, a release candidate. Upgrades are lock-step with fastembed.
- Very fast release cadence: 7.3.2 to 7.6.1 within 2026-10-23 to 2026-11-06. Pin exactly and bump deliberately.
- A new network egress to Hugging Face on first use. It belongs in THREAT-MODEL.md and `doctor` output.
- `cargo deny` would need an exception: `hf-hub -> dirs -> dirs-sys -> option-ext` is **MPL-2.0**, not in the allow list. A comment was added to `deny.toml`; no exception was added. Alternatives: disable `hf-hub` and do our own download (needed anyway for pinning), which removes the dependency.
- Model licences are separate from the crate's and **were not checked** (see below).

## Verification
Run 2026-11-06, Linux x86_64, rustc 1.97.0. Spike crates are outside the repo.

| claim | how verified | result |
|---|---|---|
| licence, MSRV, version | crates.io API for `fastembed` | 7.6.1 (2026-11-06), Apache-2.0, `rust_version` 1.88 (workspace is 1.89, fine) |
| ORT binary source | read `ort-sys-2.0.0-rc.15/build/download/dist.tsv` | `https://cdn.pyke.io/0/pyke:ort-rs/ms@1.31.0/<target>.tar.lzma2` with a sha256 per file; skipped by `ORT_SKIP_DOWNLOAD`, `ORT_OFFLINE` or `CARGO_NET_OFFLINE`; `ORT_LIB_PATH` points at a system build |
| default features fail offline | `cargo build -p ort-sys` with default features | `ort-sys failed to download prebuilt binaries from https://cdn.pyke.io/...: proxy server responded 403` (sandbox block, not a crate bug) |
| builds with `ort-load-dynamic` + rustls hf-hub | `cargo add fastembed --no-default-features --features ort-load-dynamic,hf-hub-rustls-tls`, compile-only spike | builds, 2m10s while sharing CPU; 7.5 MB stripped (ORT not linked, so not representative); 142 crate versions |
| default features weight | `cargo tree -e normal` | 320 crate versions, includes `native-tls` -> `openssl-sys` |
| models at runtime | read `src/common.rs::pull_from_hf` | fetched via `hf-hub` from `$HF_ENDPOINT` (default `https://huggingface.co`) into `$HF_HOME` or the cache dir; token from `HF_TOKEN` |
| model catalogue | spike printed `TextEmbedding::list_supported_models().len()`; read `src/models/*.rs` | 46 text models (including `BGESmallENV15`, `NomicEmbedTextV15`, `BGEM3`); sparse: `Qdrant/Splade_PP_en_v1`, `BAAI/bge-m3`, an OpenSearch sparse model; rerankers: `BAAI/bge-reranker-base`, `rozgo/bge-reranker-v2-m3`, two Jina rerankers |
| licence tree | script over `cargo metadata` vs the `deny.toml` allow list | one miss: `option-ext 0.2.0` MPL-2.0 |

**Not verified (UNVERIFIED):** that any model downloads and embeds here; inference latency and memory; the statically linked binary size; per-model licences (Hugging Face unreachable); whether `ort` rc.15 behaves on macOS arm64; checksum or revision pinning through `hf-hub`.

**Steps for the networked spike:** build with default features, embed 100 sentences with `BGESmallENV15`, run `SPLADEPPV1` sparse and `BGERerankerBase`, record release binary size, p50 and p95 latency and RSS, then repeat with the cache pre-seeded and network off to confirm the offline path.
