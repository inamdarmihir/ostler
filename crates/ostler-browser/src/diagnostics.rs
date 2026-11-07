//! Per-tab debugging signal: console messages, uncaught exceptions and network problems, kept in a
//! bounded ring buffer and read back with a cursor.
//!
//! Everything in an entry is derived from the page (its console, its URLs, its error text) and must be
//! treated as untrusted by the caller. URLs are reduced to scheme, host and path: query strings,
//! fragments and credentials never reach the buffer. Secrets in free text (a token a page logs) are not
//! detected; only URLs inside text are reduced.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Entries kept per tab; older ones are dropped and counted.
pub const MAX_ENTRIES: usize = 500;
/// Characters kept of one entry's text.
pub const MAX_TEXT: usize = 1000;
/// Characters kept of one entry's URL.
pub const MAX_URL: usize = 300;
/// Requests remembered so a failure event (which carries no URL) can be named.
const MAX_REQUESTS: usize = 1000;

/// A position in a tab's diagnostics stream: the sequence number of the last entry already seen.
/// `Cursor::default()` reads from the start of what is retained.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Cursor(pub u64);

/// What an entry reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagKind {
    /// `console.*` call.
    Console,
    /// Uncaught exception or unhandled rejection.
    Exception,
    /// Browser log entry (deprecations, security warnings, CSP violations).
    Log,
    /// HTTP response with status 400 or above.
    HttpError,
    /// Request that failed at the network level (DNS, connection, TLS, aborted by the server).
    RequestFailed,
    /// Request Ostler's policy stopped (allowlist, private-network block, submit gate).
    Blocked,
}

/// One diagnostic entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagEntry {
    /// Position in the stream; pass the last one back as the next [`Cursor`].
    pub seq: u64,
    /// Unix time in milliseconds when the entry was recorded.
    pub ts_ms: u64,
    pub kind: DiagKind,
    /// `error`, `warning`, `info`, `debug` or `log`.
    pub level: String,
    /// Message text, capped at `MAX_TEXT` characters, URLs inside reduced to path-only.
    pub text: String,
    /// Related URL (script, resource), path-only.
    pub url: Option<String>,
    /// HTTP status for [`DiagKind::HttpError`].
    pub status: Option<u16>,
    /// Always true: the text and URL come from the page. The caller must fence them as untrusted content.
    pub untrusted: bool,
}

/// A batch of entries after a cursor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Diagnostics {
    pub entries: Vec<DiagEntry>,
    /// Pass this as `since` to continue after the last returned entry.
    pub next_cursor: Cursor,
    /// Entries after `since` that were evicted from the buffer before they could be read.
    pub dropped: u64,
}

#[derive(Default)]
struct Inner {
    entries: VecDeque<DiagEntry>,
    next_seq: u64,
    requests: HashMap<String, (String, String)>,
    order: VecDeque<String>,
}

/// The ring buffer. Cheap to push to from the event loop.
#[derive(Default)]
pub(crate) struct DiagBuf {
    inner: Mutex<Inner>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn clamp(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

/// Reduce a URL to scheme, host, port and path. Query, fragment and credentials are dropped.
pub(crate) fn redact_url(raw: &str) -> String {
    let raw = raw.trim();
    let lower = raw.get(..5).unwrap_or("").to_ascii_lowercase();
    if lower.starts_with("data:") {
        return "data:…".into();
    }
    if lower.starts_with("blob:") {
        return "blob:…".into();
    }
    let reduced = match url::Url::parse(raw) {
        Ok(u) if u.has_host() => {
            let port = u.port().map(|p| format!(":{p}")).unwrap_or_default();
            format!(
                "{}://{}{}{}",
                u.scheme(),
                u.host_str().unwrap_or(""),
                port,
                u.path()
            )
        }
        _ => raw.split(['?', '#']).next().unwrap_or("").to_string(),
    };
    clamp(&reduced, MAX_URL)
}

/// Reduce every URL inside free text to path-only, then cap the length.
pub(crate) fn redact_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(MAX_TEXT * 2));
    for piece in text.split_inclusive(char::is_whitespace) {
        let Some(i) = piece.find("://") else {
            out.push_str(piece);
            continue;
        };
        let start = piece[..i]
            .rfind(|c: char| !c.is_ascii_alphabetic())
            .map_or(0, |p| p + 1);
        let tail = &piece[start..];
        let body = tail.trim_end_matches(char::is_whitespace);
        let core = body.trim_end_matches([')', ']', '}', '>', '"', '\'', ',', ';', '.']);
        out.push_str(&piece[..start]);
        out.push_str(&redact_url(core));
        out.push_str(&tail[core.len()..]);
        if out.chars().count() > MAX_TEXT * 2 {
            break;
        }
    }
    clamp(&out, MAX_TEXT)
}

/// Text of a `console.*` call's arguments.
pub(crate) fn console_text(args: &Value) -> String {
    let Some(args) = args.as_array() else {
        return String::new();
    };
    args.iter()
        .map(|a| match &a["value"] {
            Value::String(s) => s.clone(),
            Value::Null => a["description"]
                .as_str()
                .or_else(|| a["unserializableValue"].as_str())
                .unwrap_or("")
                .to_string(),
            v => v.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Level name for a `console.*` method.
pub(crate) fn console_level(kind: &str) -> &'static str {
    match kind {
        "error" | "assert" => "error",
        "warning" => "warning",
        "info" => "info",
        "debug" | "trace" => "debug",
        _ => "log",
    }
}

impl DiagBuf {
    /// Record an entry (text and url are redacted and capped here, so callers cannot forget).
    pub(crate) fn push(
        &self,
        kind: DiagKind,
        level: &str,
        text: &str,
        url: Option<&str>,
        status: Option<u16>,
    ) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.next_seq += 1;
        let entry = DiagEntry {
            seq: g.next_seq,
            ts_ms: now_ms(),
            kind,
            level: level.to_string(),
            text: redact_text(text),
            url: url.filter(|u| !u.is_empty()).map(redact_url),
            status,
            untrusted: true,
        };
        if g.entries.len() >= MAX_ENTRIES {
            g.entries.pop_front();
        }
        g.entries.push_back(entry);
    }

    /// Remember a request's URL and method by its network id.
    pub(crate) fn note_request(&self, id: &str, url: &str, method: &str) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.requests
            .insert(id.to_string(), (url.to_string(), method.to_string()))
            .is_none()
        {
            g.order.push_back(id.to_string());
        }
        while g.order.len() > MAX_REQUESTS {
            if let Some(old) = g.order.pop_front() {
                g.requests.remove(&old);
            }
        }
    }

    pub(crate) fn request(&self, id: &str) -> Option<(String, String)> {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.requests.get(id).cloned()
    }

    /// Cursor at the current end: entries recorded from now on are "new".
    pub(crate) fn head(&self) -> Cursor {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        Cursor(g.next_seq)
    }

    pub(crate) fn read(&self, since: Cursor, limit: usize) -> Diagnostics {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let first = g.entries.front().map_or(g.next_seq + 1, |e| e.seq);
        let dropped = first.saturating_sub(1).saturating_sub(since.0);
        let entries: Vec<DiagEntry> = g
            .entries
            .iter()
            .filter(|e| e.seq > since.0)
            .take(limit)
            .cloned()
            .collect();
        let next_cursor = entries.last().map_or(since, |e| Cursor(e.seq));
        Diagnostics {
            entries,
            next_cursor,
            dropped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_lose_query_fragment_and_credentials() {
        assert_eq!(
            redact_url("https://user:pw@example.com:8443/a/b?token=SECRET#frag"),
            "https://example.com:8443/a/b"
        );
        assert_eq!(redact_url("data:text/plain;base64,AAAA"), "data:…");
        assert_eq!(redact_url("/rel/path?x=1"), "/rel/path");
    }

    #[test]
    fn urls_inside_text_are_reduced() {
        let t = redact_text("Failed to load https://api.test/v1/x?key=SECRET&b=2, retrying");
        assert!(!t.contains("SECRET"), "{t}");
        assert!(t.contains("https://api.test/v1/x,"), "{t}");
    }

    #[test]
    fn ring_drops_oldest_and_counts_them() {
        let b = DiagBuf::default();
        for i in 0..(MAX_ENTRIES + 10) {
            b.push(DiagKind::Console, "log", &format!("m{i}"), None, None);
        }
        let d = b.read(Cursor(0), 3);
        assert_eq!(d.dropped, 10);
        assert_eq!(d.entries.len(), 3);
        assert_eq!(d.entries[0].text, "m10");
        let d2 = b.read(d.next_cursor, 1000);
        assert_eq!(d2.dropped, 0);
        assert_eq!(d2.entries.len(), MAX_ENTRIES - 3);
        assert_eq!(b.read(d2.next_cursor, 10).entries.len(), 0);
    }

    #[test]
    fn long_text_is_capped() {
        let b = DiagBuf::default();
        b.push(DiagKind::Console, "log", &"x".repeat(5000), None, None);
        assert_eq!(
            b.read(Cursor(0), 1).entries[0].text.chars().count(),
            MAX_TEXT
        );
    }
}
