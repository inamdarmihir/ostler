//! Source memory: chunks of pages the agent read, found again by meaning *and* by exact words.
//!
//! One chunk is one point in its own collection (`ostler_sources`), with two vectors: `dense` (the
//! embedder, for meaning) and `bm25` (a sparse BM25-style vector computed here, for names, identifiers and
//! part numbers; see [`crate::sparse`]). A recall runs both, fuses them with weighted Reciprocal Rank Fusion
//! ([`crate::fusion`]), then picks the result with MMR so five chunks of one page do not crowd out other
//! sources, and caps how many chunks any one URL contributes.
//!
//! **Where each step runs.** On Qdrant 1.17 or newer the two rankings and the fusion are one Query API call
//! (two prefetches, `rrf` with weights). On older servers, the in-process store and the local file store the
//! same fusion runs on the client from two rankings. MMR and per-URL grouping always run on the client,
//! over the dense vectors the store returns, so results do not depend on the server version. (Qdrant also
//! offers server-side MMR since 1.15, probed against 1.20.0; it is not used because it cannot also apply the
//! per-URL cap in the same pass and would make the behaviour differ between stores.)
//!
//! **Sealing.** With a [`crate::SealedStore`] the chunk text, headings, title and URL are encrypted like every
//! other kind, and the sparse term ids are keyed hashes; `docs` on [`crate::sparse`] lists what the server
//! can still observe. The fetch time stays in clear (as `verified_ts`) because the server expires on it.
//!
//! **Retention.** Chunks expire `source_retain_days` after they were fetched (default
//! [`DEFAULT_SOURCE_RETAIN_DAYS`], `0` keeps them), independent of the learned-skill retention. Reading a
//! page again refreshes its clock. [`Memory::prune`] enforces it.
//!
//! **Migration of the older kinds.** `ostler_profile`, `ostler_playbooks` and `ostler_episodes` keep their
//! single unnamed vector and are not changed by this module. They can gain a sparse vector later without
//! being recreated, using Qdrant's "update vector schema" (`PATCH /collections/{name}` adding the sparse
//! vector, available from Qdrant 1.18 per the build plan); that is future work and nothing here depends on it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::fusion::{mmr_select, rrf_max, MmrItem, DEFAULT_RRF_K};
use crate::sparse::{doc_vector, query_vector};
use crate::store::{Filter, HybridQuery, Kind};
use crate::{contains_card_number, now, point_id, scroll_all, stripe, Memory, MemoryError, Result};

/// Default for [`Memory::with_source_retain_days`]: how long a fetched page stays in memory.
pub const DEFAULT_SOURCE_RETAIN_DAYS: u64 = 30;
/// Most chunks kept for one URL. More is almost certainly a chunker bug, and refusing beats filling memory.
pub const MAX_CHUNKS_PER_URL: usize = 2000;
/// Longest chunk text kept, in characters; the rest is cut.
pub const MAX_CHUNK_CHARS: usize = 8000;
const MAX_URL_LEN: usize = 2048;

/// One passage of a page the agent read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceChunk {
    /// Address of the page (the caller's canonical form: it is what `forget` and `replace` match on).
    pub url: String,
    /// Stable id of this chunk within the page. Saving the same `(url, chunk_id)` again replaces it.
    pub chunk_id: String,
    /// Position of the chunk in the page, from 0.
    pub ordinal: u32,
    /// Headings above the chunk, outermost first.
    #[serde(default)]
    pub heading_path: Vec<String>,
    /// The passage itself.
    pub text: String,
    /// When the page was fetched (seconds since the epoch). `0` means "now".
    pub fetched_at: u64,
    /// Hash of the whole page content, so a changed page is recognised.
    pub content_hash: String,
    /// The page title.
    #[serde(default)]
    pub title: Option<String>,
}

impl SourceChunk {
    /// What the embedder and the tokenizer see: title, headings, then the passage.
    fn index_text(&self) -> String {
        let mut s = String::new();
        if let Some(t) = &self.title {
            s.push_str(t);
            s.push('\n');
        }
        if !self.heading_path.is_empty() {
            s.push_str(&self.heading_path.join(" > "));
            s.push('\n');
        }
        s.push_str(&self.text);
        s
    }
}

/// How [`Memory::recall_sources`] searches.
#[derive(Debug, Clone)]
pub struct SourceRecallOptions {
    /// At most this many chunks from one URL (`0` = no cap).
    pub max_per_url: usize,
    /// MMR diversity from `0.0` (pure relevance) to `1.0` (as different from each other as possible).
    pub diversity: f32,
    /// Only chunks of this URL.
    pub url: Option<String>,
    /// Drop results whose score is below this. Scores are fused scores scaled to `(0, 1]`: a chunk ranked
    /// first by every ranking scores `1.0`, one that only a single ranking found at best about `0.5`
    /// (with equal weights), so `0.6` or more means "both the meaning and the words agree".
    pub min_score: f32,
    /// RRF weight of the dense (meaning) ranking.
    pub dense_weight: f32,
    /// RRF weight of the sparse (exact words) ranking. `0.0` leaves it out.
    pub sparse_weight: f32,
    /// RRF constant ([`DEFAULT_RRF_K`]), rounded to a whole number (Qdrant takes an integer).
    pub rrf_k: f32,
}

impl Default for SourceRecallOptions {
    fn default() -> Self {
        Self {
            max_per_url: 2,
            diversity: 0.3,
            url: None,
            min_score: 0.0,
            dense_weight: 1.0,
            sparse_weight: 1.0,
            rrf_k: DEFAULT_RRF_K,
        }
    }
}

/// A recalled passage.
#[derive(Debug, Clone)]
pub struct SourceHit {
    /// The passage and where it came from.
    pub chunk: SourceChunk,
    /// Fused relevance in `(0, 1]`; see [`SourceRecallOptions::min_score`].
    pub score: f32,
}

/// What [`Memory::save_sources`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SaveSourcesReport {
    /// Chunks written (new, or whose text or metadata changed).
    pub stored: usize,
    /// Chunks already stored identically (only their fetch time was refreshed, if it differed).
    pub unchanged: usize,
    /// Earlier chunks of the same URL that are no longer part of the page, now removed.
    pub removed: usize,
    /// Chunks not stored because they contain a card-number-like run.
    pub skipped_sensitive: usize,
}

/// One remembered page, for `memory list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSummary {
    /// Address of the page.
    pub url: String,
    /// Title, when known.
    pub title: Option<String>,
    /// Chunks stored for it.
    pub chunks: usize,
    /// When it was last fetched.
    pub fetched_at: u64,
    /// Content hash of the stored version.
    pub content_hash: String,
    /// Characters of text stored.
    pub chars: usize,
}

/// Totals for `doctor`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SourceStats {
    /// Distinct pages.
    pub urls: usize,
    /// Chunks.
    pub chunks: usize,
    /// Characters of chunk text.
    pub chars: usize,
    /// Fetch time of the oldest chunk.
    pub oldest_fetched_at: Option<u64>,
    /// Fetch time of the newest chunk.
    pub newest_fetched_at: Option<u64>,
    /// Chunks already past the retention period, waiting for the next prune.
    pub expired_chunks: usize,
    /// The retention period in force, in days (`0` = keep).
    pub retain_days: u64,
}

fn chunk_from(payload: &Value) -> Option<SourceChunk> {
    serde_json::from_value(payload["source"].clone()).ok()
}

impl Memory {
    /// Keep pages for this many days (`0` = forever). Default [`DEFAULT_SOURCE_RETAIN_DAYS`].
    #[must_use]
    pub fn with_source_retain_days(mut self, days: u64) -> Self {
        self.source_retain_days = days;
        self
    }

    /// The source retention in force, in days (`0` = forever).
    pub fn source_retain_days(&self) -> u64 {
        self.source_retain_days
    }

    fn source_id(&self, url: &str, chunk_id: &str) -> String {
        point_id(&[&self.tenant, "source", url, chunk_id])
    }

    fn source_payload(&self, c: &SourceChunk) -> Value {
        json!({
            "tenant": self.tenant, "url": c.url,
            // The only clear timestamp: Qdrant expires sources on it.
            "verified_ts": c.fetched_at,
            "source": c,
        })
    }

    async fn put_chunk(&self, c: &SourceChunk) -> Result<String> {
        let text = c.index_text();
        let sparse = doc_vector(&text, &self.store.term_hasher());
        let id = self.source_id(&c.url, &c.chunk_id);
        self.store
            .upsert_source(&id, &text, sparse, self.source_payload(c))
            .await?;
        Ok(id)
    }

    /// Replace everything remembered for `url` with `chunks`.
    ///
    /// Chunks are matched by `chunk_id`: an identical one is left alone (its fetch time is refreshed, so a
    /// page that keeps being read does not expire), a changed one is overwritten, a new one is added, and an
    /// earlier chunk of this URL that is not in `chunks` is removed. New data is written before old data is
    /// removed, so a failure part-way leaves a superset and the call can simply be repeated. An empty `chunks`
    /// removes the page. Chunks with a card-number-like run are not stored (counted in the report), and text
    /// over [`MAX_CHUNK_CHARS`] is cut.
    pub async fn save_sources(
        &self,
        url: &str,
        chunks: Vec<SourceChunk>,
    ) -> Result<SaveSourcesReport> {
        let url = url.trim();
        if url.is_empty() || url.len() > MAX_URL_LEN {
            return Err(MemoryError::Invalid(format!(
                "url must be 1 to {MAX_URL_LEN} bytes"
            )));
        }
        if chunks.len() > MAX_CHUNKS_PER_URL {
            return Err(MemoryError::Invalid(format!(
                "{} chunks for one page; the limit is {MAX_CHUNKS_PER_URL}",
                chunks.len()
            )));
        }
        let now = now();
        let mut wanted: BTreeMap<String, SourceChunk> = BTreeMap::new();
        let mut report = SaveSourcesReport::default();
        for mut c in chunks {
            if c.url.trim() != url {
                return Err(MemoryError::Invalid(format!(
                    "chunk {:?} belongs to {:?}, not {url:?}",
                    c.chunk_id, c.url
                )));
            }
            if c.chunk_id.is_empty() {
                return Err(MemoryError::Invalid("chunk_id must not be empty".into()));
            }
            if wanted.contains_key(&c.chunk_id) {
                return Err(MemoryError::Invalid(format!(
                    "chunk_id {:?} appears twice for {url:?}",
                    c.chunk_id
                )));
            }
            c.url = url.to_string();
            if c.fetched_at == 0 {
                c.fetched_at = now;
            }
            if c.text.chars().count() > MAX_CHUNK_CHARS {
                c.text = c.text.chars().take(MAX_CHUNK_CHARS).collect();
            }
            if contains_card_number(&c.text) {
                report.skipped_sensitive += 1;
                continue;
            }
            wanted.insert(c.chunk_id.clone(), c);
        }

        let _guard = stripe(&point_id(&[&self.tenant, "source-url", url]))
            .lock()
            .await;
        let existing: BTreeMap<String, crate::Hit> = scroll_all(
            self.store.as_ref(),
            Kind::Source,
            &Filter::tenant(&self.tenant).and("url", json!(url)),
        )
        .await?
        .into_iter()
        .filter_map(|h| Some((chunk_from(&h.payload)?.chunk_id, h)))
        .collect();

        for (chunk_id, c) in &wanted {
            if let Some(old) = existing.get(chunk_id) {
                if let Some(prev) = chunk_from(&old.payload) {
                    let same = SourceChunk {
                        fetched_at: c.fetched_at,
                        ..prev.clone()
                    } == *c;
                    if same {
                        if prev.fetched_at != c.fetched_at {
                            self.store
                                .replace_payload(Kind::Source, &old.id, self.source_payload(c))
                                .await?;
                        }
                        report.unchanged += 1;
                        continue;
                    }
                }
            }
            self.put_chunk(c).await?;
            report.stored += 1;
        }
        for (chunk_id, old) in &existing {
            if !wanted.contains_key(chunk_id) {
                self.store.delete(Kind::Source, &old.id).await?;
                report.removed += 1;
            }
        }
        Ok(report)
    }

    /// Passages that answer `query`, best first, at most `k`.
    ///
    /// Dense and sparse rankings are fused (weighted RRF), then [`mmr_select`] picks `k` with
    /// [`SourceRecallOptions::diversity`] and at most [`SourceRecallOptions::max_per_url`] chunks per page.
    pub async fn recall_sources(
        &self,
        query: &str,
        k: usize,
        opts: &SourceRecallOptions,
    ) -> Result<Vec<SourceHit>> {
        if k == 0 || query.trim().is_empty() {
            return Ok(vec![]);
        }
        let (wd, ws) = (opts.dense_weight.max(0.0), opts.sparse_weight.max(0.0));
        if !(wd.is_finite() && ws.is_finite()) || wd + ws <= 0.0 {
            return Err(MemoryError::Invalid(
                "dense_weight and sparse_weight must be finite and not both zero".into(),
            ));
        }
        if !(opts.rrf_k.is_finite() && opts.rrf_k >= 1.0) {
            return Err(MemoryError::Invalid("rrf_k must be at least 1".into()));
        }
        // Qdrant takes `k` as an integer; use the same value on every path.
        let rrf_k = opts.rrf_k.round();
        let mut filter = Filter::tenant(&self.tenant);
        if let Some(u) = &opts.url {
            filter = filter.and("url", json!(u.trim()));
        }
        let sparse = query_vector(query, &self.store.term_hasher());
        let pool = k.saturating_mul(6).clamp(30, 300);
        let hits = self
            .store
            .search_hybrid(&HybridQuery {
                text: query,
                sparse: &sparse,
                filter: &filter,
                pool,
                dense_weight: wd,
                sparse_weight: ws,
                rrf_k,
            })
            .await?;
        let mut weights = vec![wd];
        if !sparse.is_empty() && ws > 0.0 {
            weights.push(ws);
        }
        let best = rrf_max(&weights, rrf_k).max(f32::MIN_POSITIVE);
        let cands: Vec<(SourceChunk, f32, Option<Vec<f32>>)> = hits
            .into_iter()
            .filter_map(|h| {
                let chunk = chunk_from(&h.hit.payload)?;
                let score = (h.hit.score / best).min(1.0);
                // A zero score means only a ranking with weight 0 listed it: not a match.
                (score > 0.0 && score >= opts.min_score).then_some((chunk, score, h.vector))
            })
            .collect();
        let items: Vec<MmrItem<'_>> = cands
            .iter()
            .map(|(c, score, v)| MmrItem {
                relevance: *score,
                vector: v.as_deref(),
                group: Some(c.url.as_str()),
            })
            .collect();
        let picked = mmr_select(&items, k, opts.diversity, opts.max_per_url);
        Ok(picked
            .into_iter()
            .map(|i| SourceHit {
                chunk: cands[i].0.clone(),
                score: cands[i].1,
            })
            .collect())
    }

    /// Forget one page (`Some(url)`) or every page (`None`). Returns how many chunks went.
    pub async fn forget_sources(&self, url: Option<&str>) -> Result<usize> {
        let mut filter = Filter::tenant(&self.tenant);
        if let Some(u) = url {
            filter = filter.and("url", json!(u.trim()));
        }
        let n = self.store.count(Kind::Source, &filter).await? as usize;
        self.store.delete_matching(Kind::Source, &filter).await?;
        Ok(n)
    }

    /// Forget pages fetched more than `retain_days` ago (`0` = keep everything). Returns how many chunks
    /// went. [`Memory::prune`] calls this with [`Memory::source_retain_days`].
    pub async fn prune_sources(&self, retain_days: u64) -> Result<usize> {
        if retain_days == 0 {
            return Ok(0);
        }
        let cutoff = now().saturating_sub(retain_days.saturating_mul(86_400));
        let old = Filter::tenant(&self.tenant).below("verified_ts", cutoff as f64);
        let n = self.store.count(Kind::Source, &old).await? as usize;
        self.store.delete_matching(Kind::Source, &old).await?;
        Ok(n)
    }

    /// Remembered pages, most recently fetched first (at most `limit`). Reads every chunk of the tenant to
    /// group them, which is fine for the thousands of chunks a person accumulates.
    pub async fn list_sources(&self, limit: usize) -> Result<Vec<SourceSummary>> {
        let mut by_url: BTreeMap<String, SourceSummary> = BTreeMap::new();
        for h in scroll_all(
            self.store.as_ref(),
            Kind::Source,
            &Filter::tenant(&self.tenant),
        )
        .await?
        {
            let Some(c) = chunk_from(&h.payload) else {
                continue;
            };
            let s = by_url
                .entry(c.url.clone())
                .or_insert_with(|| SourceSummary {
                    url: c.url.clone(),
                    title: None,
                    chunks: 0,
                    fetched_at: 0,
                    content_hash: String::new(),
                    chars: 0,
                });
            s.chunks += 1;
            s.chars += c.text.chars().count();
            if s.title.is_none() {
                s.title = c.title.clone();
            }
            if c.fetched_at >= s.fetched_at {
                s.fetched_at = c.fetched_at;
                s.content_hash = c.content_hash.clone();
            }
        }
        let mut v: Vec<SourceSummary> = by_url.into_values().collect();
        v.sort_by(|a, b| {
            b.fetched_at
                .cmp(&a.fetched_at)
                .then_with(|| a.url.cmp(&b.url))
        });
        v.truncate(limit);
        Ok(v)
    }

    /// Counts and ages of source memory, for `doctor`.
    pub async fn source_stats(&self) -> Result<SourceStats> {
        let mut stats = SourceStats {
            retain_days: self.source_retain_days,
            ..Default::default()
        };
        let cutoff = (self.source_retain_days > 0)
            .then(|| now().saturating_sub(self.source_retain_days.saturating_mul(86_400)));
        let mut urls = std::collections::BTreeSet::new();
        for h in scroll_all(
            self.store.as_ref(),
            Kind::Source,
            &Filter::tenant(&self.tenant),
        )
        .await?
        {
            let Some(c) = chunk_from(&h.payload) else {
                continue;
            };
            urls.insert(c.url.clone());
            stats.chunks += 1;
            stats.chars += c.text.chars().count();
            stats.oldest_fetched_at = Some(
                stats
                    .oldest_fetched_at
                    .map_or(c.fetched_at, |o| o.min(c.fetched_at)),
            );
            stats.newest_fetched_at = Some(
                stats
                    .newest_fetched_at
                    .map_or(c.fetched_at, |o| o.max(c.fetched_at)),
            );
            if cutoff.is_some_and(|cut| c.fetched_at < cut) {
                stats.expired_chunks += 1;
            }
        }
        stats.urls = urls.len();
        Ok(stats)
    }

    /// Part of [`Memory::forget`]: drop remembered page text that echoes the forgotten fact's value (3+
    /// characters, case-insensitive), the same rule as for episodes. A page that mentions your city is
    /// fetched again if you need it; a copy of your email address in the cache is not worth keeping.
    pub(crate) async fn forget_sources_echoing(&self, needle: &str) -> Result<()> {
        for h in scroll_all(
            self.store.as_ref(),
            Kind::Source,
            &Filter::tenant(&self.tenant),
        )
        .await?
        {
            let Some(c) = chunk_from(&h.payload) else {
                continue;
            };
            let hay =
                format!("{} {}", c.title.as_deref().unwrap_or_default(), c.text).to_lowercase();
            if hay.contains(needle) {
                self.store.delete(Kind::Source, &h.id).await?;
            }
        }
        Ok(())
    }

    /// For `migrate_sealed`: save a chunk read back from the store again under the current format.
    pub(crate) async fn resave_source(&self, payload: &Value) -> Result<Option<String>> {
        match chunk_from(payload) {
            Some(c) => Ok(Some(self.put_chunk(&c).await?)),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HashEmbedder, InMemoryStore, Store};
    use std::sync::Arc;

    fn mem() -> Memory {
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new(Arc::new(HashEmbedder::default())));
        Memory::new(store, "t1")
    }

    fn chunk(url: &str, id: &str, ordinal: u32, text: &str, hash: &str) -> SourceChunk {
        SourceChunk {
            url: url.into(),
            chunk_id: id.into(),
            ordinal,
            heading_path: vec!["Guide".into(), format!("Part {ordinal}")],
            text: text.into(),
            fetched_at: now(),
            content_hash: hash.into(),
            title: Some("A page".into()),
        }
    }

    fn page(url: &str, hash: &str, texts: &[&str]) -> Vec<SourceChunk> {
        texts
            .iter()
            .enumerate()
            .map(|(i, t)| chunk(url, &format!("c{i}"), i as u32, t, hash))
            .collect()
    }

    const A: &str = "https://a.example/rust";
    const B: &str = "https://b.example/cooking";

    async fn seeded() -> Memory {
        let m = mem();
        m.save_sources(
            A,
            page(
                A,
                "h1",
                &[
                    "Rust ownership means each value has a single owner and borrowing rules",
                    "The borrow checker rejects dangling references at compile time",
                    "Cargo builds crates and resolves dependency versions",
                ],
            ),
        )
        .await
        .unwrap();
        m.save_sources(
            B,
            page(
                B,
                "h2",
                &[
                    "Whisk the eggs then fold in flour for a light sponge cake",
                    "Preheat the oven to 180 degrees and grease the baking tin",
                ],
            ),
        )
        .await
        .unwrap();
        m
    }

    #[tokio::test]
    async fn save_and_recall_round_trip() {
        let m = seeded().await;
        let hits = m
            .recall_sources("borrow checker dangling references", 3, &Default::default())
            .await
            .unwrap();
        assert_eq!(hits[0].chunk.chunk_id, "c1");
        assert_eq!(hits[0].chunk.url, A);
        assert_eq!(hits[0].chunk.heading_path, ["Guide", "Part 1"]);
        assert!(hits[0].score > 0.0 && hits[0].score <= 1.0);
        let cake = m
            .recall_sources("sponge cake eggs flour", 1, &Default::default())
            .await
            .unwrap();
        assert_eq!(cake[0].chunk.url, B);
    }

    #[tokio::test]
    async fn exact_terms_win_through_the_sparse_side() {
        let m = mem();
        let url = "https://parts.example/catalogue";
        m.save_sources(
            url,
            page(
                url,
                "h",
                &[
                    "The gasket kit ships in grey packaging with a printed manual",
                    "Replacement part ZX-4471 fits the left hinge only",
                    "Hinges are sold separately and come in several finishes",
                ],
            ),
        )
        .await
        .unwrap();
        let opts = SourceRecallOptions {
            dense_weight: 0.0,
            max_per_url: 0,
            ..Default::default()
        };
        // Lexical only: the identifier finds its chunk and nothing unrelated is invented.
        let hits = m.recall_sources("zx 4471", 3, &opts).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].chunk.chunk_id, "c1");
        assert!(hits[0].score > 0.99, "first in every ranking it is in");
    }

    #[tokio::test]
    async fn saving_again_is_idempotent() {
        let m = seeded().await;
        let r = m
            .save_sources(
                A,
                page(
                    A,
                    "h1",
                    &[
                        "Rust ownership means each value has a single owner and borrowing rules",
                        "The borrow checker rejects dangling references at compile time",
                        "Cargo builds crates and resolves dependency versions",
                    ],
                ),
            )
            .await
            .unwrap();
        assert_eq!(
            r,
            SaveSourcesReport {
                stored: 0,
                unchanged: 3,
                removed: 0,
                skipped_sensitive: 0
            }
        );
        assert_eq!(m.source_stats().await.unwrap().chunks, 5);
    }

    #[tokio::test]
    async fn a_changed_page_replaces_its_chunks_and_drops_the_stale_ones() {
        let m = seeded().await;
        let r = m
            .save_sources(
                A,
                page(
                    A,
                    "h1-v2",
                    &["Rust now has a completely rewritten chapter on lifetimes"],
                ),
            )
            .await
            .unwrap();
        assert_eq!((r.stored, r.removed, r.unchanged), (1, 2, 0));
        let in_a = m
            .recall_sources(
                "borrow checker",
                10,
                &SourceRecallOptions {
                    url: Some(A.into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(in_a.len(), 1);
        assert_eq!(in_a[0].chunk.content_hash, "h1-v2");
        assert!(in_a[0].chunk.text.contains("lifetimes"));
        // The other page is untouched.
        assert_eq!(m.list_sources(10).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn rereading_an_unchanged_page_refreshes_its_clock_only() {
        let m = mem();
        let mut old = page(A, "h", &["Rust ownership and borrowing"]);
        old[0].fetched_at = now() - 20 * 86_400;
        m.save_sources(A, old.clone()).await.unwrap();
        old[0].fetched_at = now();
        let r = m.save_sources(A, old).await.unwrap();
        assert_eq!((r.stored, r.unchanged), (0, 1));
        let s = m.source_stats().await.unwrap();
        assert!(s.newest_fetched_at.unwrap() >= now() - 5);
        // And it now survives a 10 day retention that the old fetch time would have failed.
        assert_eq!(m.prune_sources(10).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn per_url_cap_and_diversity_spread_the_results() {
        let m = mem();
        let long = (0..6)
            .map(|i| format!("Rust ownership detail number {i} about borrowing and moves"))
            .collect::<Vec<_>>();
        let refs: Vec<&str> = long.iter().map(String::as_str).collect();
        m.save_sources(A, page(A, "h", &refs)).await.unwrap();
        m.save_sources(
            B,
            page(
                B,
                "h",
                &["Rust ownership explained again by another author about borrowing"],
            ),
        )
        .await
        .unwrap();
        let capped = m
            .recall_sources(
                "rust ownership borrowing",
                5,
                &SourceRecallOptions {
                    max_per_url: 2,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(capped.len(), 3, "2 from A, 1 from B");
        assert_eq!(capped.iter().filter(|h| h.chunk.url == A).count(), 2);
        let uncapped = m
            .recall_sources(
                "rust ownership borrowing",
                5,
                &SourceRecallOptions {
                    max_per_url: 0,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(uncapped.len(), 5);
    }

    #[tokio::test]
    async fn url_filter_and_min_score() {
        let m = seeded().await;
        let only_b = m
            .recall_sources(
                "rust ownership",
                5,
                &SourceRecallOptions {
                    url: Some(B.into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(only_b.iter().all(|h| h.chunk.url == B));
        // An unreachable bar returns nothing.
        let none = m
            .recall_sources(
                "rust ownership",
                5,
                &SourceRecallOptions {
                    min_score: 1.01,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(none.is_empty());
        // Empty and zero asks are empty, not errors.
        assert!(m
            .recall_sources("  ", 5, &Default::default())
            .await
            .unwrap()
            .is_empty());
        assert!(m
            .recall_sources("rust", 0, &Default::default())
            .await
            .unwrap()
            .is_empty());
        // Nonsense settings are refused.
        let bad = SourceRecallOptions {
            dense_weight: 0.0,
            sparse_weight: 0.0,
            ..Default::default()
        };
        assert!(m.recall_sources("rust", 3, &bad).await.is_err());
    }

    #[tokio::test]
    async fn forget_one_page_or_all() {
        let m = seeded().await;
        assert_eq!(m.forget_sources(Some(A)).await.unwrap(), 3);
        let left = m.list_sources(10).await.unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!((left[0].url.as_str(), left[0].chunks), (B, 2));
        assert_eq!(m.forget_sources(None).await.unwrap(), 2);
        assert_eq!(
            m.source_stats().await.unwrap(),
            SourceStats {
                retain_days: 30,
                ..Default::default()
            }
        );
    }

    #[tokio::test]
    async fn retention_expires_old_pages_and_prune_runs_it() {
        let m = mem().with_source_retain_days(7);
        let mut old = page(A, "h", &["An old page about rust"]);
        old[0].fetched_at = now() - 10 * 86_400;
        m.save_sources(A, old).await.unwrap();
        m.save_sources(B, page(B, "h", &["A fresh page about rust"]))
            .await
            .unwrap();
        let s = m.source_stats().await.unwrap();
        assert_eq!((s.chunks, s.expired_chunks, s.retain_days), (2, 1, 7));
        assert_eq!(
            m.prune(180 * 86_400).await.unwrap(),
            1,
            "prune covers sources"
        );
        let left = m.list_sources(10).await.unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].url, B);
        // 0 keeps everything.
        let keep = mem().with_source_retain_days(0);
        let mut ancient = page(A, "h", &["ancient"]);
        ancient[0].fetched_at = 1;
        keep.save_sources(A, ancient).await.unwrap();
        assert_eq!(keep.prune(1).await.unwrap(), 0);
        assert_eq!(keep.source_stats().await.unwrap().chunks, 1);
        assert_eq!(keep.source_stats().await.unwrap().expired_chunks, 0);
    }

    #[tokio::test]
    async fn default_retention_is_thirty_days() {
        assert_eq!(mem().source_retain_days(), 30);
    }

    #[tokio::test]
    async fn wipe_and_forget_cascade_include_sources() {
        let m = seeded().await;
        m.remember("city", json!("Quillfeather"), None, None, "user")
            .await
            .unwrap();
        m.save_sources(
            "https://c.example/x",
            page(
                "https://c.example/x",
                "h",
                &["Weather today in Quillfeather is mild"],
            ),
        )
        .await
        .unwrap();
        assert!(m.forget("city").await.unwrap());
        let urls: Vec<String> = m
            .list_sources(10)
            .await
            .unwrap()
            .into_iter()
            .map(|s| s.url)
            .collect();
        assert_eq!(
            urls.len(),
            2,
            "the echoing page went, the others stayed: {urls:?}"
        );
        assert!(!urls.contains(&"https://c.example/x".to_string()));
        m.wipe().await.unwrap();
        assert_eq!(m.source_stats().await.unwrap().chunks, 0);
    }

    #[tokio::test]
    async fn tenants_do_not_see_each_others_sources() {
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new(Arc::new(HashEmbedder::default())));
        let a = Memory::new(store.clone(), "a");
        let b = Memory::new(store, "b");
        a.save_sources(A, page(A, "h", &["secret research about rust"]))
            .await
            .unwrap();
        assert!(b
            .recall_sources("rust", 5, &Default::default())
            .await
            .unwrap()
            .is_empty());
        assert_eq!(b.forget_sources(None).await.unwrap(), 0);
        assert_eq!(a.source_stats().await.unwrap().chunks, 1);
    }

    #[tokio::test]
    async fn bad_input_is_refused_and_card_numbers_are_not_stored() {
        let m = mem();
        assert!(m.save_sources(" ", vec![]).await.is_err());
        let mut wrong = page(A, "h", &["x"]);
        wrong[0].url = B.into();
        assert!(
            m.save_sources(A, wrong).await.is_err(),
            "chunk of another page"
        );
        let mut dup = page(A, "h", &["x", "y"]);
        dup[1].chunk_id = "c0".into();
        assert!(m.save_sources(A, dup).await.is_err());
        let r = m
            .save_sources(
                A,
                page(
                    A,
                    "h",
                    &["Pay with 4111 1111 1111 1111 today", "Plain text here"],
                ),
            )
            .await
            .unwrap();
        assert_eq!((r.stored, r.skipped_sensitive), (1, 1));
        // fetched_at 0 means now, long text is cut.
        let mut c = chunk(B, "big", 0, &"word ".repeat(5000), "h");
        c.fetched_at = 0;
        m.save_sources(B, vec![c]).await.unwrap();
        let got = m.list_sources(10).await.unwrap();
        let big = got.iter().find(|s| s.url == B).unwrap();
        assert_eq!(big.chars, MAX_CHUNK_CHARS);
        assert!(big.fetched_at >= now() - 5);
    }

    #[tokio::test]
    async fn an_empty_save_removes_the_page() {
        let m = seeded().await;
        let r = m.save_sources(A, vec![]).await.unwrap();
        assert_eq!(r.removed, 3);
        assert_eq!(m.list_sources(10).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn listing_and_stats() {
        let m = seeded().await;
        let list = m.list_sources(10).await.unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list.iter().map(|s| s.chunks).sum::<usize>(), 5);
        assert_eq!(list[0].title.as_deref(), Some("A page"));
        assert_eq!(m.list_sources(1).await.unwrap().len(), 1);
        let s = m.source_stats().await.unwrap();
        assert_eq!((s.urls, s.chunks, s.expired_chunks), (2, 5, 0));
        assert!(s.chars > 100 && s.oldest_fetched_at <= s.newest_fetched_at);
    }

    // ---- sealing

    use crate::SealedStore;

    fn sealed_pair() -> (Arc<InMemoryStore>, Memory) {
        let raw = Arc::new(InMemoryStore::new(Arc::new(HashEmbedder::default())));
        let sealed: Arc<dyn Store> = Arc::new(SealedStore::new(raw.clone(), [5; 32]));
        (raw, Memory::new(sealed, "t1"))
    }

    #[tokio::test]
    async fn sealed_sources_round_trip_and_the_server_sees_no_plaintext() {
        let (raw, m) = sealed_pair();
        let url = "https://hospital.example/records/ada-lovelace";
        m.save_sources(
            url,
            page(
                url,
                "h",
                &[
                    "Ada Lovelace appointment with the cardiology clinic on Tuesday",
                    "Parking is limited near building seven",
                ],
            ),
        )
        .await
        .unwrap();
        let hits = m
            .recall_sources("cardiology appointment", 2, &Default::default())
            .await
            .unwrap();
        assert_eq!(hits[0].chunk.chunk_id, "c0");
        assert_eq!(hits[0].chunk.url, url);
        assert_eq!(m.list_sources(5).await.unwrap().len(), 1);

        // Everything the server holds for the kind.
        let all = raw
            .scroll(Kind::Source, &Filter::default(), 100)
            .await
            .unwrap();
        assert_eq!(all.len(), 2);
        for h in &all {
            let s = h.payload.to_string();
            for leak in [
                "Lovelace",
                "cardiology",
                "hospital",
                "Parking",
                "A page",
                "Guide",
            ] {
                assert!(!s.contains(leak), "leaks {leak:?}: {s}");
            }
            assert_ne!(h.id, m.source_id(url, "c0"), "ids are keyed");
        }
        // Filtering by url works on the keyed hash and the other operations follow.
        let one = m
            .recall_sources(
                "parking",
                5,
                &SourceRecallOptions {
                    url: Some(url.into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(one.len(), 2);
        assert_eq!(m.forget_sources(Some(url)).await.unwrap(), 2);
        assert_eq!(
            raw.scroll(Kind::Source, &Filter::default(), 100)
                .await
                .unwrap()
                .len(),
            0
        );
    }

    #[tokio::test]
    async fn sealed_sparse_vectors_hold_opaque_ids_only() {
        let (raw, m) = sealed_pair();
        let text =
            "Quarterly revenue grew while the Zurich office closed and invoice 40291 stayed open";
        m.save_sources(A, page(A, "h", &[text])).await.unwrap();
        let stored = raw.sparse_of(Kind::Source);
        assert_eq!(stored.len(), 1);
        let plain = doc_vector(
            &chunk(A, "c0", 0, text, "h").index_text(),
            &crate::sparse::TermHasher::Plain,
        );
        assert_eq!(stored[0].indices.len(), plain.indices.len());
        assert!(
            stored[0].indices.iter().all(|i| !plain.indices.contains(i)),
            "a sealed id equals the public hash of a word"
        );
        // A different key stores different ids for the same text.
        let raw2 = Arc::new(InMemoryStore::new(Arc::new(HashEmbedder::default())));
        let other: Arc<dyn Store> = Arc::new(SealedStore::new(raw2.clone(), [6; 32]));
        Memory::new(other, "t1")
            .save_sources(A, page(A, "h", &[text]))
            .await
            .unwrap();
        let other_ids = &raw2.sparse_of(Kind::Source)[0].indices;
        assert!(stored[0].indices.iter().all(|i| !other_ids.contains(i)));
        // And the wrong key cannot find or read it.
        let wrong: Arc<dyn Store> = Arc::new(SealedStore::new(raw.clone(), [6; 32]));
        let intruder = Memory::new(wrong, "t1");
        assert!(intruder
            .recall_sources("zurich invoice", 5, &Default::default())
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn sealed_replace_and_refresh_work() {
        let (_raw, m) = sealed_pair();
        let mut c = page(A, "h", &["rust ownership"]);
        c[0].fetched_at = now() - 5 * 86_400;
        m.save_sources(A, c.clone()).await.unwrap();
        c[0].fetched_at = now();
        assert_eq!(m.save_sources(A, c).await.unwrap().unchanged, 1);
        assert!(m.source_stats().await.unwrap().newest_fetched_at.unwrap() >= now() - 5);
        m.save_sources(A, page(A, "h2", &["entirely new text about cargo"]))
            .await
            .unwrap();
        assert_eq!(m.list_sources(5).await.unwrap()[0].chunks, 1);
    }

    #[tokio::test]
    async fn sealed_retention_uses_the_clear_timestamp() {
        let (_raw, m) = sealed_pair();
        let m = m.with_source_retain_days(1);
        let mut old = page(A, "h", &["old"]);
        old[0].fetched_at = now() - 3 * 86_400;
        m.save_sources(A, old).await.unwrap();
        assert_eq!(m.prune_sources(1).await.unwrap(), 1);
    }
}
