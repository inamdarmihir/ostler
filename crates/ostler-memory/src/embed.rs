use async_trait::async_trait;

use std::time::Duration;

use serde_json::{json, Value};

use crate::{MemoryError, Result};

/// Client-side embedding. Production uses Qdrant Cloud Inference (server-side, no
/// embedder needed); this trait exists for tests, the ephemeral mode and future local backends.
#[async_trait]
pub trait Embedder: Send + Sync {
    fn dim(&self) -> usize;
    /// Names the model that produces the vectors. Stores record it so that switching to a different
    /// model of the same dimension (whose vectors are not comparable) is detected on open.
    fn model_id(&self) -> String {
        format!("custom-{}d", self.dim())
    }
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
}

/// Deterministic feature-hashing embedder (word unigrams + character trigrams).
///
/// This is **not** a semantic model. It is good enough to exercise the storage and
/// retrieval paths offline and in CI, and gives sensible results for near-lexical
/// matches ("email" vs "email address"). Real deployments use Qdrant Cloud Inference.
#[derive(Debug, Clone)]
pub struct HashEmbedder {
    dim: usize,
}

impl Default for HashEmbedder {
    fn default() -> Self {
        Self { dim: 256 }
    }
}

impl HashEmbedder {
    pub fn new(dim: usize) -> Self {
        Self { dim }
    }

    fn fnv(s: &str) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in s.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }

    pub fn embed_one(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0f32; self.dim];
        let lower = text.to_lowercase();
        for word in lower
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
        {
            let h = Self::fnv(word);
            v[(h % self.dim as u64) as usize] += if (h >> 63) & 1 == 0 { 1.0 } else { -1.0 };
            let padded: Vec<char> = format!("^{word}$").chars().collect();
            for tri in padded.windows(3) {
                let t: String = tri.iter().collect();
                let h = Self::fnv(&t);
                v[(h % self.dim as u64) as usize] += if (h >> 63) & 1 == 0 { 0.5 } else { -0.5 };
            }
        }
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if n > 0.0 {
            v.iter_mut().for_each(|x| *x /= n);
        }
        v
    }
}

#[async_trait]
impl Embedder for HashEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }
    fn model_id(&self) -> String {
        format!("hash-{}d", self.dim)
    }
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| self.embed_one(t)).collect())
    }
}

/// Embeddings from a local [Ollama](https://ollama.com) server (`POST /api/embed`).
///
/// This is the privacy-preserving choice: the text is turned into a vector on your machine and only the
/// vector reaches Qdrant. The URL must be loopback or https (plain http to another host is refused).
pub struct OllamaEmbedder {
    url: String,
    model: String,
    dim: usize,
    http: reqwest::Client,
}

impl OllamaEmbedder {
    pub fn new(url: &str, model: &str, dim: usize, allow_insecure: bool) -> Result<Self> {
        crate::qdrant::check_transport(url, allow_insecure)?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(|e| MemoryError::Embed(e.to_string()))?;
        Ok(Self {
            url: url.trim_end_matches('/').to_string(),
            model: model.to_string(),
            dim,
            http,
        })
    }
}

#[async_trait]
impl Embedder for OllamaEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }
    fn model_id(&self) -> String {
        format!("ollama:{}", self.model)
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let resp = self
            .http
            .post(format!("{}/api/embed", self.url))
            .json(&json!({ "model": self.model, "input": texts }))
            .send()
            .await
            .map_err(|e| {
                // The URL may carry credentials (http://user:pass@host): show it without, and without the
                // copy reqwest puts in its own error text.
                let mut shown = reqwest::Url::parse(&self.url)
                    .map(|mut u| {
                        let _ = u.set_username("");
                        let _ = u.set_password(None);
                        u.to_string()
                    })
                    .unwrap_or_else(|_| "the configured URL".into());
                shown = shown.trim_end_matches('/').to_string();
                MemoryError::Embed(crate::qdrant::clip(
                    &format!(
                        "cannot reach the local embedder at {shown}: {}",
                        e.without_url()
                    ),
                    200,
                ))
            })?;
        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .map_err(|e| MemoryError::Embed(e.to_string()))?;
        if !status.is_success() {
            return Err(MemoryError::Embed(format!(
                "embedder returned {status}: {}",
                crate::qdrant::clip(body["error"].as_str().unwrap_or("unknown error"), 200)
            )));
        }
        let vectors: Vec<Vec<f32>> = body["embeddings"]
            .as_array()
            .ok_or_else(|| MemoryError::Embed("embedder reply has no `embeddings`".into()))?
            .iter()
            .map(|v| {
                v.as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_f64().map(|f| f as f32))
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .collect();
        if vectors.len() != texts.len() {
            return Err(MemoryError::Embed(
                "embedder returned the wrong number of vectors".into(),
            ));
        }
        if let Some(bad) = vectors.iter().find(|v| v.len() != self.dim) {
            return Err(MemoryError::Embed(format!(
                "model {} produces {}-dim vectors but --embedding-dim is {}",
                self.model,
                bad.len(),
                self.dim
            )));
        }
        Ok(vectors)
    }
}

#[cfg(test)]
mod ollama_tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn embeds_via_api_embed_and_checks_dimensions() {
        let srv = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/embed"))
            .and(body_partial_json(
                json!({"model": "m", "input": ["a", "b"]}),
            ))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"embeddings": [[0.1, 0.2, 0.3], [0.4, 0.5, 0.6]]})),
            )
            .mount(&srv)
            .await;
        let ok = OllamaEmbedder::new(&srv.uri(), "m", 3, false).unwrap();
        let v = ok.embed(&["a".into(), "b".into()]).await.unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[1].len(), 3);
        let wrong = OllamaEmbedder::new(&srv.uri(), "m", 4, false).unwrap();
        assert!(wrong.embed(&["a".into(), "b".into()]).await.is_err());
    }

    #[test]
    fn refuses_plain_http_to_a_remote_host() {
        assert!(OllamaEmbedder::new("http://embed.internal:11434", "m", 3, false).is_err());
        assert!(OllamaEmbedder::new("http://localhost:11434", "m", 3, false).is_ok());
    }
}
