use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::fusion::rrf_fuse;
use crate::sparse::{idf, SparseVector, TermHasher};
use crate::{Embedder, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Profile,
    Playbook,
    Episode,
    /// Chunks of pages the agent read ([`crate::SourceChunk`]). Unlike the other kinds this collection
    /// has *named* vectors (`dense` and a sparse `bm25`), see [`Store::search_hybrid`].
    Source,
}

impl Kind {
    pub fn collection(self) -> &'static str {
        match self {
            Kind::Profile => "ostler_profile",
            Kind::Playbook => "ostler_playbooks",
            Kind::Episode => "ostler_episodes",
            Kind::Source => "ostler_sources",
        }
    }
    pub const ALL: [Kind; 4] = [Kind::Profile, Kind::Playbook, Kind::Episode, Kind::Source];

    pub(crate) fn from_collection(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.collection() == name)
    }
}

/// Conjunction of payload conditions. Every query carries the tenant.
///
/// Equality on an array payload matches if any element is equal (Qdrant semantics), so a
/// playbook's `labels_norm` list can be filtered by a single label. A condition whose value is itself an
/// array means *any of these* (Qdrant `match.any`): `host` in `["a.com", "b.com"]`. An empty list matches nothing.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub must: Vec<(String, Value)>,
    /// Numeric `key < value` conditions.
    pub below: Vec<(String, f64)>,
}

impl Filter {
    pub fn tenant(t: &str) -> Self {
        Self {
            must: vec![("tenant".into(), json!(t))],
            below: vec![],
        }
    }
    pub fn below(mut self, key: &str, value: f64) -> Self {
        self.below.push((key.into(), value));
        self
    }
    pub fn and(mut self, key: &str, value: Value) -> Self {
        self.must.push((key.into(), value));
        self
    }
    pub fn to_qdrant(&self) -> Value {
        let mut must: Vec<Value> = self
            .must
            .iter()
            .map(|(k, v)| match v {
                Value::Array(_) => json!({"key": k, "match": {"any": v}}),
                _ => json!({"key": k, "match": {"value": v}}),
            })
            .collect();
        must.extend(
            self.below
                .iter()
                .map(|(k, v)| json!({"key": k, "range": {"lt": v}})),
        );
        json!({ "must": must })
    }
    /// Whether the filter pins a tenant. Bulk deletes require it: an unscoped one would reach every tenant.
    pub fn is_tenant_scoped(&self) -> bool {
        self.must.iter().any(|(k, _)| k == "tenant")
    }

    fn matches(&self, payload: &Value) -> bool {
        fn one(have: &Value, want: &Value) -> bool {
            match have {
                Value::Array(a) => a.contains(want),
                other => other == want,
            }
        }
        self.must.iter().all(|(k, v)| match v {
            Value::Array(any) => any.iter().any(|w| one(&payload[k.as_str()], w)),
            _ => one(&payload[k.as_str()], v),
        }) && self
            .below
            .iter()
            .all(|(k, v)| payload[k.as_str()].as_f64().is_some_and(|x| x < *v))
    }
}

/// How to rank learned records beyond raw similarity: the Qdrant score-boosting formula
///
/// `similarity + success_weight * (wins+1)/(wins+losses+2) + recency_weight * decay(age)`
///
/// where `decay` halves every `half_life_secs`. Reads payload keys `wins`, `losses`, `verified_ts`.
/// The success term is a Laplace-smoothed win rate, so an untested record starts at 0.5.
#[derive(Debug, Clone, Copy)]
pub struct Rank {
    pub now: u64,
    pub success_weight: f32,
    pub recency_weight: f32,
    pub half_life_secs: u64,
    /// Candidates below this raw similarity are discarded before boosting.
    pub min_similarity: f32,
}

impl Rank {
    /// The same formula, evaluated client-side (in-memory store, and for tests to pin the maths).
    pub fn score(&self, similarity: f32, payload: &Value) -> f32 {
        let wins = payload["wins"].as_f64().unwrap_or(0.0) as f32;
        let losses = payload["losses"].as_f64().unwrap_or(0.0) as f32;
        let ts = payload["verified_ts"].as_f64().unwrap_or(0.0);
        let rate = (wins + 1.0) / (wins + losses + 2.0);
        let age = (self.now as f64 - ts).abs();
        let decay = (0.5f64.ln() / self.half_life_secs.max(1) as f64 * age).exp() as f32;
        similarity + self.success_weight * rate + self.recency_weight * decay
    }

    /// Qdrant Query API body: vector prefetch (with the similarity floor), then formula rescoring.
    pub fn to_qdrant_query(&self, vector: Value, filter: &Filter, limit: usize) -> Value {
        json!({
            "prefetch": {
                "query": vector,
                "filter": filter.to_qdrant(),
                "limit": limit.saturating_mul(4).max(16),
                "score_threshold": self.min_similarity,
            },
            "query": { "formula": {
                "sum": [
                    "$score",
                    { "mult": [ self.success_weight, { "div": {
                        "left": { "sum": ["wins", 1.0] },
                        "right": { "sum": ["wins", "losses", 2.0] },
                    } } ] },
                    { "mult": [ self.recency_weight, { "exp_decay": {
                        "x": "verified_ts", "target": self.now,
                        "scale": self.half_life_secs.max(1), "midpoint": 0.5,
                    } } ] },
                ]
            }, "defaults": { "wins": 0, "losses": 0, "verified_ts": 0 } },
            "limit": limit,
            "with_payload": true,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub id: String,
    pub score: f32,
    pub payload: Value,
}

/// A hybrid (dense + sparse) search over [`Kind::Source`]. See [`Store::search_hybrid`].
#[derive(Debug, Clone, Copy)]
pub struct HybridQuery<'a> {
    /// Text to embed for the dense side.
    pub text: &'a str,
    /// Query terms for the sparse side (ids from [`Store::term_hasher`]). May be empty: dense only.
    pub sparse: &'a SparseVector,
    /// Payload filter; always carries the tenant.
    pub filter: &'a Filter,
    /// How many candidates to return (also how deep each ranking is read before fusion).
    pub pool: usize,
    /// RRF weight of the dense ranking.
    pub dense_weight: f32,
    /// RRF weight of the sparse ranking. `0.0` turns lexical matching off.
    pub sparse_weight: f32,
    /// RRF constant.
    pub rrf_k: f32,
}

/// One fused candidate: its payload, the raw RRF score (see [`crate::fusion::rrf_term`]) and its dense
/// vector, which client-side MMR needs to measure how alike two candidates are.
#[derive(Debug, Clone)]
pub struct HybridHit {
    /// Id, fused score and payload.
    pub hit: Hit,
    /// The stored dense vector, when the store could return it.
    pub vector: Option<Vec<f32>>,
}

/// Fuse a dense and a sparse ranking (ids best first) the way a Qdrant `rrf` query would, then keep `pool`.
pub(crate) fn fuse_rankings(
    dense: &[String],
    sparse: &[String],
    q: &HybridQuery<'_>,
) -> Vec<(String, f32)> {
    let mut fused = rrf_fuse(
        &[(dense, q.dense_weight), (sparse, q.sparse_weight)],
        q.rrf_k,
    );
    fused.truncate(q.pool);
    fused
}

const NO_SOURCES: &str = "this store does not support source memory";

#[async_trait]
pub trait Store: Send + Sync {
    /// How this store turns a word into a sparse term id. Plain unless the store hides words from the
    /// server (see [`crate::SealedStore`]).
    fn term_hasher(&self) -> TermHasher {
        TermHasher::Plain
    }
    /// Store (or replace) one [`Kind::Source`] point: dense vector from `text`, plus `sparse`.
    async fn upsert_source(
        &self,
        _id: &str,
        _text: &str,
        _sparse: SparseVector,
        _payload: Value,
    ) -> Result<()> {
        Err(crate::MemoryError::Invalid(NO_SOURCES.into()))
    }
    /// Dense search and sparse (BM25-style, IDF-weighted) search over [`Kind::Source`], fused with weighted
    /// RRF. Returns at most `pool` candidates, best first, `hit.score` being the raw fused score.
    async fn search_hybrid(&self, _q: &HybridQuery<'_>) -> Result<Vec<HybridHit>> {
        Err(crate::MemoryError::Invalid(NO_SOURCES.into()))
    }
    /// Overwrite the payload of an existing point, keeping its vectors. `id` is a [`Hit::id`] (the id the
    /// point is stored under). A no-op for a missing id.
    async fn replace_payload(&self, _kind: Kind, _id: &str, _payload: Value) -> Result<()> {
        Err(crate::MemoryError::Invalid(NO_SOURCES.into()))
    }
    /// Why the store is currently being skipped, if it is.
    fn degraded(&self) -> Option<String> {
        None
    }
    /// Create collections and indexes if missing.
    async fn ensure(&self) -> Result<()>;
    async fn upsert(&self, kind: Kind, id: &str, text: &str, payload: Value) -> Result<()>;
    async fn search(
        &self,
        kind: Kind,
        text: &str,
        filter: &Filter,
        limit: usize,
    ) -> Result<Vec<Hit>>;
    /// Similarity search re-ranked by evidence (wins, losses, recency). See [`Rank`].
    async fn search_ranked(
        &self,
        kind: Kind,
        text: &str,
        filter: &Filter,
        limit: usize,
        rank: &Rank,
    ) -> Result<Vec<Hit>>;
    async fn scroll(&self, kind: Kind, filter: &Filter, limit: usize) -> Result<Vec<Hit>>;
    async fn delete(&self, kind: Kind, id: &str) -> Result<()>;
    async fn delete_matching(&self, kind: Kind, filter: &Filter) -> Result<()>;
    async fn count(&self, kind: Kind, filter: &Filter) -> Result<u64>;
}

struct Point {
    id: String,
    vector: Vec<f32>,
    sparse: Option<SparseVector>,
    payload: Value,
    kind: Kind,
}

/// Everything a persisted point consists of.
pub(crate) type Stored = (Kind, String, Vec<f32>, Option<SparseVector>, Value);

/// In-process store: for tests and the `--memory ephemeral` demo mode. Not durable.
pub struct InMemoryStore {
    embedder: Arc<dyn Embedder>,
    points: Mutex<Vec<Point>>,
}

impl InMemoryStore {
    /// Test helper: what a malicious server could do, rewrite a stored payload behind our back.
    #[cfg(test)]
    pub(crate) fn replace_payload_for_test(&self, id: &str, payload: Value) {
        for p in self.points.lock().unwrap().iter_mut() {
            if p.id == id {
                p.payload = payload.clone();
            }
        }
    }

    pub fn new(embedder: Arc<dyn Embedder>) -> Self {
        Self {
            embedder,
            points: Mutex::default(),
        }
    }

    pub(crate) fn embedder(&self) -> &Arc<dyn Embedder> {
        &self.embedder
    }

    /// Insert or replace one point whose vectors are already computed.
    pub(crate) fn put_vector(
        &self,
        kind: Kind,
        id: &str,
        vector: Vec<f32>,
        sparse: Option<SparseVector>,
        payload: Value,
    ) {
        let mut pts = self.points.lock().unwrap();
        pts.retain(|p| !(p.kind == kind && p.id == id));
        pts.push(Point {
            id: id.into(),
            vector,
            sparse,
            payload,
            kind,
        });
    }

    /// Overwrite one point's payload (a missing id is ignored).
    pub(crate) fn put_payload(&self, kind: Kind, id: &str, payload: Value) {
        for p in self.points.lock().unwrap().iter_mut() {
            if p.kind == kind && p.id == id {
                p.payload = payload.clone();
            }
        }
    }

    /// Every sparse vector stored for a kind. Tests use it to check what a server would see.
    #[cfg(test)]
    pub(crate) fn sparse_of(&self, kind: Kind) -> Vec<SparseVector> {
        self.points
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.kind == kind)
            .filter_map(|p| p.sparse.clone())
            .collect()
    }

    /// Every point, for persistence.
    pub(crate) fn export(&self) -> Vec<Stored> {
        self.points
            .lock()
            .unwrap()
            .iter()
            .map(|p| {
                (
                    p.kind,
                    p.id.clone(),
                    p.vector.clone(),
                    p.sparse.clone(),
                    p.payload.clone(),
                )
            })
            .collect()
    }

    /// Replace the whole content, for reloading what another process saved.
    pub(crate) fn replace_all(&self, pts: Vec<Stored>) {
        *self.points.lock().unwrap() = pts
            .into_iter()
            .map(|(kind, id, vector, sparse, payload)| Point {
                id,
                vector,
                sparse,
                payload,
                kind,
            })
            .collect();
    }
}

pub(crate) fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

#[async_trait]
impl Store for InMemoryStore {
    async fn ensure(&self) -> Result<()> {
        Ok(())
    }

    async fn upsert(&self, kind: Kind, id: &str, text: &str, payload: Value) -> Result<()> {
        let vector = self.embedder.embed(&[text.to_string()]).await?.remove(0);
        self.put_vector(kind, id, vector, None, payload);
        Ok(())
    }

    async fn upsert_source(
        &self,
        id: &str,
        text: &str,
        sparse: SparseVector,
        payload: Value,
    ) -> Result<()> {
        let vector = self.embedder.embed(&[text.to_string()]).await?.remove(0);
        self.put_vector(Kind::Source, id, vector, Some(sparse), payload);
        Ok(())
    }

    async fn replace_payload(&self, kind: Kind, id: &str, payload: Value) -> Result<()> {
        self.put_payload(kind, id, payload);
        Ok(())
    }

    /// Both rankings are computed here with exact scans (a few thousand chunks at most): cosine for the
    /// dense side, and for the sparse side `sum over query terms of idf * stored weight`, with the IDF taken
    /// over the chunks that pass the filter, as a Qdrant sparse vector with the `idf` modifier does.
    async fn search_hybrid(&self, q: &HybridQuery<'_>) -> Result<Vec<HybridHit>> {
        if q.pool == 0 {
            return Ok(vec![]);
        }
        let query = self.embedder.embed(&[q.text.to_string()]).await?.remove(0);
        let pts = self.points.lock().unwrap();
        let cands: Vec<&Point> = pts
            .iter()
            .filter(|p| p.kind == Kind::Source && q.filter.matches(&p.payload))
            .collect();
        let mut dense: Vec<(&Point, f32)> = cands
            .iter()
            .map(|p| (*p, cosine(&query, &p.vector)))
            .collect();
        dense.sort_by(|a, b| b.1.total_cmp(&a.1));
        dense.truncate(q.pool);
        let weights: Vec<(u32, f32)> = q
            .sparse
            .indices
            .iter()
            .zip(&q.sparse.values)
            .map(|(id, w)| {
                let df = cands
                    .iter()
                    .filter(|p| p.sparse.as_ref().is_some_and(|s| s.get(*id).is_some()))
                    .count();
                (*id, w * idf(cands.len(), df))
            })
            .collect();
        let mut sparse: Vec<(&Point, f32)> = cands
            .iter()
            .filter_map(|p| {
                let s = p.sparse.as_ref()?;
                let score: f32 = weights
                    .iter()
                    .filter_map(|(id, w)| s.get(*id).map(|v| v * w))
                    .sum();
                (score > 0.0).then_some((*p, score))
            })
            .collect();
        sparse.sort_by(|a, b| b.1.total_cmp(&a.1));
        sparse.truncate(q.pool);
        let d_ids: Vec<String> = dense.iter().map(|(p, _)| p.id.clone()).collect();
        let s_ids: Vec<String> = sparse.iter().map(|(p, _)| p.id.clone()).collect();
        Ok(fuse_rankings(&d_ids, &s_ids, q)
            .into_iter()
            .filter_map(|(id, score)| {
                let p = cands.iter().find(|p| p.id == id)?;
                Some(HybridHit {
                    hit: Hit {
                        id,
                        score,
                        payload: p.payload.clone(),
                    },
                    vector: Some(p.vector.clone()),
                })
            })
            .collect())
    }

    async fn search(
        &self,
        kind: Kind,
        text: &str,
        filter: &Filter,
        limit: usize,
    ) -> Result<Vec<Hit>> {
        let q = self.embedder.embed(&[text.to_string()]).await?.remove(0);
        let pts = self.points.lock().unwrap();
        let mut hits: Vec<Hit> = pts
            .iter()
            .filter(|p| p.kind == kind && filter.matches(&p.payload))
            .map(|p| Hit {
                id: p.id.clone(),
                score: cosine(&q, &p.vector),
                payload: p.payload.clone(),
            })
            .collect();
        hits.sort_by(|a, b| b.score.total_cmp(&a.score));
        hits.truncate(limit);
        Ok(hits)
    }

    async fn search_ranked(
        &self,
        kind: Kind,
        text: &str,
        filter: &Filter,
        limit: usize,
        rank: &Rank,
    ) -> Result<Vec<Hit>> {
        let mut hits: Vec<Hit> = self
            .search(kind, text, filter, usize::MAX)
            .await?
            .into_iter()
            .filter(|h| h.score >= rank.min_similarity)
            .map(|mut h| {
                h.score = rank.score(h.score, &h.payload);
                h
            })
            .collect();
        hits.sort_by(|a, b| b.score.total_cmp(&a.score));
        hits.truncate(limit);
        Ok(hits)
    }

    async fn scroll(&self, kind: Kind, filter: &Filter, limit: usize) -> Result<Vec<Hit>> {
        let pts = self.points.lock().unwrap();
        Ok(pts
            .iter()
            .filter(|p| p.kind == kind && filter.matches(&p.payload))
            .take(limit)
            .map(|p| Hit {
                id: p.id.clone(),
                score: 1.0,
                payload: p.payload.clone(),
            })
            .collect())
    }

    async fn delete(&self, kind: Kind, id: &str) -> Result<()> {
        self.points
            .lock()
            .unwrap()
            .retain(|p| !(p.kind == kind && p.id == id));
        Ok(())
    }

    async fn delete_matching(&self, kind: Kind, filter: &Filter) -> Result<()> {
        if !filter.is_tenant_scoped() {
            return Err(crate::MemoryError::Invalid(
                "refusing a bulk delete that is not scoped to a tenant".into(),
            ));
        }
        self.points
            .lock()
            .unwrap()
            .retain(|p| !(p.kind == kind && filter.matches(&p.payload)));
        Ok(())
    }

    async fn count(&self, kind: Kind, filter: &Filter) -> Result<u64> {
        Ok(self
            .points
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.kind == kind && filter.matches(&p.payload))
            .count() as u64)
    }
}
