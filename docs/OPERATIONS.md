# Operations runbook

For people running `ostler serve` for an agent, or `ostler a2a` for other agents. Symptom-by-symptom fixes are in [TROUBLESHOOTING.md](TROUBLESHOOTING.md); this page is the order of operations. There is no hosted service: you are the operator.

## 1. First check, always

```sh
ostler doctor            # launches Chrome, round-trips a page, reports memory and Qdrant, prints a `fix:` line per failure
ostler doctor --fix      # applies only the safe fixes (creates the config dir, tightens key file permissions)
```

`doctor` never creates the sealing key. Include its output and your OS and Chrome version in any issue.

## 2. Logs and signals

- Logs go to **stderr**; stdout carries MCP messages only. `ostler serve` prints one `ready` line on stderr. Capture stderr in the MCP client's log or run `ostler serve` in a terminal.
- `--verbose` (`OSTLER_VERBOSE`) gives fuller tool output.
- The `stats` tool (profile `all`) reports calls, errors, average and max ms per tool, estimated output tokens and browser memory for the session.
- A tool result starting with `Note: memory is unavailable` means the Qdrant circuit breaker is open (two failures; retries after 30 s).
- A2A: `GET /.well-known/agent-card.json` is a liveness check today. `GET /healthz` (and `/readyz`) also exists in `ostler-a2a/src/server.rs` and is exempt from the bearer check; verify its exact response for your version before wiring an orchestrator to it.

## 3. Common failures (short form)

| you see | do |
|---|---|
| Chrome fails to launch | `ostler doctor`; set `OSTLER_CHROME`; in containers `OSTLER_NO_SANDBOX=1` and a larger `/dev/shm` |
| Agent sees no tools | something wrote to stdout; run `ostler serve` in a terminal; reconnect the client |
| `refusing to create a new memory key` / key unreadable | start Qdrant and retry, or restore the key backup with `ostler memory key import < memory.key.backup` |
| `retired sealed format` / `older key normalisation` | `ostler memory seal --tenant T` |
| Qdrant unreachable | fills continue without memory; fix URL/key, then `ostler doctor` |
| `SERVER ERROR` from `form_submit` | do not blindly retry; the data may be stored. Check the site first |
| A2A refuses to start | non-loopback bind needs `--a2a-token` |
| Tasks `failed` by timeout | raise `--task-timeout-secs` (default 120) or check for a challenge page |
| Disk fills with temp profiles | a SIGKILLed server leaves its temp profile; delete stale `ostler` profile dirs in the temp directory when no server is running |

## 4. Upgrade

1. Read the CHANGELOG entries between your version and the target, especially "Breaking" ([COMPATIBILITY.md](COMPATIBILITY.md)).
2. Back up the key: `ostler memory key export --yes-print > memory.key.backup && chmod 600 memory.key.backup`. Back up the config file (path: `ostler config path`).
3. Install the new binary (`pip install -U ostler-mcp`, the install script, a release archive from [RELEASING.md](RELEASING.md), or `cargo install`). The launcher verifies checksums and refuses a mismatch.
4. Run `ostler doctor`. If it asks for `ostler memory seal`, run it; start-up refuses until stored data is current.
5. Reconnect the MCP client; clients keep the old server process until restarted (Claude Code: `/mcp`).

## 5. Rollback

1. Reinstall the previous version of the binary (`pip install ostler-mcp==X.Y.Z` or reinstall the previous archive).
2. Check the CHANGELOG for a data-format change between the two versions. Sealed `v2` memory is not readable by builds that only know `v1`, and a migration (`memory seal`) is one-way for the points it upgrades. If a migration ran, either stay on the new version or restore Qdrant from a snapshot taken before the upgrade (take one in step 2 of the upgrade if memory matters; Qdrant snapshots are your operator's responsibility).
3. `ostler doctor`, then reconnect the client.

## 6. Secrets and key rotation

| secret | where it lives | rotate by |
|---|---|---|
| Sealing key | `memory.key` beside the config (owner-only), `OSTLER_MEMORY_KEY`, or the OS keychain (`key_store = keychain`) | There is no in-place re-key command. To change it: export old key for backup, `ostler memory wipe --yes` for the tenant, create a new key, let facts be re-entered (`ostler memory remember ...`) and skills relearn. `ostler memory key import --force` replaces the key and keeps a backup as `memory.key.old-*`; it does not re-encrypt existing points. |
| Qdrant API key | config file (`ostler config`, masked on print) or `QDRANT_API_KEY` | create a new key in Qdrant, `ostler config set` it, revoke the old one |
| Tenant tokens | held by clients | mint a new one with `ostler memory token` (expires in 30 days by default; optional `--read-only`); let the old expire or rotate the Qdrant admin key that signs them |
| A2A bearer token | `--a2a-token` / `OSTLER_A2A_TOKEN` | restart `ostler a2a` with a new token and update callers; there is no overlap window |

Never put keys in a shared shell history or an MCP client config that is committed to a repo.

## 7. Verifying a release candidate

```sh
cargo test --workspace                                 # Chrome + Qdrant suites (see ci.yml)
./target/release/ostler fixtures &                     # demo forms on http://127.0.0.1:8787
SOAK_MIN=20 bun scripts/soak/soak.js                   # security, stress and timed soak; kills Chrome once on purpose
scripts/bench-compare.sh                               # comparison against other browser MCPs
```

Read the soak report's failures together with its `SLOW/ERR` lines ([scripts/soak/README.md](../scripts/soak/README.md)). Targets are in [SLOS.md](SLOS.md); failure analysis is in [FAILURE-MODES.md](FAILURE-MODES.md).
