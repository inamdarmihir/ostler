//! Immutable source-version archive: the exact page bodies that evidence cites.
//!
//! Every fetched version of a page is one [`SourceVersion`] row. Rows are insert-only: a new
//! content hash for a known URL creates a new version, and an existing version is never changed,
//! so a citation that names a version id keeps resolving to the bytes it was checked against.
//!
//! Persistence is append-only JSON Lines, one record per line (file mode 0600 on unix), the same
//! approach as [`crate::ledger`]. Records are either versions or passages. Reload is tolerant:
//! unparsable lines, duplicate ids and version ids that do not match their content are skipped
//! and counted in [`Archive::corrupt_lines`].

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::hex;
use crate::search::normalise_url;

/// How completely a version's body was read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum Completeness {
    /// The whole document was read.
    Complete,
    /// Only `read` of `total` bytes were read (size cap or budget).
    Truncated { read: u64, total: u64 },
    /// Some pages of a paged document were not fetched.
    Partial { missing_pages: Vec<u32> },
    /// The body was fetched but could not be parsed.
    ParseError(String),
}

/// One immutable fetched version of a source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceVersion {
    /// Deterministic id, see [`version_id`]. Filled in by [`Archive::append_version`] when empty.
    pub id: String,
    /// Requested URL.
    pub url: String,
    /// Canonical URL declared by the page, if any.
    pub canonical_url: Option<String>,
    /// URLs passed through on the way to `url`, in order.
    pub redirect_chain: Vec<String>,
    /// Unix seconds of retrieval.
    pub retrieved_at: u64,
    /// Hex SHA-256 of the body.
    pub content_hash: String,
    /// Publication date as the page states it, if known.
    pub published_at: Option<String>,
    /// How completely the body was read.
    pub completeness: Completeness,
    /// The text of the version.
    pub body: String,
}

/// A span of a version's body that evidence can cite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Passage {
    /// The version this passage belongs to.
    pub version_id: String,
    /// Position of the passage within the version.
    pub ord: u32,
    /// Page number, for paged sources.
    pub page: Option<u32>,
    /// Byte offset of the start within the body.
    pub start: usize,
    /// Byte offset of the end within the body.
    pub end: usize,
    /// The passage text.
    pub text: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "rec", rename_all = "snake_case")]
enum Record {
    Version(SourceVersion),
    Passage(Passage),
}

/// Deterministic version id: first 16 hex chars of `sha256(url + "\n" + content_hash + "\n" + retrieved_at)`.
pub fn version_id(url: &str, content_hash: &str, retrieved_at: u64) -> String {
    let mut h = Sha256::new();
    h.update(url.as_bytes());
    h.update(b"\n");
    h.update(content_hash.as_bytes());
    h.update(b"\n");
    h.update(retrieved_at.to_string().as_bytes());
    hex(&h.finalize())[..16].to_owned()
}

/// Key for grouping versions by URL: the normalised URL, or the trimmed raw string.
fn url_key(url: &str) -> String {
    normalise_url(url).unwrap_or_else(|| url.trim().to_owned())
}

/// The append-only source archive. Optionally backed by a JSONL file.
#[derive(Debug, Default)]
pub struct Archive {
    path: Option<PathBuf>,
    file: Option<File>,
    versions: Vec<SourceVersion>,
    by_id: HashMap<String, usize>,
    by_url: HashMap<String, Vec<usize>>,
    passages: HashMap<String, Vec<Passage>>,
    corrupt_lines: usize,
}

impl Archive {
    /// Empty archive with no file behind it.
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Open (or create) a file-backed archive, loading every intact record.
    ///
    /// Refuses a path that is a symlink. The file is created with mode 0600 on unix.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            fs::create_dir_all(dir)?;
        }
        refuse_symlink(&path)?;
        let bytes = if path.exists() {
            fs::read(&path)?
        } else {
            Vec::new()
        };
        let mut archive = Self {
            path: Some(path.clone()),
            ..Self::default()
        };
        archive.ingest(&bytes);
        let mut file = open_append(&path)?;
        // A crash mid-write can leave a final line without its newline; start a fresh line so the
        // next record is not glued onto the damaged one.
        if bytes.last().is_some_and(|&b| b != b'\n') {
            file.write_all(b"\n")?;
        }
        archive.file = Some(file);
        Ok(archive)
    }

    /// Lines skipped as corrupt, duplicate or inconsistent when the file was loaded.
    pub fn corrupt_lines(&self) -> usize {
        self.corrupt_lines
    }

    /// Path of the backing file, if any.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Append a version. Insert-only.
    ///
    /// * If the latest stored version for this URL has the same `content_hash`, this is a no-op.
    /// * Otherwise a new row is written; existing rows are never modified.
    ///
    /// `v.id` may be empty (it is then computed) or must equal [`version_id`] of the fields.
    /// The record is written to disk before it becomes visible in memory.
    pub fn append_version(&mut self, mut v: SourceVersion) -> io::Result<()> {
        if v.url.trim().is_empty() || v.content_hash.is_empty() {
            return Err(invalid("version needs a url and a content hash"));
        }
        let key = url_key(&v.url);
        if let Some(&idx) = self.by_url.get(&key).and_then(|ix| ix.last()) {
            if self.versions[idx].content_hash == v.content_hash {
                return Ok(());
            }
        }
        let id = version_id(&v.url, &v.content_hash, v.retrieved_at);
        if !v.id.is_empty() && v.id != id {
            return Err(invalid(
                "version id does not match its url, hash and retrieval time",
            ));
        }
        if self.by_id.contains_key(&id) {
            return Ok(());
        }
        v.id = id;
        self.write_record(&Record::Version(v.clone()))?;
        let idx = self.versions.len();
        self.by_id.insert(v.id.clone(), idx);
        self.by_url.entry(key).or_default().push(idx);
        self.versions.push(v);
        Ok(())
    }

    /// All versions of `url`, oldest first.
    pub fn versions_for_url(&self, url: &str) -> Vec<&SourceVersion> {
        self.by_url
            .get(&url_key(url))
            .map(|ix| ix.iter().map(|&i| &self.versions[i]).collect())
            .unwrap_or_default()
    }

    /// The newest version of `url`.
    pub fn latest_for_url(&self, url: &str) -> Option<&SourceVersion> {
        self.by_url
            .get(&url_key(url))
            .and_then(|ix| ix.last())
            .map(|&i| &self.versions[i])
    }

    /// The version with this id.
    pub fn get(&self, version_id: &str) -> Option<&SourceVersion> {
        self.by_id.get(version_id).map(|&i| &self.versions[i])
    }

    /// Add passages to existing versions. Insert-only: a passage whose `(version_id, ord)` is
    /// already stored is ignored. Every `version_id` must name a stored version, and `start <= end`.
    pub fn add_passages(&mut self, passages: Vec<Passage>) -> io::Result<()> {
        for p in &passages {
            if !self.by_id.contains_key(&p.version_id) {
                return Err(invalid("passage names an unknown version"));
            }
            if p.start > p.end {
                return Err(invalid("passage start is after its end"));
            }
        }
        for p in passages {
            let exists = self
                .passages
                .get(&p.version_id)
                .is_some_and(|ps| ps.iter().any(|q| q.ord == p.ord));
            if exists {
                continue;
            }
            self.write_record(&Record::Passage(p.clone()))?;
            let list = self.passages.entry(p.version_id.clone()).or_default();
            list.push(p);
            list.sort_by_key(|q| q.ord);
        }
        Ok(())
    }

    /// Passages of a version, in `ord` order.
    pub fn passages_for(&self, version_id: &str) -> Vec<&Passage> {
        self.passages
            .get(version_id)
            .map(|ps| ps.iter().collect())
            .unwrap_or_default()
    }

    fn write_record(&mut self, rec: &Record) -> io::Result<()> {
        let Some(file) = self.file.as_mut() else {
            return Ok(());
        };
        let mut line = serde_json::to_string(rec)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        line.push('\n');
        file.write_all(line.as_bytes())
    }

    fn ingest(&mut self, bytes: &[u8]) {
        for raw in bytes.split(|&b| b == b'\n') {
            let text = String::from_utf8_lossy(raw);
            let t = text.trim();
            if t.is_empty() {
                continue;
            }
            match serde_json::from_str::<Record>(t) {
                Ok(Record::Version(v)) => {
                    let ok = version_id(&v.url, &v.content_hash, v.retrieved_at) == v.id
                        && !self.by_id.contains_key(&v.id);
                    if ok {
                        let idx = self.versions.len();
                        self.by_id.insert(v.id.clone(), idx);
                        self.by_url.entry(url_key(&v.url)).or_default().push(idx);
                        self.versions.push(v);
                    } else {
                        self.corrupt_lines += 1;
                    }
                }
                Ok(Record::Passage(p)) => {
                    let dup = self
                        .passages
                        .get(&p.version_id)
                        .is_some_and(|ps| ps.iter().any(|q| q.ord == p.ord));
                    if self.by_id.contains_key(&p.version_id) && p.start <= p.end && !dup {
                        let list = self.passages.entry(p.version_id.clone()).or_default();
                        list.push(p);
                        list.sort_by_key(|q| q.ord);
                    } else {
                        self.corrupt_lines += 1;
                    }
                }
                Err(_) => self.corrupt_lines += 1,
            }
        }
    }
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg)
}

/// Same check as the trace writer in `ostler-workflow`: refuse to follow a symlink at `path`.
fn refuse_symlink(path: &Path) -> io::Result<()> {
    if let Ok(md) = fs::symlink_metadata(path) {
        if md.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "archive path is a symlink; refusing to follow it",
            ));
        }
    }
    Ok(())
}

/// Open for appending with mode 0600 on unix (same approach as the trace writer).
fn open_append(path: &Path) -> io::Result<File> {
    refuse_symlink(path)?;
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
        f.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    Ok(f)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn hash(body: &str) -> String {
        hex(&Sha256::digest(body.as_bytes()))
    }

    fn ver(url: &str, body: &str, at: u64) -> SourceVersion {
        SourceVersion {
            id: String::new(),
            url: url.into(),
            canonical_url: None,
            redirect_chain: vec![],
            retrieved_at: at,
            content_hash: hash(body),
            published_at: Some("2026-01-01".into()),
            completeness: Completeness::Complete,
            body: body.into(),
        }
    }

    fn passage(vid: &str, ord: u32, text: &str) -> Passage {
        Passage {
            version_id: vid.into(),
            ord,
            page: Some(1),
            start: 0,
            end: text.len(),
            text: text.into(),
        }
    }

    fn file_in(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("archive.jsonl")
    }

    #[test]
    fn restart_keeps_cited_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let path = file_in(&dir);
        let body = "Line one.\nLine \"two\" with unicode \u{e9}.";
        let vid = {
            let mut a = Archive::open(&path).unwrap();
            a.append_version(ver("https://example.org/a", body, 100))
                .unwrap();
            let vid = a
                .latest_for_url("https://example.org/a")
                .unwrap()
                .id
                .clone();
            a.add_passages(vec![passage(&vid, 0, "Line one.")]).unwrap();
            vid
        };
        let a = Archive::open(&path).unwrap();
        let got = a.get(&vid).expect("version survives restart");
        assert_eq!(got.body, body);
        assert_eq!(got.content_hash, hash(body));
        let ps = a.passages_for(&vid);
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].text, "Line one.");
        assert_eq!(a.corrupt_lines(), 0);
    }

    #[test]
    fn version_id_is_deterministic_and_checked() {
        let id = version_id("https://example.org/a", &hash("x"), 7);
        assert_eq!(id.len(), 16);
        assert_eq!(id, version_id("https://example.org/a", &hash("x"), 7));
        let mut a = Archive::in_memory();
        let mut v = ver("https://example.org/a", "x", 7);
        v.id = "0000000000000000".into();
        assert!(a.append_version(v).is_err());
    }

    #[test]
    fn changed_page_creates_new_version_old_citation_still_verifies_against_old() {
        let dir = tempfile::tempdir().unwrap();
        let path = file_in(&dir);
        let url = "https://example.org/a";
        let (old_id, new_id) = {
            let mut a = Archive::open(&path).unwrap();
            a.append_version(ver(url, "original text", 100)).unwrap();
            let old_id = a.latest_for_url(url).unwrap().id.clone();
            a.append_version(ver(url, "edited text", 200)).unwrap();
            let new_id = a.latest_for_url(url).unwrap().id.clone();
            (old_id, new_id)
        };
        assert_ne!(old_id, new_id);
        let a = Archive::open(&path).unwrap();
        let all = a.versions_for_url(url);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].body, "original text");
        assert_eq!(all[1].body, "edited text");
        assert_eq!(a.latest_for_url(url).unwrap().id, new_id);
        assert_eq!(a.get(&old_id).unwrap().body, "original text");
        assert_eq!(a.get(&old_id).unwrap().content_hash, hash("original text"));
    }

    #[test]
    fn identical_reappend_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        let path = file_in(&dir);
        let url = "https://example.org/a";
        let mut a = Archive::open(&path).unwrap();
        a.append_version(ver(url, "same", 100)).unwrap();
        a.append_version(ver(url, "same", 100)).unwrap();
        a.append_version(ver(url, "same", 300)).unwrap();
        assert_eq!(a.versions_for_url(url).len(), 1);
        drop(a);
        let raw = fs::read_to_string(&path).unwrap();
        assert_eq!(raw.lines().count(), 1);
    }

    #[test]
    fn corrupt_line_is_skipped_and_counted() {
        let dir = tempfile::tempdir().unwrap();
        let path = file_in(&dir);
        let url = "https://example.org/a";
        let vid = {
            let mut a = Archive::open(&path).unwrap();
            a.append_version(ver(url, "good body", 100)).unwrap();
            a.latest_for_url(url).unwrap().id.clone()
        };
        {
            let mut f = OpenOptions::new().append(true).open(&path).unwrap();
            f.write_all(b"{\"rec\":\"version\",\"id\":\"trunc\n")
                .unwrap();
            f.write_all(b"not json at all\n").unwrap();
        }
        let a = Archive::open(&path).unwrap();
        assert_eq!(a.corrupt_lines(), 2);
        assert_eq!(a.get(&vid).unwrap().body, "good body");
        assert_eq!(a.versions_for_url(url).len(), 1);
    }

    #[test]
    fn tampered_version_id_is_rejected_on_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = file_in(&dir);
        {
            let mut a = Archive::open(&path).unwrap();
            a.append_version(ver("https://example.org/a", "body", 100))
                .unwrap();
        }
        let raw = fs::read_to_string(&path)
            .unwrap()
            .replace("\"retrieved_at\":100", "\"retrieved_at\":101");
        assert!(raw.contains("\"retrieved_at\":101"));
        fs::write(&path, raw).unwrap();
        let a = Archive::open(&path).unwrap();
        assert_eq!(a.corrupt_lines(), 1);
        assert!(a.versions_for_url("https://example.org/a").is_empty());
    }

    #[test]
    fn passages_require_a_known_version() {
        let mut a = Archive::in_memory();
        assert!(a.add_passages(vec![passage("nope", 0, "x")]).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_path_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        fs::write(&target, "keep me").unwrap();
        let link = dir.path().join("archive.jsonl");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(Archive::open(&link).is_err());
        let mut s = String::new();
        File::open(&target).unwrap().read_to_string(&mut s).unwrap();
        assert_eq!(s, "keep me");
    }

    #[cfg(unix)]
    #[test]
    fn file_mode_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = file_in(&dir);
        let mut a = Archive::open(&path).unwrap();
        a.append_version(ver("https://example.org/a", "b", 1))
            .unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
