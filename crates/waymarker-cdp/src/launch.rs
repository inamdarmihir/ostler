use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use crate::{CdpError, Connection, Result};

#[derive(Debug, Clone)]
pub struct LaunchOptions {
    pub chrome_path: Option<PathBuf>,
    pub headless: bool,
    /// Persistent profile directory. `None` uses a throwaway temp profile.
    pub user_data_dir: Option<PathBuf>,
    pub window_size: (u32, u32),
    pub extra_args: Vec<String>,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        Self {
            chrome_path: None,
            headless: true,
            user_data_dir: None,
            window_size: (1280, 900),
            extra_args: vec![],
        }
    }
}

/// Locate a Chrome/Chromium executable.
pub fn find_chrome(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return if p.exists() {
            Ok(p.to_path_buf())
        } else {
            Err(CdpError::ChromeNotFound)
        };
    }
    if let Ok(p) = std::env::var("WAYMARKER_CHROME") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Ok(p);
        }
    }
    let fixed = [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/snap/bin/chromium",
        "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
        "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe",
    ];
    for p in fixed {
        if Path::new(p).exists() {
            return Ok(PathBuf::from(p));
        }
    }
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            for name in ["google-chrome", "chromium", "chromium-browser", "chrome"] {
                let cand = dir.join(name);
                if cand.exists() {
                    return Ok(cand);
                }
            }
        }
    }
    Err(CdpError::ChromeNotFound)
}

/// A running browser process. Killed on drop.
pub struct Chrome {
    child: Child,
    pub ws_url: String,
    _profile: Option<tempfile::TempDir>,
}

impl Chrome {
    pub async fn launch(opts: LaunchOptions) -> Result<Self> {
        let exe = find_chrome(opts.chrome_path.as_deref())?;
        let (profile_dir, temp) = match &opts.user_data_dir {
            Some(p) => (p.clone(), None),
            None => {
                let t = tempfile::tempdir().map_err(|e| CdpError::Launch(e.to_string()))?;
                (t.path().to_path_buf(), Some(t))
            }
        };
        let mut cmd = Command::new(exe);
        cmd.arg("--remote-debugging-port=0")
            .arg(format!("--user-data-dir={}", profile_dir.display()))
            .arg(format!(
                "--window-size={},{}",
                opts.window_size.0, opts.window_size.1
            ))
            .args([
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-extensions",
                "--disable-background-timer-throttling",
                "--disable-backgrounding-occluded-windows",
                "--disable-renderer-backgrounding",
                "--disable-popup-blocking",
                "--mute-audio",
                "--hide-scrollbars",
                "--password-store=basic",
                "--use-mock-keychain",
            ]);
        if opts.headless {
            cmd.arg("--headless=new");
        }
        if std::env::var("WAYMARKER_NO_SANDBOX").is_ok() {
            cmd.args(["--no-sandbox", "--disable-setuid-sandbox"]);
        }
        cmd.args(&opts.extra_args)
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd.spawn().map_err(|e| CdpError::Launch(e.to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| CdpError::Launch("no stderr".into()))?;
        let mut lines = BufReader::new(stderr).lines();

        let find = async {
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(rest) = line.split("DevTools listening on ").nth(1) {
                    return Some(rest.trim().to_string());
                }
            }
            None
        };
        let ws_url = match tokio::time::timeout(Duration::from_secs(20), find).await {
            Ok(Some(url)) => url,
            Ok(None) => {
                return Err(CdpError::Launch(
                    "browser exited before DevTools was ready".into(),
                ))
            }
            Err(_) => {
                return Err(CdpError::Launch(
                    "timed out waiting for DevTools endpoint".into(),
                ))
            }
        };
        // Keep draining stderr so the pipe never fills and blocks the browser.
        tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });

        Ok(Self {
            child,
            ws_url,
            _profile: temp,
        })
    }

    pub async fn connect(&self) -> Result<std::sync::Arc<Connection>> {
        Connection::connect(&self.ws_url).await
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    pub async fn close(mut self) {
        let _ = self.child.kill().await;
    }
}

/// Resident memory (KiB) of a process and all its descendants. Unix only; `None` elsewhere.
pub fn process_tree_rss_kb(root: u32) -> Option<u64> {
    let out = std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,rss="])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let rows: Vec<(u32, u32, u64)> = text
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            Some((
                it.next()?.parse().ok()?,
                it.next()?.parse().ok()?,
                it.next()?.parse().ok()?,
            ))
        })
        .collect();
    let mut members = std::collections::HashSet::from([root]);
    loop {
        let before = members.len();
        for (pid, ppid, _) in &rows {
            if members.contains(ppid) {
                members.insert(*pid);
            }
        }
        if members.len() == before {
            break;
        }
    }
    Some(
        rows.iter()
            .filter(|(p, _, _)| members.contains(p))
            .map(|(_, _, r)| r)
            .sum(),
    )
}
