# Testing

Layers, cheapest first. Everything except the last three runs offline with no browser.

| Layer | What it proves | Where | Run |
|---|---|---|---|
| Unit | One function, hand-picked cases | `#[cfg(test)]` modules in each crate | `cargo test --workspace` |
| Property | Invariants over generated input (security policy, form matching, date/time/option normalisation) | `crates/ostler-browser/tests/policy_prop.rs`, `crates/ostler-forms/src/prop_tests.rs` (proptest) | `cargo test -p ostler-browser --test policy_prop` and `cargo test -p ostler-forms` |
| End to end | The engine against real Chrome and the local fixture pages | `crates/ostler-browser/tests/*_e2e.rs` | `cargo test --workspace`; tests skip without Chrome unless `OSTLER_REQUIRE_CHROME=1` (CI sets it) |
| Soak and security | Release binary over MCP stdio: hostile pages, exfiltration channels, stress pages, a timed run of form workflows | `scripts/soak/` | see `scripts/soak/README.md` (`SOAK_MIN=20 bun scripts/soak/soak.js`) |
| Fuzz | No panics, and policy/matching invariants hold, on arbitrary bytes | `fuzz/` (cargo-fuzz, not a workspace member) | see below |
| Bench | Cost and latency, not correctness | `crates/ostler-bench`, `scripts/bench-compare.sh` | see [BENCHMARKS.md](BENCHMARKS.md) |

## Property tests

Properties cover: private, loopback, link-local, CGNAT and metadata addresses are refused in decimal, hex, octal, trailing-dot, IPv4-mapped and IPv6 forms and with userinfo or ports; allow-list entries never match look-alike hosts (`allowed.com.evil.net`, `allowed.com@evil.net`, path or query tricks); no panic on arbitrary input; matching never claims a field twice, never picks a disabled field or option, never guesses between identical labels or an ambiguous day/month date; normalisation is idempotent.

A failing case is saved to a `*.proptest-regressions` file next to the test; commit it. Run more cases with `PROPTEST_CASES=10000`.

`policy_prop.rs` has one `#[ignore]` test documenting a known gap (IPv4-compatible IPv6 such as `[::127.0.0.1]` is not treated as private). Run it with `-- --include-ignored`.

## Fuzzing

Needs nightly and cargo-fuzz (`cargo install cargo-fuzz`).

```
cd fuzz
cargo +nightly fuzz run policy_check     # URL policy, all modes
cargo +nightly fuzz run forms_match      # field matching and value normalisation
cargo +nightly fuzz run formir_json      # untrusted form-IR JSON
```

Add `-- -max_total_time=60` to bound a run. Crashes land in `fuzz/artifacts/`; turn each into a unit or property test before fixing. Without cargo-fuzz, `cd fuzz && cargo check --bins` confirms the targets still compile.
