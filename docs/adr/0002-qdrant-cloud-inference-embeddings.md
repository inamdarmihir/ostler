# ADR-0002: Qdrant Cloud Inference for embeddings in v1

Status: accepted (2026-10-30), revisit at v1.1

## Context
Memory needs embeddings. Options were local models, Qdrant Cloud Inference, or bring-your-own API.

## Decision
v1 supports Qdrant Cloud Inference only, behind an `Embedder` trait. Non-memory tools work without any Qdrant connection.

## Consequences
- Best Qdrant showcase and no model management, but memory needs a Qdrant Cloud cluster and key, which adds try-out friction.
- The trait leaves room for a local backend in v1.1 without changing callers.
- Model list, limits and Rust client support must be confirmed in Phase 0.

## Update (implementation)
Implemented as `VectorSource::CloudInference { model, dim }`: points are upserted with `vector: {text, model}` and queried with `query: {text, model}` per Qdrant's Cloud Inference documentation, default model `sentence-transformers/all-minilm-l6-v2` (384-dim, listed as free). Request shape is pinned by mock-server tests; **it has not been run against a live Cloud cluster.** A client-side `Embedder` path exists for tests and for development against open-source Qdrant (`--embeddings dev-hash`, deliberately non-semantic).
