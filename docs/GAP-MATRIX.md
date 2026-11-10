# Gap matrix: E01-E36 against the code

Step 0.1 of `docs/PLAN-LEVEL-100.md`. Every row was checked with grep or a file read against this
branch (HEAD `4f8ef86` at the time of writing). Paths are relative to the repo root. Line numbers are
hints; locate by the symbol name.

Status key: **missing** = nothing in the code does this; **partial** = a building block exists but the
spec behaviour does not; **done** = the spec behaviour exists (no row is done yet).

## Corrections to the plan's status table

| Row | Plan said | Verified |
|---|---|---|
| E02 | `serve_stdio` lib.rs:152-163 | `serve_stdio` starts at `crates/ostler-mcp/src/lib.rs:116`; messages are awaited one at a time in the loop below it. Line numbers drifted. |
| E03 | `Mutex<Option<Browser>>` app.rs:307 | app.rs:306 (drift). |
| E05 | `App::browser` app.rs:653 | app.rs:646 (drift). |
| E06 | `BudgetTracker` built per call, research.rs ~625 | `BudgetTracker::new` is at `crates/ostler-mcp/src/app/research.rs:643` (drift). Status unchanged. |
| E12 | expect/receipt "not exposed through MCP" | Correct, and stronger than stated: `ostler-mcp` does not depend on `ostler-workflow` at all (`crates/ostler-mcp/Cargo.toml`). |
| E25 | `Policy` policy.rs:12 | Correct for the browser policy (`crates/ostler-browser/src/policy.rs:12`). A second, separate `Policy` exists in `crates/ostler-workflow/src/risk.rs:816`; the profiles work must reconcile the two. |
| E28-E36 | "mostly missing" | Row-by-row below. E28 is partial (a persistent profile directory exists), E32 partial, E34 partial, E35 partial, E36 partial, E30 partial. E29, E31, E33 are missing. |
| E27 | `confirm` flag only | Correct. `risk.rs` has no grant concept (grep for `grant` finds nothing). |
| E35 | not in the table | `scripts/eval/` exists (`run.py`, `tasks.py`, `scoring.py`, `fixture.py`), but there is no `scripts/eval/tasks/` JSON suite, no `bench-results/baseline-*/`, and `scoring.py` has no `useful_completion` / `false_completion`. |

## Matrix

| ID | Status | Pointer (file:function) | Notes |
|---|---|---|---|
| E01 | partial | `crates/ostler-mcp/src/app.rs`: `ToolError` (108), `ErrKind` (114) | Code and retry are text only. No `outputSchema` or `structuredContent` anywhere in `crates/`. |
| E02 | missing | `crates/ostler-mcp/src/lib.rs`: `serve_stdio` (116) | Serial: `handle(&app, &m).await` per message. |
| E03 | missing | `crates/ostler-mcp/src/app.rs`: field `browser` (306), `br!` (21), `App::browser` (646) | One mutex; `br!` holds it for the call. |
| E04 | partial | `crates/ostler-workflow/src/task.rs`: `Task` (356) | Library only; `ostler-mcp` does not use it. |
| E05 | partial | `crates/ostler-mcp/src/app.rs`: `App::browser` (646) | Relaunch on a dead Chrome exists. No state restore, no journal. |
| E06 | partial | `crates/ostler-research/src/budget.rs`: `BudgetTracker` (79); built in `crates/ostler-mcp/src/app/research.rs` (643) | Built per `open_many` call, so budgets do not span a mission. |
| E07 | partial | `crates/ostler-browser/src/page.rs`: `Page::click` (1310), JS fallback (1322-1330) | Covered or zero-size targets get `jsClick`. No actionability wait before the click. |
| E08 | missing | `crates/ostler-browser/src/runtime.js` ref assignment (138-141) | Refs are `'e' + (++counter)`; no document generation. |
| E09 | partial | `crates/ostler-browser/src/page.rs`: `ActionOutcome` (328) | Has `navigated`, `dialogs`, `changed`. No batch tool in `crates/ostler-mcp/src/tools.rs`. |
| E10 | partial | `crates/ostler-browser/src/page.rs`: `look` (1657); `runtime.js` `targets` (1344) | No `find`, `click_at` or `scroll_collect` tool in `tools.rs`. |
| E11 | partial | `crates/ostler-browser/src/fill.rs`: `fill_form` (436), `verify` (1427) | No whole-form re-verify pass after settle (grep-level check). |
| E12 | partial | `crates/ostler-workflow/src/expect.rs`: `parse_expect` (164); `receipt.rs`: `extract_receipt` (292) | Not exposed through MCP. |
| E13 | missing | `crates/ostler-browser/src/page.rs`: `non_html_note` (~381) | Only a note that Ostler cannot read the file. No download handling. |
| E14 | partial | `crates/ostler-research/src/search/` (`mod.rs`, `brave.rs`, `searxng.rs`, `http.rs`) | Providers exist; `search` tool is in `tools.rs`. |
| E15 | missing | `crates/ostler-browser/src/page.rs`: `non_html_note` (~381-387) | Says "PDF/non-HTML document ... Ostler cannot read it". No PDF parsing. |
| E16 | missing | `crates/ostler-mcp/src/app/research.rs`: `open_many_tool` (561) | One scratch tab, URLs fetched in a loop (tab close/switch at 703-704). Not concurrent. |
| E17 | partial | `crates/ostler-research/src/ledger.rs`: `Ledger` (160); `crates/ostler-mcp/src/evidence.rs`: `Evidence` (298) | Ledger persists outcomes. `Evidence` is in memory only. |
| E18 | partial | `crates/ostler-mcp/src/evidence.rs`: `Doc::passages` (154) | BM25 plus MMR per document. No cross-document index. |
| E19-E21 | missing | `crates/ostler-mcp/src/evidence.rs`: `Evidence::cite` (384) | Quote match only. No claims, assessment or independence model. |
| E22-E23 | missing | `crates/ostler-mcp/src/prompts.rs` (`list` 68, `get` 87) | Prompts only. No research controller or stop rules. |
| E24 | partial | `crates/ostler-mcp/src/app/research.rs`: `extract_tool` (782); `crates/ostler-extract/src/completeness.rs` | Tables and lists with completeness hints. No schema extraction. |
| E25 | partial | `crates/ostler-browser/src/policy.rs`: `Policy` (12), `check_resolved` (284); `crates/ostler-workflow/src/risk.rs`: `Policy` (816) | Two flat policies; no profiles. |
| E26 | missing | `crates/ostler-browser/src/page.rs` (683): `Page.addScriptToEvaluateOnNewDocument` | Runtime is injected into the main world. No isolated world. |
| E27 | partial | `crates/ostler-mcp/src/app.rs`: `require_confirm_submit` (38-39), gate (660); `tools.rs` `form_submit` `confirm` param | Confirm flag only. `risk.rs` is not bound to grants. |
| E28 | partial | `crates/ostler-cdp/src/launch.rs`: `LaunchOptions.user_data_dir` (14); `crates/ostler-cli/src/main.rs`: `profile_dir` (57, 167) | One persistent directory. No named profiles, no refusal of the default Chrome path, no namespacing of evidence. |
| E29 | missing | none; `crates/ostler-workflow/src/trace.rs` is a trace writer, not replay | No `replay.rs`. |
| E30 | partial | `crates/ostler-memory/src/lib.rs`: `forget` (722); tests at 1861 and 1912 | Forget cascades to playbook mappings and episodes. No `embedding_version`. `crates/ostler-memory/src/qdrant.rs` (47) reserves an embedding-model meta point, a partial precedent. No migration command. |
| E31 | missing | none; `crates/ostler-cli/src/main.rs` has no `watch` subcommand | No journal to read (2.1 is also missing). |
| E32 | partial | `crates/ostler-browser/src/page.rs`: `diagnostics` (842); `crates/ostler-browser/src/diagnostics.rs` | Not an MCP tool. No `bundle` command in `ostler-cli`. |
| E33 | missing | none; no `sdk/` directory | `ostler-cli` has no `--json` flag. |
| E34 | partial | `crates/ostler-research/src/budget.rs`: `BudgetTracker` (79) | Byte and time budgets exist. No dependency health checks. |
| E35 | partial | `scripts/eval/` (`run.py`, `tasks.py`, `scoring.py`, `fixture.py`) | No `scripts/eval/tasks/` suite, no `bench-results/baseline-*/`, no completion split in `scoring.py`. |
| E36 | partial | `crates/ostler-browser/src/lib.rs`: `Browser::rss_kb` (545); `crates/ostler-cdp/src/launch.rs`: `process_tree_rss_kb` (369) | RSS exists. No per-tool output byte counter (`ToolOutput` has no `bytes`). No migration fixture test. |
