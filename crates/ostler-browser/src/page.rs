use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use ostler_cdp::{CdpError, Session};
use ostler_forms::{FormIr, SubmitControl};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::time::{sleep, Instant};

use crate::policy::Policy;

pub const RUNTIME_JS: &str = include_str!("runtime.js");

#[derive(Debug, thiserror::Error)]
pub enum BrowserError {
    #[error(transparent)]
    Cdp(#[from] CdpError),
    #[error("page script error: {0}")]
    Js(String),
    #[error("blocked by policy: {0}")]
    Policy(String),
    #[error("navigation failed: {0}")]
    Navigation(String),
    #[error("{0}")]
    Other(String),
}

/// A human-verification wall on the current page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Challenge {
    pub vendor: String,
    /// True when the challenge replaced the page ("Just a moment..."), false for an embedded widget.
    pub interstitial: bool,
}

pub type Result<T> = std::result::Result<T, BrowserError>;

#[derive(Default)]
struct Shared {
    main_frame: Mutex<Option<String>>,
    started: AtomicU64,
    committed: AtomicU64,
    same_doc: AtomicU64,
    loaded: AtomicU64,
    /// Count of `settle` calls that hit their time limit with work still pending.
    unsettled: AtomicU64,
    dialogs: Mutex<Vec<String>>,
    /// `--require-confirm-submit`: block every non-GET request unless a confirmed `form_submit` is running.
    gate: AtomicBool,
    armed: AtomicBool,
    /// A popup opened by a confirmed submit's click: its first request may carry the confirmation (once).
    inherit_armed: AtomicBool,
    /// Things that went wrong with our own bookkeeping (lost events, a frame that could not be secured),
    /// reported with the next action's warnings.
    notices: Mutex<Vec<String>>,
    blocked: Mutex<Vec<String>>,
    /// Latest main-frame document response (status, content type): redirects overwrite, so the final one wins.
    doc: Mutex<Option<(u16, String)>>,
    /// Sessions of cross-site iframes and workers attached to this page, kept so the gate reaches them too.
    children: Mutex<Vec<Session>>,
}

/// Pause new related targets (cross-site iframes, workers) until interception is set up on them.
static AUTO_ATTACH: std::sync::LazyLock<Value> = std::sync::LazyLock::new(
    || json!({ "autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true }),
);

/// What interception must see: everything under an allowlist or the private-network block (whatever the
/// request type: an <img src>, a stylesheet or a font carries data out exactly as well as fetch() does, and a
/// public URL can redirect inward); only requests that can have a body under the submit gate; else nothing.
fn fetch_patterns(policy: &Policy, gate: bool) -> Option<Vec<Value>> {
    if policy.restricts() || policy.block_private_networks {
        Some(vec![
            json!({ "urlPattern": "*", "requestStage": "Request" }),
        ])
    } else if gate {
        Some(
            ["Document", "XHR", "Fetch", "Ping", "EventSource", "Other"]
                .iter()
                .map(|t| json!({ "urlPattern": "*", "resourceType": t, "requestStage": "Request" }))
                .collect(),
        )
    } else {
        None
    }
}

/// Decide a paused request. Runs as its own task: never let a slow reply (or a DNS lookup) stall an event loop.
fn spawn_verdict(params: &Value, sess: Session, policy: Arc<Policy>, shared: Arc<Shared>) {
    let id = params["requestId"].clone();
    let url = params["request"]["url"].as_str().unwrap_or("").to_string();
    let method = params["request"]["method"]
        .as_str()
        .unwrap_or("GET")
        .to_uppercase();
    let is_doc = params["resourceType"].as_str() == Some("Document");
    tokio::spawn(async move {
        let mut verdict = policy.check_resolved(&url).await;
        // A popup spawned by the click of a confirmed submit carries that confirmation for its first request.
        let inherited = shared.inherit_armed.swap(false, Ordering::SeqCst);
        if verdict.is_ok()
            && shared.gate.load(Ordering::SeqCst)
            && !shared.armed.load(Ordering::SeqCst)
            && !inherited
            && !matches!(method.as_str(), "GET" | "HEAD" | "OPTIONS")
        {
            shared
                .blocked
                .lock()
                .unwrap()
                .push(format!("{method} {url}"));
            verdict = Err("blocked by --require-confirm-submit".into());
        }
        let _ = match (verdict, is_doc) {
            (Ok(()), _) => {
                sess.call("Fetch.continueRequest", json!({ "requestId": id }))
                    .await
            }
            // A blocked navigation answered with "204 No Content" leaves the current page in
            // place; failing it would replace the page with a browser error screen.
            (Err(_), true) => {
                sess.call(
                    "Fetch.fulfillRequest",
                    json!({ "requestId": id, "responseCode": 204, "responseHeaders": [] }),
                )
                .await
            }
            (Err(_), false) => {
                sess.call(
                    "Fetch.failRequest",
                    json!({ "requestId": id, "errorReason": "BlockedByClient" }),
                )
                .await
            }
        };
    });
}

/// Take over a session Chrome attached to (a child of a page): intercept first, then let it run.
fn spawn_child(
    conn: &Arc<ostler_cdp::Connection>,
    params: &Value,
    shared: Arc<Shared>,
    policy: Arc<Policy>,
) {
    let (Some(sid), Some(tid)) = (
        params["sessionId"].as_str(),
        params["targetInfo"]["targetId"].as_str(),
    ) else {
        return;
    };
    let kind = params["targetInfo"]["type"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let waiting = params["waitingForDebugger"].as_bool().unwrap_or(false);
    let child = Session::from_attached(conn.clone(), sid.to_string(), tid.to_string());
    let mut rx = conn.subscribe();
    let conn = conn.clone();
    shared.children.lock().unwrap().push(child.clone());
    tokio::spawn(async move {
        let sid = child.session_id().to_string();
        let ready = async {
            if let Some(p) = fetch_patterns(&policy, shared.gate.load(Ordering::SeqCst)) {
                child.call("Fetch.enable", json!({ "patterns": p })).await?;
            }
            child
                .call("Target.setAutoAttach", AUTO_ATTACH.clone())
                .await
        };
        // If interception cannot be set up on a frame or worker, never let it run unchecked: it stays
        // paused (our session keeps it so) and is closed if the browser lets us.
        if ready.await.is_ok() {
            if waiting {
                let _ = child
                    .call("Runtime.runIfWaitingForDebugger", json!({}))
                    .await;
            }
        } else {
            shared.notices.lock().unwrap().push(format!(
                "a {kind} could not be put under the request policy, so it was left paused"
            ));
            let _ = conn
                .call(
                    "Target.closeTarget",
                    json!({ "targetId": child.target_id }),
                    None,
                    ostler_cdp::DEFAULT_TIMEOUT,
                )
                .await;
        }
        // A probe that finds the session gone ends the loop (a lost detach event would never).
        let gone = Arc::new(tokio::sync::Notify::new());
        loop {
            let ev = tokio::select! {
                ev = rx.recv() => match ev {
                    Ok(ev) => ev,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        // Dropped events cannot be replayed: a request whose `Fetch.requestPaused` was lost
                        // stays paused (fail closed, but hangs), and a lost attach leaves a frame paused.
                        shared.notices.lock().unwrap().push(format!(
                            "{n} browser events were lost; a request or frame may stay stuck until the next navigation"
                        ));
                        let (c, g) = (child.clone(), gone.clone());
                        tokio::spawn(async move {
                            if let Err(ostler_cdp::CdpError::Protocol { .. }) =
                                c.call("Target.setAutoAttach", AUTO_ATTACH.clone()).await
                            {
                                g.notify_one();
                            }
                        });
                        continue;
                    }
                    Err(_) => break,
                },
                _ = gone.notified() => break,
            };
            if ev.method == "Target.detachedFromTarget"
                && ev.params["sessionId"].as_str() == Some(&sid)
            {
                break;
            }
            if ev.session_id.as_deref() != Some(&sid) {
                continue;
            }
            match ev.method.as_str() {
                "Fetch.requestPaused" => {
                    spawn_verdict(&ev.params, child.clone(), policy.clone(), shared.clone())
                }
                "Target.attachedToTarget" => {
                    spawn_child(&conn, &ev.params, shared.clone(), policy.clone())
                }
                _ => {}
            }
        }
        shared
            .children
            .lock()
            .unwrap()
            .retain(|c| c.session_id() != sid);
    });
}

#[derive(Default)]
pub(crate) struct PageState {
    prev_lines: Vec<String>,
    prev_url: String,
    /// Fields filled and verified since the last submit: (ref, label, value). Re-read before submitting.
    pub(crate) filled: Vec<(String, String, String)>,
    /// Ref of the form the last `fill_form` worked on, so a bare `form_submit` sends that form and not
    /// whichever form comes first on the page (a header newsletter box, a search form).
    pub(crate) last_form: Option<String>,
}

#[derive(Clone)]
pub struct Page {
    pub(crate) session: Session,
    shared: Arc<Shared>,
    pub(crate) state: Arc<Mutex<PageState>>,
    pub(crate) policy: Arc<Policy>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PageInfo {
    pub url: String,
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotMode {
    #[default]
    Full,
    /// Only what changed since the previous snapshot of the same URL.
    Delta,
    /// Interactive elements and headings only.
    Interactive,
}

#[derive(Debug, Clone, Default)]
pub struct SnapshotOpts {
    pub mode: SnapshotMode,
    pub scope: Option<String>,
    pub max_chars: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SnapshotOut {
    pub url: String,
    pub title: String,
    pub text: String,
    pub truncated: bool,
    pub lines: usize,
}

/// Result of an action that may have changed the page.
#[derive(Debug, Clone, Serialize, Default)]
pub struct ActionOutcome {
    pub navigated: bool,
    pub url: String,
    pub title: String,
    pub warnings: Vec<String>,
    pub dialogs: Vec<String>,
    /// HTTP status of the main document when this action loaded one.
    pub status: Option<u16>,
    /// Plain observations (not problems), e.g. "redirected to a sign-in page".
    pub notes: Vec<String>,
    /// False when the page's markup was identical before and after a click.
    pub changed: bool,
}

/// Turn a failed `Page.navigate` into something an agent can act on.
pub fn explain_nav_error(err: &str, url: &str, status: Option<u16>, ctype: Option<&str>) -> String {
    let mut s = format!("{err} ({url})");
    if let Some(st) = status {
        let why = match st {
            401 | 403 => "the site is refusing automated access",
            429 => "rate limited (too many requests); wait before retrying or use another source",
            404 | 410 => "page not found",
            500..=599 => "server error; try again later or use another source",
            _ => "",
        };
        let why = if why.is_empty() {
            String::new()
        } else {
            format!(": {why}")
        };
        s = format!("HTTP {st}{why} - {s}");
    } else {
        let hint = friendly_net_error(err, url);
        if !hint.is_empty() {
            s = format!("{hint} [{err}] ({url})");
        }
    }
    if let Some(ct) = ctype.filter(|c| is_unreadable_type(c)) {
        s.push_str(&format!(". {}", non_html_note(ct)));
    }
    s
}

fn is_unreadable_type(ct: &str) -> bool {
    let ct = ct.to_lowercase();
    !(ct.is_empty()
        || ct.contains("html")
        || ct.contains("xml")
        || ct.starts_with("text/")
        || ct.contains("json"))
}

fn non_html_note(ct: &str) -> String {
    let kind = if ct.to_lowercase().contains("pdf") {
        "PDF/non-HTML document"
    } else {
        "Binary or non-HTML file"
    };
    format!("{kind} (content-type {ct}); Ostler cannot read it, fetch it another way")
}

fn looks_like_login(url: &str, title: &str) -> bool {
    let path = url::Url::parse(url)
        .map(|u| u.path().to_lowercase())
        .unwrap_or_default();
    let t = title.to_lowercase();
    [
        "authwall", "login", "signin", "sign-in", "sign_in", "log-in",
    ]
    .iter()
    .any(|k| path.contains(k))
        || ["sign in", "log in", "login"].iter().any(|k| t.contains(k))
}

const CONTEXT_GONE: [&str; 4] = [
    "Execution context was destroyed",
    "Cannot find context with specified id",
    "Inspected target navigated or closed",
    "Target closed",
];

impl Page {
    pub fn target_id(&self) -> &str {
        &self.session.target_id
    }

    pub async fn attach(session: Session, policy: Policy) -> Result<Self> {
        Self::attach_inner(session, policy, false, false, false).await
    }

    /// Attach to a target Chrome is holding at start (`waitForDebuggerOnStart`) and release it only once
    /// interception is on, so the target's very first request is already subject to the policy and the gate.
    /// `inherit_armed`: the popup was opened by the click of a confirmed submit, so its first request may
    /// carry that confirmation.
    pub(crate) async fn attach_held(
        session: Session,
        policy: Policy,
        gate: bool,
        inherit_armed: bool,
    ) -> Result<Self> {
        Self::attach_inner(session, policy, true, gate, inherit_armed).await
    }

    async fn attach_inner(
        session: Session,
        policy: Policy,
        held: bool,
        gate: bool,
        inherit_armed: bool,
    ) -> Result<Self> {
        let page = Self {
            session,
            shared: Arc::default(),
            state: Arc::default(),
            policy: Arc::new(policy),
        };
        page.shared.gate.store(gate, Ordering::SeqCst);
        page.shared
            .inherit_armed
            .store(inherit_armed, Ordering::SeqCst);
        let s = &page.session;
        // Subscribe before anything can attach, so no event is missed.
        let mut events = s.events();
        // With an allowlist, hold every navigation and every script-initiated request that could carry data
        // out, and let only allowed ones proceed. Without this, a form action, redirect or fetch() reaches the
        // forbidden origin (cookies and body included) before any after-the-fact check could notice.
        page.sync_fetch().await?;
        let conn = s.connection().clone();
        let shared = page.shared.clone();
        let sess = s.clone();
        let fetch_policy = page.policy.clone();
        tokio::spawn(async move {
            while let Some(ev) = events.next().await {
                // The target is gone: nothing more will come, so the task (and its subscription) ends.
                if ev.method == "Target.detachedFromTarget" {
                    break;
                }
                if ev.method == ostler_cdp::LAGGED {
                    // Lost events cannot be replayed. A request whose `Fetch.requestPaused` was lost stays
                    // paused (it hangs rather than escaping), and a lost attach leaves a frame paused.
                    // Say so, and ask again for the frames we should be holding.
                    shared.notices.lock().unwrap().push(format!(
                        "{} browser events were lost; a request or frame may stay stuck until the next navigation",
                        ev.params["missed"]
                    ));
                    let s2 = sess.clone();
                    tokio::spawn(async move {
                        let _ = s2.call("Target.setAutoAttach", AUTO_ATTACH.clone()).await;
                    });
                    continue;
                }
                let is_main = |frame_id: Option<&str>| {
                    frame_id.is_some() && frame_id == shared.main_frame.lock().unwrap().as_deref()
                };
                match ev.method.as_str() {
                    "Page.frameStartedLoading" if is_main(ev.params["frameId"].as_str()) => {
                        shared.started.fetch_add(1, Ordering::SeqCst);
                    }
                    "Page.frameNavigated" if ev.params["frame"].get("parentId").is_none() => {
                        shared.committed.fetch_add(1, Ordering::SeqCst);
                    }
                    "Page.navigatedWithinDocument" if is_main(ev.params["frameId"].as_str()) => {
                        shared.same_doc.fetch_add(1, Ordering::SeqCst);
                    }
                    "Network.responseReceived"
                        if ev.params["type"].as_str() == Some("Document")
                            && is_main(ev.params["frameId"].as_str()) =>
                    {
                        let r = &ev.params["response"];
                        let status = r["status"].as_u64().unwrap_or(0) as u16;
                        let mime = r["mimeType"].as_str().unwrap_or("").to_string();
                        *shared.doc.lock().unwrap() = Some((status, mime));
                    }
                    "Fetch.requestPaused" => {
                        spawn_verdict(
                            &ev.params,
                            sess.clone(),
                            fetch_policy.clone(),
                            shared.clone(),
                        );
                    }
                    // An out-of-process iframe (or worker) arrives paused on its own session; it gets the
                    // same interception before it is allowed to send anything.
                    "Target.attachedToTarget" => {
                        spawn_child(&conn, &ev.params, shared.clone(), fetch_policy.clone());
                    }
                    // A navigation that never commits (blocked by policy, answered with 204) fires no load
                    // event; "stopped loading" is the signal that the attempt is over either way.
                    "Page.frameStoppedLoading" if is_main(ev.params["frameId"].as_str()) => {
                        shared.loaded.fetch_add(1, Ordering::SeqCst);
                    }
                    "Page.loadEventFired" => {
                        shared.loaded.fetch_add(1, Ordering::SeqCst);
                    }
                    "Page.javascriptDialogOpening" => {
                        let kind = ev.params["type"].as_str().unwrap_or("dialog");
                        let msg = ev.params["message"].as_str().unwrap_or("");
                        // alert and beforeunload ask for nothing. confirm and prompt ask for a decision the
                        // page is about to act on ("Delete everything?"), so a browser nobody is watching
                        // says no unless it was told otherwise.
                        let accept =
                            matches!(kind, "alert" | "beforeunload") || fetch_policy.accept_dialogs;
                        shared.dialogs.lock().unwrap().push(format!(
                            "{} {kind}: {msg}",
                            if accept { "accepted" } else { "dismissed" }
                        ));
                        let default = ev.params["defaultPrompt"].as_str().unwrap_or("");
                        let _ = sess
                            .call(
                                "Page.handleJavaScriptDialog",
                                json!({ "accept": accept, "promptText": default }),
                            )
                            .await;
                    }
                    _ => {}
                }
            }
        });

        // The rest of the setup. For a held target the release is sent alongside it: the renderer only
        // starts (and so only answers these) once released, and its first request already faces interception.
        // Order matters for a held target: only Page.enable goes out before the release (sending more first
        // lets the held target's first request escape), the rest is sent back to back after it.
        let s = &page.session;
        if held {
            let (enabled, released) = tokio::join!(
                s.call("Page.enable", json!({})),
                s.call("Runtime.runIfWaitingForDebugger", json!({}))
            );
            enabled?;
            released?;
        } else {
            s.call("Page.enable", json!({})).await?;
        }
        // A session handles its commands in order, so sending these together has the same effect as one
        // after another, without a round trip each.
        let (_, _, _, _, _, tree, _) = tokio::try_join!(
            // Cross-site iframes live in other processes with sessions of their own. Chrome pauses them at
            // start until we have enabled interception there, so their very first request is seen too.
            s.call("Target.setAutoAttach", AUTO_ATTACH.clone()),
            s.call("Runtime.enable", json!({})),
            s.call("DOM.enable", json!({})),
            // Headless Chrome otherwise reports the page as unfocused, which suppresses focus/blur/change
            // events.
            s.call(
                "Emulation.setFocusEmulationEnabled",
                json!({ "enabled": true }),
            ),
            s.call(
                "Page.addScriptToEvaluateOnNewDocument",
                json!({ "source": RUNTIME_JS, "runImmediately": true }),
            ),
            s.call("Page.getFrameTree", json!({})),
            // Main-document HTTP status comes from the network layer; the page itself never shows it.
            s.call("Network.enable", json!({})),
        )?;
        *page.shared.main_frame.lock().unwrap() =
            tree["frameTree"]["frame"]["id"].as_str().map(String::from);
        Ok(page)
    }

    // ------------------------------------------------------------------ evaluation

    /// Evaluate an expression in the page, retrying across navigations.
    ///
    /// Only for expressions that are safe to run twice (reads). When the page is replaced mid-call
    /// the whole expression is run again on the new page, so anything with side effects must use
    /// [`Page::eval_once`].
    pub async fn eval(&self, expression: &str) -> Result<Value> {
        self.eval_attempts(expression, 8).await
    }

    /// Evaluate at most once. If the page is replaced while the script runs, the script may or may not
    /// have finished, so report that instead of replaying it on whatever page came next.
    pub async fn eval_once(&self, expression: &str) -> Result<Value> {
        self.eval_attempts(expression, 1).await
    }

    async fn eval_attempts(&self, expression: &str, attempts: u32) -> Result<Value> {
        let mut last = String::new();
        for attempt in 0..attempts {
            if attempt > 0 {
                sleep(Duration::from_millis(60)).await;
            }
            let res = self
                .session
                .call(
                    "Runtime.evaluate",
                    json!({
                        "expression": expression,
                        "returnByValue": true,
                        "awaitPromise": true,
                        "userGesture": true,
                    }),
                )
                .await;
            let res = match res {
                Ok(r) => r,
                Err(CdpError::Protocol { message, .. })
                    if CONTEXT_GONE.iter().any(|m| message.contains(m)) =>
                {
                    last = message;
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            if let Some(ex) = res.get("exceptionDetails") {
                let text = ex["exception"]["description"]
                    .as_str()
                    .or_else(|| ex["exception"]["value"].as_str())
                    .or_else(|| ex["text"].as_str())
                    .unwrap_or("script error")
                    .to_string();
                if CONTEXT_GONE.iter().any(|m| text.contains(m)) {
                    last = text;
                    continue;
                }
                let first = text
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim_start_matches("Error: ")
                    .to_string();
                if first.starts_with("unknown ref") {
                    return Err(BrowserError::Other(first));
                }
                return Err(BrowserError::Js(first));
            }
            return Ok(res["result"].get("value").cloned().unwrap_or(Value::Null));
        }
        if attempts == 1 {
            return Err(BrowserError::Navigation(format!(
                "the page changed while the action was running ({last}); it may or may not have completed"
            )));
        }
        Err(BrowserError::Js(format!("page kept navigating: {last}")))
    }

    fn rt_expr(name: &str, args: &[Value]) -> String {
        let argv = args
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "(typeof __wm==='undefined'?(()=>{{throw new Error('page runtime unavailable on this page')}})():__wm.{name}({argv}))"
        )
    }

    /// Call a read-only function on the injected runtime: `__wm.name(args...)`. Retried across navigations.
    pub async fn rt(&self, name: &str, args: &[Value]) -> Result<Value> {
        self.eval(&Self::rt_expr(name, args)).await
    }

    /// A human-verification wall (CAPTCHA widget or "verify you are human" interstitial) that blocks
    /// this page right now. Ostler reports these; it never tries to solve or evade them.
    pub async fn challenge(&self) -> Result<Option<Challenge>> {
        let v = self.rt("challenge", &[]).await?;
        Ok(v.as_object().map(|o| Challenge {
            vendor: o
                .get("vendor")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
            interstitial: matches!(
                o.get("kind").and_then(Value::as_str),
                Some("interstitial" | "block")
            ),
        }))
    }

    /// Call a runtime function that changes the page (click, write, pick). Never retried: refs are
    /// per-document, so a replay after a navigation would act on a different element or fail confusingly.
    pub async fn rt_effect(&self, name: &str, args: &[Value]) -> Result<Value> {
        self.eval_once(&Self::rt_expr(name, args)).await
    }

    /// Skip downloading images, video, audio and fonts. Forms rarely need them and they dominate bytes on real sites.
    /// Screenshots will show missing images while this is on.
    pub async fn block_media(&self) -> Result<()> {
        let urls: Vec<String> = [
            "png", "jpg", "jpeg", "gif", "webp", "avif", "svg", "ico", "mp4", "webm", "mp3", "ogg",
            "woff", "woff2", "ttf", "otf",
        ]
        .iter()
        .map(|e| format!("*.{e}"))
        .collect();
        self.session.call("Network.enable", json!({})).await?;
        self.session
            .call("Network.setBlockedURLs", json!({ "urls": urls }))
            .await?;
        Ok(())
    }

    pub async fn info(&self) -> Result<PageInfo> {
        let v = self
            .eval("({url: location.href, title: document.title})")
            .await?;
        Ok(PageInfo {
            url: v["url"].as_str().unwrap_or("").to_string(),
            title: v["title"].as_str().unwrap_or("").to_string(),
        })
    }

    pub fn take_dialogs(&self) -> Vec<String> {
        std::mem::take(&mut *self.shared.dialogs.lock().unwrap())
    }

    // ------------------------------------------------------------------ navigation

    pub async fn navigate(&self, url: &str) -> Result<ActionOutcome> {
        self.policy
            .check_resolved(url)
            .await
            .map_err(BrowserError::Policy)?;
        let loaded_before = self.shared.loaded.load(Ordering::SeqCst);
        *self.shared.doc.lock().unwrap() = None;
        let res = self
            .session
            .call("Page.navigate", json!({ "url": url }))
            .await?;
        if let Some(err) = res["errorText"].as_str() {
            // The response event can trail the navigate reply by a few ms.
            sleep(Duration::from_millis(150)).await;
            let doc = self.shared.doc.lock().unwrap().clone();
            let status = doc.as_ref().map(|d| d.0).filter(|s| *s > 0);
            return Err(BrowserError::Navigation(explain_nav_error(
                err,
                url,
                status,
                doc.as_ref().map(|d| d.1.as_str()),
            )));
        }
        // Same-document navigations (hash changes) never fire a load event.
        if res.get("loaderId").is_some() {
            self.wait_loaded(loaded_before, Duration::from_secs(30))
                .await;
        }
        self.settle(3000, 150).await;
        let mut out = self.finish_outcome(true, vec![]).await?;
        let strip = |u: &str| {
            u.split('#')
                .next()
                .unwrap_or("")
                .trim_end_matches('/')
                .to_string()
        };
        if strip(&out.url) != strip(url)
            && !looks_like_login(url, "")
            && looks_like_login(&out.url, &out.title)
        {
            out.notes.push("redirected to a sign-in page".into());
        }
        Ok(out)
    }

    async fn wait_loaded(&self, loaded_before: u64, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.shared.loaded.load(Ordering::SeqCst) > loaded_before {
                return true;
            }
            sleep(Duration::from_millis(15)).await;
        }
        false
    }

    pub async fn settle(&self, max_ms: u64, quiet_ms: u64) -> Value {
        let v = self
            .rt("settle", &[json!(max_ms), json!(quiet_ms)])
            .await
            .unwrap_or(Value::Null);
        if v.get("settled").and_then(Value::as_bool) == Some(false) {
            self.shared.unsettled.fetch_add(1, Ordering::SeqCst);
        }
        v
    }

    /// Settle after a form action: wait only for what the action can have caused. DOM changes outside `scope`
    /// (a form or control ref) are ignored, and so are background requests and timers (beacons, ad hosts,
    /// requests repeated over and over, anything pending longer than `bg_ms`). Falls back to the whole page
    /// when the scope is unknown or gone.
    pub async fn settle_scoped(
        &self,
        max_ms: u64,
        quiet_ms: u64,
        scope: Option<&str>,
        bg_ms: u64,
    ) -> Value {
        let opts = json!({ "scope": scope, "bgMs": bg_ms });
        let v = self
            .rt("settle", &[json!(max_ms), json!(quiet_ms), opts])
            .await
            .unwrap_or(Value::Null);
        if v.get("settled").and_then(Value::as_bool) == Some(false) {
            self.shared.unsettled.fetch_add(1, Ordering::SeqCst);
        }
        v
    }

    /// Number of timed-out settles so far; fill_form diffs this to report unstable pages.
    pub(crate) fn unsettled_count(&self) -> u64 {
        self.shared.unsettled.load(Ordering::SeqCst)
    }

    /// Run `action`, then wait for whatever it caused (navigation or in-page updates) to finish.
    pub async fn after<F, Fut, T>(&self, action: F) -> Result<(T, ActionOutcome)>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let started = self.shared.started.load(Ordering::SeqCst);
        let committed = self.shared.committed.load(Ordering::SeqCst);
        let same_doc = self.shared.same_doc.load(Ordering::SeqCst);
        let loaded = self.shared.loaded.load(Ordering::SeqCst);
        *self.shared.doc.lock().unwrap() = None;
        let value = action().await?;
        // Give a navigation a moment to begin; most start within a frame or two.
        let probe = Instant::now() + Duration::from_millis(180);
        let mut nav = false;
        while Instant::now() < probe {
            if self.shared.started.load(Ordering::SeqCst) > started
                || self.shared.committed.load(Ordering::SeqCst) > committed
            {
                nav = true;
                break;
            }
            sleep(Duration::from_millis(15)).await;
        }
        let mut warnings = vec![];
        if nav && !self.wait_loaded(loaded, Duration::from_secs(20)).await {
            warnings.push("page did not finish loading within 20s; continuing".into());
        }
        self.settle(3000, 150).await;
        let spa = self.shared.same_doc.load(Ordering::SeqCst) > same_doc;
        let outcome = self.finish_outcome(nav || spa, warnings).await?;
        Ok((value, outcome))
    }

    async fn finish_outcome(
        &self,
        navigated: bool,
        mut warnings: Vec<String>,
    ) -> Result<ActionOutcome> {
        let info = self.info().await.unwrap_or(PageInfo {
            url: String::new(),
            title: String::new(),
        });
        if let Err(msg) = self.policy.check_resolved(&info.url).await {
            // A redirect or link led somewhere forbidden: leave immediately.
            let _ = self
                .session
                .call("Page.navigate", json!({ "url": "about:blank" }))
                .await;
            return Err(BrowserError::Policy(format!(
                "page moved to {}: {msg}",
                info.url
            )));
        }
        warnings.append(&mut self.shared.notices.lock().unwrap());
        for b in std::mem::take(&mut *self.shared.blocked.lock().unwrap()) {
            warnings.push(format!(
                "blocked {b}: --require-confirm-submit is on, so only form_submit with confirm=true may send data"
            ));
        }
        let dialogs = self.take_dialogs();
        if !dialogs.is_empty() {
            warnings.push("JavaScript dialogs appeared (confirm and prompt are dismissed unless --accept-dialogs)".into());
        }
        let doc = if navigated {
            self.shared.doc.lock().unwrap().clone()
        } else {
            None
        };
        let mut notes = vec![];
        if let Some((_, ct)) = doc.as_ref().filter(|d| is_unreadable_type(&d.1)) {
            notes.push(non_html_note(ct));
        }
        Ok(ActionOutcome {
            navigated,
            url: info.url,
            title: info.title,
            warnings,
            dialogs,
            status: doc.map(|d| d.0).filter(|s| *s > 0),
            notes,
            changed: true,
        })
    }

    pub async fn back(&self) -> Result<ActionOutcome> {
        // Nothing to go back to when the previous history entry is the tab's initial blank page.
        if let Ok(h) = self
            .session
            .call("Page.getNavigationHistory", json!({}))
            .await
        {
            let idx = h["currentIndex"].as_i64().unwrap_or(0);
            let prev = if idx > 0 {
                h["entries"][(idx - 1) as usize]["url"]
                    .as_str()
                    .unwrap_or("")
            } else {
                ""
            };
            if idx <= 0 || prev == "about:blank" || prev.is_empty() {
                let cur = h["entries"][idx.max(0) as usize].clone();
                return Ok(ActionOutcome {
                    url: cur["url"].as_str().unwrap_or("").to_string(),
                    title: cur["title"].as_str().unwrap_or("").to_string(),
                    notes: vec!["Still on the same page: this tab has no earlier page in its history (a tab opened by a popup or by tabs new starts empty; use tabs to switch to the original one)".into()],
                    changed: false,
                    ..Default::default()
                });
            }
        }
        let before = self
            .eval("location.href")
            .await
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        let (_, mut out) = self
            .after(|| async { self.eval("history.back()").await })
            .await?;
        if out.url == before {
            out.notes
                .push("Still on the same page: this tab has no earlier page in its history (a tab opened by a popup starts empty; use tabs to switch to the original one)".into());
            out.changed = false;
        }
        Ok(out)
    }

    // ------------------------------------------------------------------ snapshots

    /// Network interception is on whenever there is something to enforce: an allowlist or the submit gate.
    /// Applies to the page and to every child session (cross-site iframes, workers) attached to it.
    async fn sync_fetch(&self) -> Result<()> {
        let Some(patterns) = fetch_patterns(&self.policy, self.shared.gate.load(Ordering::SeqCst))
        else {
            return Ok(());
        };
        self.session
            .call("Fetch.enable", json!({ "patterns": patterns }))
            .await?;
        let kids: Vec<Session> = self.shared.children.lock().unwrap().clone();
        for k in kids {
            // A child that has just gone away is not a failure.
            let _ = k
                .call("Fetch.enable", json!({ "patterns": patterns }))
                .await;
        }
        Ok(())
    }

    /// With the gate on, only a confirmed `form_submit` may send a non-GET request.
    pub async fn set_submit_gate(&self, on: bool) -> Result<()> {
        self.shared.gate.store(on, Ordering::SeqCst);
        self.sync_fetch().await
    }

    /// True while a confirmed `form_submit` is in flight on this page.
    pub(crate) fn is_armed(&self) -> bool {
        self.shared.armed.load(Ordering::SeqCst)
    }

    pub(crate) fn arm_submit(&self, on: bool) {
        self.shared.armed.store(on, Ordering::SeqCst);
    }

    /// Refuse to act on a page that is outside the policy (a delayed redirect or meta refresh can land
    /// somewhere forbidden between tool calls).
    pub async fn guard(&self) -> Result<()> {
        let info = self.info().await?;
        if let Err(msg) = self.policy.check_resolved(&info.url).await {
            let _ = self
                .session
                .call("Page.navigate", json!({ "url": "about:blank" }))
                .await;
            return Err(BrowserError::Policy(format!(
                "page is at {}: {msg}",
                info.url
            )));
        }
        Ok(())
    }

    /// True when `url` is the page the last snapshot described (ignoring query and hash), so a delta is meaningful.
    pub fn baseline_matches(&self, url: &str) -> bool {
        let st = self.state.lock().unwrap();
        !st.prev_lines.is_empty() && page_key(&st.prev_url) == page_key(url)
    }

    pub async fn snapshot(&self, opts: SnapshotOpts) -> Result<SnapshotOut> {
        self.guard().await?;
        let o = json!({
            "interactiveOnly": opts.mode == SnapshotMode::Interactive,
            "collapseFooter": opts.mode != SnapshotMode::Full,
            "ref": opts.scope,
            "maxChars": opts.max_chars.unwrap_or(20000),
        });
        let v = self.rt("snapshot", &[o]).await?;
        let lines: Vec<String> = v["lines"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|l| l.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let url = v["url"].as_str().unwrap_or("").to_string();
        let title = v["title"].as_str().unwrap_or("").to_string();
        let truncated = v["truncated"].as_bool().unwrap_or(false);

        // Interactive views leave out text, so a later delta (which includes text) would have nothing to
        // compare with and print the whole page. Record the text-inclusive baseline silently.
        let baseline: Option<Vec<String>> = if opts.mode == SnapshotMode::Interactive
            && opts.scope.is_none()
        {
            let o2 = json!({ "interactiveOnly": false, "collapseFooter": true, "maxChars": 20000 });
            self.rt("snapshot", &[o2]).await.ok().map(|v2| {
                v2["lines"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|l| l.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default()
            })
        } else {
            None
        };
        let mut st = self.state.lock().unwrap();
        // Delta also applies across URLs of the same page (query/hash differ, e.g. search results).
        let same_page = page_key(&st.prev_url) == page_key(&url) && opts.scope.is_none();
        let body = if opts.mode == SnapshotMode::Delta && same_page && !st.prev_lines.is_empty() {
            let (added, removed, unchanged) = diff_lines(&st.prev_lines, &lines);
            // Separators such as `text "|"` churn without meaning; never report them as changes.
            let meaningful = |l: &String| l.chars().any(|c| c.is_alphanumeric());
            let added: Vec<String> = added.into_iter().filter(meaningful).collect();
            let removed: Vec<String> = removed.into_iter().filter(meaningful).collect();
            let mut s = format!("(delta: {unchanged} unchanged lines omitted)\n");
            if added.is_empty() && removed.is_empty() {
                s.push_str("no changes\n");
            }
            for l in &added {
                s.push_str("+ ");
                s.push_str(l.trim_start());
                s.push('\n');
            }
            for l in removed.iter().take(15) {
                s.push_str("- ");
                s.push_str(l.trim_start());
                s.push('\n');
            }
            if removed.len() > 15 {
                s.push_str(&format!("- … and {} more removed\n", removed.len() - 15));
            }
            s
        } else {
            let mut s = lines.join("\n");
            s.push('\n');
            s
        };
        if opts.scope.is_none() && opts.mode != SnapshotMode::Interactive {
            st.prev_lines = lines.clone();
            st.prev_url = url.clone();
        } else if let Some(b) = baseline {
            st.prev_lines = b;
            st.prev_url = url.clone();
        }
        Ok(SnapshotOut {
            url,
            title,
            text: body,
            truncated,
            lines: lines.len(),
        })
    }

    // ------------------------------------------------------------------ input

    /// Cheap fingerprint of the page's markup, to tell a click that did nothing from one that updated the page.
    async fn page_sig(&self) -> Option<String> {
        self.eval(
            "(()=>{const h=document.documentElement.outerHTML;let x=0;for(let i=0;i<h.length;i++)x=(x*31+h.charCodeAt(i))|0;return location.href+'|'+h.length+'|'+x})()",
        )
        .await
        .ok()
        .and_then(|v| v.as_str().map(String::from))
    }

    pub async fn click(&self, r: &str) -> Result<ActionOutcome> {
        let before = self.page_sig().await;
        let (warn, mut out) = self
            .after(|| async {
                let info = self.rt("clickInfo", &[json!(r)]).await?;
                let mut warn = None;
                if info["disabled"].as_bool() == Some(true) {
                    return Err(BrowserError::Other(format!("{r} is disabled")));
                }
                if info["zero"].as_bool() == Some(true) || info["covered"].as_bool() == Some(true) {
                    if info["covered"].as_bool() == Some(true) {
                        warn = Some(format!(
                            "{r} was covered by {}; used a script click instead of a real pointer click",
                            info["covered_by"].as_str().unwrap_or("another element")
                        ));
                    }
                    match self.rt_effect("jsClick", &[json!(r)]).await {
                        // The click replaced the page it was running in: that is the click working.
                        Ok(_) | Err(BrowserError::Navigation(_)) => {}
                        Err(e) => return Err(e),
                    }
                } else {
                    self.mouse_click(info["x"].as_f64().unwrap_or(0.0), info["y"].as_f64().unwrap_or(0.0)).await?;
                }
                Ok(warn)
            })
            .await?;
        if let Some(w) = warn {
            out.warnings.push(w);
        }
        if !out.navigated {
            if let (Some(b), Some(a)) = (before, self.page_sig().await) {
                out.changed = b != a;
            }
        }
        Ok(out)
    }

    async fn mouse_click(&self, x: f64, y: f64) -> Result<()> {
        let s = &self.session;
        s.call(
            "Input.dispatchMouseEvent",
            json!({ "type": "mouseMoved", "x": x, "y": y }),
        )
        .await?;
        s.call(
            "Input.dispatchMouseEvent",
            json!({ "type": "mousePressed", "x": x, "y": y, "button": "left", "buttons": 1, "clickCount": 1 }),
        )
        .await?;
        s.call(
            "Input.dispatchMouseEvent",
            json!({ "type": "mouseReleased", "x": x, "y": y, "button": "left", "buttons": 0, "clickCount": 1 }),
        )
        .await?;
        Ok(())
    }

    /// Focus an element and replace its contents with `text`, as a user typing would.
    pub async fn type_into(&self, r: &str, text: &str) -> Result<()> {
        let p = self.rt("prepare", &[json!(r)]).await?;
        if p["disabled"].as_bool() == Some(true) {
            return Err(BrowserError::Other(format!("{r} is disabled")));
        }
        if p["readonly"].as_bool() == Some(true) {
            return Err(BrowserError::Other(format!("{r} is read-only")));
        }
        if text.is_empty() {
            self.rt_effect("setNative", &[json!(r), json!("")]).await?;
        } else {
            self.session
                .call("Input.insertText", json!({ "text": text }))
                .await?;
        }
        Ok(())
    }

    pub async fn type_text(&self, r: &str, text: &str, submit: bool) -> Result<ActionOutcome> {
        let (_, out) = self
            .after(|| async {
                self.type_into(r, text).await?;
                if submit {
                    self.press_raw("Enter").await?;
                } else {
                    self.rt_effect("finish", &[json!(r)]).await?;
                }
                Ok(())
            })
            .await?;
        Ok(out)
    }

    pub async fn press(&self, key: &str) -> Result<ActionOutcome> {
        let (_, out) = self.after(|| async { self.press_raw(key).await }).await?;
        Ok(out)
    }

    async fn press_raw(&self, combo: &str) -> Result<()> {
        let parts: Vec<&str> = combo.split('+').collect();
        let (key, mods) = parts
            .split_last()
            .ok_or_else(|| BrowserError::Other("empty key".into()))?;
        let mut modifiers = 0;
        for m in mods {
            modifiers |= match m.to_lowercase().as_str() {
                "alt" => 1,
                "control" | "ctrl" => 2,
                "meta" | "cmd" | "command" => 4,
                "shift" => 8,
                other => return Err(BrowserError::Other(format!("unknown modifier {other}"))),
            };
        }
        let (name, code, vk, text): (String, String, u32, Option<String>) =
            match key.to_lowercase().as_str() {
                "enter" | "return" => ("Enter".into(), "Enter".into(), 13, Some("\r".into())),
                "tab" => ("Tab".into(), "Tab".into(), 9, None),
                "escape" | "esc" => ("Escape".into(), "Escape".into(), 27, None),
                "backspace" => ("Backspace".into(), "Backspace".into(), 8, None),
                "delete" => ("Delete".into(), "Delete".into(), 46, None),
                "arrowdown" | "down" => ("ArrowDown".into(), "ArrowDown".into(), 40, None),
                "arrowup" | "up" => ("ArrowUp".into(), "ArrowUp".into(), 38, None),
                "arrowleft" | "left" => ("ArrowLeft".into(), "ArrowLeft".into(), 37, None),
                "arrowright" | "right" => ("ArrowRight".into(), "ArrowRight".into(), 39, None),
                "home" => ("Home".into(), "Home".into(), 36, None),
                "end" => ("End".into(), "End".into(), 35, None),
                "pageup" => ("PageUp".into(), "PageUp".into(), 33, None),
                "pagedown" => ("PageDown".into(), "PageDown".into(), 34, None),
                "space" | " " => (" ".into(), "Space".into(), 32, Some(" ".into())),
                k if k.chars().count() == 1 => {
                    let c = key.chars().next().unwrap();
                    let up = c.to_ascii_uppercase();
                    (
                        c.to_string(),
                        format!("Key{up}"),
                        up as u32,
                        Some(c.to_string()),
                    )
                }
                other => return Err(BrowserError::Other(format!("unsupported key {other:?}"))),
            };
        let printable = modifiers & 0b0110 == 0;
        let mut down = json!({
            "type": if text.is_some() && printable { "keyDown" } else { "rawKeyDown" },
            "key": name, "code": code, "windowsVirtualKeyCode": vk, "modifiers": modifiers,
        });
        if let (Some(t), true) = (&text, printable) {
            down["text"] = json!(t);
        }
        self.session.call("Input.dispatchKeyEvent", down).await?;
        self.session
            .call(
                "Input.dispatchKeyEvent",
                json!({ "type": "keyUp", "key": name, "code": code, "windowsVirtualKeyCode": vk, "modifiers": modifiers }),
            )
            .await?;
        Ok(())
    }

    pub async fn upload(&self, r: &str, paths: &[String]) -> Result<()> {
        // Only files inside a configured --upload-dir, and the browser gets the resolved path, so a symlink
        // swapped in afterwards cannot redirect it.
        let mut real = Vec::with_capacity(paths.len());
        for p in paths {
            real.push(self.policy.check_upload(p).map_err(BrowserError::Policy)?);
        }
        let obj = self
            .session
            .call(
                "Runtime.evaluate",
                json!({ "expression": format!("__wm.el({})", json!(r)), "returnByValue": false }),
            )
            .await?;
        let object_id = obj["result"]["objectId"]
            .as_str()
            .ok_or_else(|| BrowserError::Other(format!("could not resolve {r}")))?;
        self.session
            .call(
                "DOM.setFileInputFiles",
                json!({ "files": real, "objectId": object_id }),
            )
            .await?;
        Ok(())
    }

    pub async fn hover(&self, r: &str) -> Result<()> {
        let info = self.rt("clickInfo", &[json!(r)]).await?;
        self.session
            .call(
                "Input.dispatchMouseEvent",
                json!({ "type": "mouseMoved", "x": info["x"], "y": info["y"] }),
            )
            .await?;
        self.settle(800, 80).await;
        Ok(())
    }

    // ------------------------------------------------------------------ waiting / screenshots

    pub async fn wait_for(
        &self,
        text: Option<&str>,
        text_gone: Option<&str>,
        selector: Option<&str>,
        timeout: Duration,
    ) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        loop {
            let mut ok = true;
            if let Some(t) = text {
                ok &= self
                    .rt("textPresent", &[json!(t)])
                    .await?
                    .as_bool()
                    .unwrap_or(false);
            }
            if let Some(t) = text_gone {
                ok &= !self
                    .rt("textPresent", &[json!(t)])
                    .await?
                    .as_bool()
                    .unwrap_or(false);
            }
            if let Some(s) = selector {
                ok &= self
                    .rt("selectorPresent", &[json!(s)])
                    .await?
                    .as_bool()
                    .unwrap_or(false);
            }
            if ok {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            sleep(Duration::from_millis(100)).await;
        }
    }

    pub async fn screenshot(
        &self,
        scope: Option<&str>,
        full_page: bool,
        jpeg: bool,
    ) -> Result<Vec<u8>> {
        self.guard().await?;
        // A capture needs a rendered frame, and Chrome does not produce one for a tab that is not in front
        // (a popup opened by the page, or another tab the agent switched from, takes the front). Without
        // this a full-page capture of a tall page waits out the 30 s timeout.
        let _ = self.session.call("Page.bringToFront", json!({})).await;
        let mut params = json!({ "format": if jpeg { "jpeg" } else { "png" } });
        if jpeg {
            params["quality"] = json!(70);
        }
        if let Some(r) = scope {
            let rect = self.rt("rect", &[json!(r)]).await?;
            params["clip"] = json!({ "x": rect["x"], "y": rect["y"], "width": rect["width"], "height": rect["height"], "scale": 1 });
            params["captureBeyondViewport"] = json!(true);
        } else if full_page {
            let m = self
                .session
                .call("Page.getLayoutMetrics", json!({}))
                .await?;
            let sz = &m["cssContentSize"];
            params["clip"] =
                json!({ "x": 0, "y": 0, "width": sz["width"], "height": sz["height"], "scale": 1 });
            params["captureBeyondViewport"] = json!(true);
        }
        let res = self.session.call("Page.captureScreenshot", params).await?;
        let data = res["data"]
            .as_str()
            .ok_or_else(|| BrowserError::Other("no screenshot data".into()))?;
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| BrowserError::Other(e.to_string()))
    }

    /// "Where to click": a viewport screenshot with every clickable thing boxed and labelled by its
    /// ref, plus the same list as data. Marks are drawn from DOM geometry and removed afterwards.
    pub async fn look(&self, max: usize) -> Result<(Vec<u8>, Value)> {
        self.guard().await?;
        let list = self.rt("targets", &[json!(max)]).await?;
        self.rt("mark", std::slice::from_ref(&list)).await?;
        let shot = self.screenshot(None, false, false).await;
        let _ = self.rt("unmark", &[]).await;
        Ok((shot?, list))
    }

    // ------------------------------------------------------------------ forms

    pub async fn forms(&self) -> Result<FormIr> {
        let v = self.rt("forms", &[]).await?;
        serde_json::from_value(v)
            .map_err(|e| BrowserError::Other(format!("could not read form model: {e}")))
    }

    pub async fn field(&self, r: &str) -> Result<ostler_forms::Field> {
        let v = self.rt("field", &[json!(r)]).await?;
        serde_json::from_value(v)
            .map_err(|e| BrowserError::Other(format!("could not read field: {e}")))
    }

    pub async fn click_submit(&self, c: &SubmitControl) -> Result<ActionOutcome> {
        self.click(&c.r#ref).await
    }
}

/// Multiset line diff: (added, removed, unchanged count).
/// URL without query and fragment.
fn page_key(url: &str) -> &str {
    url.split(['?', '#']).next().unwrap_or(url)
}

/// Line identity for diffing: ref numbers are dropped so renumbering is not a change.
fn line_key(l: &str) -> String {
    let mut out = String::with_capacity(l.len());
    let mut rest = l;
    while let Some(i) = rest.find(" [e") {
        let tail = &rest[i + 3..];
        let n = tail.bytes().take_while(u8::is_ascii_digit).count();
        if n > 0 && tail[n..].starts_with(']') {
            out.push_str(&rest[..i]);
            rest = &tail[n + 1..];
        } else {
            out.push_str(&rest[..i + 3]);
            rest = tail;
        }
    }
    out.push_str(rest);
    out
}

fn diff_lines(prev: &[String], next: &[String]) -> (Vec<String>, Vec<String>, usize) {
    use std::collections::HashMap;
    let mut counts: HashMap<String, i64> = HashMap::new();
    for l in prev {
        *counts.entry(line_key(l)).or_default() += 1;
    }
    let mut added = vec![];
    let mut unchanged = 0;
    for l in next {
        match counts.get_mut(&line_key(l)) {
            Some(c) if *c > 0 => {
                *c -= 1;
                unchanged += 1;
            }
            _ => added.push(l.clone()),
        }
    }
    let mut removed = vec![];
    for l in prev {
        if let Some(c) = counts.get_mut(&line_key(l)) {
            if *c > 0 {
                *c -= 1;
                removed.push(l.clone());
            }
        }
    }
    (added, removed, unchanged)
}

/// Turn a raw Chromium `net::ERR_*` code into something an agent can act on.
fn friendly_net_error(err: &str, url: &str) -> String {
    let host = url
        .split("://")
        .nth(1)
        .and_then(|r| r.split(['/', '?', '#']).next())
        .unwrap_or(url);
    let hint = match err.trim_start_matches("net::") {
        "ERR_NAME_NOT_RESOLVED" => format!(
            "{host} does not resolve: the domain is misspelled, does not exist, or DNS is unreachable. Check the spelling or search for the site instead"
        ),
        e if e.starts_with("ERR_CERT_") || e.starts_with("ERR_SSL_") => format!(
            "{host} has a certificate/TLS problem (expired, self-signed, wrong host or untrusted). Ostler does not click through certificate warnings; tell the user or use a different source"
        ),
        "ERR_HTTP_RESPONSE_CODE_FAILURE" => format!(
            "{host} answered with an HTTP error status (4xx/5xx) and no page body. Often the page does not exist (404) or the site is refusing automated browsers (403/429). Try the site's homepage or another source"
        ),
        "ERR_ABORTED" => format!(
            "navigation to {host} was aborted. This is what a file download looks like (Ostler does not save downloads), or the page replaced itself with another navigation. Try snapshot to see where the tab is now"
        ),
        "ERR_CONNECTION_REFUSED" => format!("{host} refused the connection: nothing is listening there or the port is wrong"),
        "ERR_CONNECTION_TIMED_OUT" | "ERR_TIMED_OUT" => format!("{host} did not answer in time: it is down, slow, or blocking this network"),
        "ERR_CONNECTION_RESET" | "ERR_CONNECTION_CLOSED" | "ERR_EMPTY_RESPONSE" => format!(
            "{host} closed the connection without answering. Sites that block bots often do this; try another source"
        ),
        "ERR_TOO_MANY_REDIRECTS" => format!("{host} redirects in a loop. Cookies may be blocked, or the site rejects this browser"),
        "ERR_BLOCKED_BY_CLIENT" | "ERR_BLOCKED_BY_RESPONSE" => format!("{host} was blocked (by policy or by the site's response headers)"),
        "ERR_INTERNET_DISCONNECTED" | "ERR_NETWORK_CHANGED" => "no network connection".to_string(),
        _ => String::new(),
    };
    hint
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_detects_changes() {
        let a: Vec<String> = ["x", "y", "z"].iter().map(|s| s.to_string()).collect();
        let b: Vec<String> = ["x", "z", "w"].iter().map(|s| s.to_string()).collect();
        let (added, removed, same) = diff_lines(&a, &b);
        assert_eq!(added, vec!["w"]);
        assert_eq!(removed, vec!["y"]);
        assert_eq!(same, 2);
    }

    #[test]
    fn diff_handles_duplicates() {
        let a: Vec<String> = ["x", "x"].iter().map(|s| s.to_string()).collect();
        let b: Vec<String> = ["x"].iter().map(|s| s.to_string()).collect();
        let (added, removed, same) = diff_lines(&a, &b);
        assert!(added.is_empty());
        assert_eq!(removed, vec!["x"]);
        assert_eq!(same, 1);
    }
}

#[cfg(test)]
mod child_tests {
    use super::*;
    use ostler_cdp::Connection;
    use std::time::Duration;

    type Calls = Arc<Mutex<Vec<String>>>;

    fn restricted() -> Arc<Policy> {
        Arc::new(Policy {
            allowed_domains: vec!["example.com".into()],
            ..Default::default()
        })
    }

    fn attached(kind: &str) -> Value {
        json!({
            "sessionId": "cs",
            "targetInfo": { "targetId": "w1", "type": kind },
            "waitingForDebugger": true,
        })
    }

    async fn wait_for(calls: &Calls, what: &str) -> bool {
        for _ in 0..100 {
            if calls.lock().unwrap().iter().any(|c| c == what) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }

    #[tokio::test]
    async fn a_worker_or_frame_that_cannot_be_intercepted_is_never_resumed() {
        for kind in ["worker", "service_worker", "iframe", "shared_worker"] {
            for fail in [true, false] {
                let (conn, mut peer) = Connection::mock(64);
                let calls: Calls = Arc::default();
                let c = calls.clone();
                peer.serve(move |m, _, _| {
                    c.lock().unwrap().push(m.to_string());
                    if fail && m == "Fetch.enable" {
                        Err("no".into())
                    } else {
                        Ok(json!({}))
                    }
                });
                let shared: Arc<Shared> = Arc::default();
                spawn_child(&conn, &attached(kind), shared.clone(), restricted());
                if fail {
                    assert!(wait_for(&calls, "Target.closeTarget").await, "{kind}");
                    assert!(
                        !calls
                            .lock()
                            .unwrap()
                            .iter()
                            .any(|c| c == "Runtime.runIfWaitingForDebugger"),
                        "{kind} was resumed without interception"
                    );
                    assert!(shared.notices.lock().unwrap()[0].contains("left paused"));
                } else {
                    // Control: with interception in place it is released.
                    assert!(
                        wait_for(&calls, "Runtime.runIfWaitingForDebugger").await,
                        "{kind}"
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn lost_events_on_a_frame_session_are_reported() {
        let (conn, mut peer) = Connection::mock(4);
        let calls: Calls = Arc::default();
        let c = calls.clone();
        peer.serve(move |m, _, _| {
            c.lock().unwrap().push(m.to_string());
            Ok(json!({}))
        });
        let shared: Arc<Shared> = Arc::default();
        spawn_child(&conn, &attached("iframe"), shared.clone(), restricted());
        assert!(wait_for(&calls, "Runtime.runIfWaitingForDebugger").await);
        // The mock is sync-emitted, the child task cannot run in between: its buffer of 4 overflows.
        for i in 0..50 {
            peer.emit("Network.dataReceived", json!({ "i": i }), Some("cs"));
        }
        for _ in 0..100 {
            if !shared.notices.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(shared.notices.lock().unwrap()[0].contains("events were lost"));
    }
}
