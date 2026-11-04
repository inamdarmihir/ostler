//! Persistent memory with no server: the in-process store, saved to one file.
//!
//! This is what runs when there is no Qdrant to talk to. An agent's memory is small (tens of points per
//! user), so an exact cosine scan over everything is instant and needs no index. The whole file is
//! encrypted with the memory key (AES-256-GCM, same primitive as sealed Qdrant memory), written atomically,
//! and shared safely between processes: `ostler serve` and `ostler memory ...` can run at the same time.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::sealed::SealKey;
use crate::store::{Filter, Hit, InMemoryStore, Kind, Rank, Store};
use crate::{Embedder, MemoryError, Result};

/// Bound into the ciphertext, so a sealed Qdrant blob cannot be passed off as a store file or the reverse.
const AAD: &str = "ostler/local-store/v1";
const FORMAT: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Doc {
    format: u32,
    dim: usize,
    /// Embedding model that produced the vectors. Absent in files written before it was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    points: Vec<Rec>,
}

#[derive(Serialize, Deserialize)]
struct Rec {
    kind: String,
    id: String,
    vector: Vec<f32>,
    payload: Value,
}

/// Modification time and length of the file as last read or written by this handle.
type Stamp = (SystemTime, u64);

pub struct LocalStore {
    inner: InMemoryStore,
    path: PathBuf,
    /// `None` stores the file as plain JSON (`--seal off`).
    key: Option<SealKey>,
    seen: Mutex<Option<Stamp>>,
}

/// Held while a write is read-modify-written; the lock is released when it is dropped.
struct FileLock {
    _file: File,
}

impl LocalStore {
    /// Open (or prepare to create) the store at `path`. The file is created by the first write.
    /// `master` is the memory key; with `None` the file is plain JSON readable by anyone who can read it.
    pub fn open(
        path: impl Into<PathBuf>,
        embedder: Arc<dyn Embedder>,
        master: Option<[u8; 32]>,
    ) -> Result<Self> {
        let store = Self {
            inner: InMemoryStore::new(embedder),
            path: path.into(),
            key: master.map(SealKey::from_bytes),
            seen: Mutex::new(None),
        };
        store.load()?;
        Ok(store)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn io(&self, what: &str, e: std::io::Error) -> MemoryError {
        MemoryError::Invalid(format!("{what} {}: {e}", self.path.display()))
    }

    fn stamp(&self) -> Option<Stamp> {
        let m = std::fs::metadata(&self.path).ok()?;
        Some((m.modified().ok()?, m.len()))
    }

    fn lock(&self) -> Result<FileLock> {
        if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(|e| self.io("creating the folder for", e))?;
        }
        let mut name = self.path.as_os_str().to_owned();
        name.push(".lock");
        let f = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(PathBuf::from(name))
            .map_err(|e| self.io("opening the lock for", e))?;
        f.lock().map_err(|e| self.io("locking", e))?;
        Ok(FileLock { _file: f })
    }

    /// Replace the in-process copy with what is on disk.
    fn load(&self) -> Result<()> {
        let raw = match std::fs::read_to_string(&self.path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.inner.replace_all(vec![]);
                *self.seen.lock().unwrap() = None;
                return Ok(());
            }
            Err(e) => return Err(self.io("reading", e)),
        };
        let stamp = self.stamp();
        let text = raw.trim();
        let json: Vec<u8> = if text.starts_with('{') {
            text.as_bytes().to_vec()
        } else {
            let key = self.key.as_ref().ok_or_else(|| {
                MemoryError::Invalid(format!(
                    "{} is encrypted but sealing is off, so it cannot be read. Turn sealing back on (`ostler config set seal on`)",
                    self.path.display()
                ))
            })?;
            key.open("", AAD, text, false).ok_or_else(|| {
                MemoryError::Invalid(format!(
                    "{} cannot be opened with the current memory key (wrong key, or the file was changed). Restore the key with `ostler memory key import`, or delete the file to start over",
                    self.path.display()
                ))
            })?
        };
        let doc: Doc = serde_json::from_slice(&json).map_err(|e| {
            MemoryError::Invalid(format!("{} is not a memory file: {e}", self.path.display()))
        })?;
        if doc.format != FORMAT {
            return Err(MemoryError::Invalid(format!(
                "{} is format {}, but this version reads format {FORMAT}; upgrade ostler",
                self.path.display(),
                doc.format
            )));
        }
        let dim = self.inner.embedder().dim();
        if doc.dim != dim {
            return Err(MemoryError::Invalid(format!(
                "{} holds {}-dim vectors but the configured embeddings produce {dim}; switch back, or delete the file to start over",
                self.path.display(),
                doc.dim
            )));
        }
        let model = self.inner.embedder().model_id();
        if let Some(stored) = doc.model.as_deref().filter(|m| *m != model) {
            return Err(MemoryError::Invalid(format!(
                "{} was built with embedding model {stored:?} but the configured model is {model:?}; their vectors are not comparable. \
                 Switch back to {stored:?}, or delete the file to start over",
                self.path.display()
            )));
        }
        let mut points = Vec::with_capacity(doc.points.len());
        for r in doc.points {
            let kind = Kind::from_collection(&r.kind).ok_or_else(|| {
                MemoryError::Invalid(format!(
                    "{} has a point of unknown kind {:?}",
                    self.path.display(),
                    r.kind
                ))
            })?;
            if r.vector.len() != dim {
                return Err(MemoryError::Invalid(format!(
                    "{} has a point with a {}-dim vector, expected {dim}",
                    self.path.display(),
                    r.vector.len()
                )));
            }
            points.push((kind, r.id, r.vector, r.payload));
        }
        self.inner.replace_all(points);
        *self.seen.lock().unwrap() = stamp;
        Ok(())
    }

    /// Reload when another process has written since this handle last looked.
    fn refresh(&self) -> Result<()> {
        if self.stamp() != *self.seen.lock().unwrap() {
            self.load()?;
        }
        Ok(())
    }

    /// Write the in-process copy atomically: temp file in the same folder, fsync, rename.
    fn save(&self) -> Result<()> {
        let doc = Doc {
            format: FORMAT,
            dim: self.inner.embedder().dim(),
            model: Some(self.inner.embedder().model_id()),
            points: self
                .inner
                .export()
                .into_iter()
                .map(|(kind, id, vector, payload)| Rec {
                    kind: kind.collection().into(),
                    id,
                    vector,
                    payload,
                })
                .collect(),
        };
        let json = serde_json::to_vec(&doc)
            .map_err(|e| MemoryError::Invalid(format!("serialising memory: {e}")))?;
        let bytes = match &self.key {
            Some(k) => k.seal(AAD, &json)?.into_bytes(),
            None => json,
        };
        let mut tmp_name = self.path.as_os_str().to_owned();
        tmp_name.push(format!(".tmp{}", std::process::id()));
        let tmp = PathBuf::from(tmp_name);
        let mut opts = OpenOptions::new();
        opts.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let write = || -> std::io::Result<()> {
            let mut f = opts.open(&tmp)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
            std::fs::rename(&tmp, &self.path)
        };
        if let Err(e) = write() {
            let _ = std::fs::remove_file(&tmp);
            return Err(self.io("writing", e));
        }
        #[cfg(unix)]
        if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            if let Ok(d) = File::open(dir) {
                let _ = d.sync_all();
            }
        }
        *self.seen.lock().unwrap() = self.stamp();
        Ok(())
    }
}

#[async_trait]
impl Store for LocalStore {
    async fn ensure(&self) -> Result<()> {
        let _lock = self.lock()?;
        self.load()
    }

    async fn upsert(&self, kind: Kind, id: &str, text: &str, payload: Value) -> Result<()> {
        let vector = self
            .inner
            .embedder()
            .embed(&[text.to_string()])
            .await?
            .pop()
            .ok_or_else(|| MemoryError::Embed("embedder returned no vector".into()))?;
        let _lock = self.lock()?;
        self.load()?;
        self.inner.put_vector(kind, id, vector, payload);
        self.save()
    }

    async fn search(
        &self,
        kind: Kind,
        text: &str,
        filter: &Filter,
        limit: usize,
    ) -> Result<Vec<Hit>> {
        self.refresh()?;
        self.inner.search(kind, text, filter, limit).await
    }

    async fn search_ranked(
        &self,
        kind: Kind,
        text: &str,
        filter: &Filter,
        limit: usize,
        rank: &Rank,
    ) -> Result<Vec<Hit>> {
        self.refresh()?;
        self.inner
            .search_ranked(kind, text, filter, limit, rank)
            .await
    }

    async fn scroll(&self, kind: Kind, filter: &Filter, limit: usize) -> Result<Vec<Hit>> {
        self.refresh()?;
        self.inner.scroll(kind, filter, limit).await
    }

    async fn delete(&self, kind: Kind, id: &str) -> Result<()> {
        let _lock = self.lock()?;
        self.load()?;
        self.inner.delete(kind, id).await?;
        self.save()
    }

    async fn delete_matching(&self, kind: Kind, filter: &Filter) -> Result<()> {
        let _lock = self.lock()?;
        self.load()?;
        self.inner.delete_matching(kind, filter).await?;
        self.save()
    }

    async fn count(&self, kind: Kind, filter: &Filter) -> Result<u64> {
        self.refresh()?;
        self.inner.count(kind, filter).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HashEmbedder, Memory};
    use serde_json::json;

    const KEY: [u8; 32] = [7; 32];

    fn open(path: &Path, key: Option<[u8; 32]>) -> Result<Arc<LocalStore>> {
        Ok(Arc::new(LocalStore::open(
            path,
            Arc::new(HashEmbedder::default()),
            key,
        )?))
    }

    fn memory(store: Arc<LocalStore>) -> Memory {
        Memory::new(store, "t1")
    }

    #[tokio::test]
    async fn memory_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("memory.local");
        let m = memory(open(&p, Some(KEY)).unwrap());
        m.remember("city", json!("Pune"), None, None, "user")
            .await
            .unwrap();
        drop(m);
        let m = memory(open(&p, Some(KEY)).unwrap());
        assert_eq!(m.fact("city").await.unwrap().unwrap().value, json!("Pune"));
        assert_eq!(m.recall("city", 3).await.unwrap()[0].0.key, "city");
    }

    #[tokio::test]
    async fn the_file_holds_no_plaintext_when_sealed() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("memory.local");
        let m = memory(open(&p, Some(KEY)).unwrap());
        m.remember("city", json!("Pune"), None, None, "user")
            .await
            .unwrap();
        let raw = std::fs::read_to_string(&p).unwrap();
        assert!(!raw.contains("Pune") && !raw.contains("city"), "{raw}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[tokio::test]
    async fn a_wrong_key_or_edited_file_is_refused_not_treated_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("memory.local");
        memory(open(&p, Some(KEY)).unwrap())
            .remember("city", json!("Pune"), None, None, "user")
            .await
            .unwrap();
        let err = open(&p, Some([8; 32]))
            .err()
            .expect("wrong key")
            .to_string();
        assert!(err.contains("memory key"), "{err}");
        assert!(
            open(&p, None).is_err(),
            "sealing off cannot read a sealed file"
        );

        let mut raw = std::fs::read_to_string(&p).unwrap();
        let last = raw.pop().unwrap();
        raw.push(if last == 'A' { 'B' } else { 'A' });
        std::fs::write(&p, raw).unwrap();
        assert!(
            open(&p, Some(KEY)).is_err(),
            "a changed byte must fail authentication"
        );
    }

    #[tokio::test]
    async fn a_plain_file_is_read_and_upgraded_to_sealed_on_the_next_write() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("memory.local");
        memory(open(&p, None).unwrap())
            .remember("city", json!("Pune"), None, None, "user")
            .await
            .unwrap();
        assert!(std::fs::read_to_string(&p).unwrap().contains("Pune"));
        let m = memory(open(&p, Some(KEY)).unwrap());
        assert!(m.fact("city").await.unwrap().is_some());
        m.remember("zip", json!("411001"), None, None, "user")
            .await
            .unwrap();
        assert!(!std::fs::read_to_string(&p).unwrap().contains("Pune"));
    }

    #[tokio::test]
    async fn two_handles_see_each_others_writes_and_neither_loses_any() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("memory.local");
        // Two handles stand in for `ostler serve` and `ostler memory remember` running together.
        let a = memory(open(&p, Some(KEY)).unwrap());
        let b = memory(open(&p, Some(KEY)).unwrap());
        a.remember("city", json!("Pune"), None, None, "user")
            .await
            .unwrap();
        b.remember("zip", json!("411001"), None, None, "user")
            .await
            .unwrap();
        a.remember("phone", json!("12345"), None, None, "user")
            .await
            .unwrap();
        for m in [&a, &b] {
            let keys: Vec<String> = m
                .facts(10)
                .await
                .unwrap()
                .into_iter()
                .map(|f| f.key)
                .collect();
            assert_eq!(keys.len(), 3, "{keys:?}");
        }
        b.forget("city").await.unwrap();
        assert!(a.fact("city").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn changing_the_embedding_size_is_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("memory.local");
        memory(open(&p, Some(KEY)).unwrap())
            .remember("city", json!("Pune"), None, None, "user")
            .await
            .unwrap();
        let err = LocalStore::open(&p, Arc::new(HashEmbedder::new(384)), Some(KEY))
            .err()
            .expect("dimension mismatch")
            .to_string();
        assert!(err.contains("256-dim") && err.contains("384"), "{err}");
    }

    #[tokio::test]
    async fn wipe_persists() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("memory.local");
        let m = memory(open(&p, Some(KEY)).unwrap());
        m.remember("city", json!("Pune"), None, None, "user")
            .await
            .unwrap();
        m.wipe().await.unwrap();
        assert!(memory(open(&p, Some(KEY)).unwrap())
            .facts(10)
            .await
            .unwrap()
            .is_empty());
    }

    /// Same dimension as `HashEmbedder`, different model.
    struct OtherModel(HashEmbedder);

    #[async_trait::async_trait]
    impl Embedder for OtherModel {
        fn dim(&self) -> usize {
            self.0.dim()
        }
        fn model_id(&self) -> String {
            "other-model".into()
        }
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            self.0.embed(texts).await
        }
    }

    #[tokio::test]
    async fn changing_the_embedding_model_at_the_same_size_is_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("memory.local");
        memory(open(&p, Some(KEY)).unwrap())
            .remember("city", json!("Pune"), None, None, "user")
            .await
            .unwrap();
        let err = LocalStore::open(&p, Arc::new(OtherModel(HashEmbedder::default())), Some(KEY))
            .err()
            .expect("model mismatch")
            .to_string();
        assert!(
            err.contains("hash-256d")
                && err.contains("other-model")
                && err.contains("delete the file"),
            "{err}"
        );
        // The same model reopens fine.
        assert!(open(&p, Some(KEY)).is_ok());
    }
}
