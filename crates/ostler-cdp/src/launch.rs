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
    /// Trade rarely-needed browser features for a smaller footprint (see [`lean_args`]).
    pub lean: bool,
    /// Attach to a Chrome you started yourself with `--remote-debugging-port` instead of launching one.
    /// Loopback only (`http://127.0.0.1:9222` or a `ws://127.0.0.1:...` DevTools URL). Ostler never opens
    /// the debugging port for you, and never kills a browser it did not start.
    pub cdp_url: Option<String>,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        Self {
            chrome_path: None,
            headless: true,
            user_data_dir: None,
            window_size: (1280, 900),
            extra_args: vec![],
            lean: false,
            cdp_url: None,
        }
    }
}

/// Flags that cut memory and background work without changing page behaviour that forms depend on.
/// Site isolation stays ON: turning it off saves processes but weakens the browser's security boundary.
pub fn lean_args() -> Vec<&'static str> {
    vec![
        "--disable-gpu",
        "--disable-software-rasterizer",
        "--disable-dev-shm-usage",
        "--disable-component-update",
        "--disable-sync",
        "--disable-breakpad",
        "--disable-domain-reliability",
        "--metrics-recording-only",
        "--no-pings",
        "--disable-features=Translate,MediaRouter,OptimizationHints,AutofillServerCommunication,BackForwardCache,InterestFeedContentSuggestions,CertificateTransparencyComponentUpdater",
        "--enable-features=NetworkServiceInProcess",
        "--js-flags=--max-old-space-size=512",
    ]
}

/// Locate a Chrome/Chromium executable.
pub fn find_chrome(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return if p.exists() {
            Ok(p.to_path_buf())
        } else {
            Err(CdpError::ChromePathMissing(p.display().to_string()))
        };
    }
    // An explicit setting that is wrong is an error, not a cue to quietly use some other browser.
    if let Ok(p) = std::env::var("OSTLER_CHROME") {
        if !p.trim().is_empty() {
            let p = PathBuf::from(p);
            return if p.exists() {
                Ok(p)
            } else {
                Err(CdpError::ChromePathMissing(p.display().to_string()))
            };
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

/// How Ostler talks to Chrome. Pipe is the default: it needs no network listener, so no other
/// local user can reach the browser. WebSocket (`OSTLER_CDP_TRANSPORT=ws`) opens an
/// unauthenticated localhost debugging port and exists only as a debugging fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transport {
    #[cfg(unix)]
    Pipe,
    Ws,
}

impl Transport {
    fn from_env() -> Result<Self> {
        match std::env::var("OSTLER_CDP_TRANSPORT").as_deref() {
            Err(_) | Ok("") | Ok("pipe") => {
                #[cfg(unix)]
                return Ok(Self::Pipe);
                #[cfg(not(unix))]
                return Ok(Self::Ws);
            }
            Ok("ws") => Ok(Self::Ws),
            Ok(other) => Err(CdpError::Launch(format!(
                "unknown OSTLER_CDP_TRANSPORT {other:?}; use \"pipe\" (default) or \"ws\""
            ))),
        }
    }
}

/// A running browser process. Killed on drop.
pub struct Chrome {
    /// `None` when attached to a browser the user started: it is theirs, so we never kill it.
    child: Option<Child>,
    /// DevTools WebSocket URL; `None` on the default pipe transport (there is no port).
    pub ws_url: Option<String>,
    /// Pipe transport: the connection, opened at launch because the pipe ends can be used once.
    #[cfg(unix)]
    pipe: Option<std::sync::Arc<Connection>>,
    _profile: Option<tempfile::TempDir>,
}

impl Chrome {
    pub async fn launch(opts: LaunchOptions) -> Result<Self> {
        if let Some(url) = opts.cdp_url.as_deref() {
            let ws_url = resolve_cdp_url(url).await?;
            return Ok(Self {
                child: None,
                ws_url: Some(ws_url),
                #[cfg(unix)]
                pipe: None,
                _profile: None,
            });
        }
        let transport = Transport::from_env()?;
        let exe = find_chrome(opts.chrome_path.as_deref())?;
        let (profile_dir, temp) = match &opts.user_data_dir {
            Some(p) => (p.clone(), None),
            None => {
                let t = tempfile::tempdir().map_err(|e| CdpError::Launch(e.to_string()))?;
                (t.path().to_path_buf(), Some(t))
            }
        };
        // Chrome allows one process per profile; a second launch on a shared --profile-dir exits at once.
        let profile_hint = match &opts.user_data_dir {
            Some(p) => format!(
                " (is another Ostler or Chrome already using profile {}? one browser per profile; use a separate --profile-dir per server, or omit it)",
                p.display()
            ),
            None => String::new(),
        };
        let mut cmd = Command::new(exe);
        match transport {
            #[cfg(unix)]
            Transport::Pipe => cmd.arg("--remote-debugging-pipe"),
            Transport::Ws => cmd.arg("--remote-debugging-port=0"),
        };
        cmd.arg(format!("--user-data-dir={}", profile_dir.display()))
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
        if std::env::var("OSTLER_NO_SANDBOX").is_ok() {
            cmd.args(["--no-sandbox", "--disable-setuid-sandbox"]);
        }
        if opts.lean {
            cmd.args(lean_args());
        }
        cmd.args(&opts.extra_args)
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        // Parent ends of the pipe pair; the child ends are moved into `cmd` as fds 3 and 4.
        #[cfg(unix)]
        let mut pipe_ends = None;
        #[cfg(unix)]
        if transport == Transport::Pipe {
            use command_fds::{CommandFdExt, FdMapping};
            let io = |e: std::io::Error| CdpError::Launch(format!("pipe setup failed: {e}"));
            // Chrome reads commands on fd 3 and writes responses and events on fd 4.
            let (cmd_rx, cmd_tx) = std::io::pipe().map_err(io)?;
            let (resp_rx, resp_tx) = std::io::pipe().map_err(io)?;
            cmd.fd_mappings(vec![
                FdMapping {
                    parent_fd: cmd_rx.into(),
                    child_fd: 3,
                },
                FdMapping {
                    parent_fd: resp_tx.into(),
                    child_fd: 4,
                },
            ])
            .map_err(|e| CdpError::Launch(e.to_string()))?;
            // `from_file` makes only our ends non-blocking; Chrome's ends stay blocking.
            let tx = tokio::net::unix::pipe::Sender::from_file(std::fs::File::from(
                std::os::fd::OwnedFd::from(cmd_tx),
            ))
            .map_err(io)?;
            let rx = tokio::net::unix::pipe::Receiver::from_file(std::fs::File::from(
                std::os::fd::OwnedFd::from(resp_rx),
            ))
            .map_err(io)?;
            pipe_ends = Some((rx, tx));
        }

        let spawned = cmd.spawn();
        // Release the parent's copies of Chrome's pipe ends so EOF reaches us when Chrome exits.
        drop(cmd);
        let mut child = spawned.map_err(|e| CdpError::Launch(e.to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| CdpError::Launch("no stderr".into()))?;
        let mut lines = BufReader::new(stderr).lines();

        #[cfg(unix)]
        if let Some((rx, tx)) = pipe_ends {
            // Keep draining stderr so the pipe never fills and blocks the browser.
            tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
            let conn = Connection::from_pipe(rx, tx);
            // The pipe has no "listening" banner, so prove Chrome is serving before returning.
            match conn
                .call(
                    "Browser.getVersion",
                    serde_json::json!({}),
                    None,
                    Duration::from_secs(20),
                )
                .await
            {
                Ok(_) => {}
                Err(CdpError::Closed) => {
                    return Err(CdpError::Launch(format!(
                        "browser exited before DevTools was ready{profile_hint}"
                    )))
                }
                Err(CdpError::Timeout(..)) => {
                    return Err(CdpError::Launch(
                        "timed out waiting for DevTools endpoint".into(),
                    ))
                }
                Err(e) => return Err(CdpError::Launch(e.to_string())),
            }
            return Ok(Self {
                child: Some(child),
                ws_url: None,
                pipe: Some(conn),
                _profile: temp,
            });
        }

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
                return Err(CdpError::Launch(format!(
                    "browser exited before DevTools was ready{profile_hint}"
                )))
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
            child: Some(child),
            ws_url: Some(ws_url),
            #[cfg(unix)]
            pipe: None,
            _profile: temp,
        })
    }

    /// The connection to this browser (pipe by default, WebSocket with `OSTLER_CDP_TRANSPORT=ws`).
    pub async fn connect(&self) -> Result<std::sync::Arc<Connection>> {
        #[cfg(unix)]
        if let Some(conn) = &self.pipe {
            return Ok(conn.clone());
        }
        match &self.ws_url {
            Some(url) => Connection::connect(url).await,
            None => Err(CdpError::Launch("no DevTools transport".into())),
        }
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().and_then(|c| c.id())
    }

    /// True when this is someone else's browser (`cdp_url`): close tabs we made, never the browser.
    pub fn is_attached(&self) -> bool {
        self.child.is_none()
    }

    pub async fn close(mut self) {
        if let Some(c) = self.child.as_mut() {
            let _ = c.kill().await;
        }
    }
}

/// How [`process_tree_stats`] measures memory on this platform.
pub fn memory_method() -> &'static str {
    if cfg!(target_os = "linux") {
        "PSS (proportional set size)"
    } else if cfg!(target_os = "macos") {
        "physical footprint (macOS `footprint`)"
    } else {
        "summed RSS (upper bound: shared pages counted per process)"
    }
}

/// Memory (KiB) of a process and all its descendants, measured per [`memory_method`].
pub fn process_tree_rss_kb(root: u32) -> Option<u64> {
    process_tree_stats(root).map(|(_, kb)| kb)
}

/// (pid, ppid, resident KiB) for every process.
#[cfg(unix)]
fn process_rows() -> Option<Vec<(u32, u32, u64)>> {
    let out = std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,rss="])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(parse_rows(&String::from_utf8_lossy(&out.stdout)))
}

/// Windows has no `ps`; PowerShell ships with every supported version and exposes the same data.
#[cfg(windows)]
fn process_rows() -> Option<Vec<(u32, u32, u64)>> {
    let out = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-CimInstance Win32_Process | ForEach-Object { \"$($_.ProcessId) $($_.ParentProcessId) $([int64]($_.WorkingSetSize / 1024))\" }",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(parse_rows(&String::from_utf8_lossy(&out.stdout)))
}

#[cfg(not(any(unix, windows)))]
fn process_rows() -> Option<Vec<(u32, u32, u64)>> {
    None
}

fn parse_rows(text: &str) -> Vec<(u32, u32, u64)> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            Some((
                it.next()?.parse().ok()?,
                it.next()?.parse().ok()?,
                it.next()?.parse().ok()?,
            ))
        })
        .collect()
}

fn descendants(root: u32) -> Option<Vec<(u32, u64)>> {
    let rows = process_rows()?;
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
            .map(|(p, _, r)| (*p, *r))
            .collect(),
    )
}

#[cfg(target_os = "linux")]
fn accurate_kb(procs: &[(u32, u64)]) -> Option<u64> {
    let mut total = 0;
    for (pid, rss) in procs {
        let pss = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup"))
            .ok()
            .and_then(|t| {
                t.lines()
                    .find(|l| l.starts_with("Pss:"))
                    .and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok())
            });
        total += pss.unwrap_or(*rss);
    }
    Some(total)
}

#[cfg(target_os = "macos")]
fn accurate_kb(procs: &[(u32, u64)]) -> Option<u64> {
    let mut cmd = std::process::Command::new("footprint");
    for (pid, _) in procs {
        cmd.arg("-p").arg(pid.to_string());
    }
    let out = cmd.output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut total_kb = 0f64;
    let mut seen = 0;
    for line in text
        .lines()
        .filter(|l| l.contains("]: ") && l.contains("Footprint:"))
    {
        let rest = line
            .split("Footprint:")
            .nth(1)?
            .split_whitespace()
            .collect::<Vec<_>>();
        let n: f64 = rest.first()?.parse().ok()?;
        total_kb += match rest.get(1).copied() {
            Some("KB") => n,
            Some("MB") => n * 1024.0,
            Some("GB") => n * 1024.0 * 1024.0,
            _ => n / 1024.0,
        };
        seen += 1;
    }
    (seen > 0).then_some(total_kb as u64)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn accurate_kb(_: &[(u32, u64)]) -> Option<u64> {
    None
}

/// (process count, memory KiB) for a process and its descendants.
pub fn process_tree_stats(root: u32) -> Option<(usize, u64)> {
    let procs = descendants(root)?;
    let kb = accurate_kb(&procs).unwrap_or_else(|| procs.iter().map(|(_, r)| r).sum());
    Some((procs.len(), kb))
}

/// Is `host` a loopback name or address? Attaching is refused for anything else: a debugging port
/// has no authentication, so reaching one over a network would hand over the whole browser.
fn is_loopback_host(host: &str) -> bool {
    let h = host.trim_start_matches('[').trim_end_matches(']');
    h.eq_ignore_ascii_case("localhost")
        || h.parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Split `scheme://host:port/path` into (scheme, host, port, path). Minimal on purpose.
fn split_url(url: &str) -> Option<(&str, &str, u16, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    let (auth, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    if auth.contains('@') {
        return None;
    }
    let (host, port) = if let Some(end) = auth.find(']') {
        let host = &auth[..=end];
        let port = auth[end + 1..].strip_prefix(':').unwrap_or("");
        (host, port)
    } else {
        auth.rsplit_once(':').unwrap_or((auth, ""))
    };
    let default = if scheme == "ws" || scheme == "http" {
        80
    } else {
        0
    };
    let port = if port.is_empty() {
        default
    } else {
        port.parse().ok()?
    };
    if host.is_empty() || port == 0 {
        return None;
    }
    Some((scheme, host, port, path))
}

/// Turn the user's `--cdp-url` into a DevTools WebSocket URL, refusing anything not on this machine.
/// `ws://` URLs pass through; `http://127.0.0.1:9222` is resolved through its `/json/version` page.
pub async fn resolve_cdp_url(url: &str) -> Result<String> {
    let bad = |m: String| CdpError::Launch(format!("--cdp-url {url:?}: {m}"));
    let (scheme, host, port, path) = split_url(url.trim()).ok_or_else(|| {
        bad("expected http://127.0.0.1:9222 or ws://127.0.0.1:9222/devtools/browser/<id>".into())
    })?;
    if scheme != "http" && scheme != "ws" {
        return Err(bad("only http:// and ws:// are accepted".into()));
    }
    if !is_loopback_host(host) {
        return Err(bad(
            "refusing a non-loopback host: a DevTools port has no authentication. Use 127.0.0.1 (tunnel with SSH if the browser is remote)".into(),
        ));
    }
    if scheme == "ws" {
        if !path.starts_with("/devtools/") {
            return Err(bad("a ws:// URL must be the browser endpoint, /devtools/browser/<id> (or use http://host:port)".into()));
        }
        return Ok(url.trim().to_string());
    }
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let addr = format!("{host}:{port}");
    let fail = |e: String| {
        bad(format!(
            "could not read {addr}/json/version ({e}). Start Chrome yourself with --remote-debugging-port={port} and a separate --user-data-dir, then retry"
        ))
    };
    let work = async {
        let mut s = tokio::net::TcpStream::connect(&addr)
            .await
            .map_err(|e| e.to_string())?;
        let req =
            format!("GET /json/version HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
        s.write_all(req.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        // Chrome may keep the connection open, so stop at Content-Length rather than waiting for EOF.
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let n = s.read(&mut chunk).await.map_err(|e| e.to_string())?;
            buf.extend_from_slice(&chunk[..n]);
            let text = String::from_utf8_lossy(&buf);
            if let Some((head, body)) = text.split_once("\r\n\r\n") {
                let want = head.lines().find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                });
                if want.is_some_and(|w| body.len() >= w) {
                    break;
                }
            }
            if n == 0 || buf.len() > (1 << 20) {
                break;
            }
        }
        Ok::<_, String>(String::from_utf8_lossy(&buf).into_owned())
    };
    let resp = tokio::time::timeout(Duration::from_secs(5), work)
        .await
        .map_err(|_| fail("timed out".into()))?
        .map_err(fail)?;
    let body = resp.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
    let v: serde_json::Value = serde_json::from_str(body.trim())
        .map_err(|_| fail("the reply was not DevTools JSON".into()))?;
    let ws = v["webSocketDebuggerUrl"]
        .as_str()
        .ok_or_else(|| fail("no webSocketDebuggerUrl in the reply".into()))?;
    // The reply is data from a local port; re-check it before trusting it.
    match split_url(ws) {
        Some(("ws", h, _, p)) if is_loopback_host(h) && p.starts_with("/devtools/") => {
            Ok(ws.to_string())
        }
        _ => Err(bad(
            "the browser advertised a non-loopback DevTools URL; refusing".into(),
        )),
    }
}

#[cfg(test)]
mod attach_tests {
    use super::*;

    #[test]
    fn loopback_only() {
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("localhost"));
        assert!(is_loopback_host("[::1]"));
        assert!(!is_loopback_host("10.0.0.5"));
        assert!(!is_loopback_host("example.com"));
        assert!(!is_loopback_host("0.0.0.0"));
    }

    #[test]
    fn splits_urls() {
        assert_eq!(
            split_url("http://127.0.0.1:9222"),
            Some(("http", "127.0.0.1", 9222, ""))
        );
        assert_eq!(
            split_url("ws://[::1]:9222/devtools/browser/x"),
            Some(("ws", "[::1]", 9222, "/devtools/browser/x"))
        );
        assert!(split_url("http://u:p@127.0.0.1:9222").is_none());
        assert!(split_url("nonsense").is_none());
    }

    #[tokio::test]
    async fn refuses_remote_and_odd_urls() {
        for u in [
            "http://example.com:9222",
            "ws://192.168.1.2:9222/devtools/browser/a",
            "ftp://127.0.0.1:1",
            "ws://127.0.0.1:9222/other",
        ] {
            let e = resolve_cdp_url(u).await.unwrap_err().to_string();
            assert!(e.contains("--cdp-url"), "{e}");
        }
        assert_eq!(
            resolve_cdp_url("ws://127.0.0.1:9222/devtools/browser/abc")
                .await
                .unwrap(),
            "ws://127.0.0.1:9222/devtools/browser/abc"
        );
    }

    #[tokio::test]
    async fn resolves_http_and_rejects_a_remote_advertised_url() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for (ws, ok) in [
            ("ws://127.0.0.1:1/devtools/browser/zz", true),
            ("ws://evil.example:1/devtools/browser/zz", false),
        ] {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = l.local_addr().unwrap().port();
            let body = format!("{{\"webSocketDebuggerUrl\":\"{ws}\"}}");
            tokio::spawn(async move {
                let (mut s, _) = l.accept().await.unwrap();
                let mut b = [0u8; 512];
                let _ = s.read(&mut b).await;
                let r = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                s.write_all(r.as_bytes()).await.unwrap();
            });
            let r = resolve_cdp_url(&format!("http://127.0.0.1:{port}")).await;
            assert_eq!(r.is_ok(), ok, "{r:?}");
        }
    }

    #[tokio::test]
    async fn nothing_listening_gives_a_fix() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let e = resolve_cdp_url(&format!("http://127.0.0.1:{port}"))
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("--remote-debugging-port"), "{e}");
    }
}
