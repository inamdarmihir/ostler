//! Qdrant over REST. Two ways to get vectors:
//!
//! * [`VectorSource::CloudInference`]: send `{text, model}` and let Qdrant Cloud embed
//!   server-side (no client model, one fewer network hop for us).
//! * [`VectorSource::Client`]: embed locally and send raw vectors (any Qdrant, incl. OSS).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::store::{Filter, Hit, Kind, Store};
use crate::{Embedder, MemoryError, Result};

#[derive(Clone)]
pub enum VectorSource {
    CloudInference { model: String, dim: usize },
    Client(Arc<dyn Embedder>),
}

impl VectorSource {
    /// Qdrant-hosted free model, 384 dimensions.
    pub fn cloud_default() -> Self {
        Self::CloudInference {
            model: "sentence-transformers/all-minilm-l6-v2".into(),
            dim: 384,
        }
    }
    fn dim(&self) -> usize {
        match self {
            Self::CloudInference { dim, .. } => *dim,
            Self::Client(e) => e.dim(),
        }
    }
}

#[derive(Clone)]
pub struct QdrantConfig {
    pub url: String,
    pub api_key: Option<String>,
    pub vectors: VectorSource,
    pub timeout: Duration,
}

pub struct QdrantStore {
    cfg: QdrantConfig,
    http: reqwest::Client,
}

impl QdrantStore {
    pub fn new(cfg: QdrantConfig) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(cfg.timeout)
            .pool_idle_timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| MemoryError::Http(e.to_string()))?;
        Ok(Self { cfg, http })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.cfg.url.trim_end_matches('/'), path)
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<(u16, Value)> {
        let mut req = self.http.request(method, self.url(path));
        if let Some(k) = &self.cfg.api_key {
            req = req.header("api-key", k);
        }
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| MemoryError::Http(e.to_string()))?;
        let status = resp.status().as_u16();
        let text = resp
            .text()
            .await
            .map_err(|e| MemoryError::Http(e.to_string()))?;
        Ok((
            status,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        ))
    }

    async fn ok(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value> {
        let (status, v) = self.send(method, path, body).await?;
        if (200..300).contains(&status) {
            Ok(v)
        } else {
            Err(MemoryError::Status {
                status,
                body: v.to_string().chars().take(500).collect(),
            })
        }
    }

    /// The vector (or inference document) for `text`, in the shape Qdrant expects.
    async fn vector_for(&self, text: &str) -> Result<Value> {
        match &self.cfg.vectors {
            VectorSource::CloudInference { model, .. } => {
                Ok(json!({ "text": text, "model": model }))
            }
            VectorSource::Client(e) => {
                let v = e.embed(&[text.to_string()]).await?.remove(0);
                Ok(json!(v))
            }
        }
    }
}

#[async_trait]
impl Store for QdrantStore {
    async fn ensure(&self) -> Result<()> {
        for kind in Kind::ALL {
            let name = kind.collection();
            let (status, info) = self
                .send(reqwest::Method::GET, &format!("/collections/{name}"), None)
                .await?;
            if status == 200 {
                let size = info["result"]["config"]["params"]["vectors"]["size"].as_u64();
                if let Some(size) = size {
                    if size as usize != self.cfg.vectors.dim() {
                        return Err(MemoryError::Invalid(format!(
                            "collection {name} has {size}-dim vectors but the configured model produces {}; \
                             pick a matching model or use a different collection prefix",
                            self.cfg.vectors.dim()
                        )));
                    }
                }
                continue;
            }
            if status != 404 {
                return Err(MemoryError::Status {
                    status,
                    body: info.to_string(),
                });
            }
            self.ok(
                reqwest::Method::PUT,
                &format!("/collections/{name}"),
                Some(json!({ "vectors": { "size": self.cfg.vectors.dim(), "distance": "Cosine" }, "on_disk_payload": true })),
            )
            .await?;
            // Tenant index first: with is_tenant Qdrant co-locates a tenant's points for fast filtered search.
            let _ = self
                .ok(
                    reqwest::Method::PUT,
                    &format!("/collections/{name}/index?wait=true"),
                    Some(json!({ "field_name": "tenant", "field_schema": { "type": "keyword", "is_tenant": true } })),
                )
                .await;
            for field in ["key_norm", "fingerprint", "autocomplete", "id_hint"] {
                let _ = self
                    .ok(
                        reqwest::Method::PUT,
                        &format!("/collections/{name}/index?wait=true"),
                        Some(json!({ "field_name": field, "field_schema": "keyword" })),
                    )
                    .await;
            }
        }
        Ok(())
    }

    async fn upsert(&self, kind: Kind, id: &str, text: &str, payload: Value) -> Result<()> {
        let vector = self.vector_for(text).await?;
        self.ok(
            reqwest::Method::PUT,
            &format!("/collections/{}/points?wait=true", kind.collection()),
            Some(json!({ "points": [{ "id": id, "vector": vector, "payload": payload }] })),
        )
        .await?;
        Ok(())
    }

    async fn search(
        &self,
        kind: Kind,
        text: &str,
        filter: &Filter,
        limit: usize,
    ) -> Result<Vec<Hit>> {
        let query = self.vector_for(text).await?;
        let v = self
            .ok(
                reqwest::Method::POST,
                &format!("/collections/{}/points/query", kind.collection()),
                Some(json!({ "query": query, "filter": filter.to_qdrant(), "limit": limit, "with_payload": true })),
            )
            .await?;
        Ok(parse_hits(&v["result"]["points"]))
    }

    async fn scroll(&self, kind: Kind, filter: &Filter, limit: usize) -> Result<Vec<Hit>> {
        let v = self
            .ok(
                reqwest::Method::POST,
                &format!("/collections/{}/points/scroll", kind.collection()),
                Some(json!({ "filter": filter.to_qdrant(), "limit": limit, "with_payload": true, "with_vector": false })),
            )
            .await?;
        Ok(parse_hits(&v["result"]["points"]))
    }

    async fn delete(&self, kind: Kind, id: &str) -> Result<()> {
        self.ok(
            reqwest::Method::POST,
            &format!("/collections/{}/points/delete?wait=true", kind.collection()),
            Some(json!({ "points": [id] })),
        )
        .await?;
        Ok(())
    }

    async fn delete_matching(&self, kind: Kind, filter: &Filter) -> Result<()> {
        self.ok(
            reqwest::Method::POST,
            &format!("/collections/{}/points/delete?wait=true", kind.collection()),
            Some(json!({ "filter": filter.to_qdrant() })),
        )
        .await?;
        Ok(())
    }

    async fn count(&self, kind: Kind, filter: &Filter) -> Result<u64> {
        let v = self
            .ok(
                reqwest::Method::POST,
                &format!("/collections/{}/points/count", kind.collection()),
                Some(json!({ "filter": filter.to_qdrant(), "exact": true })),
            )
            .await?;
        Ok(v["result"]["count"].as_u64().unwrap_or(0))
    }
}

fn parse_hits(points: &Value) -> Vec<Hit> {
    points
        .as_array()
        .map(|a| {
            a.iter()
                .map(|p| Hit {
                    id: p["id"]
                        .as_str()
                        .map(String::from)
                        .unwrap_or_else(|| p["id"].to_string()),
                    score: p["score"].as_f64().unwrap_or(1.0) as f32,
                    payload: p["payload"].clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn cloud(url: String) -> QdrantStore {
        QdrantStore::new(QdrantConfig {
            url,
            api_key: Some("k3y".into()),
            vectors: VectorSource::cloud_default(),
            timeout: Duration::from_secs(5),
        })
        .unwrap()
    }

    /// Cloud Inference must send text+model documents (no client-side vectors) with the api-key header.
    /// This pins the request shape from Qdrant's docs; it cannot prove a live cluster accepts it.
    #[tokio::test]
    async fn cloud_inference_upsert_sends_text_and_model() {
        let srv = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/collections/waymarker_profile/points"))
            .and(header("api-key", "k3y"))
            .and(body_partial_json(json!({"points":[{"id":"abc","vector":{"text":"email","model":"sentence-transformers/all-minilm-l6-v2"}}]})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status":"ok"})))
            .expect(1)
            .mount(&srv)
            .await;
        cloud(srv.uri())
            .upsert(Kind::Profile, "abc", "email", json!({"tenant":"t"}))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn cloud_inference_query_sends_text_and_tenant_filter() {
        let srv = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/collections/waymarker_profile/points/query"))
            .and(body_partial_json(json!({
                "query": {"text": "zip", "model": "sentence-transformers/all-minilm-l6-v2"},
                "filter": {"must": [{"key": "tenant", "match": {"value": "t1"}}]},
                "limit": 3
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"result":{"points":[{"id":"x","score":0.8,"payload":{"key":"zip"}}]}}),
            ))
            .expect(1)
            .mount(&srv)
            .await;
        let hits = cloud(srv.uri())
            .search(Kind::Profile, "zip", &Filter::tenant("t1"), 3)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert!((hits[0].score - 0.8).abs() < 1e-6);
    }

    #[tokio::test]
    async fn dimension_mismatch_is_a_clear_error() {
        let srv = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/collections/waymarker_profile"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"result":{"config":{"params":{"vectors":{"size":1024,"distance":"Cosine"}}}}})))
            .mount(&srv)
            .await;
        let err = cloud(srv.uri()).ensure().await.unwrap_err().to_string();
        assert!(err.contains("1024") && err.contains("384"), "{err}");
    }

    #[tokio::test]
    async fn http_errors_surface_status() {
        let srv = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(403).set_body_string("forbidden"))
            .mount(&srv)
            .await;
        let err = cloud(srv.uri())
            .scroll(Kind::Profile, &Filter::tenant("t"), 1)
            .await
            .unwrap_err();
        assert!(
            matches!(err, MemoryError::Status { status: 403, .. }),
            "{err}"
        );
    }
}
