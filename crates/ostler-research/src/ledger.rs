//! The source ledger: what was fetched, what could not be, and why.
//!
//! In memory it is a map keyed by normalised URL. Optionally it is persisted as JSON Lines: one
//! entry per line, appended with a single `write` (file mode 0600 on unix), compacted atomically
//! (temp file + rename) when it would exceed the size bound, and read back tolerantly (corrupt or
//! oversized lines are skipped and counted). Later lines override earlier ones.
//!
//! Gaps (every non-`Ok` entry) are never dropped silently; [`Ledger::gaps`] lists them all.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::search::normalise_url;
use crate::{hex, sanitise_text};

/// How a fetch ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "reason", rename_all = "snake_case")]
pub enum Outcome {
    /// Content was fetched and read.
    Ok,
    /// A bot wall / challenge was hit (handed off, never bypassed).
    Blocked(String),
    /// Content is behind a paywall (not bypassed).
    Paywalled,
    /// Network or HTTP failure.
    Failed(String),
    /// robots.txt disallowed the fetch.
    RobotsDisallowed,
    /// The session budget stopped this fetch.
    BudgetExceeded,
}

impl Outcome {
    /// Stable grouping key, e.g. `failed: timeout`.
    pub fn reason_key(&self) -> String {
        match self {
            Self::Ok => "ok".into(),
            Self::Blocked(r) => format!("blocked: {r}"),
            Self::Paywalled => "paywalled".into(),
            Self::Failed(r) => format!("failed: {r}"),
            Self::RobotsDisallowed => "robots_disallowed".into(),
            Self::BudgetExceeded => "budget_exceeded".into(),
        }
    }
}

/// One fetched (or attempted) source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceEntry {
    /// Requested URL.
    pub url: String,
    /// URL after redirects, when different.
    pub final_url: Option<String>,
    /// HTTP status, if a response arrived.
    pub status: Option<u16>,
    /// Unix seconds of the fetch.
    pub fetched_at: u64,
    /// Hex SHA-256 of the content bytes, when content was read.
    pub content_hash: Option<String>,
    /// Content bytes read.
    pub bytes: u64,
    /// Content was cut short (size cap or budget).
    pub truncated: bool,
    /// How the fetch ended.
    pub outcome: Outcome,
    /// Page title, if known.
    pub title: Option<String>,
    /// Set by [`Ledger::record`] when an earlier entry for this URL had a different content hash.
    #[serde(default)]
    pub changed: bool,
}

impl SourceEntry {
    /// Bare entry for `url` with the given outcome, stamped now.
    pub fn new(url: impl Into<String>, outcome: Outcome) -> Self {
        Self {
            url: url.into(),
            final_url: None,
            status: None,
            fetched_at: now_secs(),
            content_hash: None,
            bytes: 0,
            truncated: false,
            outcome,
            title: None,
            changed: false,
        }
    }

    /// Hex SHA-256 of `content`, the format of [`content_hash`](Self::content_hash).
    pub fn hash_content(content: &[u8]) -> String {
        hex(&Sha256::digest(content))
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// All entries sharing one non-Ok reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gap {
    /// Reason key (see [`Outcome::reason_key`]).
    pub reason: String,
    /// Number of URLs.
    pub count: usize,
    /// The URLs, sorted.
    pub urls: Vec<String>,
}

/// Persistence and size bounds.
#[derive(Debug, Clone, Copy)]
pub struct LedgerOptions {
    /// Maximum entries kept in memory; the oldest are evicted first.
    pub max_entries: usize,
    /// Maximum JSONL file size; exceeding it triggers compaction.
    pub max_file_bytes: u64,
}

impl Default for LedgerOptions {
    fn default() -> Self {
        Self {
            max_entries: 5000,
            max_file_bytes: 8 * 1024 * 1024,
        }
    }
}

const MAX_LINE_BYTES: usize = 64 * 1024;

struct Stored {
    entry: SourceEntry,
    seq: u64,
}

struct Persist {
    path: PathBuf,
    size: u64,
}

struct Inner {
    map: HashMap<String, Stored>,
    seq: u64,
    persist: Option<Persist>,
    opts: LedgerOptions,
    corrupt_lines: usize,
}

/// Thread-safe source ledger.
pub struct Ledger {
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for Ledger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ledger").field("len", &self.len()).finish()
    }
}

fn key_for(url: &str) -> String {
    normalise_url(url).unwrap_or_else(|| url.trim().to_owned())
}

fn clean(mut e: SourceEntry) -> SourceEntry {
    e.url = sanitise_text(&e.url, 2048);
    e.final_url = e.final_url.map(|u| sanitise_text(&u, 2048));
    e.title = e
        .title
        .map(|t| sanitise_text(&t, 300))
        .filter(|t| !t.is_empty());
    e.outcome = match e.outcome {
        Outcome::Blocked(r) => Outcome::Blocked(sanitise_text(&r, 200)),
        Outcome::Failed(r) => Outcome::Failed(sanitise_text(&r, 200)),
        o => o,
    };
    e
}

impl Ledger {
    /// Empty in-memory ledger.
    pub fn in_memory() -> Self {
        Self::with_options(LedgerOptions::default())
    }

    /// Empty in-memory ledger with explicit bounds.
    pub fn with_options(opts: LedgerOptions) -> Self {
        Self {
            inner: Mutex::new(Inner {
                map: HashMap::new(),
                seq: 0,
                persist: None,
                opts,
                corrupt_lines: 0,
            }),
        }
    }

    /// Open (or create) a JSONL-backed ledger. Existing entries are loaded tolerantly: if the
    /// file is larger than `max_file_bytes` only its tail is read, and unparsable lines are skipped.
    pub fn open(path: impl AsRef<Path>, opts: LedgerOptions) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            fs::create_dir_all(dir)?;
        }
        let mut inner = Inner {
            map: HashMap::new(),
            seq: 0,
            persist: None,
            opts,
            corrupt_lines: 0,
        };
        let mut size = 0;
        if path.exists() {
            let mut f = File::open(&path)?;
            let len = f.metadata()?.len();
            let mut skip_first = false;
            if len > opts.max_file_bytes {
                f.seek(SeekFrom::Start(len - opts.max_file_bytes))?;
                skip_first = true;
            }
            let mut reader = BufReader::new(f.take(opts.max_file_bytes));
            let mut buf = Vec::new();
            let mut first = true;
            loop {
                buf.clear();
                // Bounded line read: a line longer than MAX_LINE_BYTES is consumed and counted corrupt.
                let n = read_line_bounded(&mut reader, &mut buf)?;
                if n == 0 {
                    break;
                }
                if first && skip_first {
                    first = false;
                    continue; // partial line from the seek
                }
                first = false;
                inner.ingest_line(&buf);
            }
            size = len.min(opts.max_file_bytes);
            set_private(&path)?;
        }
        inner.persist = Some(Persist { path, size });
        let compact = inner
            .persist
            .as_ref()
            .is_some_and(|p| p.size >= opts.max_file_bytes)
            || inner.corrupt_lines > 0;
        let ledger = Self {
            inner: Mutex::new(inner),
        };
        if compact {
            ledger.lock().compact()?;
        }
        Ok(ledger)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.lock().map.len()
    }

    /// True when empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Lines skipped as corrupt when the file was loaded.
    pub fn corrupt_lines(&self) -> usize {
        self.lock().corrupt_lines
    }

    /// Record an entry (replacing any earlier one for the URL). If the earlier entry and this one
    /// both have content hashes and they differ, the stored entry gets `changed = true`.
    /// The entry is always kept in memory; `Err` means only that persisting it failed.
    pub fn record(&self, entry: SourceEntry) -> io::Result<()> {
        let mut entry = clean(entry);
        let key = key_for(&entry.url);
        let mut g = self.lock();
        if let Some(prev) = g.map.get(&key) {
            if let (Some(a), Some(b)) = (&prev.entry.content_hash, &entry.content_hash) {
                entry.changed = a != b;
            }
        }
        g.seq += 1;
        let seq = g.seq;
        g.map.insert(
            key,
            Stored {
                entry: entry.clone(),
                seq,
            },
        );
        g.evict();
        g.append(&entry)
    }

    /// The entry for `url`, if any.
    pub fn get(&self, url: &str) -> Option<SourceEntry> {
        self.lock().map.get(&key_for(url)).map(|s| s.entry.clone())
    }

    /// True when the ledger has no `Ok` entry for `url` with exactly this hash (unknown URLs count as changed).
    pub fn changed_since(&self, url: &str, hash: &str) -> bool {
        match self.get(url) {
            Some(e) if e.outcome == Outcome::Ok => e.content_hash.as_deref() != Some(hash),
            _ => true,
        }
    }

    /// Every non-Ok entry grouped by reason, largest group first (ties by reason).
    pub fn gaps(&self) -> Vec<Gap> {
        let g = self.lock();
        let mut by: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for s in g.map.values() {
            if s.entry.outcome != Outcome::Ok {
                by.entry(s.entry.outcome.reason_key())
                    .or_default()
                    .push(s.entry.url.clone());
            }
        }
        let mut out: Vec<Gap> = by
            .into_iter()
            .map(|(reason, mut urls)| {
                urls.sort();
                Gap {
                    reason,
                    count: urls.len(),
                    urls,
                }
            })
            .collect();
        out.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.reason.cmp(&b.reason)));
        out
    }

    /// Terse one-line summary for tool output, e.g.
    /// `5 sources: 3 ok (1 truncated, 1 changed); 2 gaps: 1 failed: timeout, 1 paywalled`.
    pub fn summary(&self) -> String {
        let (total, ok, trunc, changed) = {
            let g = self.lock();
            let ok: Vec<&SourceEntry> = g
                .map
                .values()
                .map(|s| &s.entry)
                .filter(|e| e.outcome == Outcome::Ok)
                .collect();
            (
                g.map.len(),
                ok.len(),
                ok.iter().filter(|e| e.truncated).count(),
                ok.iter().filter(|e| e.changed).count(),
            )
        };
        let gaps = self.gaps();
        let mut s = format!("{total} sources: {ok} ok");
        let mut notes = Vec::new();
        if trunc > 0 {
            notes.push(format!("{trunc} truncated"));
        }
        if changed > 0 {
            notes.push(format!("{changed} changed"));
        }
        if !notes.is_empty() {
            s.push_str(&format!(" ({})", notes.join(", ")));
        }
        if !gaps.is_empty() {
            let n: usize = gaps.iter().map(|g| g.count).sum();
            let parts: Vec<String> = gaps
                .iter()
                .map(|g| format!("{} {}", g.count, g.reason))
                .collect();
            s.push_str(&format!("; {n} gaps: {}", parts.join(", ")));
        }
        s
    }
}

/// Read up to the next `\n`, keeping at most `MAX_LINE_BYTES`; returns bytes consumed (0 at EOF).
/// An over-long line yields an empty (thus corrupt) buffer marker of one byte.
fn read_line_bounded<R: BufRead>(r: &mut R, out: &mut Vec<u8>) -> io::Result<usize> {
    let mut consumed = 0;
    let mut overflow = false;
    loop {
        let (done, used) = {
            let chunk = r.fill_buf()?;
            if chunk.is_empty() {
                break;
            }
            match chunk.iter().position(|&b| b == b'\n') {
                Some(i) => {
                    if !overflow && out.len() + i <= MAX_LINE_BYTES {
                        out.extend_from_slice(&chunk[..i]);
                    } else {
                        overflow = true;
                    }
                    (true, i + 1)
                }
                None => {
                    if !overflow && out.len() + chunk.len() <= MAX_LINE_BYTES {
                        out.extend_from_slice(chunk);
                    } else {
                        overflow = true;
                    }
                    (false, chunk.len())
                }
            }
        };
        r.consume(used);
        consumed += used;
        if done {
            break;
        }
    }
    if overflow {
        out.clear();
        out.push(0); // not valid JSON: counted as corrupt
    }
    Ok(consumed)
}

impl Inner {
    fn ingest_line(&mut self, line: &[u8]) {
        let text = String::from_utf8_lossy(line);
        let t = text.trim();
        if t.is_empty() {
            return;
        }
        match serde_json::from_str::<SourceEntry>(t) {
            Ok(e) => {
                let e = clean(e);
                self.seq += 1;
                let seq = self.seq;
                self.map.insert(key_for(&e.url), Stored { entry: e, seq });
                self.evict();
            }
            Err(_) => self.corrupt_lines += 1,
        }
    }

    fn evict(&mut self) {
        while self.map.len() > self.opts.max_entries.max(1) {
            let oldest = self
                .map
                .iter()
                .min_by_key(|(_, s)| s.seq)
                .map(|(k, _)| k.clone());
            match oldest {
                Some(k) => {
                    self.map.remove(&k);
                }
                None => break,
            }
        }
    }

    fn append(&mut self, entry: &SourceEntry) -> io::Result<()> {
        let Some(p) = &self.persist else {
            return Ok(());
        };
        let mut line = serde_json::to_vec(entry).map_err(io::Error::other)?;
        line.push(b'\n');
        if p.size + line.len() as u64 > self.opts.max_file_bytes {
            return self.compact(); // rewrites everything, including this entry
        }
        let mut f = open_private(&p.path, true)?;
        f.write_all(&line)?;
        if let Some(p) = &mut self.persist {
            p.size += line.len() as u64;
        }
        Ok(())
    }

    /// Rewrite the file from memory: temp file, fsync, rename.
    fn compact(&mut self) -> io::Result<()> {
        let Some(p) = &self.persist else {
            return Ok(());
        };
        let mut entries: Vec<&Stored> = self.map.values().collect();
        entries.sort_by_key(|s| s.seq);
        let mut tmp_name = p.path.as_os_str().to_owned();
        tmp_name.push(".tmp");
        let tmp = PathBuf::from(tmp_name);
        let mut size = 0u64;
        {
            let mut f = open_private(&tmp, false)?;
            for s in entries {
                let mut line = serde_json::to_vec(&s.entry).map_err(io::Error::other)?;
                line.push(b'\n');
                size += line.len() as u64;
                f.write_all(&line)?;
            }
            f.sync_all()?;
        }
        fs::rename(&tmp, &p.path)?;
        if let Some(p) = &mut self.persist {
            p.size = size;
        }
        self.corrupt_lines = 0;
        Ok(())
    }
}

/// Open for append (`append = true`) or truncate-create, mode 0600 on unix.
fn open_private(path: &Path, append: bool) -> io::Result<File> {
    let mut o = OpenOptions::new();
    o.create(true);
    if append {
        o.append(true);
    } else {
        o.write(true).truncate(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)
}

fn set_private(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_entry(url: &str, content: &str) -> SourceEntry {
        let mut e = SourceEntry::new(url, Outcome::Ok);
        e.status = Some(200);
        e.content_hash = Some(SourceEntry::hash_content(content.as_bytes()));
        e.bytes = content.len() as u64;
        e.title = Some("T".into());
        e
    }

    #[test]
    fn record_get_and_change_detection() {
        let l = Ledger::in_memory();
        l.record(ok_entry("https://a.com/x?utm_a=1#f", "one"))
            .unwrap();
        let got = l.get("https://A.com/x").unwrap();
        assert!(!got.changed);
        l.record(ok_entry("https://a.com/x", "one")).unwrap();
        assert!(!l.get("https://a.com/x").unwrap().changed);
        l.record(ok_entry("https://a.com/x", "two")).unwrap();
        assert!(l.get("https://a.com/x").unwrap().changed);
        let h1 = SourceEntry::hash_content(b"one");
        let h2 = SourceEntry::hash_content(b"two");
        assert!(l.changed_since("https://a.com/x", &h1));
        assert!(!l.changed_since("https://a.com/x", &h2));
        assert!(l.changed_since("https://unknown.com/", &h2));
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn gaps_and_summary() {
        let l = Ledger::in_memory();
        l.record(ok_entry("https://a.com/1", "a")).unwrap();
        let mut t = ok_entry("https://a.com/2", "b");
        t.truncated = true;
        l.record(t).unwrap();
        l.record(SourceEntry::new(
            "https://b.com/1",
            Outcome::Failed("timeout".into()),
        ))
        .unwrap();
        l.record(SourceEntry::new(
            "https://b.com/2",
            Outcome::Failed("timeout".into()),
        ))
        .unwrap();
        l.record(SourceEntry::new("https://c.com/", Outcome::Paywalled))
            .unwrap();
        l.record(SourceEntry::new(
            "https://d.com/",
            Outcome::Blocked("captcha".into()),
        ))
        .unwrap();
        l.record(SourceEntry::new(
            "https://e.com/",
            Outcome::RobotsDisallowed,
        ))
        .unwrap();
        l.record(SourceEntry::new("https://f.com/", Outcome::BudgetExceeded))
            .unwrap();
        let gaps = l.gaps();
        assert_eq!(gaps.len(), 5);
        assert_eq!(gaps[0].reason, "failed: timeout");
        assert_eq!(gaps[0].count, 2);
        assert_eq!(gaps[0].urls, vec!["https://b.com/1", "https://b.com/2"]);
        let s = l.summary();
        assert!(
            s.starts_with("8 sources: 2 ok (1 truncated); 6 gaps: 2 failed: timeout"),
            "{s}"
        );
        assert!(
            s.contains("1 paywalled") && s.contains("1 blocked: captcha"),
            "{s}"
        );
        assert_eq!(Ledger::in_memory().summary(), "0 sources: 0 ok");
    }

    #[test]
    fn sanitises_reasons_and_titles() {
        let l = Ledger::in_memory();
        let mut e = SourceEntry::new("https://a.com/", Outcome::Failed("bad\n\u{0}thing".into()));
        e.title = Some("ti\u{7}tle".into());
        l.record(e).unwrap();
        let g = l.get("https://a.com/").unwrap();
        assert_eq!(g.outcome, Outcome::Failed("bad thing".into()));
        assert_eq!(g.title.as_deref(), Some("title"));
    }

    #[test]
    fn eviction_bounds_memory() {
        let l = Ledger::with_options(LedgerOptions {
            max_entries: 3,
            ..LedgerOptions::default()
        });
        for i in 0..6 {
            l.record(ok_entry(&format!("https://a.com/{i}"), "x"))
                .unwrap();
        }
        assert_eq!(l.len(), 3);
        assert!(l.get("https://a.com/0").is_none());
        assert!(l.get("https://a.com/5").is_some());
    }

    #[test]
    fn persistence_round_trip_and_mode() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("sub").join("ledger.jsonl");
        {
            let l = Ledger::open(&p, LedgerOptions::default()).unwrap();
            l.record(ok_entry("https://a.com/x", "one")).unwrap();
            l.record(ok_entry("https://a.com/x", "two")).unwrap();
            l.record(SourceEntry::new(
                "https://b.com/",
                Outcome::Blocked("cf".into()),
            ))
            .unwrap();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let l = Ledger::open(&p, LedgerOptions::default()).unwrap();
        assert_eq!(l.len(), 2);
        let a = l.get("https://a.com/x").unwrap();
        assert!(a.changed);
        assert_eq!(a.content_hash, Some(SourceEntry::hash_content(b"two")));
        assert_eq!(
            l.get("https://b.com/").unwrap().outcome,
            Outcome::Blocked("cf".into())
        );
        assert_eq!(l.corrupt_lines(), 0);
    }

    #[test]
    fn tolerates_corrupt_and_oversized_lines_and_repairs() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("l.jsonl");
        let good = serde_json::to_string(&ok_entry("https://a.com/", "x")).unwrap();
        let huge = "x".repeat(MAX_LINE_BYTES + 10);
        let content = format!("{{broken\n{good}\n\n\u{0}\u{1}garbage\n{huge}\n{{\"url\":1}}\n");
        fs::write(&p, content).unwrap();
        let l = Ledger::open(&p, LedgerOptions::default()).unwrap();
        assert_eq!(l.len(), 1);
        assert_eq!(
            l.corrupt_lines(),
            0,
            "compaction on open resets the counter"
        );
        // file was rewritten clean
        let text = fs::read_to_string(&p).unwrap();
        assert_eq!(text.lines().count(), 1);
        let l2 = Ledger::open(&p, LedgerOptions::default()).unwrap();
        assert_eq!(l2.len(), 1);
    }

    #[test]
    fn corrupt_count_reported_before_repair_is_observable_via_fresh_inner() {
        let mut inner = Inner {
            map: HashMap::new(),
            seq: 0,
            persist: None,
            opts: LedgerOptions::default(),
            corrupt_lines: 0,
        };
        inner.ingest_line(b"nope");
        inner.ingest_line(b"");
        assert_eq!(inner.corrupt_lines, 1);
    }

    #[test]
    fn file_size_is_bounded_by_compaction() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("l.jsonl");
        let opts = LedgerOptions {
            max_entries: 10,
            max_file_bytes: 4096,
        };
        let l = Ledger::open(&p, opts).unwrap();
        for i in 0..200 {
            l.record(ok_entry(
                &format!("https://a.com/{}", i % 5),
                &format!("v{i}"),
            ))
            .unwrap();
            assert!(fs::metadata(&p).unwrap().len() <= 4096);
        }
        assert_eq!(l.len(), 5);
        let l2 = Ledger::open(&p, opts).unwrap();
        assert_eq!(l2.len(), 5);
        assert_eq!(
            l2.get("https://a.com/4").unwrap().content_hash,
            Some(SourceEntry::hash_content(b"v199"))
        );
    }

    #[test]
    fn large_existing_file_reads_only_tail() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("l.jsonl");
        let mut s = String::new();
        for i in 0..100 {
            s.push_str(
                &serde_json::to_string(&ok_entry(&format!("https://a.com/{i}"), "x")).unwrap(),
            );
            s.push('\n');
        }
        fs::write(&p, &s).unwrap();
        let opts = LedgerOptions {
            max_entries: 1000,
            max_file_bytes: 2000,
        };
        let l = Ledger::open(&p, opts).unwrap();
        assert!(l.len() < 100 && !l.is_empty());
        assert!(l.get("https://a.com/99").is_some());
        assert!(l.get("https://a.com/0").is_none());
    }
}
