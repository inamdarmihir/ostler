# Changelog

## Unreleased

- **New:** `waymarker setup <claude|codex|cursor|vscode|json> [--apply] [-- serve flags]` prints client config with the binary's absolute path; `--apply` runs `claude mcp add`. `doctor` now points to it.
- **Docs:** README rewritten (quickstart, real tool output, measured-vs-modelled labelling, known gaps), `docs/TROUBLESHOOTING.md`, and the [design review](docs/REVIEW-2026-10.md).

## 0.2.0

Found by writing harder fixtures first; each fix has a regression test.

- **Fixed:** custom ARIA radio groups (`role=radio` inside `role=radiogroup`) were reported with no options and, on re-read, as a multi-select. They are now a single `radio` field with options from the widgets' labels.
- **New:** calendar date pickers on read-only inputs. When a text field is read-only and the value is a date, the engine opens the popup, navigates months and clicks the day (`pickDate`). Fails with a reason, not silently, when no popup or navigation is found.
- **Tests:** 4 new e2e tests (custom ARIA widgets and contenteditable, conditional fields revealed mid-fill, hidden-branch validation message surfaced, calendar picker) and a hostile-input test that sends malformed arguments to every MCP tool. 73 tests total.
- **Efficiency:** release profile with thin LTO, one codegen unit, stripped symbols. `waymarker` binary 9.3 MiB to 6.0 MiB (same machine, same commit apart from the profile). Task latency was not re-measured and is not claimed to change.
- Fixtures: `aria`, `conditional`, `datepicker` (14 total).

Known limits of the new pieces: the date picker is heuristic (month header text plus prev/next controls plus day cells) and has only been tried on one fixture; year-only or dropdown-based pickers are not handled.

## 0.1.0

First public MVP. See the README.
