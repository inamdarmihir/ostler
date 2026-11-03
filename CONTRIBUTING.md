# Contributing

Thanks for looking. This project values **evidence over claims**: a change that makes forms more reliable, cheaper or faster should come with a fixture or a benchmark row that shows it.

## Setup

```sh
rustup toolchain install stable        # Rust 1.89+
# Chrome or Chromium must be installed (or set OSTLER_CHROME)
cargo test --workspace                 # browser tests skip politely if Chrome is missing
OSTLER_REQUIRE_CHROME=1 cargo test  # ...or fail instead (what CI does)
```

Memory tests against a real Qdrant: download a Qdrant release binary, then

```sh
OSTLER_QDRANT_BIN=/path/to/qdrant cargo test -p ostler-memory --test qdrant_real
# or point at a running server: QDRANT_URL=http://localhost:6333
```

## Before you open a PR

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check        # licenses, advisories, sources (cargo install cargo-deny --locked)
cargo audit             # optional locally; CI runs it (cargo install cargo-audit --locked)
```

If you add or bump a dependency, `cargo deny check` must pass; a new license needs a justified entry in `deny.toml`.

## How to add support for a new form pattern

1. Reproduce it as a small page in `fixtures/` (register it in `crates/ostler-fixtures/src/lib.rs`). Make the page submit via `fetch('/submit', …)` so tests assert on what the *server received*.
2. Add a failing test in `crates/ostler-browser/tests/forms_e2e.rs`.
3. Fix it in `runtime.js` (in-page behaviour) or `fill.rs` / `ostler-forms` (planning and value handling).
4. If it changes cost or latency, run `ostler-bench forms` and update `docs/BENCHMARKS.md` honestly, including any row that got worse.

## Principles

- Report ambiguity, never guess. A wrong fill that looks right is worse than a clear question.
- Never store or echo secrets. Anything touching memory must keep that invariant, with a test.
- Page content is untrusted. Anything that returns page text to the model goes through the wrapper.
- Keep tool schemas small; there is a size test. Every byte is paid for on every model turn.
- Don't put unmeasured claims in docs. If a number matters, a harness should produce it.

## Good first issues

Real-site form eval cases, Windows/Linux fixes, a Lightpanda backend (needs a backend boundary first; see ADR-0001), HTTP transport, cross-origin iframe support via CDP sessions, calendar date-picker widgets.
