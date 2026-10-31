# Changelog

## Unreleased

Fixes from the [design review](docs/REVIEW-2026-10.md) (status table there), plus three features.

- **Tool output is terse by default.** Verified fields are named once instead of one line each, form names no longer include the form's whole text, and fingerprints, timings and repeated page titles are gone. About 30% less text on a four-call form flow (1.5 KB to 1.0 KB). `--verbose` (or `verbose = true` in the config) restores the detail.
- **Tabs and shared context.** New `tabs` tool (list, new, switch, close). All tabs share one browser context, so cookies carry across; popups open as tabs under the same policy; new `context` tool is a notepad every tab and later call can read, kept across browser restarts.
- **A2A.** `ostler a2a` serves Agent2Agent v1.0 (JSON-RPC, SSE streaming): Agent Card, `SendMessage`, `SendStreamingMessage`, `GetTask`, `ListTasks`, `CancelTask`, `SubscribeToTask`. Tasks move to `input-required` for missing values, rejected input and submit confirmation. Per-conversation browsers; loopback by default; bearer token to listen elsewhere; `Origin`/`Host` checks. Verified with the official A2A Python SDK. See [docs/A2A.md](docs/A2A.md).
- **Wrong-fill prevention:** keys resolve against every visible field once, so an ambiguous key cannot win by elimination; forms re-mounted by a framework are followed; keys can no longer be silently dropped, and `FillReport::ok()` counts unmatched keys.
- **Memory:** learned values never overwrite facts you saved; playbooks persist only exact (playbook/autocomplete) matches; semantic matches need a margin over the runner-up and never apply to labels about other people; fixed the playbook key being cut at the first space.
- **Safety:** the allowlist is enforced at the network layer; `--require-confirm-submit` blocks all non-GET requests outside a confirmed submit; the untrusted-content wrapper cannot be closed by page text; secrets are masked in snapshots and notes and scrubbed from stored episodes.
- **Reliability:** a crashed Chrome is relaunched and the agent is told; closed connections fail immediately; larger WebSocket frames; `panic = "abort"` removed.
- **MSRV** is now Rust 1.82.

- **New:** `ostler setup <claude|codex|cursor|vscode|json> [--apply] [-- serve flags]` prints client config with the binary's absolute path; `--apply` runs `claude mcp add`. `doctor` now points to it.
- **New:** `ostler init` (interactive or flag-driven; saves Qdrant URL/key, tenant, defaults to `~/.config/ostler/config.toml` with `0600` permissions and tests the connection) and `ostler config show|path|set|unset`. Precedence: flag, environment variable, config file, default. Qdrant Cloud URLs get `:6333` added. Key is masked in `config show`.
- **Docs:** README rewritten (quickstart, real tool output, measured-vs-modelled labelling, known gaps), `docs/TROUBLESHOOTING.md`, and the [design review](docs/REVIEW-2026-10.md).

## 0.2.0

Found by writing harder fixtures first; each fix has a regression test.

- **Fixed:** custom ARIA radio groups (`role=radio` inside `role=radiogroup`) were reported with no options and, on re-read, as a multi-select. They are now a single `radio` field with options from the widgets' labels.
- **New:** calendar date pickers on read-only inputs. When a text field is read-only and the value is a date, the engine opens the popup, navigates months and clicks the day (`pickDate`). Fails with a reason, not silently, when no popup or navigation is found.
- **Tests:** 4 new e2e tests (custom ARIA widgets and contenteditable, conditional fields revealed mid-fill, hidden-branch validation message surfaced, calendar picker) and a hostile-input test that sends malformed arguments to every MCP tool. 73 tests total.
- **Efficiency:** release profile with thin LTO, one codegen unit, stripped symbols. `ostler` binary 9.3 MiB to 6.0 MiB (same machine, same commit apart from the profile). Task latency was not re-measured and is not claimed to change.
- Fixtures: `aria`, `conditional`, `datepicker` (14 total).

Known limits of the new pieces: the date picker is heuristic (month header text plus prev/next controls plus day cells) and has only been tried on one fixture; year-only or dropdown-based pickers are not handled.

## 0.1.0

First public MVP. See the README.

### Changed
- **Renamed Waymarker to Ostler** (crates `ostler-*`, binary `ostler`, env vars `OSTLER_*`, config `~/.config/ostler/config.toml`). Move an existing `~/.config/waymarker` directory to `~/.config/ostler` and rename `WAYMARKER_*` variables.
