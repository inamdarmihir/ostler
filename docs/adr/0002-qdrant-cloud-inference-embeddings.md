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
