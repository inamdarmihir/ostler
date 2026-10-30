use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::{Embedder, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Profile,
    Playbook,
    Episode,
}

impl Kind {
    pub fn collection(self) -> &'static str {
        match self {
            Kind::Profile => "waymarker_profile",
            Kind::Playbook => "waymarker_playbooks",
            Kind::Episode => "waymarker_episodes",
        }
    }
    pub const ALL: [Kind; 3] = [Kind::Profile, Kind::Playbook, Kind::Episode];
}

/// Conjunction of payload equality conditions. Every query carries the tenant.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub must: Vec<(String, Value)>,
}

impl Filter {
    pub fn tenant(t: &str) -> Self {
        Self {
            must: vec![("tenant".into(), json!(t))],
        }
    }
    pub fn and(mut self, key: &str, value: Value) -> Self {
        self.must.push((key.into(), value));
        self
    }
    pub fn to_qdrant(&self) -> Value {
        json!({ "must": self.must.iter().map(|(k, v)| json!({"key": k, "match": {"value": v}})).collect::<Vec<_>>() })
    }
    fn matches(&self, payload: &Value) -> bool {
        self.must.iter().all(|(k, v)| &payload[k.as_str()] == v)
    }
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub id: String,
    pub score: f32,
    pub payload: Value,
}

#[async_trait]
pub trait Store: Send + Sync {
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
    async fn scroll(&self, kind: Kind, filter: &Filter, limit: usize) -> Result<Vec<Hit>>;
    async fn delete(&self, kind: Kind, id: &str) -> Result<()>;
    async fn delete_matching(&self, kind: Kind, filter: &Filter) -> Result<()>;
    async fn count(&self, kind: Kind, filter: &Filter) -> Result<u64>;
}

struct Point {
    id: String,
    vector: Vec<f32>,
    payload: Value,
    kind: Kind,
}

/// In-process store: for tests and the `--memory ephemeral` demo mode. Not durable.
pub struct InMemoryStore {
    embedder: Arc<dyn Embedder>,
    points: Mutex<Vec<Point>>,
}

impl InMemoryStore {
    pub fn new(embedder: Arc<dyn Embedder>) -> Self {
        Self {
            embedder,
            points: Mutex::default(),
        }
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
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
        let mut pts = self.points.lock().unwrap();
        pts.retain(|p| !(p.kind == kind && p.id == id));
        pts.push(Point {
            id: id.into(),
            vector,
            payload,
            kind,
        });
        Ok(())
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
