//! Client-side sparse (BM25-style) vectors for lexical matching of source chunks.
//!
//! Dense vectors find passages that *mean* the same; sparse vectors find the exact name, error code or
//! part number a dense model blurs. The sparse vector is computed here, on the client, so it works with
//! every store and, crucially, with sealed memory, where the server must never see the words.
//!
//! * [`tokenize`] lower-cases and splits on anything that is not a letter or digit.
//!   **Minimal on purpose**: no stemming, no language detection. The shared tokenizer from `ostler-extract`
//!   can replace it behind [`tokenize`] without touching the rest.
//! * A **document** vector holds, per distinct term, the BM25 term-frequency component
//!   `tf (k1 + 1) / (tf + k1 (1 - b + b len / avg_len))`. The inverse document frequency is *not* part of it:
//!   Qdrant applies it at query time to a sparse vector declared with `"modifier": "idf"`, and the in-process
//!   stores do the same arithmetic ([`idf`]).
//! * A **query** vector holds `1.0` per distinct term.
//! * Term ids are `u32` hashes of the token ([`TermHasher`]). Unsealed memory uses a plain stable hash.
//!   Sealed memory uses a **keyed** one (HMAC-SHA-256 under a sub-key of the index key, truncated to 32 bits),
//!   so Qdrant stores opaque numbers.
//!
//! ## What the server still observes with keyed ids
//!
//! Keyed ids hide *which words* a chunk contains, not the *shape* of the text:
//!
//! * the **term-frequency pattern**: how many distinct terms a chunk has, and the relative weight of each,
//!   so a term that repeats a lot stands out;
//! * the **length** of each chunk (through the weights, and the number of terms);
//! * **co-occurrence**: which opaque ids appear together, and across how many chunks. The same word always
//!   maps to the same id, so a word's document frequency is visible, and with a guess about the language an
//!   adversary with background text can mount frequency analysis (the usual attack on searchable
//!   encryption). Rare-term queries are the most exposed;
//! * at **query time**, the ids searched for, so repeated queries are linkable and the set of chunks that
//!   matched is visible;
//! * a 32-bit id space means unrelated words occasionally collide (about one pair in a vocabulary of 100 000
//!   words); that costs a little precision and hides nothing.
//!
//! It does not reveal the key, the words, or the text, which stays AES-256-GCM encrypted. People who need
//! less than this can use dense-only retrieval (weights `0.0` for the sparse side) with a local embedder.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Okapi BM25 `k1`.
pub const BM25_K1: f32 = 1.2;
/// Okapi BM25 `b`.
pub const BM25_B: f32 = 0.75;
/// Average chunk length (in tokens) assumed for length normalisation. A real average needs the whole corpus,
/// which a client that writes one chunk at a time does not have; Qdrant's own BM25 uses a constant too.
pub const BM25_AVG_LEN: f32 = 256.0;
/// Longest token kept. Longer runs are almost always blobs (base64, hashes) that only bloat the vector.
const MAX_TOKEN_CHARS: usize = 48;
/// Distinct terms kept per chunk (most frequent first), bounding vector size.
const MAX_TERMS: usize = 2048;

/// A sparse vector in Qdrant's shape: parallel, strictly increasing `indices` and their `values`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SparseVector {
    /// Term ids, sorted ascending, no duplicates.
    pub indices: Vec<u32>,
    /// Weight of each term, same length as `indices`.
    pub values: Vec<f32>,
}

impl SparseVector {
    /// Whether the vector has no terms.
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Qdrant JSON form: `{"indices": [...], "values": [...]}`.
    pub fn to_qdrant(&self) -> serde_json::Value {
        serde_json::json!({ "indices": self.indices, "values": self.values })
    }

    /// Weight of a term id, if present.
    pub fn get(&self, id: u32) -> Option<f32> {
        self.indices.binary_search(&id).ok().map(|i| self.values[i])
    }
}

/// How a token becomes a 32-bit term id.
#[derive(Clone)]
pub enum TermHasher {
    /// FNV-1a of the token. Stable and public: anyone can compute it, so use it only for memory the server
    /// may read anyway (unsealed Qdrant, local stores).
    Plain,
    /// HMAC-SHA-256 under a 32-byte sub-key, truncated to 32 bits. Used by sealed memory.
    Keyed([u8; 32]),
}

impl std::fmt::Debug for TermHasher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Plain => f.write_str("TermHasher::Plain"),
            Self::Keyed(_) => f.write_str("TermHasher::Keyed(..)"),
        }
    }
}

impl TermHasher {
    /// The id of an (already normalised) token.
    pub fn id(&self, token: &str) -> u32 {
        match self {
            Self::Plain => {
                let mut h: u32 = 0x811c_9dc5;
                for b in token.bytes() {
                    h ^= u32::from(b);
                    h = h.wrapping_mul(0x0100_0193);
                }
                h
            }
            Self::Keyed(key) => {
                let mac = crate::sealed::hmac(
                    key,
                    &[b"ostler/sparse/term/v1", &[0x1f], token.as_bytes()],
                );
                u32::from_le_bytes([mac[0], mac[1], mac[2], mac[3]])
            }
        }
    }
}

const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "but", "by", "for", "if", "in", "into", "is", "it",
    "of", "on", "or", "such", "that", "the", "their", "then", "there", "these", "they", "this",
    "to", "was", "will", "with",
];

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || crate::combining(c)
}

/// Lower-cased words of `text`, in order, with very common English words and one-letter words dropped
/// (digits are kept: "3" in "python 3"). When *everything* would be dropped (a query such as "the"), the
/// unfiltered words are returned instead, so a query is never empty just because it is short.
pub fn tokenize(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let all: Vec<&str> = lower
        .split(|c: char| !is_word_char(c))
        .filter(|w| !w.is_empty() && w.chars().count() <= MAX_TOKEN_CHARS)
        .collect();
    let kept: Vec<String> = all
        .iter()
        .filter(|w| {
            !STOPWORDS.contains(w) && (w.chars().count() > 1 || w.chars().all(char::is_numeric))
        })
        .map(|w| (*w).to_string())
        .collect();
    if kept.is_empty() {
        all.into_iter().map(String::from).collect()
    } else {
        kept
    }
}

fn collect(weights: BTreeMap<u32, f32>) -> SparseVector {
    // BTreeMap iterates in ascending id order, which is what Qdrant requires.
    let (indices, values) = weights.into_iter().unzip();
    SparseVector { indices, values }
}

/// The BM25 document vector of `text` (term-frequency component only; IDF is applied at query time).
pub fn doc_vector(text: &str, hasher: &TermHasher) -> SparseVector {
    let tokens = tokenize(text);
    let len = tokens.len() as f32;
    let mut tf: BTreeMap<String, u32> = BTreeMap::new();
    for t in tokens {
        *tf.entry(t).or_default() += 1;
    }
    // Keep the most frequent terms when a chunk is huge; ties break alphabetically so this is deterministic.
    let mut terms: Vec<(String, u32)> = tf.into_iter().collect();
    if terms.len() > MAX_TERMS {
        terms.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        terms.truncate(MAX_TERMS);
    }
    let norm = BM25_K1 * (1.0 - BM25_B + BM25_B * len / BM25_AVG_LEN);
    let mut weights: BTreeMap<u32, f32> = BTreeMap::new();
    for (term, n) in terms {
        let tf = n as f32;
        // Two words hashing to one id add up, which is what a shared posting would do anyway.
        *weights.entry(hasher.id(&term)).or_default() += tf * (BM25_K1 + 1.0) / (tf + norm);
    }
    collect(weights)
}

/// The query vector of `text`: weight `1.0` for each distinct term.
pub fn query_vector(text: &str, hasher: &TermHasher) -> SparseVector {
    collect(tokenize(text).iter().map(|t| (hasher.id(t), 1.0)).collect())
}

/// Inverse document frequency as Qdrant computes it for a sparse vector with the `idf` modifier:
/// `ln(1 + (N - n + 0.5) / (n + 0.5))` for `n` of `N` documents containing the term.
pub fn idf(n_docs: usize, n_with_term: usize) -> f32 {
    let (n, df) = (n_docs as f32, n_with_term as f32);
    (1.0 + (n - df + 0.5) / (df + 0.5)).ln()
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_A: [u8; 32] = [1; 32];
    const KEY_B: [u8; 32] = [2; 32];

    #[test]
    fn tokenizes_words_numbers_and_part_numbers() {
        assert_eq!(
            tokenize("The Quick-Brown fox, model XJ-900 v2!"),
            ["quick", "brown", "fox", "model", "xj", "900", "v2"]
        );
        // Never empty just because the query is all stopwords.
        assert_eq!(tokenize("the"), ["the"]);
        assert!(tokenize("   ").is_empty());
    }

    #[test]
    fn keeps_accented_words_whole() {
        assert_eq!(tokenize("Café au lait"), ["café", "au", "lait"]);
        // Decomposed accent: the combining mark must not split the word.
        assert_eq!(tokenize("cafe\u{301} noir"), ["cafe\u{301}", "noir"]);
    }

    #[test]
    fn doc_vectors_are_sorted_unique_and_reward_repetition_with_saturation() {
        let v = doc_vector("zebra zebra zebra apple", &TermHasher::Plain);
        assert!(v.indices.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(v.indices.len(), v.values.len());
        let zebra = v.get(TermHasher::Plain.id("zebra")).unwrap();
        let apple = v.get(TermHasher::Plain.id("apple")).unwrap();
        assert!(zebra > apple, "repetition counts");
        assert!(zebra < 3.0 * apple, "but saturates (BM25), not linear");
        // A longer document dilutes the same term.
        let long = doc_vector(
            &format!("zebra {}", "filler ".repeat(600)),
            &TermHasher::Plain,
        );
        let short = doc_vector("zebra", &TermHasher::Plain);
        let id = TermHasher::Plain.id("zebra");
        assert!(long.get(id).unwrap() < short.get(id).unwrap());
    }

    #[test]
    fn bm25_term_weight_matches_the_formula() {
        // One term, tf=1, 1 token: 1*(k1+1)/(1 + k1*(1-b+b*1/256)).
        let v = doc_vector("zebra", &TermHasher::Plain);
        let want = (BM25_K1 + 1.0) / (1.0 + BM25_K1 * (1.0 - BM25_B + BM25_B / BM25_AVG_LEN));
        assert!(
            (v.values[0] - want).abs() < 1e-6,
            "{} vs {want}",
            v.values[0]
        );
    }

    #[test]
    fn query_vectors_are_binary_and_distinct() {
        let q = query_vector("rust rust async", &TermHasher::Plain);
        assert_eq!(q.indices.len(), 2);
        assert!(q.values.iter().all(|v| *v == 1.0));
    }

    #[test]
    fn idf_matches_the_documented_formula() {
        // Rare term in 100 docs scores higher than a common one; formula spot check.
        assert!(idf(100, 1) > idf(100, 50));
        let want = (1.0f32 + (100.0 - 10.0 + 0.5) / (10.0 + 0.5)).ln();
        assert!((idf(100, 10) - want).abs() < 1e-6);
        assert!(idf(10, 10) > 0.0, "never negative");
    }

    #[test]
    fn plain_hash_is_the_stable_fnv() {
        // FNV-1a 32 test vectors.
        assert_eq!(TermHasher::Plain.id(""), 0x811c_9dc5);
        assert_eq!(TermHasher::Plain.id("a"), 0xe40c_292c);
        assert_eq!(TermHasher::Plain.id("foobar"), 0xbf9c_f968);
    }

    const TEXT: &str = "Quarterly revenue grew while the Zurich office closed; contact support at \
        ostler dot example about invoice 40291 and the passport renewal appointment schedule";

    #[test]
    fn keyed_ids_do_not_match_the_unkeyed_hash() {
        let plain = doc_vector(TEXT, &TermHasher::Plain);
        let keyed = doc_vector(TEXT, &TermHasher::Keyed(KEY_A));
        assert_eq!(
            plain.indices.len(),
            keyed.indices.len(),
            "same terms, other ids"
        );
        let overlap = keyed
            .indices
            .iter()
            .filter(|i| plain.indices.contains(i))
            .count();
        assert_eq!(overlap, 0, "no keyed id equals the public hash of its word");
        // Same weights, though: only the labels changed.
        let mut a = plain.values.clone();
        let mut b = keyed.values.clone();
        a.sort_by(f32::total_cmp);
        b.sort_by(f32::total_cmp);
        assert_eq!(a, b);
    }

    #[test]
    fn different_keys_give_different_ids_and_one_key_is_stable() {
        let a = doc_vector(TEXT, &TermHasher::Keyed(KEY_A));
        let b = doc_vector(TEXT, &TermHasher::Keyed(KEY_B));
        let again = doc_vector(TEXT, &TermHasher::Keyed(KEY_A));
        assert_eq!(
            a, again,
            "deterministic under one key, or search could not work"
        );
        let shared = a.indices.iter().filter(|i| b.indices.contains(i)).count();
        assert_eq!(shared, 0, "another key shares no ids");
    }

    #[test]
    fn a_query_matches_a_document_only_under_the_same_hasher() {
        let h = TermHasher::Keyed(KEY_A);
        let doc = doc_vector("invoice 40291 overdue", &h);
        let hit = query_vector("40291", &h);
        let miss = query_vector("40291", &TermHasher::Keyed(KEY_B));
        assert!(hit.indices.iter().all(|i| doc.get(*i).is_some()));
        assert!(miss.indices.iter().all(|i| doc.get(*i).is_none()));
    }

    #[test]
    fn huge_chunks_are_bounded() {
        let text: String = (0..5000).map(|i| format!("word{i} ")).collect();
        assert_eq!(
            doc_vector(&text, &TermHasher::Plain).indices.len(),
            MAX_TERMS
        );
    }
}
