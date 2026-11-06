# ostler-memory

Qdrant-backed agent memory for Ostler: profile facts, learned form playbooks, episodes (internal crate).

Part of [Ostler](https://github.com/inamdarmihir/ostler), a browser for AI agents that fills forms, verifies every value, and learns from each fill. This crate is an implementation detail with no stability guarantee; install the `ostler` crate instead:

```sh
cargo install ostler
```

## Source memory

`Memory::save_sources` / `recall_sources` keep chunks of pages the agent read in their own collection
(`ostler_sources`, named vectors `dense` and sparse `bm25`) and find them again by meaning and by exact words:
dense and BM25-style rankings fused with weighted RRF (`fusion.rs`), then MMR and a per-URL cap. Qdrant 1.17+
does the fusion in one Query API call; older servers, the in-process store and the local file store fuse on the
client with the same formula. Sealed memory encrypts the text and keys the sparse term ids (`sparse.rs` lists
what the server can still observe). Pages expire after `source_retain_days` (default 30, `0` keeps them).
`sparse.rs` holds a minimal tokenizer meant to be swapped for the shared one in `ostler-extract`.

Licensed under Apache-2.0.
