//! Browser control for agents: launch Chrome, drive a page, and fill forms reliably.

mod fill;
mod page;
mod policy;

pub use fill::{
    AutoMode, FieldError, FieldResult, FillReport, FillRequest, FillStatus, MissingField,
    SubmitOutcome, SubmitStatus, Suggester, Suggestion,
};
pub use page::{
    ActionOutcome, BrowserError, Page, PageInfo, SnapshotMode, SnapshotOpts, SnapshotOut,
};
pub use policy::Policy;
pub use waymarker_cdp::LaunchOptions;

use std::sync::Arc;

use waymarker_cdp::{Chrome, Connection, Session};

/// A running browser with one page.
pub struct Browser {
    chrome: Chrome,
    conn: Arc<Connection>,
    pub page: Page,
}

impl Browser {
    pub async fn launch(opts: LaunchOptions, policy: Policy) -> page::Result<Self> {
        let lean = opts.lean;
        let chrome = Chrome::launch(opts).await?;
        let conn = chrome.connect().await?;
        let session = Session::new_page(conn.clone()).await?;
        let page = Page::attach(session, policy).await?;
        if lean {
            page.block_media().await?;
        }
        Ok(Self { chrome, conn, page })
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
        self.chrome
            .pid()
            .and_then(waymarker_cdp::process_tree_rss_kb)
    }

    pub async fn close(self) {
        self.chrome.close().await;
    }
}
