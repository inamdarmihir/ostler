# ADR-0011: Reliability principles

Status: accepted (2026-11-03)

## Context
Ostler acts on a user's behalf in a browser and stores personal data. The failures that cost users are not crashes; they are quiet wrong answers (a value reported filled that was not), unbounded waits, duplicate submissions, and degraded dependencies that block the main path. Earlier ADRs each fixed one instance ([0005](0005-form-ir-and-verification.md), [0007](0007-measurement-first.md), [0009](0009-challenges-are-handed-off.md)); the 2026-10 review ([REVIEW-2026-10.md](../REVIEW-2026-10.md)) found the same few causes behind most of its 74 findings. This ADR writes the principles down so new code is judged against them.

## Decision
Seven principles. Each has an example already in the tree and a test or check that keeps it honest.

1. **Fail loudly, not silently.** An operation that did not do what was asked says so, with the reason and the next step. Examples: `ServerError` is never reported as submitted; unmatched keys make a fill not-ok; Qdrant outages put `Note: memory is unavailable` on tool results; a key that cannot open existing memory stops start-up. Errors carry a class so the caller can act on them (typed, retry-aware tool errors: *in progress this cycle*).
2. **Never guess.** Ambiguity is reported with candidates, never resolved by elimination, recency or a lucky score. Memory suggestions are advisory unless they have a margin and a type match; learned mappings come only from verified, user- or agent-provided values; user facts are never overwritten by `--learn`.
3. **Bound everything: time, size, concurrency.** Every wait has a deadline (CDP requests 30 s default, navigate 30 s, Qdrant 4 s, A2A task 120 s), every collection has a cap (8 popups, `--max-contexts`, tool schemas under 7 KB, `max_chars` on snapshots), and every dependency failure has a circuit breaker (two failures, skip lookups 30 s). A2A has a request body limit (1 MiB default) and `/healthz`; a per-tool deadline across all MCP tools is *in progress, not confirmed in the tree*.
4. **Classify operations by retry safety.** Reads and idempotent actions (navigate, snapshot, inspect, hover, scroll, memory lookups) are *safe to retry*. Fills are *safe to retry if re-inspected first* (they overwrite, and read-back says the result). Submits and other non-GET requests are *not safe to retry* and are guarded by the confirm gate; after an ambiguous outcome (`ServerError`, `Unchanged`, timeout) the agent is told the data may or may not have been stored and must verify before resubmitting (a double-submit guard exists in `ostler-browser/src/page.rs`). Memory writes are idempotent upserts keyed by tenant, kind and key, so background retries are safe.
5. **Least privilege, closed by default where the caller is remote.** Uploads off unless `--upload-dir`; `confirm()` dismissed; metadata addresses always refused; `a2a` blocks private networks and requires a token off loopback; secrets are refused by the memory API; Qdrant sees ciphertext. Where a default is open (`serve` with no allowlist) it is because the local-developer case needs it, and SECURITY.md says so.
6. **Degrade, don't block.** Optional subsystems (memory, embedder, challenge helpers) failing must not stop the main path. No Qdrant means in-process memory; a slow Qdrant means lookups are skipped, not awaited; a dead Chrome is relaunched on the next call. The reverse also holds: a subsystem that protects the user (allowlist, sealing key) fails closed.
7. **Measure before claiming.** A speed, cost, scale or safety claim needs a harness, a baseline and its caveats in the repo. Gaps are written as gaps ("planned", "not measured", "open"). A fix for a bug that a reviewer found comes with a test that fails without it. See [SLOS.md](../SLOS.md) for how targets are kept honest.

## Consequences
- Some calls return "refused" where a guess would have succeeded. We accept this: a refused call costs one turn, a wrong fill costs trust.
- New tools must state their retry class and bounds in review. A tool without a bound does not merge.
- The principles can conflict (loud failure versus degrade). Rule of thumb: degrade the *capability*, but always *say* it degraded.
- Items marked "in progress" are claims about the current release cycle; the CHANGELOG is the authority on what shipped. [FAILURE-MODES.md](../FAILURE-MODES.md) tracks status per failure.

## Alternatives considered
- *Best-effort fills with a confidence score.* Rejected: callers cannot act on a number, and the review showed scores being promoted to rules.
- *Automatic retries inside the server for all tools.* Rejected: unsafe for submits; retry policy belongs to the caller once it knows the class.
