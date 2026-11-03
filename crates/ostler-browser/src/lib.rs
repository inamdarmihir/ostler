//! Browser control for agents: launch Chrome, drive pages, and fill forms reliably.

mod fill;
mod page;
mod policy;

pub use fill::{
    AutoMode, FieldDrift, FieldError, FieldResult, FillReport, FillRequest, FillStatus,
    MissingField, SubmitOutcome, SubmitStatus, Suggester, Suggestion,
};
pub use ostler_cdp::LaunchOptions;
pub use page::{
    ActionOutcome, BrowserError, Challenge, Page, PageInfo, SnapshotMode, SnapshotOpts, SnapshotOut,
};
pub use policy::Policy;

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use ostler_cdp::{Chrome, Connection, Session};
use serde::Serialize;

struct Tab {
    id: String,
    page: Page,
    /// Opened by a page (adopted), as opposed to created by us. Only these are capped and reaped.
    popup: bool,
}

/// Popups kept at once (adopted or being set up). A page that opens windows in a loop gets no more than
/// this; the rest are closed unseen.
const MAX_POPUPS: usize = 8;

#[derive(Default)]
struct Tabs {
    list: Vec<Tab>,
    active: usize,
    next: u32,
    /// Popups Chrome announced that are being set up: they count against the cap.
    pending: HashSet<String>,
}

impl Tabs {
    fn push(&mut self, page: Page) -> String {
        self.push_as(page, false)
    }

    fn push_as(&mut self, page: Page, popup: bool) -> String {
        self.next += 1;
        let id = format!("t{}", self.next);
        self.list.push(Tab {
            id: id.clone(),
            page,
            popup,
        });
        id
    }

    /// Claim a popup slot for `tid`; false when the cap is reached.
    fn reserve(&mut self, tid: &str) -> bool {
        let live = self.list.iter().filter(|t| t.popup).count() + self.pending.len();
        live < MAX_POPUPS && self.pending.insert(tid.to_string())
    }

    /// Turn a reservation into a tab. None when the target went away while it was being set up.
    fn adopt(&mut self, tid: &str, page: Page) -> Option<String> {
        self.pending.remove(tid).then(|| self.push_as(page, true))
    }

    fn knows(&self, tid: &str) -> bool {
        self.pending.contains(tid) || self.list.iter().any(|t| t.page.target_id() == tid)
    }

    /// Forget a popup that closed (its reservation, or its tab). Our own tabs are only closed by `close_tab`.
    fn reap(&mut self, tid: &str) -> bool {
        if self.pending.remove(tid) {
            return true;
        }
        let Some(idx) = self
            .list
            .iter()
            .position(|t| t.popup && t.page.target_id() == tid)
        else {
            return false;
        };
        self.list.remove(idx);
        if idx < self.active {
            self.active -= 1;
        } else if idx == self.active {
            self.active = 0;
        }
        true
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TabInfo {
    pub id: String,
    pub url: String,
    pub title: String,
    pub active: bool,
}

/// State every tab needs in order to be created identically (so a popup obeys the same rules as the first page).
#[derive(Clone)]
struct TabSetup {
    policy: Policy,
    lean: bool,
    gate: Arc<AtomicBool>,
    /// Targets this process created itself (as opposed to ones a page opened).
    own: Arc<Mutex<HashSet<String>>>,
}

impl TabSetup {
    /// Create a blank tab of our own and attach to it.
    async fn new_session(&self, conn: &Arc<Connection>) -> page::Result<Session> {
        let created = conn
            .call(
                "Target.createTarget",
                serde_json::json!({ "url": "about:blank" }),
                None,
                ostler_cdp::DEFAULT_TIMEOUT,
            )
            .await?;
        let id = created["targetId"]
            .as_str()
            .ok_or_else(|| BrowserError::Other("no targetId".into()))?
            .to_string();
        self.own.lock().unwrap().insert(id.clone());
        Ok(Session::attach(conn.clone(), id).await?)
    }

    /// A popup Chrome is holding at start: interception is on before it is released.
    async fn held_page_for(&self, session: Session, inherit_armed: bool) -> page::Result<Page> {
        let page = Page::attach_held(
            session,
            self.policy.clone(),
            self.gate.load(Ordering::SeqCst),
            inherit_armed,
        )
        .await?;
        if self.lean {
            page.block_media().await?;
        }
        Ok(page)
    }

    async fn page_for(&self, session: Session) -> page::Result<Page> {
        let page = Page::attach(session, self.policy.clone()).await?;
        if self.lean {
            page.block_media().await?;
        }
        if self.gate.load(Ordering::SeqCst) {
            page.set_submit_gate(true).await?;
        }
        Ok(page)
    }
}

/// A running browser. All tabs live in one browser context, so cookies and storage are shared between them.
pub struct Browser {
    chrome: Chrome,
    conn: Arc<Connection>,
    setup: TabSetup,
    tabs: Arc<Mutex<Tabs>>,
    notes: Arc<Mutex<Vec<String>>>,
}

impl Browser {
    pub async fn launch(mut opts: LaunchOptions, policy: Policy) -> page::Result<Self> {
        let lean = opts.lean;
        if let Some(rules) = policy.host_resolver_rules() {
            opts.extra_args
                .push(format!("--host-resolver-rules={rules}"));
        }
        let chrome = Chrome::launch(opts).await?;
        let conn = chrome.connect().await?;
        let setup = TabSetup {
            policy,
            lean,
            gate: Arc::default(),
            own: Arc::default(),
        };
        let first = setup.page_for(setup.new_session(&conn).await?).await?;
        let tabs = Arc::new(Mutex::new(Tabs::default()));
        tabs.lock().unwrap().push(first);
        let notes = Arc::new(Mutex::new(Vec::new()));
        Self::watch_popups(
            Arc::downgrade(&conn),
            setup.clone(),
            tabs.clone(),
            notes.clone(),
        )
        .await?;
        Ok(Self {
            chrome,
            conn,
            setup,
            tabs,
            notes,
        })
    }

    /// Pages opened by the page itself (`window.open`, `target=_blank`) become tabs under the same policy,
    /// instead of loading unseen and unchecked. Chrome is told to hold every new page at start until a
    /// debugger has attached, so interception (allowlist, submit gate) is on before its first request.
    /// At most `MAX_POPUPS` are kept; extra ones are closed without being adopted, and tabs that close are reaped.
    async fn watch_popups(
        conn: Weak<Connection>,
        setup: TabSetup,
        tabs: Arc<Mutex<Tabs>>,
        notes: Arc<Mutex<Vec<String>>>,
    ) -> page::Result<()> {
        let Some(c) = conn.upgrade() else {
            return Ok(());
        };
        let mut rx = c.subscribe();
        c.call(
            "Target.setAutoAttach",
            serde_json::json!({
                "autoAttach": true,
                "waitForDebuggerOnStart": true,
                "flatten": true,
                "filter": [{ "type": "page" }, { "exclude": true }],
            }),
            None,
            ostler_cdp::DEFAULT_TIMEOUT,
        )
        .await?;
        // `targetDestroyed` is only sent to those who asked to discover targets.
        c.call(
            "Target.setDiscoverTargets",
            serde_json::json!({ "discover": true }),
            None,
            ostler_cdp::DEFAULT_TIMEOUT,
        )
        .await?;
        // Pages that exist now are not ours to manage; a re-sync never touches them.
        let baseline: Arc<HashSet<String>> = Arc::new(
            page_targets(&c)
                .await
                .unwrap_or_default()
                .into_iter()
                .collect(),
        );
        drop(c);
        let resyncing = Arc::new(AtomicBool::new(false));
        tokio::spawn(async move {
            loop {
                let ev = match rx.recv().await {
                    Ok(ev) => ev,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        // A lost `attachedToTarget` leaves a popup held forever, a lost `targetDestroyed` a
                        // dead tab listed. Compare with what the browser has, off the event loop.
                        if !resyncing.swap(true, Ordering::SeqCst) {
                            let Some(conn) = conn.upgrade() else { break };
                            let (setup, tabs, notes) = (setup.clone(), tabs.clone(), notes.clone());
                            let (baseline, resyncing) = (baseline.clone(), resyncing.clone());
                            tokio::spawn(async move {
                                resync(&conn, &setup, &tabs, &notes, &baseline).await;
                                resyncing.store(false, Ordering::SeqCst);
                            });
                        }
                        continue;
                    }
                    Err(_) => break,
                };
                if ev.session_id.is_some() {
                    continue;
                }
                if ev.method == "Target.targetDestroyed" {
                    if let Some(tid) = ev.params["targetId"].as_str() {
                        tabs.lock().unwrap().reap(tid);
                    }
                    continue;
                }
                // Browser-level events only; frames of a page are handled on that page's own session.
                if ev.method != "Target.attachedToTarget" {
                    continue;
                }
                let info = &ev.params["targetInfo"];
                let (Some(sid), Some(tid)) =
                    (ev.params["sessionId"].as_str(), info["targetId"].as_str())
                else {
                    continue;
                };
                // Targets that already existed are announced too, but running; only new ones are held.
                if info["type"] != "page" || ev.params["waitingForDebugger"] != true {
                    continue;
                }
                let opener = info["openerId"].as_str().filter(|s| !s.is_empty());
                let Some(conn) = conn.upgrade() else { break };
                let (sid, tid) = (sid.to_string(), tid.to_string());
                // Decided here, in event order and without waiting: a flood must not outrun the cap, and
                // the opener's armed state is read while the click that opened this window is in flight.
                let (reserved, inherit) = {
                    let mut t = tabs.lock().unwrap();
                    let inherit = opener.is_some_and(|o| {
                        t.list
                            .iter()
                            .any(|x| x.page.target_id() == o && x.page.is_armed())
                    });
                    (opener.is_some() && t.reserve(&tid), inherit)
                };
                let has_opener = opener.is_some();
                let (setup, tabs, notes) = (setup.clone(), tabs.clone(), notes.clone());
                if has_opener && !reserved {
                    tokio::spawn(async move { refuse_popup(&conn, &sid, &tid, &notes).await });
                    continue;
                }
                // One task per target: waiting on a slow one must not hold up the next popup.
                tokio::spawn(async move {
                    // A tab we created ourselves is set up by its creator; just let it run. The creator learns
                    // the id a moment after Chrome announces the target, so give it a moment. A page that
                    // is not ours and has no opener (`noopener` popups) is adopted like any other popup.
                    if !has_opener {
                        for _ in 0..40 {
                            if setup.own.lock().unwrap().contains(&tid) {
                                break;
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                        }
                        if setup.own.lock().unwrap().contains(&tid) {
                            let _ = conn
                                .call(
                                    "Runtime.runIfWaitingForDebugger",
                                    serde_json::json!({}),
                                    Some(&sid),
                                    ostler_cdp::DEFAULT_TIMEOUT,
                                )
                                .await;
                            let _ = conn
                                .call(
                                    "Target.detachFromTarget",
                                    serde_json::json!({ "sessionId": sid }),
                                    None,
                                    ostler_cdp::DEFAULT_TIMEOUT,
                                )
                                .await;
                            return;
                        }
                        if !tabs.lock().unwrap().reserve(&tid) {
                            refuse_popup(&conn, &sid, &tid, &notes).await;
                            return;
                        }
                    }
                    let session = Session::from_attached(conn.clone(), sid.clone(), tid.clone());
                    match setup.held_page_for(session, inherit).await {
                        Ok(page) => {
                            // None: it closed while being set up (the reservation is gone), nothing to list.
                            let id = tabs.lock().unwrap().adopt(&tid, page);
                            if let Some(id) = id {
                                notes.lock().unwrap().push(format!(
                                    "a popup opened as tab {id}; use tabs to switch to it"
                                ));
                            }
                        }
                        Err(e) => {
                            tabs.lock().unwrap().pending.remove(&tid);
                            // Never let a page we could not secure run: close it instead of resuming.
                            let _ = conn
                                .call(
                                    "Target.closeTarget",
                                    serde_json::json!({ "targetId": tid }),
                                    None,
                                    ostler_cdp::DEFAULT_TIMEOUT,
                                )
                                .await;
                            notes.lock().unwrap().push(format!(
                                "a popup opened but could not be secured, so it was closed: {e}"
                            ));
                        }
                    }
                });
            }
        });
        Ok(())
    }

    /// The active tab.
    pub fn page(&self) -> Page {
        let t = self.tabs.lock().unwrap();
        t.list[t.active].page.clone()
    }

    pub fn active_tab(&self) -> String {
        let t = self.tabs.lock().unwrap();
        t.list[t.active].id.clone()
    }

    /// Things that happened to tabs on their own (popups) since the last call.
    pub fn take_notes(&self) -> Vec<String> {
        std::mem::take(&mut *self.notes.lock().unwrap())
    }

    /// Wait up to `max` for a note (a popup opening) and take everything pending.
    pub async fn wait_notes(&self, max: std::time::Duration) -> Vec<String> {
        let end = tokio::time::Instant::now() + max;
        loop {
            let n = self.take_notes();
            if !n.is_empty() || tokio::time::Instant::now() >= end {
                return n;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }

    pub async fn tabs(&self) -> Vec<TabInfo> {
        let snapshot: Vec<(String, Page, bool)> = {
            let t = self.tabs.lock().unwrap();
            t.list
                .iter()
                .enumerate()
                .map(|(i, tab)| (tab.id.clone(), tab.page.clone(), i == t.active))
                .collect()
        };
        let mut out = Vec::with_capacity(snapshot.len());
        for (id, page, active) in snapshot {
            let info = page.info().await.ok();
            out.push(TabInfo {
                id,
                url: info.as_ref().map(|i| i.url.clone()).unwrap_or_default(),
                title: info.map(|i| i.title).unwrap_or_default(),
                active,
            });
        }
        out
    }

    /// Open a new tab, make it active, and optionally load `url` (checked against the policy first).
    pub async fn new_tab(&self, url: Option<&str>) -> page::Result<String> {
        if let Some(u) = url {
            self.setup.policy.check(u).map_err(BrowserError::Policy)?;
        }
        let page = self
            .setup
            .page_for(self.setup.new_session(&self.conn).await?)
            .await?;
        let id = {
            let mut t = self.tabs.lock().unwrap();
            let id = t.push(page.clone());
            t.active = t.list.len() - 1;
            id
        };
        if let Some(u) = url {
            page.navigate(u).await?;
        }
        Ok(id)
    }

    pub async fn switch_tab(&self, id: &str) -> page::Result<()> {
        let target = {
            let mut t = self.tabs.lock().unwrap();
            let idx = t
                .list
                .iter()
                .position(|x| x.id == id)
                .ok_or_else(|| BrowserError::Other(format!("no tab {id:?}")))?;
            t.active = idx;
            t.list[idx].page.target_id().to_string()
        };
        // Bring it to the front so focus-dependent page behaviour matches what the agent now sees.
        self.conn
            .call(
                "Target.activateTarget",
                serde_json::json!({ "targetId": target }),
                None,
                ostler_cdp::DEFAULT_TIMEOUT,
            )
            .await?;
        Ok(())
    }

    /// Close a tab. Closing the last one leaves a fresh blank tab so there is always a page to act on.
    pub async fn close_tab(&self, id: &str) -> page::Result<()> {
        let (target, was_last) = {
            let mut t = self.tabs.lock().unwrap();
            let idx = t
                .list
                .iter()
                .position(|x| x.id == id)
                .ok_or_else(|| BrowserError::Other(format!("no tab {id:?}")))?;
            let tab = t.list.remove(idx);
            if t.active >= idx && t.active > 0 {
                t.active -= 1;
            }
            (tab.page.target_id().to_string(), t.list.is_empty())
        };
        let _ = self
            .conn
            .call(
                "Target.closeTarget",
                serde_json::json!({ "targetId": target }),
                None,
                ostler_cdp::DEFAULT_TIMEOUT,
            )
            .await;
        if was_last {
            self.new_tab(None).await?;
        }
        Ok(())
    }

    /// Apply `--require-confirm-submit` to every tab, including ones opened later.
    pub async fn set_submit_gate(&self, on: bool) -> page::Result<()> {
        self.setup.gate.store(on, Ordering::SeqCst);
        let pages: Vec<Page> = self
            .tabs
            .lock()
            .unwrap()
            .list
            .iter()
            .map(|t| t.page.clone())
            .collect();
        for p in pages {
            p.set_submit_gate(on).await?;
        }
        Ok(())
    }

    /// CDP round-trips so far: a proxy for browser-side work per task.
    pub fn cdp_calls(&self) -> u64 {
        self.conn.call_count()
    }

    /// False once the connection to Chrome has dropped; the owner should relaunch.
    pub fn is_alive(&self) -> bool {
        !self.conn.is_closed()
    }

    pub fn pid(&self) -> Option<u32> {
        self.chrome.pid()
    }

    /// Resident memory of Chrome and all its helper processes, in KiB (Unix only).
    pub fn rss_kb(&self) -> Option<u64> {
        self.chrome.pid().and_then(ostler_cdp::process_tree_rss_kb)
    }

    pub async fn close(self) {
        self.chrome.close().await;
    }
}

/// Ids of all page targets the browser has.
async fn page_targets(conn: &Connection) -> Option<Vec<String>> {
    let r = conn
        .call(
            "Target.getTargets",
            serde_json::json!({}),
            None,
            ostler_cdp::DEFAULT_TIMEOUT,
        )
        .await
        .ok()?;
    Some(
        r["targetInfos"]
            .as_array()?
            .iter()
            .filter(|t| t["type"] == "page")
            .filter_map(|t| t["targetId"].as_str().map(String::from))
            .collect(),
    )
}

/// Close a popup beyond the cap without adopting it. A held page cannot be closed until it runs (its renderer
/// has to answer, and the opener's `window.open` is waiting on it), so interception that never answers is put
/// on first: whatever it requests stays paused and nothing reaches the network before it is closed.
async fn refuse_popup(conn: &Connection, sid: &str, tid: &str, notes: &Mutex<Vec<String>>) {
    let t = ostler_cdp::DEFAULT_TIMEOUT;
    let held = conn
        .call(
            "Fetch.enable",
            serde_json::json!({ "patterns": [{ "urlPattern": "*", "requestStage": "Request" }] }),
            Some(sid),
            t,
        )
        .await;
    // If interception could not be set up the page stays held (closing it is best effort below).
    if held.is_ok() {
        let _ = conn
            .call(
                "Runtime.runIfWaitingForDebugger",
                serde_json::json!({}),
                Some(sid),
                t,
            )
            .await;
    }
    let _ = conn
        .call(
            "Target.closeTarget",
            serde_json::json!({ "targetId": tid }),
            None,
            t,
        )
        .await;
    let _ = conn
        .call(
            "Target.detachFromTarget",
            serde_json::json!({ "sessionId": sid }),
            None,
            t,
        )
        .await;
    let note = format!("popup limit reached ({MAX_POPUPS} open): further popups are closed unseen");
    let mut n = notes.lock().unwrap();
    if !n.contains(&note) {
        n.push(note);
    }
}

/// After lost events: forget popups that no longer exist, and close any new page nobody adopted (a popup
/// whose announcement was lost is still held by Chrome and would never run). Anything found unknown is
/// confirmed twice, a moment apart, so a page that is merely about to be registered is left alone.
async fn resync(
    conn: &Connection,
    setup: &TabSetup,
    tabs: &Mutex<Tabs>,
    notes: &Mutex<Vec<String>>,
    baseline: &HashSet<String>,
) {
    let settle = std::time::Duration::from_millis(300);
    tokio::time::sleep(settle).await;
    let Some(now) = page_targets(conn).await else {
        return;
    };
    {
        let alive: HashSet<&String> = now.iter().collect();
        let mut t = tabs.lock().unwrap();
        let gone: Vec<String> = t
            .list
            .iter()
            .filter(|x| x.popup && !alive.contains(&x.page.target_id().to_string()))
            .map(|x| x.page.target_id().to_string())
            .chain(t.pending.iter().filter(|p| !alive.contains(p)).cloned())
            .collect();
        for g in gone {
            t.reap(&g);
        }
    }
    let unknown = |ids: &[String]| -> Vec<String> {
        let t = tabs.lock().unwrap();
        let own = setup.own.lock().unwrap();
        ids.iter()
            .filter(|i| !baseline.contains(*i) && !own.contains(*i) && !t.knows(i))
            .cloned()
            .collect()
    };
    let first = unknown(&now);
    if first.is_empty() {
        return;
    }
    tokio::time::sleep(settle).await;
    let Some(again) = page_targets(conn).await else {
        return;
    };
    for tid in unknown(&again).into_iter().filter(|i| first.contains(i)) {
        let _ = conn
            .call(
                "Target.closeTarget",
                serde_json::json!({ "targetId": tid }),
                None,
                ostler_cdp::DEFAULT_TIMEOUT,
            )
            .await;
        notes.lock().unwrap().push(
            "a popup was announced while events were being dropped, so it was closed unseen".into(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn setup() -> TabSetup {
        TabSetup {
            policy: Policy::default(),
            lean: false,
            gate: Arc::default(),
            own: Arc::default(),
        }
    }

    /// A held popup whose `attachedToTarget` was dropped is found by the re-sync and closed; pages that
    /// existed before, and ones we created, are left alone.
    #[tokio::test]
    async fn a_popup_announcement_lost_to_lag_is_closed_by_the_resync() {
        let (conn, mut peer) = ostler_cdp::Connection::mock(4);
        let targets: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(vec!["old"]));
        let closed: Arc<Mutex<Vec<String>>> = Arc::default();
        let (t, c) = (targets.clone(), closed.clone());
        peer.serve(move |m, p, _| match m {
            "Target.getTargets" => Ok(json!({ "targetInfos": t.lock().unwrap().iter()
                .map(|id| json!({ "targetId": id, "type": "page" })).collect::<Vec<Value>>() })),
            "Target.closeTarget" => {
                c.lock()
                    .unwrap()
                    .push(p["targetId"].as_str().unwrap().into());
                Ok(json!({ "success": true }))
            }
            _ => Ok(json!({})),
        });
        let s = setup();
        s.own.lock().unwrap().insert("mine".into());
        let tabs: Arc<Mutex<Tabs>> = Arc::default();
        let notes: Arc<Mutex<Vec<String>>> = Arc::default();
        Browser::watch_popups(Arc::downgrade(&conn), s, tabs, notes.clone())
            .await
            .unwrap();
        // Burst without yielding: the watcher's buffer of 4 overflows and the announcement is lost.
        peer.emit(
            "Target.attachedToTarget",
            json!({ "sessionId": "ps", "waitingForDebugger": true,
                "targetInfo": { "targetId": "lost", "type": "page", "openerId": "old" } }),
            None,
        );
        for i in 0..20 {
            peer.emit("Target.targetInfoChanged", json!({ "i": i }), None);
        }
        targets.lock().unwrap().extend(["lost", "mine"]);
        for _ in 0..100 {
            if !closed.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(*closed.lock().unwrap(), vec!["lost".to_string()]);
        assert!(notes.lock().unwrap()[0].contains("closed unseen"));
    }

    #[test]
    fn popup_slots_are_capped_and_released() {
        let mut t = Tabs::default();
        for i in 0..MAX_POPUPS {
            assert!(t.reserve(&format!("p{i}")));
        }
        assert!(!t.reserve("extra"));
        assert!(t.reap("p0"));
        assert!(t.reserve("extra"));
        assert!(!t.reap("never-heard-of"));
    }
}
