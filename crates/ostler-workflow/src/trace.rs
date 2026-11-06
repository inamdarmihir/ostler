//! Append-only action trace: redaction, a JSONL writer and an OpenTelemetry-shaped export.
//!
//! # Redaction
//!
//! The principles follow `docs/SAFETY.md`: secrets are never stored or echoed, and detection is
//! heuristic, so it errs on the side of masking. [`Redactor`] masks
//!
//! 1. values explicitly registered as sensitive ([`Redactor::add_sensitive`], e.g. what the agent
//!    typed into a password field), anywhere they occur (ASCII case-insensitive);
//! 2. `Bearer <token>` credentials and values of keys such as `password=`, `token=`, `"secret":`;
//! 3. passwords inside `scheme://user:pass@host` URLs;
//! 4. well-known key prefixes (`sk-`, `ghp_`, `AKIA`, `xoxb-`, JWTs starting `eyJ`, ...);
//! 5. digit runs of 9 or more digits, allowing single spaces or dashes between groups (card, bank
//!    and national-id numbers, long numeric ids, epoch milliseconds);
//! 6. long mixed letter/digit tokens (24+ characters) such as API keys and hashes;
//! 7. email addresses (on by default).
//!
//! Known limits: a secret that is short, low-entropy, unregistered and under an innocuous key is
//! not caught; slug-like tokens with hyphens are deliberately not masked; long numeric order ids
//! ARE masked even though they are not secret. The writer never sees unredacted events: it redacts
//! on [`TraceWriter::append`].

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::expect::{ExpectResult, Failure};

/// Replacement text for masked spans.
pub const MASK: &str = "[REDACTED]";
const MAX_FIELD: usize = 2048;
const MAX_SECRETS: usize = 256;

/// One line of the trace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceEvent {
    /// Start time, milliseconds since the Unix epoch.
    pub ts: u64,
    /// Task this action belongs to, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// Tool name, e.g. `click`.
    pub tool: String,
    /// URL at the time of the action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// What was acted on (element description, never a typed value).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Short outcome text, e.g. `ok`, `error: timeout`, `submitted`.
    pub outcome: String,
    /// Postcondition result, when the action carried an `expect`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect: Option<ExpectResult>,
    /// Duration in milliseconds.
    #[serde(default)]
    pub duration_ms: u64,
}

/// Masks sensitive values; see the module docs for the rules and their limits.
#[derive(Debug, Clone)]
pub struct Redactor {
    secrets: Vec<Vec<char>>,
    mask_emails: bool,
}

impl Default for Redactor {
    fn default() -> Self {
        Redactor {
            secrets: Vec::new(),
            mask_emails: true,
        }
    }
}

const SENSITIVE_KEYS: &[&str] = &[
    "password",
    "passwd",
    "passcode",
    "pwd",
    "secret",
    "token",
    "apikey",
    "api_key",
    "api-key",
    "key",
    "auth",
    "authorization",
    "session",
    "sessionid",
    "sid",
    "code",
    "otp",
    "pin",
    "cvv",
    "cvc",
    "ssn",
    "signature",
    "credential",
    "credentials",
];

const PREFIXES: &[(&str, usize)] = &[
    ("sk-", 16),
    ("pk_live_", 12),
    ("sk_live_", 12),
    ("xoxb-", 10),
    ("xoxp-", 10),
    ("xoxa-", 10),
    ("xoxr-", 10),
    ("xoxs-", 10),
    ("ghp_", 20),
    ("gho_", 20),
    ("ghu_", 20),
    ("ghs_", 20),
    ("github_pat_", 20),
    ("glpat-", 16),
    ("AKIA", 12),
    ("AIza", 30),
    ("eyJ", 10),
];

fn lower(c: &[char]) -> Vec<char> {
    c.iter().map(|c| c.to_ascii_lowercase()).collect()
}

fn at(hay: &[char], i: usize, needle: &str) -> bool {
    needle
        .chars()
        .enumerate()
        .all(|(k, n)| hay.get(i + k) == Some(&n))
}

fn boundary(c: &[char], i: usize) -> bool {
    i == 0 || !c[i - 1].is_ascii_alphanumeric()
}

fn is_tail(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')
}

impl Redactor {
    /// Default redactor (emails masked).
    pub fn new() -> Self {
        Self::default()
    }

    /// Turn email masking on or off.
    pub fn with_emails(mut self, on: bool) -> Self {
        self.mask_emails = on;
        self
    }

    /// Register a value that must never appear in the trace. Empty values are ignored; very
    /// short values are masked everywhere they occur, which can garble the trace but cannot leak.
    /// At most 256 values are kept (the oldest are dropped).
    pub fn add_sensitive(&mut self, value: &str) {
        let v: Vec<char> = value.trim().to_ascii_lowercase().chars().collect();
        if v.is_empty() || self.secrets.contains(&v) {
            return;
        }
        self.secrets.push(v);
        if self.secrets.len() > MAX_SECRETS {
            self.secrets.remove(0);
        }
        self.secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    }

    /// Mask `value` entirely when `sensitive`, else apply [`Redactor::redact`].
    pub fn redact_value(&self, value: &str, sensitive: bool) -> String {
        if sensitive {
            MASK.to_string()
        } else {
            self.redact(value)
        }
    }

    /// Apply every rule to `s`.
    pub fn redact(&self, s: &str) -> String {
        let mut out = self.mask_registered(s);
        out = bearer(&out);
        out = sensitive_params(&out);
        out = userinfo(&out);
        out = prefixed(&out);
        out = digit_runs(&out);
        out = long_tokens(&out);
        if self.mask_emails {
            out = emails(&out);
        }
        out
    }

    fn mask_registered(&self, s: &str) -> String {
        if self.secrets.is_empty() {
            return s.to_string();
        }
        let c: Vec<char> = s.chars().collect();
        let l = lower(&c);
        let mut out = String::with_capacity(s.len());
        let mut i = 0;
        'outer: while i < c.len() {
            for sec in &self.secrets {
                if l.len() - i >= sec.len() && l[i..i + sec.len()] == sec[..] {
                    out.push_str(MASK);
                    i += sec.len();
                    continue 'outer;
                }
            }
            out.push(c[i]);
            i += 1;
        }
        out
    }

    /// Redact every string field of an event and cap field lengths.
    pub fn redact_event(&self, e: &TraceEvent) -> TraceEvent {
        let r = |s: &str| cap(&self.redact(s));
        let ro = |o: &Option<String>| o.as_deref().map(r);
        TraceEvent {
            ts: e.ts,
            task_id: ro(&e.task_id),
            tool: r(&e.tool),
            url: ro(&e.url),
            target: ro(&e.target),
            outcome: r(&e.outcome),
            expect: e.expect.as_ref().map(|x| ExpectResult {
                ok: x.ok,
                passed: x.passed.iter().map(|p| p.map_strings(&r)).collect(),
                failed: x
                    .failed
                    .iter()
                    .map(|f| Failure {
                        expect: f.expect.map_strings(&r),
                        why: r(&f.why),
                    })
                    .collect(),
            }),
            duration_ms: e.duration_ms,
        }
    }
}

fn cap(s: &str) -> String {
    if s.chars().count() <= MAX_FIELD {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(MAX_FIELD).collect();
        t.push('…');
        t
    }
}

fn bearer(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let l = lower(&c);
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        if boundary(&c, i) && at(&l, i, "bearer") {
            let mut j = i + 6;
            let ws = j;
            while j < c.len() && c[j] == ' ' {
                j += 1;
            }
            let ts = j;
            while j < c.len() && (c[j].is_ascii_alphanumeric() || "._~+/=-".contains(c[j])) {
                j += 1;
            }
            if j > ws && j - ts >= 8 {
                out.extend(&c[i..ts]);
                out.push_str(MASK);
                i = j;
                continue;
            }
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

fn sensitive_params(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let l = lower(&c);
    let n = c.len();
    let mut out = String::new();
    let mut i = 0;
    'outer: while i < n {
        if boundary(&c, i) {
            for key in SENSITIVE_KEYS {
                if !at(&l, i, key) {
                    continue;
                }
                let mut j = i + key.len();
                if j < n && (c[j] == '"' || c[j] == '\'') {
                    j += 1;
                }
                while j < n && c[j] == ' ' {
                    j += 1;
                }
                if j >= n || (c[j] != '=' && c[j] != ':') {
                    continue;
                }
                j += 1;
                while j < n && c[j] == ' ' {
                    j += 1;
                }
                let quote = (j < n && (c[j] == '"' || c[j] == '\'')).then(|| c[j]);
                if quote.is_some() {
                    j += 1;
                }
                let vs = j;
                let whole_line = *key == "authorization";
                while j < n {
                    let ch = c[j];
                    let stop = match quote {
                        Some(q) => ch == q || ch == '\n',
                        None => {
                            if whole_line {
                                ch == '\n' || ch == '"' || ch == '\''
                            } else {
                                ch.is_whitespace() || "&\"',;<>)}".contains(ch)
                            }
                        }
                    };
                    if stop {
                        break;
                    }
                    j += 1;
                }
                if j > vs {
                    out.extend(&c[i..vs]);
                    out.push_str(MASK);
                    i = j;
                    continue 'outer;
                }
            }
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

fn userinfo(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        if at(&c, i, "://") {
            out.push_str("://");
            let st = i + 3;
            let mut j = st;
            while j < c.len() && !c[j].is_whitespace() && !"/?#\"'<>".contains(c[j]) {
                j += 1;
            }
            let auth = &c[st..j];
            if let Some(at_pos) = auth.iter().rposition(|x| *x == '@') {
                if let Some(colon) = auth[..at_pos].iter().position(|x| *x == ':') {
                    out.extend(&auth[..=colon]);
                    out.push_str(MASK);
                    out.extend(&auth[at_pos..]);
                    i = j;
                    continue;
                }
            }
            i = st;
            continue;
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

fn prefixed(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    'outer: while i < c.len() {
        if boundary(&c, i) {
            for (p, min) in PREFIXES {
                if at(&c, i, p) {
                    let st = i + p.chars().count();
                    let mut j = st;
                    while j < c.len() && is_tail(c[j]) {
                        j += 1;
                    }
                    if j - st >= *min {
                        out.push_str(MASK);
                        i = j;
                        continue 'outer;
                    }
                }
            }
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

fn digit_runs(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        if c[i].is_ascii_digit() {
            let mut j = i;
            let mut digits = 0;
            let mut end = i;
            loop {
                while j < c.len() && c[j].is_ascii_digit() {
                    j += 1;
                    digits += 1;
                    end = j;
                }
                if j + 1 < c.len() && (c[j] == ' ' || c[j] == '-') && c[j + 1].is_ascii_digit() {
                    j += 1;
                } else {
                    break;
                }
            }
            if digits >= 9 {
                out.push_str(MASK);
            } else {
                out.extend(&c[i..end]);
            }
            i = end;
        } else {
            out.push(c[i]);
            i += 1;
        }
    }
    out
}

fn long_tokens(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        if c[i].is_ascii_alphanumeric() || c[i] == '_' {
            let mut j = i;
            while j < c.len() && (c[j].is_ascii_alphanumeric() || c[j] == '_') {
                j += 1;
            }
            let t = &c[i..j];
            let has_d = t.iter().any(|x| x.is_ascii_digit());
            let has_l = t.iter().any(|x| x.is_ascii_alphabetic());
            if t.len() >= 24 && has_d && has_l {
                out.push_str(MASK);
            } else {
                out.extend(t);
            }
            i = j;
        } else {
            out.push(c[i]);
            i += 1;
        }
    }
    out
}

fn emails(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let local = |x: char| x.is_ascii_alphanumeric() || "._%+-".contains(x);
    let dom = |x: char| x.is_ascii_alphanumeric() || ".-".contains(x);
    let mut out: Vec<char> = Vec::new();
    let mut i = 0;
    while i < c.len() {
        if c[i] == '@' {
            let mut a = out.len();
            while a > 0 && local(out[a - 1]) {
                a -= 1;
            }
            let mut j = i + 1;
            while j < c.len() && dom(c[j]) {
                j += 1;
            }
            let domain: String = c[i + 1..j].iter().collect();
            let domain = domain.trim_end_matches('.');
            if a < out.len() && domain.contains('.') && !domain.starts_with('.') {
                out.truncate(a);
                out.extend(MASK.chars());
                i += 1 + domain.chars().count();
                continue;
            }
        }
        out.push(c[i]);
        i += 1;
    }
    out.into_iter().collect()
}

/// Writer limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraceConfig {
    /// Rotate before the file would exceed this many bytes. Default 5 MiB.
    pub max_bytes: u64,
    /// Rotated files to keep (`trace.jsonl.1` .. `.N`). Default 3. `0` truncates instead.
    pub keep: usize,
}

impl Default for TraceConfig {
    fn default() -> Self {
        TraceConfig {
            max_bytes: 5 * 1024 * 1024,
            keep: 3,
        }
    }
}

/// Append-only JSONL trace file. Created with mode 0600 on Unix (existing files are tightened to
/// 0600), refuses a symlink at the path, rotates by size. Events are redacted before writing.
#[derive(Debug)]
pub struct TraceWriter {
    path: PathBuf,
    cfg: TraceConfig,
    redactor: Redactor,
    file: File,
    size: u64,
}

fn open_append(path: &Path) -> io::Result<File> {
    if let Ok(md) = std::fs::symlink_metadata(path) {
        if md.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "trace path is a symlink; refusing to follow it",
            ));
        }
    }
    let mut o = OpenOptions::new();
    o.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let f = o.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(f)
}

fn numbered(path: &Path, n: usize) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(format!(".{n}"));
    PathBuf::from(s)
}

impl TraceWriter {
    /// Open (creating parent directories and the file) for appending.
    pub fn open(
        path: impl Into<PathBuf>,
        cfg: TraceConfig,
        redactor: Redactor,
    ) -> io::Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let file = open_append(&path)?;
        let size = file.metadata()?.len();
        Ok(TraceWriter {
            path,
            cfg,
            redactor,
            file,
            size,
        })
    }

    /// The redactor, to register sensitive values as the task proceeds.
    pub fn redactor_mut(&mut self) -> &mut Redactor {
        &mut self.redactor
    }

    /// Path of the live file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Redact and append one event as one line. Rotates first if the line would push the file
    /// past `max_bytes` (a single oversize line is still written, alone).
    pub fn append(&mut self, e: &TraceEvent) -> io::Result<()> {
        let mut line = serde_json::to_string(&self.redactor.redact_event(e))
            .map_err(|x| io::Error::new(io::ErrorKind::InvalidData, x))?;
        line.push('\n');
        let len = line.len() as u64;
        if self.size > 0 && self.size + len > self.cfg.max_bytes {
            self.rotate()?;
        }
        self.file.write_all(line.as_bytes())?;
        self.size += len;
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        let keep = self.cfg.keep;
        if keep == 0 {
            std::fs::remove_file(&self.path)?;
        } else {
            let _ = std::fs::remove_file(numbered(&self.path, keep));
            for i in (1..keep).rev() {
                let from = numbered(&self.path, i);
                if from.exists() {
                    std::fs::rename(&from, numbered(&self.path, i + 1))?;
                }
            }
            std::fs::rename(&self.path, numbered(&self.path, 1))?;
        }
        self.file = open_append(&self.path)?;
        self.size = 0;
        Ok(())
    }
}

/// Parse JSONL text; returns the events and the number of lines that could not be parsed.
pub fn parse_jsonl(text: &str) -> (Vec<TraceEvent>, usize) {
    let mut ok = Vec::new();
    let mut bad = 0;
    for l in text.lines().filter(|l| !l.trim().is_empty()) {
        match serde_json::from_str(l) {
            Ok(e) => ok.push(e),
            Err(_) => bad += 1,
        }
    }
    (ok, bad)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn attr(k: &str, v: impl Into<String>) -> Value {
    json!({"key": k, "value": {"stringValue": v.into()}})
}

/// OpenTelemetry-shaped (OTLP/JSON) export of a trace. No OpenTelemetry dependency: this builds
/// the `resourceSpans` document by hand, so it has not been validated against a collector.
///
/// One span per event, named `ostler.<tool>`. All spans of a task share a trace id (a hash of the
/// task id; events without a task share one fixed id). Span ids are derived from the event, so
/// exporting twice gives the same ids. Status is ERROR when the expectation failed or the outcome
/// text starts with `error` or `fail` (heuristic), else OK. Events are redacted first.
pub fn otel_export(events: &[TraceEvent], service_name: &str, redactor: &Redactor) -> Value {
    let spans: Vec<Value> = events
        .iter()
        .enumerate()
        .map(|(i, raw)| {
            let e = redactor.redact_event(raw);
            let tid = Sha256::digest(e.task_id.as_deref().unwrap_or("(no task)").as_bytes());
            let sid = Sha256::digest(format!(
                "{}|{}|{}|{}",
                e.task_id.as_deref().unwrap_or(""),
                i,
                e.ts,
                e.tool
            ));
            let start_ns = u128::from(e.ts) * 1_000_000;
            let end_ns = start_ns + u128::from(e.duration_ms) * 1_000_000;
            let mut attrs = vec![
                attr("ostler.tool", e.tool.clone()),
                attr("ostler.outcome", e.outcome.clone()),
            ];
            if let Some(t) = &e.task_id {
                attrs.push(attr("ostler.task_id", t.clone()));
            }
            if let Some(u) = &e.url {
                attrs.push(attr("url.full", u.clone()));
            }
            if let Some(t) = &e.target {
                attrs.push(attr("ostler.target", t.clone()));
            }
            let mut expect_failed = false;
            if let Some(x) = &e.expect {
                expect_failed = !x.ok;
                attrs.push(json!({"key":"ostler.expect.ok","value":{"boolValue": x.ok}}));
                if !x.ok {
                    attrs.push(attr("ostler.expect.failed", x.render()));
                }
            }
            let lo = e.outcome.to_lowercase();
            let is_err = expect_failed || lo.starts_with("error") || lo.starts_with("fail");
            json!({
                "traceId": hex(&tid[..16]),
                "spanId": hex(&sid[..8]),
                "name": format!("ostler.{}", e.tool),
                "kind": 1,
                "startTimeUnixNano": start_ns.to_string(),
                "endTimeUnixNano": end_ns.to_string(),
                "attributes": attrs,
                "status": {"code": if is_err { 2 } else { 1 }},
            })
        })
        .collect();
    json!({"resourceSpans": [{
        "resource": {"attributes": [attr("service.name", service_name)]},
        "scopeSpans": [{"scope": {"name": "ostler-workflow"}, "spans": spans}],
    }]})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expect::{evaluate, Expect, Observation};

    fn ev(tool: &str, outcome: &str) -> TraceEvent {
        TraceEvent {
            ts: 1_700_000_000_000,
            task_id: Some("t1".into()),
            tool: tool.into(),
            url: Some("https://a.test/x".into()),
            target: Some("button Pay".into()),
            outcome: outcome.into(),
            expect: None,
            duration_ms: 42,
        }
    }

    #[test]
    fn masks_patterns() {
        let r = Redactor::new();
        let cases = [
            ("card 4111 1111 1111 1111 ok", "4111"),
            ("card 4111-1111-1111-1111", "1111"),
            ("ssn 123-45-6789", "6789"),
            ("Authorization: Bearer abcDEF123456.xyz", "abcDEF"),
            ("bearer abcdefghijkl", "abcdefghijkl"),
            ("https://x.test/cb?code=AbC9&state=1", "AbC9"),
            ("https://x.test/?password=hunter2&a=1", "hunter2"),
            ("{\"password\":\"hunter two\"}", "hunter"),
            ("https://bob:s3cr3tpw@host.test/p", "s3cr3tpw"),
            ("key sk-abcdefghijklmnopqrstuvwx", "abcdefghijkl"),
            ("AKIAIOSFODNN7EXAMPLE", "IOSFODNN7"),
            ("ghp_abcdefghijklmnopqrstuvwxyz0123456789", "abcdefghijkl"),
            (
                "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.sig-nature_x",
                "eyJzdWIi",
            ),
            (
                "hash 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b",
                "9f86d081884c",
            ),
            ("mail jane.doe+x@example.co.uk now", "jane.doe"),
        ];
        for (input, leak) in cases {
            let out = r.redact(input);
            assert!(!out.contains(leak), "leaked {leak:?} in {out:?}");
            assert!(out.contains(MASK), "{out:?}");
        }
    }

    #[test]
    fn leaves_ordinary_text_alone() {
        let r = Redactor::new();
        for s in [
            "click button Continue",
            "https://shop.test/product-12345-blue-widget-extra-large-size",
            "2024-11-06 order of 12 items",
            "keyboard=us monkey=3",
            "Total: $1,234.50",
        ] {
            assert_eq!(r.redact(s), s);
        }
    }

    #[test]
    fn registered_values_everywhere_case_insensitive() {
        let mut r = Redactor::new();
        r.add_sensitive("Hunter2!");
        r.add_sensitive("Hunter2!x");
        r.add_sensitive("  ");
        let out = r.redact("typed hunter2!X then HUNTER2! again");
        assert!(!out.to_lowercase().contains("hunter"), "{out}");
        assert_eq!(out.matches(MASK).count(), 2, "longest match first: {out}");
        assert_eq!(r.redact_value("anything", true), MASK);
        assert_eq!(r.redact_value("anything", false), "anything");
    }

    #[test]
    fn email_masking_optional() {
        let r = Redactor::new().with_emails(false);
        assert_eq!(r.redact("a@b.test"), "a@b.test");
        assert_eq!(
            Redactor::new().redact("to a@b.test."),
            format!("to {MASK}.")
        );
        assert_eq!(
            Redactor::new().redact("at @home and a@b"),
            "at @home and a@b"
        );
    }

    #[test]
    fn idempotent() {
        let r = Redactor::new();
        let s = "Bearer abcdefghijkl 4111 1111 1111 1111 a@b.test ?token=xyz";
        let once = r.redact(s);
        assert_eq!(r.redact(&once), once);
    }

    #[test]
    fn event_redaction_covers_expect() {
        let mut r = Redactor::new();
        r.add_sensitive("SecretWord");
        let mut e = ev("type", "typed SecretWord");
        let obs = Observation {
            url: "https://a.test/?token=zzz".into(),
            ..Default::default()
        };
        e.expect = Some(evaluate(&[Expect::TextPresent("SecretWord".into())], &obs));
        e.target = Some("x".repeat(5000));
        let out = r.redact_event(&e);
        let json = serde_json::to_string(&out).unwrap();
        assert!(!json.contains("SecretWord"), "{json}");
        assert!(out.target.unwrap().chars().count() <= MAX_FIELD + 1);
    }

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ostler-wf-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn writer_appends_redacted_lines_with_private_mode() {
        let d = tmpdir("w1");
        let p = d.join("sub").join("trace.jsonl");
        let mut w = TraceWriter::open(&p, TraceConfig::default(), Redactor::new()).unwrap();
        w.redactor_mut().add_sensitive("hunter2");
        w.append(&ev("type", "typed hunter2")).unwrap();
        w.append(&ev("click", "ok")).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(!text.contains("hunter2"));
        let (evs, bad) = parse_jsonl(&text);
        assert_eq!((evs.len(), bad), (2, 0));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn existing_file_is_tightened_and_symlink_refused() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmpdir("w2");
        let p = d.join("t.jsonl");
        std::fs::write(&p, b"").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        TraceWriter::open(&p, TraceConfig::default(), Redactor::new()).unwrap();
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let l = d.join("link.jsonl");
        std::os::unix::fs::symlink(&p, &l).unwrap();
        assert!(TraceWriter::open(&l, TraceConfig::default(), Redactor::new()).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn rotation_respects_cap_and_keep() {
        let d = tmpdir("w3");
        let p = d.join("t.jsonl");
        let cfg = TraceConfig {
            max_bytes: 600,
            keep: 2,
        };
        let mut w = TraceWriter::open(&p, cfg, Redactor::new()).unwrap();
        for i in 0..30 {
            w.append(&ev("click", &format!("ok {i}"))).unwrap();
        }
        assert!(p.exists() && numbered(&p, 1).exists() && numbered(&p, 2).exists());
        assert!(!numbered(&p, 3).exists());
        for f in [p.clone(), numbered(&p, 1), numbered(&p, 2)] {
            assert!(std::fs::metadata(&f).unwrap().len() <= 600, "{f:?}");
        }
        // newest event is in the live file
        assert!(std::fs::read_to_string(&p).unwrap().contains("ok 29"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn keep_zero_truncates() {
        let d = tmpdir("w4");
        let p = d.join("t.jsonl");
        let mut w = TraceWriter::open(
            &p,
            TraceConfig {
                max_bytes: 300,
                keep: 0,
            },
            Redactor::new(),
        )
        .unwrap();
        for _ in 0..10 {
            w.append(&ev("click", "ok")).unwrap();
        }
        assert!(!numbered(&p, 1).exists());
        assert!(std::fs::metadata(&p).unwrap().len() <= 300);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn otel_shape() {
        let mut e = ev("click", "ok");
        let obs = Observation::default();
        e.expect = Some(evaluate(&[Expect::TextPresent("x".into())], &obs));
        let v = otel_export(
            &[ev("navigate", "ok"), e, ev("click", "error: timeout")],
            "ostler",
            &Redactor::new(),
        );
        let spans = v["resourceSpans"][0]["scopeSpans"][0]["spans"]
            .as_array()
            .unwrap();
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0]["name"], "ostler.navigate");
        assert_eq!(spans[0]["traceId"].as_str().unwrap().len(), 32);
        assert_eq!(spans[0]["spanId"].as_str().unwrap().len(), 16);
        assert_eq!(spans[0]["traceId"], spans[1]["traceId"]);
        assert_ne!(spans[0]["spanId"], spans[1]["spanId"]);
        assert_eq!(spans[0]["startTimeUnixNano"], "1700000000000000000");
        assert_eq!(spans[0]["endTimeUnixNano"], "1700000000042000000");
        assert_eq!(spans[0]["status"]["code"], 1);
        assert_eq!(spans[1]["status"]["code"], 2, "failed expect");
        assert_eq!(spans[2]["status"]["code"], 2, "error outcome");
        assert_eq!(
            v["resourceSpans"][0]["resource"]["attributes"][0]["value"]["stringValue"],
            "ostler"
        );
    }

    #[test]
    fn otel_redacts() {
        let mut e = ev("click", "ok");
        e.url = Some("https://a.test/?token=abc123".into());
        let s = otel_export(&[e], "o", &Redactor::new()).to_string();
        assert!(!s.contains("abc123"));
    }

    #[test]
    fn jsonl_skips_bad_lines() {
        let (e, bad) = parse_jsonl("{\"nope\":1}\n\n");
        assert!(e.is_empty());
        assert_eq!(bad, 1);
    }
}

#[cfg(test)]
mod prop_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn registered_secrets_never_leak(
            secrets in proptest::collection::vec("[A-Za-z0-9]{6,20}", 1..4),
            pre in "[ -~]{0,30}",
            mid in "[ -~]{0,30}",
            post in "[ -~]{0,30}",
        ) {
            let mut r = Redactor::new();
            for s in &secrets { r.add_sensitive(s); }
            let text = format!("{pre}{}{mid}{}{post}", secrets[0], secrets[secrets.len() - 1]);
            let out = r.redact(&text);
            let low = out.to_lowercase();
            for s in &secrets {
                let s = s.to_lowercase();
                // the marker itself may legitimately contain a short secret like "redact"
                if MASK.to_lowercase().contains(&s) { continue; }
                prop_assert!(!low.contains(&s), "leaked {s} in {out}");
            }
        }

        #[test]
        fn event_redaction_never_leaks(secret in "[A-Za-z0-9]{8,20}", tool in "[a-z_]{1,10}") {
            let mut r = Redactor::new();
            r.add_sensitive(&secret);
            let e = TraceEvent {
                ts: 1, task_id: Some(format!("id-{secret}")), tool,
                url: Some(format!("https://a.test/{secret}")),
                target: Some(secret.clone()), outcome: format!("typed {secret}"),
                expect: None, duration_ms: 0,
            };
            let json = serde_json::to_string(&r.redact_event(&e)).unwrap().to_lowercase();
            prop_assert!(!json.contains(&secret.to_lowercase()));
        }

        #[test]
        fn redact_is_total_and_idempotent(s in ".{0,200}") {
            let r = Redactor::new();
            let once = r.redact(&s);
            prop_assert_eq!(r.redact(&once), once);
        }
    }
}
