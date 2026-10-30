use async_trait::async_trait;

use crate::Result;

/// Client-side embedding. Production uses Qdrant Cloud Inference (server-side, no
/// embedder needed); this trait exists for tests, the ephemeral mode and future local backends.
#[async_trait]
pub trait Embedder: Send + Sync {
    fn dim(&self) -> usize;
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
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| self.embed_one(t)).collect())
    }
}
