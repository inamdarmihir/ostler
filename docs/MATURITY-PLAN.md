# Maturity plan: E01–E36 mapped to code

This plan turns the engineering-gap spec (E01–E36) into changes at the function and line level. Line numbers
refer to commit `1aa62f3` and will drift once work starts, so each entry also names the function. Status
words follow the spec: **wire** (the code exists but is not exposed), **extend** (it is exposed but too weak),
**add** (it does not exist).

## What the code review found (read this first)

These facts drive the phase order. Each was checked against the source.

| # | Finding | Where |
|---|---|---|
| F1 | `form_submit` documents an `again` flag, but the handler calls `Page::submit`, which always passes `repeat = true`. The double-submit guard in `submit_inner` is therefore **never active over MCP**. | `ostler-mcp/src/app.rs:1733-1735`, `ostler-browser/src/fill.rs:1170-1172`, `fill.rs:1239-1252` |
| F2 | `ostler-workflow` (`expect`, `task`, `risk`, `trace`, `receipt`, `idempotency`) is not a dependency of any other crate. Postconditions, task contracts, risk classes, the redacted trace and receipts exist as tested pure logic but are unreachable. | `crates/ostler-mcp/Cargo.toml` (no `ostler-workflow`), `ostler-workflow/src/lib.rs` |
| F3 | `Page::expect_observation`, `Page::diagnostics` and `Page::diagnostics_head` are only called from `ostler-browser/tests/read_e2e.rs`. | `ostler-browser/src/read.rs:353`, `page.rs:842-853` |
| F4 | Source memory (`Memory::save_sources`, `recall_sources`, `forget_sources`, `prune_sources`) has no caller outside `ostler-memory`. | `ostler-memory/src/source.rs:228-464` |
| F5 | `Evidence::insert` replaces the earlier `Doc` for a URL and rewrites each citation's `hash`/`fetched_at` in place. It also evicts beyond `MAX_DOCS = 60`, so the text behind an old citation can disappear. | `ostler-mcp/src/evidence.rs:309-363`, `evidence.rs:23` |
| F6 | One `tokio::sync::Mutex<Option<Browser>>` serialises every browser tool. `open_many` holds it for up to `TOTAL_SECS = 90` while it reads pages one at a time in a single scratch tab. | `app.rs:307`, `app.rs:653-673` (`browser()`), `app.rs:20-27` (`br!`), `app/research.rs:641-707` |
| F7 | `serve_stdio` awaits `handle()` inline, so `ping` and every other request wait behind a running tool. `with_deadline` cancels by dropping the future; nothing reports which CDP calls had already gone out. | `ostler-mcp/src/lib.rs:118-172`, `app.rs:201-221` |
| F8 | Refs are `'e' + counter` per document (`runtime.js:139-150`). A new document starts again at `e1`, so a stale `e1` silently resolves on the next page. `DOC_ID` exists (`runtime.js:1483`) but only the submit path uses it (`fill.rs:1161`). | `runtime.js:134-150` |
| F9 | `Page::after` decides "did this action navigate?" by comparing shared counters (`started`, `committed`). A navigation that a previous action triggered late is credited to the current action. | `page.rs:1019-1054` |
| F10 | When a target is covered or has zero size, `click` falls back to `jsClick`, which records only a warning; the result does not mark the click as lower assurance. | `page.rs:1310-1346`, `runtime.js:1238-1256` |
| F11 | The runtime is injected into the page's **main world** (`addScriptToEvaluateOnNewDocument` without `worldName`, `page.rs:682-685`), as `window.__wm` (`runtime.js:2225`). Page scripts can replace prototypes that it relies on. | `page.rs:670-696` |
| F12 | Field drift between fill and submit is computed by `drifted_fields` and returned in `SubmitOutcome.drift`, but the submit click still happens. | `fill.rs:1071-1088`, `fill.rs:1253` |
| F13 | PDFs and other non-HTML documents are detected and then refused with a note. | `page.rs:371-387` (`is_unreadable_type`, `non_html_note`) |
| F14 | When Chrome dies, `App::browser()` relaunches it and sets a one-line notice. Tabs, pending writes and the last fill are lost without reconciliation. | `app.rs:653-673` |
| F15 | Results are text only: `ToolOutput::to_json` emits `{content, isError}`, and `ToolError::tagged` puts the code into prose (`"ERROR STALE_REF (retry: …)"`). | `app.rs:131-146`, `app.rs:254-267` |

Fix F1 straight away, before any phase work. It is a one-line change: `form_submit` reads
`a["again"]` and calls `submit_with(form, ctl, again)`. Add a regression test in
`ostler-browser/tests/forms_e2e.rs` that submits twice inside the 30-second window and expects
`NotActionable` the second time.

---

## Phase 1: Foundation (E01–E03, E07–E09, E25–E27)

### E01: typed result contract (extend)

**New module `ostler-mcp/src/outcome.rs`.**
- `pub struct Outcome { status: Status, code: Option<Code>, effect: Option<Effect>, checks: Vec<Check>, evidence_ids: Vec<String>, retry: RetryPolicy, next: Option<NextAction>, mission_id: Option<String>, action_id: Option<String> }`, with serde derive and `#[serde(rename_all = "snake_case")]`.
- `enum Status { Ok, Failed, Rejected, Uncertain, Blocked, NeedsUser }`. Protocol success stays in JSON-RPC; `Status` is about the task.
- `enum Code`: absorbs `ErrKind` (`app.rs:114-120`) and adds `ValidationRejected`, `WriteOutcomeUnknown`, `NoEffect`, `Cancelled`, `BudgetExceeded`, `DependencyDown`, `ApprovalRequired`.
- `enum RetryPolicy { Safe, AfterSnapshot, ReconcileFirst, Never, AfterStateChange }`, moved from the strings in `ToolError::tagged`.
- `enum Effect { Navigated{url}, ChangedInPlace, NewTab{id}, Dialog{text}, Download{id}, RequestSent{persisted: Tri}, None }`, where `Tri` is `yes`, `no` or `unknown`.

**`ostler-mcp/src/app.rs`**
- `ToolOutput` (`223-279`): add `pub outcome: Outcome`. `text()` and `err()` build a default outcome. Add `ToolOutput::with(outcome)`.
- `ToolOutput::to_json` (`254-267`): when the negotiated protocol is `2025-06-18`, emit `structuredContent: outcome` next to `content`. Keep `isError` for clients that only read text.
- `ToolError` (`108-147`): replace `kind: Option<ErrKind>` with `code: Option<Code>` plus `retry`. `tagged()` still renders the same prose, so text-only clients see no change. Add `fn into_output(self) -> ToolOutput`.
- `From<BrowserError>` (`173-190`): map to `Code`. `BrowserError::Uncertain` maps to `WriteOutcomeUnknown` when the tool is a write (see the `is_write(name)` helper below).
- `App::call` (`675-793`): set `outcome.action_id`, and copy the mission id once E04 lands. The prefixes added by `notice`, the challenge check and the memory-degraded line also go into `outcome.checks` as `{kind, result}`, so callers need not parse them.
- `dispatch` arms such as `click` (`919-950`): build `Effect` from `ActionOutcome` instead of the sentences `"Result: page updated in place"` / `"no visible change"`. Do the same for `type`, `press`, `hover`, `scroll`, `back` and `navigate`.
- `form_submit` (`1719-1740`) and `run_form_task_inner` (`1690-1716`): map `SubmitStatus` to `Status`. `ServerError` becomes `Uncertain` with `WriteOutcomeUnknown` and `ReconcileFirst`. `Unchanged` becomes `Failed` with `NoEffect`.

**`ostler-mcp/src/tools.rs`**
- `struct Def` (`48-54`): add `output: fn() -> Value` (an `outputSchema`). All tools share one envelope schema, plus a tool-specific `data` field.
- `list_revealed` (`142-154`): include `outputSchema` only when the client negotiated `2025-06-18`. Thread the version through `App::listed_tools` (`543`) and store it in `App` at `initialize` (`lib.rs:44-62`).
- The test `schemas_are_small` (`246-253`) caps the size at 11 000 / 5 400 bytes. Keep the caps for text-only clients and add a separate cap for the structured listing. Make the shared envelope a `$ref`, so its size is paid once.

**`ostler-mcp/src/lib.rs`**: `handle` (`36-92`). `tools/call` validation failures stay JSON-RPC errors (`-32602`). Argument errors from `need_str` become `Status::Rejected` / `ValidationRejected`, not `isError` text.

**Tests.** Add `outcome_tests` in `app.rs` with three cases: a missing argument gives `Rejected`, `StaleRef` gives `Failed` + `AfterSnapshot`, and a deadline gives `Uncertain` + `ReconcileFirst`. Extend `ostler-cli/tests/mcp_e2e.rs` to assert on `structuredContent` with no string matching.

### E02: request scheduling and cancellation (extend)

**`ostler-mcp/src/lib.rs`**
- `serve_stdio` (`118-172`): split it into three parts.
  1. A **reader task** that parses lines and sends `(id, msg)` into a bounded `mpsc` channel (capacity about 32). When the channel is full, it answers `-32000 server busy` instead of blocking stdin.
  2. A **writer task** that owns `stdout` and receives responses and notifications from an `mpsc::UnboundedSender<Value>`, so writes never interleave mid-line.
  3. A **dispatcher** that answers `ping`, `initialize`, `tools/list` and `notifications/cancelled` inline. It `tokio::spawn`s each `tools/call` with a `CancellationToken` (`tokio-util`), stored in `HashMap<RequestId, Inflight>`.
- Handle `notifications/cancelled {requestId}`: look up the token and cancel it. Today `handle` drops every notification at `39-41`.
- Progress: when `params._meta.progressToken` is set, pass a `Progress` sender into `App::call`. `open_many`, `run`, `wait_for`, `wait_for_human` and navigation emit `notifications/progress`.
- Graceful shutdown (`165-171`): on stdin EOF or a signal, cancel every in-flight request, wait up to 5 s, and send each one a `Cancelled` outcome before `app.shutdown()`.

**`ostler-mcp/src/app.rs`**
- `App::call` (`675`) gains `ctx: CallCtx { id, cancel: CancellationToken, progress: Option<Progress> }`. Keep the old signature as a wrapper for A2A, the bench crate and `run`.
- `with_deadline` (`201-221`) becomes `with_deadline_and_cancel`, a `select!` over the timeout, the cancel token and the future. For cancel or timeout on a tool where `is_write(name)` is true (`click`, `type`, `press`, `form_submit`, `fill_field`, `form_fill`, `run`), return `Uncertain` / `WriteOutcomeUnknown`. Read tools return `Cancelled` / `Safe`.
- Add `fn is_write(name) -> bool` next to `tools::allowed`. Better still, add a `write: bool` flag to `Def` in `tools.rs:48`.
- `run_tool` (`1306-1371`): give the script worker the cancel token. Add `Limits::cancel: Arc<AtomicBool>` in `script.rs:26`, and check it from Rhai's `on_progress` callback. Replace `unbounded_channel` (`1313`) with a bounded channel.

**`ostler-a2a/src/server.rs`**: `cancel` (`758`) and `execute` (`951`). Pass a token into `run_form_task` instead of relying on `tokio::time::timeout` alone.

**Tests.** In `lib.rs` tests, start a `wait_for` with a 10 s timeout, then send `ping` and expect the reply in under 100 ms. Then cancel and expect `Cancelled`. In `mcp_e2e.rs`, cancel a `click` that submits a slow fixture form and expect `WriteOutcomeUnknown`.

### E03: browser ownership (extend)

**`ostler-browser/src/lib.rs`**
- `Browser` (`170-176`): already `Send + Sync` through `Arc<Mutex<Tabs>>`. Add `pub fn page_by_id(&self, tab: &str) -> Option<Page>` and `pub async fn scratch_page(&self) -> Result<ScratchPage>`. `ScratchPage` closes its target on `Drop` through a spawned task, and is never made active.
- `Tab` (`32-37`): add `lock: Arc<tokio::sync::Mutex<()>>`, the per-page mutation lock.
- `new_tab` (`440-458`): add `new_tab_inactive` so research does not switch the agent's active tab (`open_many` does this today at `research.rs:643` and `705-706`).

**`ostler-browser/src/page.rs`**
- `Page` (`286-291`): add `mutate: Arc<tokio::sync::Mutex<()>>`. Wrap `click`, `type_text`, `press`, `hover_action`, `scroll`, `fill_form`, `submit_with` and `upload` in `let _g = self.mutate.lock().await;`. Reads (`snapshot`, `read`, `forms`, `extract_*`, `links`, `challenge`) do not take it.

**`ostler-mcp/src/app.rs`**
- `browser: Mutex<Option<Browser>>` (`307`) becomes `browser: tokio::sync::RwLock<Option<Arc<Browser>>>`. The write lock is only taken to launch or relaunch (`browser()` at `653-673`). Calls clone the `Arc` and drop the guard.
- `br!` (`20-27`): bind `$b: Arc<Browser>` and stop holding a guard across the body.
- Optional `tab` argument on every page tool: resolve with `page_by_id`, defaulting to `b.page()`. Refs are then scoped per tab (with E08).
- `App::call` (`703-743`, `745-762`): `self.browser.try_lock()` after every call becomes a read of the `Arc`. The challenge probe runs on the page the call used, not on whichever page is active now.
- `shutdown` (`636-641`): take the write lock and close.

**Tests.** Add `ostler-browser/tests/concurrency_e2e.rs` with three tests. Two `read`s on different tabs overlap, checked by timing against a slow fixture. Two `click`s on one tab serialise. A ref from tab t1 used with `tab: t2` returns `StaleRef`.

### E07: actionability (extend)

**`runtime.js` `clickInfo` (`1238-1255`)**: return `{visible, enabled, stable, hit, covered_by, inert, pointerEvents}`.
- `stable`: sample `getBoundingClientRect` twice, one `requestAnimationFrame` apart, and require movement under 1 px. Make `clickInfo` async (it returns a Promise; `Runtime.evaluate` already uses `awaitPromise: true`, `page.rs:727`).
- `hit`: `deepElementFromPoint` (`1229`) is inside `el` or its label. Today this is only checked when `el.ownerDocument === document` (`1244`); for frames, translate through `absoluteCenter`.
- `inert`, `aria-disabled` and `pointer-events: none` count as not enabled.

**`page.rs` `click` (`1310-1346`)**
- Add a pre-action wait: poll `clickInfo` until it is actionable or `ACTION_WAIT_MS = 3000`.
- Remove the silent `jsClick` fallback (`1318-1330`). If the target is still covered or zero-size after the wait, return `BrowserError::NotActionable("{r} is covered by {covered_by}")`.
- Add `pub async fn click_with(&self, r, ClickOpts { force: bool })`. `force` uses `jsClick` and sets `ActionOutcome.assurance = Assurance::Script`.
- `ActionOutcome` (`328-341`): add `assurance: Assurance { Pointer, Script, Keyboard }` and `blocker: Option<String>`.
- `hover_action` (`1349-1373`) and `type_into` (`1437`): run the same wait.

**`tools.rs:80`**: `click` gains `"force": {"type":"boolean","description":"script click; lower assurance"}`.

**Tests.** Two new fixtures in `ostler-fixtures/src/lib.rs`: a cookie banner over the button, and a button inside a CSS transition. Assert `NOT_ACTIONABLE` with `covered_by`, and assert that `force:true` reports `assurance: script`.

### E08: ref identity and navigation correlation (extend)

**`runtime.js` refs (`134-150`)**
- Keep the visible refs short. Track a generation: `const GEN = DOC_ID.slice(0,4)`. Snapshot output still prints `e12`, but the Rust side records which generation was current.
- `elOf(ref)`: accept `e12` or `e12~gen`. If a generation is given and differs from `GEN`, throw `stale ref … (document changed)`. This maps to `StaleRef` through `eval_attempts` (`page.rs:756-765`).

**`page.rs`**
- `PageState` (`270-283`): add `ref_gen: String` and `frame_id`, set in `snapshot` (`1210-1298`) from the runtime's `docId`.
- `rt_expr` (`779-788`): for every function that takes a ref (`clickInfo`, `jsClick`, `prepare`, `rect`, `field`, `apply`), append `~{ref_gen}` to a bare `eN` before evaluating. A ref from an earlier document then fails instead of hitting the new `e1`.
- `after` (`1019-1054`): correlate the navigation with this action. Read `Page.frameStartedLoading` / `Page.frameRequestedNavigation` from events (`sync_events`, `855`), and keep a navigation only if it started after `action()` returned its first CDP reply, on `main_frame`. Replace the bare counter comparison at `1033-1043` with a `NavEpoch` taken just before `action()`. Events from an earlier epoch are logged in `diag` and not credited.
- `navigate` (`882-930`): use the `loaderId` returned by `Page.navigate` and wait for the `Page.lifecycleEvent` with that `loaderId` in `wait_page_ready` (`936`), not for whichever load arrives next.

**`fill.rs`**: the semantic re-resolve in `read_back` (`1093-1110`) already requires a unique match. Apply the same rule as a shared helper `Page::reresolve_unique(planned)` and use it from `click` when a ref is stale and the agent passed `reresolve: true`.

**Tests.** In `read_e2e.rs`, snapshot page A (refs e1–e5), navigate to B, and click `e1`. Expect `STALE_REF`, not a click on B's first element. Also add a link whose navigation starts after 500 ms, followed at once by a `type`: the `type` result must not report `navigated`.

### E09: effect-aware actions and batches (extend)

**`page.rs`**
- `ActionOutcome` (`328-341`): add `effects: Vec<Effect>`, filled in `finish_outcome` (`1056-1105`) from `navigated`, `changed`, `dialogs`, the new-tab notes (`Browser::wait_notes`, `lib.rs:406`) and download events (E13). `changed == false && !navigated && no popup` gives `Effect::None`.
- Add `pub async fn changed_regions(&self) -> Vec<String>`. It is a cheap variant of `diff_lines` (`snapshot`, `1257`) that returns the nearest heading or landmark for each added block.

**`ostler-mcp/src/app.rs`**
- New tool `batch` (Core group). `{actions: [{tool, args}], stop_on: "failure"|"no_effect"}`. Run the actions in order through `self.call`, and stop at the first failure. The result lists every action with `executed | failed | not_executed`.
- Unlike `run`, `batch` holds the target page's `mutate` lock across all of its steps, so nothing interleaves.
- `click` (`919-950`): when `Effect::None`, set `Status::Failed` / `NoEffect` instead of `isError: false` with the text "no visible change". Keep the text.

**`tools.rs`**: add the `batch` `Def`. Re-check the `schemas_are_small` caps.

**Tests.** In `mcp_e2e.rs`: a batch whose second click targets a disabled button reports step 3 as `not_executed`, and a no-op click is never `Status::Ok`.

### E25: policy profiles (extend)

**`ostler-browser/src/policy.rs`**
- `Policy` (`12-26`): add `profile: PolicyProfile { PublicResearch, LocalDev { hosts: Vec<Entry> }, Authenticated { origins: Vec<Entry> } }`. `PublicResearch` forces `block_private_networks = true` and refuses loopback.
- `check` (`219-279`): enforce the profile. In `LocalDev`, only the listed loopback or LAN host:port pairs pass, and every other private address is refused.
- `check_resolved` (`284-302`): close the documented DNS-rebinding gap (comment at `281-283`). Resolve the name, then pin it: add `Policy::pin(host, addrs)` and pass `--host-resolver-rules=MAP host addr` for that host. Alternatively, re-check `Network.responseReceived.remoteIPAddress` in `spawn_verdict` (`page.rs:107-135`) and abort the frame on a private IP.
- Redirect binding: in `spawn_verdict`, check every `Fetch.requestPaused` with `redirectedRequestId` / `responseStatusCode` 3xx against the policy and the **originating approval scope** (E27), not only the host allowlist.

**`ostler-mcp/src/app/research.rs`**: the robots fetch (`550-559`) and the `ostler-research/src/search/http.rs` clients must call `Policy::check_resolved` before connecting. Today only `open_many` checks first (`588-601`).

**`ostler-cli/src/main.rs`**: add `serve --policy public|local-dev|authenticated` and `--allow-local host:port`.

**Tests.** In `ostler-browser/tests/security_e2e.rs`: public research cannot reach `127.0.0.1:8787`; `--allow-local 127.0.0.1:8787` can, and cannot reach `:8788`; an allowed host that 302s to `169.254.169.254` is blocked; a rebinding resolver stub that flips to `127.0.0.1` is blocked.

### E26: runtime and data isolation (extend)

**`page.rs` `attach_inner` (`431-700`)**
- `Page.addScriptToEvaluateOnNewDocument` (`682-685`): add `"worldName": "ostler"`. Keep a small main-world shim only for what must observe page activity: the fetch/XHR/timer hooks in `runtime.js:85-130`. That shim exposes no functions and posts counts by `CustomEvent`.
- `eval_attempts` (`714-776`): evaluate in the isolated context. Look up `executionContextId` from `Runtime.executionContextCreated` where `auxData.isDefault == false && name == "ostler"`, cached per frame in `Shared`, and pass `contextId`.
- `rt_expr` (`779-788`): the `typeof __wm` check stays, now in the isolated world.

**`runtime.js`**: split the file into `runtime_isolated.js` (refs, snapshot, forms, read; DOM access is shared between worlds) and `runtime_probe.js` (activity counters only).

**Secrets.** `ostler-workflow::trace::Redactor` (`trace.rs:66-220`) becomes the single redaction pass for E17's archive, E32's support bundles and `ToolOutput` when `looks_secret` matched (`runtime.js:311`).

**Disclosure.** When the isolated world cannot be created (some extension pages, `about:`), `ActionOutcome.warnings` gets "helper ran in page context". The `doctor` command reports it.

**Tests.** A hostile fixture that overrides `Element.prototype.getBoundingClientRect`, `Array.prototype.map` and `window.__wm` must not change snapshot refs or click targets. Plant a secret in `localStorage` and assert it never appears in any tool output.

### E27: scoped approvals and handoff (wire and extend)

**Wire `ostler-workflow::risk`.**
- Add `ostler-workflow` to `ostler-mcp/Cargo.toml`.
- `App` (`305-321`): add `approvals: StdMutex<Approvals>` and `risk_policy: ostler_workflow::risk::Policy`, loaded from config.
- Before the `click`, `type(submit)`, `press(Enter)` and `form_submit` arms in `dispatch` (`869-1292`): build `risk::ActionDesc` (`risk.rs:73`) from the target's role/name (`clickInfo`) and the fields being sent (`PageState.filled`), call `risk::classify` (`606`) and `risk::decide` (`878`). `Decision::Confirm` returns `Status::NeedsUser` with `next_action: {kind: "approve", grant_request}`.

**New `ostler-workflow/src/approval.rs`.**
- `Grant { action: RiskClass, origin, params_digest, expires_at, mission_id }`. `params_digest` reuses `idempotency::Fingerprint` (`idempotency.rs:65-100`) over the destination, amounts and recipients.
- `Approvals::check(&ActionDesc) -> Option<&Grant>` requires an exact digest match, so a changed amount or recipient finds no grant.
- `approve` tool (Forms group): `{grant_request_id, confirm: true}`. It replaces the global `--require-confirm-submit` boolean (`app.rs:1720-1728`) with per-action grants, and keeps the flag as "every submit needs a grant".

**Handoff states.** Add `Status::NeedsUser { reason: Login | Mfa | Captcha | Ambiguous | Consequential }`.
- Login: `looks_like_login` (`page.rs:389`).
- CAPTCHA: `challenge` (`page.rs:797`, used at `app.rs:715-741`).
- Ambiguous: `rival_forms` (`fill.rs:241`).
- Consequential: `RiskClass` ≥ Payment.

**Tests.** Approve a $10 transfer; changing the amount to $11 needs a new grant. A grant from mission A is rejected in mission B. A grant expires after its TTL.

---

## Phase 2: Reliable work (E04–E06, E11–E13, E28, E34)

### E04: mission state machine (wire and extend)

**Wire `ostler-workflow::task`.** `Task::begin` (`task.rs:378`), `TaskSpec::from_json` (`95`), `Verdict` (`190`) and `Budget` (`71`) already implement begin → record_action → end.
- New `ostler-workflow/src/mission.rs`: `Mission { id, goal, criteria: Vec<Expect>, budget, state: Lifecycle, checkpoints, pending: Vec<PendingAction>, evidence_ids }` and `enum Lifecycle { Active, Paused, Finished(Verdict), Cancelled, Uncertain }`.
- New `ostler-workflow/src/journal.rs`: append-only JSONL. Reuse the `ostler-research::ledger` persistence pattern (`ledger.rs:210-310`: 0600, one write per line, atomic compaction). Events: `Begin`, `ActionStarted{action_id, tool, args_digest}`, `ActionFinished{action_id, outcome}`, `Checkpoint{summary}`, `Finish`, `Cancel`.
- `ostler-mcp`: new tool `mission` with actions `begin | status | checkpoint | resume | finish | cancel`. `App` holds `mission: StdMutex<Option<Mission>>`. `App::call` (`675`) writes `ActionStarted` **before** dispatching a write tool and `ActionFinished` after it.
- `status` / `resume` return goal, criteria, the last checkpoint, pending (unfinished) actions, budget used and evidence ids, without the transcript.
- Journal location: `~/.ostler/missions/<id>.jsonl`, overridable with `--state-dir`. Plumb it through `AppConfig` (`app.rs:31-43`).

**Tests.** Begin a mission, run three actions, drop the `App`, build a new `App` and `resume`: goal and progress come back. A pending write without `ActionFinished` is listed as `unknown`.

### E05: stateful crash recovery (add)

**`ostler-mcp/src/app.rs` `browser()` (`653-673`).** On a dead browser:
1. Snapshot what can be restored from `Browser` before reaping it: tab URLs (`Browser::tabs`, `lib.rs:417`) and the active id. Cache them on every `tabs`/`navigate` call in `App.tab_state`, because a dead browser cannot be queried.
2. Relaunch as today. Reopen the tabs (GET navigations only) through `new_tab_inactive`, then switch to the old active tab.
3. Bump a `browser_epoch: AtomicU64`. Each `PageState` carries the epoch, and refs from an older epoch fail with `StaleRef` (this extends E08).
4. Reconcile: every journal entry (E04) with `ActionStarted` but no `ActionFinished` is marked `Uncertain` and returned in the notice as `next_action: {kind: "reconcile", action_id}`.
5. Block repeating a pending submit: a `form_submit` whose `Fingerprint` matches a pending one returns `ReconcileFirst` until the agent calls `mission reconcile {action_id, observed: persisted|not_persisted}`, or an `expect` check (E12) settles it.

The idempotency guard currently lives in `PageState.last_submit` (`fill.rs:1239-1261`), which does not survive a crash. Move it to `App` and use `ostler_workflow::idempotency::Guard` (`idempotency.rs:116-195`), saved in the journal.

**Fault-injection test** (new `ostler-cli/tests/crash_e2e.rs`): use `Browser::pid` (`lib.rs:540`) and send SIGKILL (a) before the submit, (b) during it (from a fixture that delays the POST response), and (c) after it. (a) resubmits normally. (b) gives `Uncertain` and refuses a blind repeat. (c) is settled by `expect` reopening the record.

### E06: resource governance (extend)

- `ostler-research::budget::Budget` (`budget.rs:18`) already covers pages, seconds and bytes. Add `tabs`, `output_tokens`, `disk_bytes` and optional `model_cost` (only when the client reports it in `_meta`).
- Mission budget (E04): `Mission.budget` is the single tracker. `App::call` (`783-792`, the metrics block) debits `out.bytes() / 4` tokens and wall time. When the budget runs out, the call returns `BudgetExceeded`, and the journal and evidence are kept.
- Tabs: `Browser::new_tab` (`lib.rs:440`) refuses beyond `max_tabs`. `MAX_POPUPS` (`lib.rs:41`) stays.
- Disk: the journal, archive (E17) and downloads (E13) report sizes to a `DiskBudget`, enforced before each write.
- Backpressure: the bounded queue from E02, and in `open_many` a pool semaphore (E16).
- Cleanup: on `mission finish|cancel`, close scratch pages and delete partial downloads. Completed downloads and archive entries are kept.

### E11: form consistency (extend)

**`fill.rs`**
- `fill_form` (`436-~1040`): after all fields are applied, run `settle_scoped` and then a **full re-verification** of every requested field (`verify`, `1427`) against a fresh `forms()` read, not only `read_back` per field. A field that changed after a later field was set is reported as `FillStatus::Reset` (new variant of the enum at `53`).
- Options: when re-verifying selects and radios, compare with the **current** `options` (`with_options`, `272`). If the option has gone, use `Mismatch` with the message "option no longer offered".
- `submit_inner` (`1186-1392`): move `drifted_fields()` (`1253`) **before** arming. If drift is non-empty, return `BrowserError::NotActionable("fields changed since fill: …")` unless `allow_drift: true`. Today drift is only reported after the click.
- Submit control: `f.submit.find(role == "submit").or_else(first)` (`1222-1227`) can pick a header search box. Require the control to be inside the form element, or associated with it through the `form=` attribute (in `runtime.js` `submitControls`, `911-929`, which already excludes `CHROME_LANDMARK` at `907`). Never fall back to `first()` when that control's role is `search`.
- Field-scoped errors: keep `messageBelongsTo` (`runtime.js:762`). Add a test showing that a page-level "Invalid email" banner is not attached to an unrelated phone field.

**Tests.** A fixture where choosing a country resets the state field 300 ms later: fill → `Reset` → no submit. A fixture with a header search and an order form gives `form_submit` without `form` → the order form.

### E12: postconditions (wire and strengthen)

**Wire `ostler-workflow::expect`.**
- Every write tool in `tools.rs` (`click`, `type`, `press`, `form_submit`, `fill_field`, `batch`) gains `"expect": {…}`, the terse object form documented at `expect.rs:1-40`.
- `App::call`: after dispatch, `parse_expect` (`expect.rs:164`), then `Page::expect_observation` (`read.rs:353`), then `evaluate` (`expect.rs:472`). The results go to `outcome.checks`.

**Extend `expect.rs`** (`enum Expect`, `40`) with:
- `FieldEquals{label, value}`
- `Count{selector|text, op, n}`
- `RecordReopened{url, contains}`: navigates in a scratch page (E03) and checks there.
- `DownloadComplete{id, sha256?}` (E13)
- `Receipt` (wires `receipt::extract_receipt`, `receipt.rs:292`)

**Evidence strength.** Each check carries `strength: UiOnly | Reopened | ExternalApi`. `RecordReopened` is the strongest that Ostler can produce itself. Render `UiOnly` as "the page says", never "confirmed".

**`run_form_task_inner` (`app.rs:1690-1716`)**: `Submitted` becomes `TaskVerdict::Completed` only if the task's `expect` passes. Otherwise the verdict is `Uncertain`.

**Tests.** A fixture that shows "Saved" without saving: `RecordReopened` fails, and the result is `Failed`, not `Ok`.

### E13: file lifecycle (add and extend)

**`ostler-browser`**
- New `download.rs`. Set `Browser.setDownloadBehavior {behavior: "allowAndName", downloadPath: <mission dir>/downloads, eventsEnabled: true}` once per browser in `Browser::launch` (`lib.rs:179`). Subscribe to `Browser.downloadWillBegin` / `downloadProgress`.
- `DownloadHandle { id: guid, url, suggested_name, state: InProgress{received,total} | Completed{path, sha256, size, mime} | Cancelled | Failed }`.
- Checks: `size ≤ max_download_bytes`, MIME sniffed from magic bytes rather than the header, and `suggested_name` sanitised. Reject `..`, absolute paths and NUL using the logic `Policy::check_upload` already has (`policy.rs:307-343`) in reverse.
- `Effect::Download{id}` (E09) is added by `finish_outcome` when `downloadWillBegin` arrived during the action.

**`ostler-mcp`**: new tool `downloads` with `list | wait {id, timeout} | cancel {id} | read {id}`. `read` feeds a completed PDF or HTML file into E15 ingestion with `Doc.requested = original URL` and `Doc.via = download:<id>`, so provenance stays the source URL.

**Tests.** A fixture that serves a 5 MB file slowly: the state goes from `InProgress` to `Completed` with the right sha256. Interrupting it leaves `Failed` and the partial file is deleted. `Content-Disposition: ../../x` is saved as `x`.

### E28: session management (extend)

- `ostler-cli/src/main.rs`, `Login` (`52-60`) and `--profile-dir` (`167-185`): add `--profile <name>`, which maps to `~/.ostler/profiles/<name>`.
- Profile lock: Chrome's `SingletonLock` gives an opaque failure. Before `Chrome::launch` (`ostler-cdp/src/launch.rs`), take an `fs2`-style exclusive lock on `<profile>/.ostler.lock` and fail with "profile 'work' is in use by pid N" when it is held.
- Isolation: the evidence archive (E17), journal (E04) and downloads (E13) live under `<state-dir>/<profile>/`. Memory uses `tenant = profile` by default (`Memory::new(store, tenant)`, `ostler-memory/src/lib.rs:593`).
- Export/import: `ostler profile export --encrypt` uses `Storage.getCookies` plus localStorage per origin, sealed with the existing `ostler-memory::sealed` key machinery (`sealed.rs`). `import` refuses a profile exported for a different tenant.
- Restored sessions start a new `browser_epoch` (E05), so no ref survives.

**Tests.** Two profiles in sequence: cookies set in A are absent in B. Two concurrent `serve --profile A` gives an actionable error.

### E34: dependency resilience (extend)

- New `ostler-mcp/src/health.rs`: `Health { browser, search, memory, ocr }`, each `Up | Degraded(reason) | Down(reason) | NotConfigured`, with a small circuit breaker (`failures ≥ 3` within 60 s opens it for 30 s).
- Memory: `Memory::degraded` (`ostler-memory/src/lib.rs:607`) already exists. Wrap every memory call in `App` (`app.rs:1121-1186` form_inspect notes, `resolve_fact_refs` `1374`, `learn_from_submit` `1742`) in `tokio::time::timeout(800 ms)`, so a hung Qdrant cannot block navigation.
- Search: `App::provider` (`research.rs:486-499`) caches a *configuration* error forever in a `OnceCell`. Split `NotConfigured` (a stable error, never "no results") from transient `Down`. `search_tool` (`501-548`) maps provider errors to `Code::DependencyDown`. An empty result list stays `Status::Ok` with `data.results: []`.
- `stats` (`app.rs:1861`) and `doctor` print `Health`.

---

## Phase 3: Real research (E15–E18 first, then E14 and E19–E24)

### E15: document ingestion (add)

- New `ostler-extract/src/pdf.rs`, built on a pure-Rust PDF text library (lopdf-based, no system dependency). The output is `PdfDoc { pages: Vec<PdfPage { n, text, tables }>, meta, scanned: bool, failures: Vec<(page, reason)> }`.
- Repeated headers and footers: drop lines that recur on more than 60% of pages at the same position.
- Tables: use positional column clustering. When a table is unsure, mark it `confidence: low` instead of guessing.
- Scanned pages: `scanned = text_chars/page < 20 && has_images`. OCR is an optional feature (`--features ocr`, Tesseract via its CLI) and labels its text `ocr: true, confidence`.
- `page.rs`, `finish_outcome` (`1092-1094`): stop rejecting PDFs with `non_html_note`. Instead, fetch the bytes through `Network.getResponseBody` or the E13 downloader, and route them to `pdf.rs`.
- `research.rs` `ingest` (`210-319`): build the `Doc` from `PdfDoc`. `Doc` (`evidence.rs:90-111`) gains `pages: Vec<(start_offset, page_no)>`, so `Citation` (`252-270`) can carry `page: Option<u32>`. `render_read` (`research.rs:104`) shows "pages 1–12 of 40 read; 13–40 unread" when truncated.
- The ledger `Outcome` (`ledger.rs:26`) gains `PartiallyRead{pages_read, pages_total}`.

**Tests.** Add fixture PDFs to `ostler-fixtures` (born-digital, 2-column, scanned, encrypted, truncated). A quote on page 7 cites `page: 7`. The scanned PDF without OCR gives a `Gap` with reason `scanned`.

### E16: concurrent `open_many` (extend; needs E02 and E03)

**`research.rs` `open_many_tool` (`561-731`)**
- Replace the single-tab loop (`641-708`) with a pool: `POOL = cfg.open_pool.unwrap_or(4)` scratch pages from `Browser::scratch_page` (E03), driven by `futures::stream::iter(planned.to_fetch).map(fetch_one).buffer_unordered(POOL)`.
- `fetch_one` keeps today's per-page logic (`650-702`): `HostLimiter::acquire` (`budget.rs:222`) already bounds concurrency per host. It returns `(input_index, Result<Ingest>)`.
- Isolation: each scratch page has its own `PageState`, so snapshots and refs cannot cross. `ingest` (`210`) takes `&Page` instead of `&Browser` (today it calls `b.page()` at `216`, the *active* tab, which would race once pages run in parallel).
- Partial results: with E02 progress, emit `notifications/progress` per finished URL with its summary line. Cancellation stops pending fetches and returns what has finished.
- Dedup is already done by `plan` (`fetch_plan.rs:157`).

**Tests.** Ten URLs against a fixture with a 1 s delay finish in under 4 s (overlap is proven). One URL blocked by policy plus one that hangs: the other eight come back and no content is mixed up (check each `doc.hash` against its fixture).

### E17: durable source archive (add)

- New `ostler-research/src/archive.rs`: content-addressed storage under `<state-dir>/<profile>/archive/`. `blobs/<sha256>.md.zst` plus `index.jsonl` rows `SourceVersion { version_id, url, requested, redirect_chain, retrieved_at, hash, title, kind, pages, bytes }`. The blobs are immutable.
- `Evidence` (`evidence.rs:298-443`)
  - `insert` (`309-363`): **stop rewriting citations**. A changed page gets a new `version_id`. Old citations keep `version_id` and `hash`, and are marked `superseded_by: Some(new_version)`. Re-checking against the new text becomes a separate fact: `Citation.current: Holds | NoLongerHolds | Unchecked`. That separates historical evidence from claims about the current state.
  - Eviction (`345-354`): evict only the in-memory `Doc` and its BM25 index. `get` falls back to `archive.load(version_id)`, and the citation stays verifiable.
  - `MAX_CITATIONS` eviction (`418-420`): move evicted citations to the archive instead of `remove(0)`.
- Redirect chain: record it in `page.rs` from `Network.requestWillBeSent.redirectResponse` into `Shared` (next to `doc`, `page.rs:73`), and expose it on `ActionOutcome`.
- Secrets: run the archive through `Redactor` (E26) before writing.
- Wire `ostler-memory::source` (F4) as an optional *index* over the archive for cross-session recall (E18). It is not the system of record.

**Tests.** Read, cite, restart `App` with the same state dir: `cite` on the same URL and quote still verifies against the stored version. Change the fixture page and read again: the old citation still renders the old text with `superseded_by`.

### E18: cross-source retrieval (extend)

- `Doc::passages` (`evidence.rs:154`) uses one BM25 index per document (`ostler-extract/src/bm25.rs`). Add `ostler-extract::bm25::Corpus`, a mission-wide index over chunks from every archived version.
- New tool `retrieve {query, k, per_source_max: 2, since?, exact: [terms]}`. It scores with BM25, applies MMR-style source diversity (at most `per_source_max` per evidence family, see E21), requires `exact` terms when given, and filters by `retrieved_at` when `since` is set.
- Semantic search is optional. When memory has an embedder (`ostler-memory/src/embed.rs:9-20`), fuse with `ostler-memory::fusion` (`fusion.rs`, RRF already implemented). The default needs no server.

**Tests.** A held-out set in `ostler-bench/src/real.rs`: 30 questions whose supporting passages span three or more documents. Measure recall@5, and check that results are not all from the last-read page.

### E14: reliable search integration (extend)

- `ostler-research/src/search/mod.rs`: `SearchError` (`35`) gets distinct variants `QuotaExhausted`, `Unauthorized`, `Unavailable`, `BadQuery`, mapped from the HTTP status in `brave.rs` and `searxng.rs`.
- `SearchQuery` (`88-101`) gains `locale` and `country` (Brave `country`/`search_lang`, SearXNG `language`).
- Batching: `search_tool` (`research.rs:501`) accepts `queries: [..]` and runs them with bounded concurrency, deduplicating by `normalise_url` (`mod.rs:312`).
- Fallback: `ProviderConfig::from_lookup` (`mod.rs:245`) reads `OSTLER_SEARCH_FALLBACK=searxng,brave`. Fall back on `Unavailable`/`QuotaExhausted` only, never on empty results, and report the provider actually used.
- Snippets stay unread: the `(already read)` marker (`research.rs:530`) is kept, and results go into `structuredContent` with `read: false`.
- Contract tests: `ostler-research/tests/live_search.rs` behind `#[ignore]` plus an env var. CI runs it nightly against real Brave and SearXNG with a small quota.

### E19: claim-to-evidence records (add)

- New `ostler-research/src/claims.rs`: `Claim { id, text, support: Vec<SpanRef{version_id, start, end}>, counter: Vec<SpanRef>, assumptions: Vec<String>, assessments: Vec<Assessment> }`.
- `Assessment { kind: QuoteMatch | Entailment | Corroboration, result: Holds | Fails | Unclear, method, assessor, at }`.
- `cite` (`research.rs:365`) stays a quote-match check. New tool `claim {action: add|support|counter|assess|list}`. `evidence_tool` (`411-484`) lists claims grouped by their **weakest** assessment. A claim with only `QuoteMatch` renders as "quoted, not assessed", never "verified".
- Deterministic guards run on `claim support` (part of E20).

### E20: semantic evidence assessment (add)

- Deterministic guards in a new `ostler-extract/src/guards.rs`: negation within the span window, years and dates (compared with `ostler-extract` number parsing), units and magnitudes (%, k/M/bn, currency), and population words (a "children" vs "adults" lexicon). A guard failure sets `Assessment{Entailment, Fails, method:"guard:<name>"}`.
- Model assessment uses MCP **sampling** when the client supports it (check `capabilities.sampling` at `initialize`, `lib.rs:44`). Otherwise return `next_action: {kind:"assess", prompt, context_spans}` so the client's own model can do it. Record the assessor model and version from the sampling response.
- Context adequacy: send the supporting span plus ±N chunks from the archived version, never the quote alone.
- Evaluation: `ostler-bench` gets a labelled set covering contradiction, timeframe shift, population shift and causal overreach. Report agreement with human labels, and no "accuracy = proof" wording.

### E21: source independence and conflict handling (add)

- New `ostler-research/src/families.rs`. SimHash/MinHash over the chunk shingles of each archived version clusters copies and syndicated text into an `EvidenceFamily`. Canonical and `og:url` / `rel=canonical` come from `ReadMeta` (`read.rs:70`). Upstream links come from the links in `Doc.md`.
- Quantity comparison reuses `guards.rs` number/unit/date parsing. Two values conflict only if unit and timeframe match; otherwise the result is `DifferentFrame`.
- `evidence_tool` shows "N sources, M independent families". The labels `primary` and `independent` are `Assessment`s with a method, not facts.

### E22: research controller (add)

- New `clients/python/ostler_research/` and `clients/ts/research/`: a thin runner over the typed SDK (E33) that loops through decompose → plan queries → `search` → `open_many` → `retrieve` → `claim` → gap-fill → synthesise → audit (`evidence`).
- Agent skill: a `prompts/get` entry `research_mission` (`ostler-mcp/src/prompts.rs`) that encodes the same loop for clients without the SDK.
- The Rust core stays model-free. The runner uses the client's model or MCP sampling.

### E23: research stopping and export (add)

- `Mission` (E04) gains `subquestions: Vec<SubQ { text, status: Open|Covered|Conflicted|Gap, claims }>`.
- Stopping rule in the runner: stop when every subquestion is `Covered|Conflicted|Gap`, when the budget is exhausted, or when the last N rounds added no new family. The last two produce a partial report.
- New tool `export {format: markdown|json|manifest}`. Markdown is the report with numbered citations to archived versions. JSON holds claims, assessments and families. The manifest lists every `SourceVersion` with hash and retrieval time. Unresolved conflicts are listed, not hidden.

### E24: schema-driven extraction (extend)

- `extract_tool` (`research.rs:782-905`) gains `schema` (a JSON Schema). Rows from `extract_tables`/`extract_lists` (`read.rs:315-332`) are mapped to schema fields by header similarity (reuse `ostler-forms::resolve`). Each cell carries `locator {version_id, table, row, col}`.
- A missing or unmappable field becomes `null` with `unknown: true`, never a guess.
- Pagination: follow `PaginationHints.next` (`read.rs:145-168`) in a scratch page (E03), up to a page budget. Dedup by a stable key (schema `x-key` or a hash of all fields).
- Coverage: `ostler-extract/src/completeness.rs` already scores list completeness. Return `coverage: complete | partial{reason} | unknown`. Hitting the page budget means `partial`.

---

## Phase 4: Broader coverage and reuse (E10, E29–E33)

### E10: difficult interaction coverage (add and extend)

| Capability | Change |
|---|---|
| Cross-origin frames | `Shared.children` (`page.rs:76`) already holds the OOPIF sessions for the gate. Inject the isolated runtime into each (in `spawn_child`) and prefix their refs `f<frame>:e<n>`. `rt_expr` routes by prefix to the child `Session`. Mouse input uses `absoluteCenter` (`runtime.js:~1210`) plus the frame offset from `DOM.getBoxModel` of the `<iframe>`. |
| Semantic `find` | New tool `find {role?, name?, text?, near?}`, served by a runtime function over the snapshot tree. It returns ranked refs and refuses to pick one when the top two score the same. |
| Screenshot coordinates | `look` (`page.rs:1657`) already numbers targets. Add `click_at {x, y, from: screenshot_id}`, which maps through `deepElementFromPoint` and runs the same actionability checks and policy as `click` (E07). |
| Zoom/crop | `screenshot` (`page.rs:1617`) gains `clip {x,y,w,h}` and `scale`, using `Page.captureScreenshot.clip`. |
| Nested scroll | `scroll` (`page.rs:1376-1414`) gains `container: ref`, scrolling that element instead of `window`. |
| Drag/drop | `drag {from, to}`: `Input.dispatchDragEvent` for native DnD, and a pointer sequence for libraries. |
| Virtualised lists | `scroll_collect {container, until_count|until_text, max_steps}` scrolls and snapshots with dedup by row identity. |
| Custom controls | Extend `comboPick` / `pickDate` (`runtime.js`, exported at `2227`) with ARIA-pattern drivers for listbox, grid date pickers and sliders, each with a fixture. |

Each row gets a deterministic fixture in `ostler-fixtures/src/packs.rs` and a test in `ostler-browser/tests/`. All of them go through `Policy` and E27's risk check.

### E29: parameterised verified replay (add)

- `ostler-memory` playbooks (`Playbook`, `lib.rs:468`) already record form mappings. Add a new `Workflow { name, params: schema, steps: Vec<Step { locator: Semantic{role,name,near}, action, value: Param|Secret(fact_key)|Literal, expect: Vec<Expect>, risk: RiskClass }> }`.
- Record: on `mission finish` with a passing verdict, offer `workflow save` from the journal (E04). Secrets become `{{fact:KEY}}` references (already supported, `app.rs:1374-1407`).
- Replay: `workflow run {name, params}` executes through `batch` (E09). Before each step it resolves the locator with `find` (E10). Zero or several matches means **drift**: stop, record `Drift{step, expected, found}`, and return `NeedsUser{Ambiguous}`. Risky steps always ask E27 for a grant, and replay cannot carry one over.

### E30: memory integrity (wire and extend)

- Wire source memory (F4) as the archive index (E17).
- `Fact` (`ostler-memory/src/lib.rs:379`) gains `provenance {source: user|learned|import, at, from_url?}` and `fresh_until`. `suggest` (`1182`) skips stale learned facts and always ranks `user` above `learned`. That rule exists for playbooks (`trusted`, `451`) but not for facts.
- `forget` (`722`) cascades: it deletes derived playbook fields whose value came from the fact, and the episodes that mention the key (`record_episode`, `1008`). Add the cascade in each `Store` impl (`local.rs`, `qdrant.rs`, `sealed.rs`) and assert it in `ostler-memory/tests/hardening_real.rs`.
- Embedding migration: `Embedder::model_id` (`embed.rs:16`) is recorded and mismatches are detected on open. Add `ostler memory reembed` to re-embed in place rather than refusing.
- Document the limits of archive and backup deletion in `docs/MEMORY.md`. Deleting a fact does not rewrite an exported backup.

### E31: local execution inspection (add)

- `ostler serve --inspect 127.0.0.1:<port>`: a small `axum` server (already a dependency of `ostler-a2a`) that serves one page and an SSE stream of journal events (E04), the current page/action, budgets (E06) and health (E34).
- Controls: `POST /pause`, `/stop`, `/resume` and `/takeover` (`/takeover` pauses and switches Chrome to headed, if it is attachable). They act through the same cancellation tokens as E02, and `stop` uses the shutdown path. The page shows "completed before stop" from the journal.
- Loopback only, with a random token in the URL.

### E32: diagnostics (wire and extend)

- Wire `Page::diagnostics` (F3) as the tool `diagnostics {since?, kinds?}`, reusing the cursor model at `diagnostics.rs:28-80`.
- Trace correlation: add `ostler-workflow::trace::TraceWriter` (`trace.rs:521-628`) to `App`, behind `--trace <path>`. Every `App::call` appends a `TraceEvent` with `action_id`, `mission_id`, tool, redacted args, outcome and the diagnostics cursor range.
- Redacted screenshots: in `screenshot`, before capture, run the runtime `mark` / `unmark` (already exported, `runtime.js:2227`) to black out `looksSecret` fields.
- `ostler support-bundle --mission <id>`: zip the trace (redacted), failed-step diagnostics, the outcome JSON and Ostler/Chrome versions. Include no cookies and no page bodies unless `--include-pages`.

### E33: stable integration surface (extend)

- Version the E01 envelope: `outcome.schema = "ostler.outcome/1"`. Check it into `docs/schemas/outcome-1.json` and generate it with `schemars`.
- `clients/python` and `clients/ts`: typed wrappers generated from the `outputSchema`s.
- CLI JSON: `ostler call <tool> --json '<args>'` runs one tool through `App::call` and prints `structuredContent`. That makes it scriptable without MCP.
- Fallbacks: test `Profile::Auto` (`tools.rs:20`) with a client that ignores `list_changed`; every tool stays callable (`allowed`, `191`). Test with no sampling and no elicitation: E20 and E27 must return `next_action` rather than block.

---

## Continuous: E35 and E36 (start with the first PR)

### E35: independent correctness tests

- `ostler-bench/src/falsesuccess.rs` already measures false success. Extend `suite.rs` so every run reports **useful completion** and **false completion** together, and treat a refusal as non-completion: a runtime that refuses everything scores 0, not "safe".
- Adversarial fixtures, one per E-item, added in the same PR as the feature: covered buttons (E07), ref reuse after navigation (E08), field reset (E11), fake "Saved" (E12), hostile prototypes (E26), redirect to metadata (E25), rebinding DNS (E25), slow and partial downloads (E13), and a page that changes between reads (E17).
- Fault injection: SIGKILL Chrome (E05), Qdrant down (E34), search 429 (E14), and a tool cancelled mid-write (E02).
- Real-model held-out tasks: `scripts/eval/` (already present). Keep a held-out split that the developers do not tune on, and check outcomes by an independent oracle (the fixture server's own records), never by the agent's claim.
- Report unsupported, flaky and regressed tasks as separate counts in `docs/BENCHMARKS.md`.

### E36: performance and compatibility discipline

- `ostler-bench/src/scale.rs` and `cost.rs`: add end-to-end latency per tool (p50/p95), output tokens per task (from `App::call` metrics, `app.rs:783-792`), cancellation delay (E02), RSS growth over a 2-hour session (`Browser::rss_kb`, `lib.rs:545`), and archive/journal growth.
- CI matrix (`.github/workflows`): Linux, macOS and Windows × Chrome stable and beta × MCP protocol `2024-11-05`, `2025-03-26` and `2025-06-18` (`SUPPORTED`, `lib.rs:21`).
- State migrations: the journal, archive index and memory stores carry a `format` version. Add a test that opens state written by the previous release.
- A perf PR may not delete or weaken a correctness assertion. Enforce this in review with the bench's false-completion gate.

---

## Order of pull requests

Each PR is small enough to review and ships with its tests and fixtures.

1. F1 fix (`again` wired), plus a regression test.
2. E01 outcome envelope, text unchanged.
3. E02 split reader/writer/dispatcher, cancellation and progress.
4. E03 `Arc<Browser>`, per-page mutation lock, `tab` argument.
5. E08 ref generations and navigation epochs.
6. E07 actionability and explicit `force`.
7. E09 effects and `batch`.
8. E25 policy profiles and redirect/rebinding checks.
9. E26 isolated world.
10. E27 wire risk, grants and handoff statuses.
11. E04 mission and journal; E06 budgets.
12. E05 crash recovery (depends on 11).
13. E11 consistency; E12 expect wiring and new checks.
14. E13 downloads; E28 profiles; E34 health.
15. E17 archive (changes `Evidence`); E15 PDF.
16. E16 pooled `open_many`; E18 `retrieve`.
17. E14 search hardening.
18. E19 claims, then E20 guards and assessment, then E21 families.
19. E24 schema extraction; E23 export; E22 runner and skill.
20. E10 items as fixtures demand; E29 replay; E30 memory; E31 inspector; E32 diagnostics; E33 SDKs.

New ADRs: 0020 outcome envelope (E01), 0021 concurrency model (E02/E03), 0022 isolated world (E26),
0023 mission journal and recovery (E04/E05), 0024 source archive versioning (E17), 0025 claims and
assessments (E19/E20).
