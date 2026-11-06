# ostler-extract

Pure-logic page reading helpers for Ostler (internal crate): chunking, BM25 retrieval, quote verification, text-fragment URLs, prompt-injection heuristics and result-completeness checks. No I/O, no async, no browser dependency.

Part of [Ostler](https://github.com/inamdarmihir/ostler), a browser for AI agents that fills forms, verifies every value, and learns from each fill. This crate is an implementation detail with no stability guarantee; install the `ostler` crate instead:

```sh
cargo install ostler
```

Licensed under Apache-2.0.
