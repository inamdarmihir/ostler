# ADR-0010: Memory has a bounded lifetime, deletion that cascades, and fails soft

Status: accepted (2026-11-03). Extends ADR-0006.

## Decision

**Retention.**
- Profile facts live until the user forgets them.
- Playbooks live while they help. `Memory::prune(max_age)` drops playbooks not verified for that long whose losses are at least their wins; proven playbooks are kept.
- Episodes are diagnostics. They expire after `EPISODE_TTL_SECS` (30 days) without being seen again, and a tenant keeps at most `MAX_EPISODES_PER_TENANT` (500); the least recently seen go first. Both limits are enforced by `Memory::trim_episodes`, which `record_episode` calls whenever a new (not repeated) episode is stored. Housekeeping errors are ignored so they never fail the write that triggered them.
- Playbooks and profile facts have no per-tenant cap yet. Playbooks are bounded in practice by the number of distinct forms filled; this is the remaining unbounded growth and is tracked as follow-up.

**Deletion.**
- `forget(key)` removes the fact **and what was derived from it**. In the data model, playbooks reference facts by `profile_key` (they hold no values), and episodes reference nothing but may echo a value in their page text. So: every playbook field mapped to the key (matched by normalised key, within the tenant) has its `profile_key` cleared, keeping the form's structure and win/loss history (the field then falls back to autocomplete, semantic search or the agent); episodes in the tenant whose detail contains the fact's string value (3+ characters, case-insensitive) are deleted. Derived records are changed first and the fact last, so a failure part-way leaves the fact in place and `forget` can be repeated.
- `wipe` deletes every point of the tenant in all three collections.
- Deletes are tenant-scoped; an unscoped bulk delete is refused by every store.
- Not deleted by `forget`: Qdrant WAL and snapshots keep data until compaction or until old snapshots are deleted (see backup). Facts whose value is not a string (numbers, objects) are not text-matched against episodes.

**Tenant isolation.** Every query and delete carries the `tenant` payload filter (keyword index, `is_tenant`). Stronger isolation is per-tenant collections via `collection_prefix` plus a scoped Qdrant token (`mint_tenant_token`), which the server enforces. Sealing (`SealedStore`) encrypts payloads and hashes lookup fields client-side, so the server holds no readable facts. The embedding-model marker point is the only point without a `tenant`; no tenant-scoped query, count, wipe or prune can reach it.

**Embedding model check.** Two models can share a dimension, in which case nothing errors and similarity is silently meaningless. The Qdrant store records the model name in a reserved point in the profile collection on first `ensure`, and refuses to open if it differs, with the old name, the new name and what to do (switch back, or use another collection prefix to start fresh). The local file store records `model` in the file and refuses the same way; a dimension mismatch is refused as before. Collections or files created before this was recorded adopt the configured model on first open. Embedders name themselves via `Embedder::model_id`. If the marker cannot be read or written (permissions, outage) the check is skipped rather than blocking memory.

**Failure modes.** Memory is an accelerator, never a dependency of form filling.
- Qdrant down or slow: a circuit breaker opens after 2 consecutive failures for 30 s, then lets exactly one trial request through. While open, calls fail immediately with `Unavailable` and `Memory::degraded()` returns the reason so callers can say "memory is off" instead of silently returning nothing. Callers treat memory errors as "no suggestion" and fill from the values the agent supplied.
- Retries: only requests that are safe to repeat are retried: reads (`search`, `search_ranked`, `scroll`, `count`) and upserts under a fixed id (`upsert`, `upsert_batch`). They are retried at most twice, only for transport errors and HTTP 429/502/503/504, with exponential backoff (100 ms base, 1 s cap) plus jitter (each wait is between half and all of the computed delay), and never beyond the request timeout in total, so a retry cannot make a lookup slower than one timeout plus a small delay. The whole sequence counts as one outcome for the breaker. Deletes, `ensure` (collection creation) and anything else are never retried.
- 4xx responses are the caller's mistake: not retried, and they do not open the breaker.
- A wrong or missing seal key, a tampered point, or a model mismatch is a configuration error and is reported, not treated as empty memory.

**Backup and restore.**
- Qdrant: use Qdrant snapshots (`POST /collections/{name}/snapshots`) of the three `ostler_*` collections (with the same `collection_prefix`) and restore into a cluster running the same embedding model. Snapshots contain the marker point, so a restore into a deployment configured with a different model is refused on open.
- With sealing on, a backup is useless without the memory key: back up the key separately (`ostler memory key export --yes-print`), and never next to the data. Restoring facts without the key leaves them unreadable.
- Local store: copy the single memory file while no write is in progress (writes are atomic renames, so a copy is always a complete file) and keep the key with it as above.
- A snapshot taken before a `forget` still holds the forgotten data. Delete old snapshots when erasure matters.

## Why
- Unbounded episode growth and episodes that outlive the fact they echo were the open lifecycle findings in `docs/REVIEW-2026-10.md`.
- Clearing a mapping instead of deleting the playbook keeps what was learned about the form while removing the link to the forgotten fact.
- Retrying only idempotent calls, bounded by the timeout and counted once by the breaker, absorbs blips without turning an outage into a pile-up of requests or hiding it from the breaker.

## Not verified
Behaviour against a live Qdrant (the retry, marker and cascade paths are tested against wiremock and the in-process store; the Qdrant-backed integration tests were not run). Snapshot restore steps are the standard Qdrant procedure, not exercised here.
