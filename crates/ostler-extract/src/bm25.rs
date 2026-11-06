//! In-memory BM25 over chunks: the no-LLM, query-focused reading engine.
//!
//! The same index can produce sparse term-weight vectors ([`SparseVector`])
//! through a pluggable [`TermHasher`], so a memory layer can later swap the
//! unkeyed hash for a keyed one without touching ranking code.
//!
//! Scoring uses the Lucene-style non-negative IDF
//! `ln(1 + (N - df + 0.5) / (df + 0.5))` and the usual length normalisation.
//! Defaults are `k1 = 1.2`, `b = 0.75`. Tokenisation is deliberately light:
//! lowercase, split on non-alphanumerics, apostrophes inside words dropped, one
//! token per CJK character, no stemming, no diacritic folding.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::chunk::{is_cjk, Chunk};

/// BM25 free parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bm25Params {
    /// Term-frequency saturation. Default 1.2.
    pub k1: f64,
    /// Length normalisation strength in `0..=1`. Default 0.75.
    pub b: f64,
}

impl Default for Bm25Params {
    fn default() -> Self {
        Self { k1: 1.2, b: 0.75 }
    }
}

const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "but", "by", "for", "from", "has", "have", "he",
    "in", "is", "it", "its", "of", "on", "or", "that", "the", "their", "this", "to", "was", "were",
    "will", "with", "you", "your",
];

const MAX_TOKEN_CHARS: usize = 64;

/// Tokenizer settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tokenizer {
    /// Drop a small built-in English stopword list. Default `true`.
    pub stopwords: bool,
}

impl Default for Tokenizer {
    fn default() -> Self {
        Self { stopwords: true }
    }
}

impl Tokenizer {
    /// Tokenise `text`. Tokens longer than 64 characters are dropped.
    pub fn tokenize(&self, text: &str) -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        let mut out = Vec::new();
        let mut cur = String::new();
        let flush = |cur: &mut String, out: &mut Vec<String>| {
            if !cur.is_empty() {
                let n = cur.chars().count();
                if n <= MAX_TOKEN_CHARS && !(self.stopwords && STOPWORDS.contains(&cur.as_str())) {
                    out.push(std::mem::take(cur));
                } else {
                    cur.clear();
                }
            }
        };
        for (i, &c) in chars.iter().enumerate() {
            if c.is_alphanumeric() {
                if is_cjk(c) {
                    flush(&mut cur, &mut out);
                    out.push(c.to_lowercase().collect());
                } else {
                    cur.extend(c.to_lowercase());
                }
            } else if (c == '\'' || c == '\u{2019}')
                && !cur.is_empty()
                && chars.get(i + 1).is_some_and(|n| n.is_alphanumeric())
            {
                // Apostrophe inside a word: "don't" -> "dont".
            } else {
                flush(&mut cur, &mut out);
            }
        }
        flush(&mut cur, &mut out);
        out
    }

    /// Tokenise a query (deduplicated). If every term was a stopword the
    /// query is retried with stopwords kept; that only helps an index that
    /// was built with stopwords kept.
    fn query_terms(&self, text: &str) -> Vec<String> {
        let mut t = self.tokenize(text);
        if t.is_empty() && self.stopwords {
            t = Tokenizer { stopwords: false }.tokenize(text);
        }
        let mut seen = BTreeSet::new();
        t.retain(|x| seen.insert(x.clone()));
        t
    }
}

/// Maps a term to a sparse-vector dimension id.
pub trait TermHasher {
    /// Stable id for `term`. Keyed implementations may use a secret.
    fn hash_term(&self, term: &str) -> u64;
}

/// Unkeyed 64-bit FNV-1a, optionally seeded. Stable across versions and platforms.
#[derive(Debug, Clone, Copy, Default)]
pub struct Fnv1a {
    /// Mixed into the offset basis; `0` is plain FNV-1a.
    pub seed: u64,
}

impl TermHasher for Fnv1a {
    fn hash_term(&self, term: &str) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ self.seed;
        for b in term.as_bytes() {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }
}

/// Sparse vector with ascending, unique indices.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SparseVector {
    /// Dimension ids (hashed terms), ascending. Hash collisions are summed.
    pub indices: Vec<u64>,
    /// Weight per index.
    pub values: Vec<f32>,
}

impl SparseVector {
    fn from_pairs(pairs: impl Iterator<Item = (u64, f32)>) -> Self {
        let mut m: BTreeMap<u64, f32> = BTreeMap::new();
        for (i, v) in pairs {
            *m.entry(i).or_insert(0.0) += v;
        }
        Self {
            indices: m.keys().copied().collect(),
            values: m.values().copied().collect(),
        }
    }

    /// Dot product with another sparse vector.
    pub fn dot(&self, other: &SparseVector) -> f32 {
        let (mut i, mut j, mut s) = (0, 0, 0.0f32);
        while i < self.indices.len() && j < other.indices.len() {
            match self.indices[i].cmp(&other.indices[j]) {
                std::cmp::Ordering::Less => i += 1,
                std::cmp::Ordering::Greater => j += 1,
                std::cmp::Ordering::Equal => {
                    s += self.values[i] * other.values[j];
                    i += 1;
                    j += 1;
                }
            }
        }
        s
    }
}

#[derive(Debug, Clone)]
struct Doc {
    id: String,
    source: String,
    len: usize,
    tf: BTreeMap<String, u32>,
}

/// A ranked result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hit {
    /// Document index in insertion order.
    pub doc: usize,
    /// Document (chunk) id.
    pub id: String,
    /// Source label, typically the page URL.
    pub source: String,
    /// BM25 score (higher is better).
    pub score: f64,
    /// Query terms that occur in the document, sorted.
    pub matched_terms: Vec<String>,
}

/// In-memory BM25 index.
#[derive(Debug, Clone, Default)]
pub struct Bm25Index {
    params: Bm25Params,
    tokenizer: Tokenizer,
    docs: Vec<Doc>,
    postings: BTreeMap<String, Vec<(usize, u32)>>,
    total_len: usize,
}

impl Bm25Index {
    /// Empty index with default parameters and tokenizer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Empty index with explicit parameters.
    pub fn with_config(params: Bm25Params, tokenizer: Tokenizer) -> Self {
        Self {
            params,
            tokenizer,
            ..Self::default()
        }
    }

    /// Number of documents.
    pub fn len(&self) -> usize {
        self.docs.len()
    }

    /// Whether the index has no documents.
    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    /// Add raw text under `id` and `source`. Returns the document index.
    pub fn add_text(&mut self, id: &str, source: &str, text: &str) -> usize {
        let toks = self.tokenizer.tokenize(text);
        let mut tf: BTreeMap<String, u32> = BTreeMap::new();
        for t in &toks {
            *tf.entry(t.clone()).or_insert(0) += 1;
        }
        let idx = self.docs.len();
        for (t, &n) in &tf {
            self.postings.entry(t.clone()).or_default().push((idx, n));
        }
        self.total_len += toks.len();
        self.docs.push(Doc {
            id: id.to_string(),
            source: source.to_string(),
            len: toks.len(),
            tf,
        });
        idx
    }

    /// Add a [`Chunk`]; `source` is usually its page URL.
    pub fn add_chunk(&mut self, chunk: &Chunk, source: &str) -> usize {
        self.add_text(&chunk.id, source, &chunk.text)
    }

    fn avg_len(&self) -> f64 {
        if self.docs.is_empty() || self.total_len == 0 {
            1.0
        } else {
            self.total_len as f64 / self.docs.len() as f64
        }
    }

    fn idf(&self, df: usize) -> f64 {
        let n = self.docs.len() as f64;
        let df = df as f64;
        (1.0 + (n - df + 0.5) / (df + 0.5)).ln()
    }

    fn tf_weight(&self, tf: u32, doc_len: usize) -> f64 {
        let tf = f64::from(tf);
        let Bm25Params { k1, b } = self.params;
        tf * (k1 + 1.0) / (tf + k1 * (1.0 - b + b * doc_len as f64 / self.avg_len()))
    }

    /// Top `k` documents for `query`, best first. Ties break by insertion order.
    /// Documents with no matching term are never returned.
    pub fn rank(&self, query: &str, k: usize) -> Vec<Hit> {
        let mut acc: BTreeMap<usize, (f64, Vec<String>)> = BTreeMap::new();
        for term in self.tokenizer.query_terms(query) {
            let Some(post) = self.postings.get(&term) else {
                continue;
            };
            let idf = self.idf(post.len());
            for &(d, tf) in post {
                let e = acc.entry(d).or_insert((0.0, Vec::new()));
                e.0 += idf * self.tf_weight(tf, self.docs[d].len);
                e.1.push(term.clone());
            }
        }
        let mut hits: Vec<Hit> = acc
            .into_iter()
            .map(|(d, (score, mut terms))| {
                terms.sort();
                Hit {
                    doc: d,
                    id: self.docs[d].id.clone(),
                    source: self.docs[d].source.clone(),
                    score,
                    matched_terms: terms,
                }
            })
            .collect();
        hits.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.doc.cmp(&b.doc)));
        hits.truncate(k);
        hits
    }

    /// Sparse document vector with BM25 term weights (`idf * saturated tf`).
    /// `dot(document_vector, query_vector)` equals the BM25 score up to hash
    /// collisions and `f32` rounding. `None` for an unknown document.
    pub fn document_vector<H: TermHasher + ?Sized>(
        &self,
        doc: usize,
        h: &H,
    ) -> Option<SparseVector> {
        let d = self.docs.get(doc)?;
        Some(SparseVector::from_pairs(d.tf.iter().map(|(t, &n)| {
            let df = self.postings.get(t).map_or(1, Vec::len);
            (
                h.hash_term(t),
                (self.idf(df) * self.tf_weight(n, d.len)) as f32,
            )
        })))
    }

    /// Sparse query vector: weight 1 per distinct query term present in the index.
    pub fn query_vector<H: TermHasher + ?Sized>(&self, query: &str, h: &H) -> SparseVector {
        SparseVector::from_pairs(
            self.tokenizer
                .query_terms(query)
                .into_iter()
                .filter(|t| self.postings.contains_key(t))
                .map(|t| (h.hash_term(&t), 1.0)),
        )
    }

    /// Term -> weight map for a document (same weights as [`Self::document_vector`], unhashed).
    pub fn term_weights(&self, doc: usize) -> Option<BTreeMap<String, f32>> {
        let d = self.docs.get(doc)?;
        Some(
            d.tf.iter()
                .map(|(t, &n)| {
                    let df = self.postings.get(t).map_or(1, Vec::len);
                    (t.clone(), (self.idf(df) * self.tf_weight(n, d.len)) as f32)
                })
                .collect(),
        )
    }

    /// Maximal-marginal-relevance rerank of `hits` down to `k` results.
    ///
    /// Relevance is the BM25 score divided by the best score; redundancy is
    /// the maximum cosine similarity (tf-idf term vectors) to anything already
    /// selected. `lambda` in `0..=1`: 1 is pure relevance, 0 pure diversity.
    pub fn mmr(&self, hits: &[Hit], lambda: f64, k: usize) -> Vec<Hit> {
        let lambda = lambda.clamp(0.0, 1.0);
        let max = hits.iter().map(|h| h.score).fold(0.0f64, f64::max);
        let vecs: Vec<BTreeMap<String, f32>> =
            hits.iter().map(|h| self.tfidf_vector(h.doc)).collect();
        let mut chosen: Vec<usize> = Vec::new();
        let mut left: Vec<usize> = (0..hits.len()).collect();
        while chosen.len() < k && !left.is_empty() {
            let mut best = (f64::NEG_INFINITY, 0usize);
            for (pos, &i) in left.iter().enumerate() {
                let rel = if max > 0.0 { hits[i].score / max } else { 0.0 };
                let red = chosen
                    .iter()
                    .map(|&j| cosine(&vecs[i], &vecs[j]))
                    .fold(0.0f64, f64::max);
                let s = lambda * rel - (1.0 - lambda) * red;
                if s > best.0 {
                    best = (s, pos);
                }
            }
            chosen.push(left.remove(best.1));
        }
        chosen.into_iter().map(|i| hits[i].clone()).collect()
    }

    fn tfidf_vector(&self, doc: usize) -> BTreeMap<String, f32> {
        let Some(d) = self.docs.get(doc) else {
            return BTreeMap::new();
        };
        d.tf.iter()
            .map(|(t, &n)| {
                let df = self.postings.get(t).map_or(1, Vec::len);
                (t.clone(), ((1.0 + f64::from(n).ln()) * self.idf(df)) as f32)
            })
            .collect()
    }
}

fn cosine(a: &BTreeMap<String, f32>, b: &BTreeMap<String, f32>) -> f64 {
    let dot: f64 = a
        .iter()
        .filter_map(|(t, &x)| b.get(t).map(|&y| f64::from(x) * f64::from(y)))
        .sum();
    let na: f64 = a
        .values()
        .map(|&x| f64::from(x).powi(2))
        .sum::<f64>()
        .sqrt();
    let nb: f64 = b
        .values()
        .map(|&x| f64::from(x).powi(2))
        .sum::<f64>()
        .sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

/// Keep at most `max_per_source` hits per `source`, preserving order.
pub fn diversify(results: &[Hit], max_per_source: usize) -> Vec<Hit> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    results
        .iter()
        .filter(|h| {
            let c = counts.entry(h.source.as_str()).or_insert(0);
            *c += 1;
            *c <= max_per_source
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idx() -> Bm25Index {
        let mut i = Bm25Index::new();
        i.add_text("a", "s1", "The quick brown fox jumps over the lazy dog");
        i.add_text("b", "s1", "Rust is a systems programming language");
        i.add_text("c", "s2", "Part number XK-7731 ships in 3 days");
        i.add_text("d", "s2", "Foxes are quick; the brown fox is a canid");
        i
    }

    #[test]
    fn tokenizer_basics() {
        let t = Tokenizer::default();
        assert_eq!(
            t.tokenize("Don't STOP, believe!"),
            vec!["dont", "stop", "believe"]
        );
        assert_eq!(t.tokenize("日本語 ok"), vec!["日", "本", "語", "ok"]);
        assert_eq!(t.tokenize("XK-7731"), vec!["xk", "7731"]);
        assert_eq!(Tokenizer { stopwords: false }.tokenize("the cat").len(), 2);
        assert_eq!(t.tokenize("the cat"), vec!["cat"]);
        assert!(t.tokenize(&"x".repeat(100)).is_empty());
    }

    #[test]
    fn rank_finds_exact_ids_and_terms() {
        let i = idx();
        let h = i.rank("XK-7731", 5);
        assert_eq!(h[0].id, "c");
        assert_eq!(h[0].matched_terms, vec!["7731", "xk"]);
        let h = i.rank("quick brown fox", 2);
        assert_eq!(h.len(), 2);
        assert!(h[0].score >= h[1].score);
        assert!(i.rank("zzzz", 5).is_empty());
        assert!(i.rank("", 5).is_empty());
    }

    #[test]
    fn stopword_only_query_matches_nothing_when_index_drops_stopwords() {
        let i = idx();
        assert!(i.rank("the", 5).is_empty());
        let mut j = Bm25Index::with_config(Bm25Params::default(), Tokenizer { stopwords: false });
        j.add_text("a", "s", "the end");
        assert_eq!(j.rank("the", 5).len(), 1);
    }

    #[test]
    fn deterministic_ties() {
        let mut i = Bm25Index::new();
        i.add_text("x", "s", "same words");
        i.add_text("y", "s", "same words");
        let h = i.rank("same", 5);
        assert_eq!((h[0].id.as_str(), h[1].id.as_str()), ("x", "y"));
    }

    #[test]
    fn diversify_caps_per_source() {
        let i = idx();
        let h = i.rank("quick brown fox rust part number", 10);
        let d = diversify(&h, 1);
        let mut srcs: Vec<_> = d.iter().map(|h| h.source.clone()).collect();
        srcs.sort();
        srcs.dedup();
        assert_eq!(srcs.len(), d.len());
        assert_eq!(diversify(&h, 0).len(), 0);
    }

    #[test]
    fn mmr_prefers_diverse() {
        let mut i = Bm25Index::new();
        i.add_text("a", "s", "rust async runtime tokio scheduler");
        i.add_text("b", "s", "rust async runtime tokio scheduler tasks");
        i.add_text("c", "s", "rust async cooking recipe");
        let h = i.rank("rust async", 3);
        let m = i.mmr(&h, 0.3, 2);
        assert_eq!(m.len(), 2);
        let ids: Vec<_> = m.iter().map(|h| h.id.as_str()).collect();
        assert!(ids.contains(&"c"), "{ids:?}");
        // lambda = 1 is plain ranking order.
        let p = i.mmr(&h, 1.0, 3);
        assert_eq!(p, h);
    }

    #[test]
    fn sparse_dot_matches_bm25() {
        let i = idx();
        let h = Fnv1a::default();
        let q = i.query_vector("quick fox", &h);
        let hits = i.rank("quick fox", 10);
        for hit in hits {
            let dv = i.document_vector(hit.doc, &h).unwrap();
            assert!((f64::from(dv.dot(&q)) - hit.score).abs() < 1e-4);
        }
        assert!(i.document_vector(99, &h).is_none());
        assert!(q.indices.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn seeded_hasher_differs() {
        let a = Fnv1a { seed: 0 }.hash_term("fox");
        let b = Fnv1a { seed: 7 }.hash_term("fox");
        assert_ne!(a, b);
        assert_eq!(a, Fnv1a::default().hash_term("fox"));
    }

    #[test]
    fn term_weights_cover_doc_terms() {
        let i = idx();
        let w = i.term_weights(1).unwrap();
        assert!(w.contains_key("rust") && !w.contains_key("is"));
    }

    #[test]
    fn add_chunk_uses_chunk_id() {
        let c = crate::chunk::chunk_text("u", "hello world", &Default::default());
        let mut i = Bm25Index::new();
        i.add_chunk(&c[0], "u");
        assert_eq!(i.rank("hello", 1)[0].id, c[0].id);
        assert_eq!(i.len(), 1);
        assert!(!i.is_empty());
    }
}
