//! Browser control for agents: launch Chrome, drive pages, and fill forms reliably.

mod fill;
mod page;
mod policy;

pub use fill::{
    AutoMode, FieldDrift, FieldError, FieldResult, FillReport, FillRequest, FillStatus, MissingField,
    SubmitOutcome, SubmitStatus, Suggester, Suggestion,
};
pub use ostler_cdp::LaunchOptions;
pub use page::{
    ActionOutcome, BrowserError, Challenge, Page, PageInfo, SnapshotMode, SnapshotOpts, SnapshotOut,
};
pub use policy::Policy;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use ostler_cdp::{Chrome, Connection, Session};
use serde::Serialize;

struct Tab {
    id: String,
    page: Page,
}

#[derive(Default)]
struct Tabs {
    list: Vec<Tab>,
    active: usize,
    next: u32,
}

impl Tabs {
    fn push(&mut self, page: Page) -> String {
        self.next += 1;
        let id = format!("t{}", self.next);
        self.list.push(Tab {
            id: id.clone(),
            page,
        });
        id
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
}

impl TabSetup {
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
        };
        let first = setup
            .page_for(Session::new_page(conn.clone()).await?)
            .await?;
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
    /// instead of loading unseen and unchecked.
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
            "Target.setDiscoverTargets",
            serde_json::json!({ "discover": true }),
            None,
            ostler_cdp::DEFAULT_TIMEOUT,
        )
        .await?;
        drop(c);
        tokio::spawn(async move {
            loop {
                let ev = match rx.recv().await {
                    Ok(ev) => ev,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                };
                if ev.method != "Target.targetCreated" {
                    continue;
                }
                let info = &ev.params["targetInfo"];
                // Only targets a page opened carry an opener; ones we create ourselves do not.
                let opened_by_page = info["type"] == "page"
                    && info["openerId"].as_str().is_some_and(|s| !s.is_empty());
                let Some(target_id) = info["targetId"].as_str().filter(|_| opened_by_page) else {
                    continue;
                };
                let Some(conn) = conn.upgrade() else { break };
                let attached = match Session::attach(conn, target_id.to_string()).await {
                    Ok(s) => setup.page_for(s).await,
                    Err(e) => Err(e.into()),
                };
                match attached {
                    Ok(page) => {
                        let id = tabs.lock().unwrap().push(page);
                        notes.lock().unwrap().push(format!(
                            "a popup opened as tab {id}; use tabs to switch to it"
                        ));
                    }
                    Err(e) => notes
                        .lock()
                        .unwrap()
                        .push(format!("a popup opened but could not be attached: {e}")),
                }
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
            .page_for(Session::new_page(self.conn.clone()).await?)
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
