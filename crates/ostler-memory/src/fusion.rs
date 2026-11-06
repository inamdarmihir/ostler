//! Rank fusion and diversity for hybrid retrieval, evaluated on the client.
//!
//! * [`rrf_fuse`]: weighted Reciprocal Rank Fusion, defined exactly as Qdrant defines it (Query API
//!   `{"rrf": {"k": .., "weights": [..]}}`, documented at qdrant.tech/documentation/search/hybrid-queries):
//!   for a point `d` and every ranking `r` it appears in,
//!   `score(d) = sum_r 1 / (k + (rank_r(d) + 1) / w_r - 1)` with `rank` **zero-based**. With `w = 1` that is
//!   the textbook `1 / (k + rank)`. Note that a weight is not a plain multiplier: it rescales the rank
//!   term. The formula was cross-checked against a real Qdrant 1.20.0 (the tests carry the numbers the
//!   server returned), so the client-side fallback and the server rank identically.
//! * [`mmr_select`]: Maximal Marginal Relevance with an optional per-group cap (per-URL grouping), over
//!   candidate vectors the store handed back. It runs on the client for every backend, so the behaviour does
//!   not depend on the server version.

use std::collections::HashMap;
use std::hash::Hash;

/// Qdrant's documented default for the RRF constant is 2 (since 1.16); the literature and this crate use 60.
pub const DEFAULT_RRF_K: f32 = 60.0;

/// One ranking's contribution for the point at zero-based `rank`. Zero for a non-positive weight.
pub fn rrf_term(weight: f32, k: f32, rank: usize) -> f32 {
    if weight <= 0.0 {
        return 0.0;
    }
    1.0 / (k + (rank as f32 + 1.0) / weight - 1.0)
}

/// The best score a point can get: first in every ranking. Dividing by it maps fused scores into `(0, 1]`.
pub fn rrf_max(weights: &[f32], k: f32) -> f32 {
    weights.iter().map(|w| rrf_term(*w, k, 0)).sum()
}

/// Fuse rankings. Each input is `(ids best first, weight)`. Returns `(id, fused score)`, best first; ties
/// keep the order in which ids were first seen, so the result is deterministic. A repeated id inside one
/// ranking counts at its best rank only.
pub fn rrf_fuse<K: Eq + Hash + Clone>(lists: &[(&[K], f32)], k: f32) -> Vec<(K, f32)> {
    let mut order: Vec<K> = Vec::new();
    let mut score: HashMap<K, f32> = HashMap::new();
    for (ids, weight) in lists {
        let mut seen: std::collections::HashSet<&K> = std::collections::HashSet::new();
        for (rank, id) in ids.iter().enumerate() {
            if !seen.insert(id) {
                continue;
            }
            let term = rrf_term(*weight, k, rank);
            match score.get_mut(id) {
                Some(s) => *s += term,
                None => {
                    order.push(id.clone());
                    score.insert(id.clone(), term);
                }
            }
        }
    }
    let mut out: Vec<(K, f32)> = order
        .into_iter()
        .map(|id| {
            let s = score[&id];
            (id, s)
        })
        .collect();
    // Stable sort: equal scores stay in first-seen order.
    out.sort_by(|a, b| b.1.total_cmp(&a.1));
    out
}

/// A candidate for [`mmr_select`].
#[derive(Debug, Clone, Copy)]
pub struct MmrItem<'a> {
    /// Relevance to the query, any scale where higher is better (fused scores are fine).
    pub relevance: f32,
    /// Embedding used to measure similarity between candidates. Without one the item is never penalised.
    pub vector: Option<&'a [f32]>,
    /// Group (for example the URL) capped by `max_per_group`.
    pub group: Option<&'a str>,
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    crate::store::cosine(a, b)
}

/// Pick up to `k` items, best first, by Maximal Marginal Relevance.
///
/// Each step takes the candidate maximising `(1 - diversity) * relevance' - diversity * max_sim`, where
/// `relevance'` is relevance scaled to `[0, 1]` by the best candidate and `max_sim` is the highest cosine
/// similarity (floored at 0) to anything already picked. `diversity = 0` returns the candidates in relevance
/// order; `diversity = 1` ignores relevance after the first pick. At most `max_per_group` items per group
/// are taken (`0` = no cap); capped candidates are skipped, never replaced by worse-scoring duplicates.
/// Returns indices into `items`.
pub fn mmr_select(
    items: &[MmrItem<'_>],
    k: usize,
    diversity: f32,
    max_per_group: usize,
) -> Vec<usize> {
    let diversity = if diversity.is_finite() {
        diversity.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let best = items
        .iter()
        .map(|i| i.relevance)
        .fold(f32::MIN, f32::max)
        .max(f32::MIN_POSITIVE);
    let mut picked: Vec<usize> = Vec::new();
    let mut per_group: HashMap<&str, usize> = HashMap::new();
    let mut taken = vec![false; items.len()];
    while picked.len() < k {
        let mut choice: Option<(usize, f32)> = None;
        for (i, item) in items.iter().enumerate() {
            if taken[i] {
                continue;
            }
            if let (Some(g), true) = (item.group, max_per_group > 0) {
                if per_group.get(g).copied().unwrap_or(0) >= max_per_group {
                    continue;
                }
            }
            let sim = match item.vector {
                Some(v) => picked
                    .iter()
                    .filter_map(|p| items[*p].vector.map(|pv| cosine(v, pv)))
                    .fold(0.0f32, f32::max),
                None => 0.0,
            };
            let value = (1.0 - diversity) * (item.relevance / best) - diversity * sim;
            // Strictly greater: on a tie the earlier (more relevant) candidate wins.
            if choice.is_none_or(|(_, v)| value > v) {
                choice = Some((i, value));
            }
        }
        let Some((i, _)) = choice else { break };
        taken[i] = true;
        if let Some(g) = items[i].group {
            *per_group.entry(g).or_default() += 1;
        }
        picked.push(i);
    }
    picked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn unweighted_rrf_is_one_over_k_plus_zero_based_rank() {
        assert!(close(rrf_term(1.0, 60.0, 0), 1.0 / 60.0));
        assert!(close(rrf_term(1.0, 60.0, 9), 1.0 / 69.0));
        assert!(close(rrf_term(1.0, 2.0, 0), 0.5));
    }

    /// The collection used for the fixtures below, queried on a real Qdrant 1.20.0:
    /// dense ranking [1, 2, 3], sparse ranking [1, 3].
    const DENSE: [u32; 3] = [1, 2, 3];
    const SPARSE: [u32; 2] = [1, 3];

    fn fused(wd: f32, ws: f32, k: f32) -> Vec<(u32, f32)> {
        rrf_fuse(&[(&DENSE[..], wd), (&SPARSE[..], ws)], k)
    }

    fn assert_matches_server(got: Vec<(u32, f32)>, server: &[(u32, f32)]) {
        assert_eq!(
            got.iter().map(|g| g.0).collect::<Vec<_>>(),
            server.iter().map(|s| s.0).collect::<Vec<_>>()
        );
        for ((_, a), (_, b)) in got.iter().zip(server) {
            assert!((a - b).abs() < 1e-6, "{a} vs server {b}");
        }
    }

    #[test]
    fn matches_qdrant_1_20_unweighted() {
        // POST /points/query  {"rrf": {"k": 60}}  returned these scores.
        assert_matches_server(
            fused(1.0, 1.0, 60.0),
            &[(1, 0.033333335), (3, 0.032522473), (2, 0.016393442)],
        );
        // Hand check: point 3 is rank 2 of the dense list and rank 1 of the sparse list.
        assert!(close(fused(1.0, 1.0, 60.0)[1].1, 1.0 / 62.0 + 1.0 / 61.0));
        // {"rrf": {"k": 1}}
        assert_matches_server(fused(1.0, 1.0, 1.0), &[(1, 2.0), (3, 0.8333334), (2, 0.5)]);
    }

    #[test]
    fn matches_qdrant_1_20_weighted() {
        // {"rrf": {"k": 60, "weights": [1.0, 3.0]}}
        assert_matches_server(
            fused(1.0, 3.0, 60.0),
            &[(1, 0.0335206), (3, 0.032888807), (2, 0.016393442)],
        );
        // weights [3.0, 1.0]
        assert_matches_server(
            fused(3.0, 1.0, 60.0),
            &[(1, 0.0335206), (3, 0.03306011), (2, 0.016759776)],
        );
        // weights [2.0, 2.0] and [0.5, 0.5]: weights are not just a global scale.
        assert_matches_server(
            fused(2.0, 2.0, 60.0),
            &[(1, 0.033613447), (3, 0.033195592), (2, 0.016666668)],
        );
        assert_matches_server(
            fused(0.5, 0.5, 60.0),
            &[(1, 0.032786883), (3, 0.031257633), (2, 0.015873017)],
        );
        // weights [1.0, 0.0]: the sparse ranking contributes nothing.
        assert_matches_server(
            fused(1.0, 0.0, 60.0),
            &[(1, 0.016666668), (2, 0.016393442), (3, 0.016129032)],
        );
    }

    #[test]
    fn a_heavier_weight_can_flip_the_order() {
        // Each ranking found a different point first.
        let dense = [1u32];
        let sparse = [2u32];
        let by = |wd, ws| rrf_fuse(&[(&dense[..], wd), (&sparse[..], ws)], 60.0)[0].0;
        assert_eq!(by(3.0, 1.0), 1);
        assert_eq!(by(1.0, 3.0), 2);
        assert_eq!(by(1.0, 1.0), 1, "an exact tie keeps first-seen order");
        // Qdrant's weighting rescales the rank term, so it cannot be relied on to override a strong
        // disagreement lower down: with dense [1, 2] and sparse [2, 1], even weight 3 on dense leaves 2 first.
        let (d, s) = ([1u32, 2], [2u32, 1]);
        assert_eq!(rrf_fuse(&[(&d[..], 3.0), (&s[..], 1.0)], 60.0)[0].0, 2);
    }

    #[test]
    fn max_score_is_first_in_every_list() {
        let top = fused(1.0, 3.0, 60.0)[0].1;
        assert!(close(top, rrf_max(&[1.0, 3.0], 60.0)));
    }

    #[test]
    fn empty_lists_and_duplicate_ids_are_handled() {
        let none: [u32; 0] = [];
        assert!(rrf_fuse(&[(&none[..], 1.0)], 60.0).is_empty());
        let dup = [5u32, 5, 6];
        let r = rrf_fuse(&[(&dup[..], 1.0)], 60.0);
        assert_eq!(r.len(), 2);
        assert!(
            close(r[0].1, 1.0 / 60.0),
            "repeat counts at its best rank only"
        );
    }

    fn item<'a>(relevance: f32, vector: &'a [f32], group: &'a str) -> MmrItem<'a> {
        MmrItem {
            relevance,
            vector: Some(vector),
            group: Some(group),
        }
    }

    #[test]
    fn mmr_picks_a_diverse_set() {
        let a = [1.0, 0.0];
        let a2 = [0.99, 0.1]; // near duplicate of a
        let b = [0.0, 1.0];
        let items = [item(1.0, &a, "x"), item(0.95, &a2, "y"), item(0.6, &b, "z")];
        assert_eq!(
            mmr_select(&items, 2, 0.0, 0),
            [0, 1],
            "no diversity: pure relevance"
        );
        assert_eq!(
            mmr_select(&items, 2, 0.5, 0),
            [0, 2],
            "diversity skips the near duplicate"
        );
        assert_eq!(mmr_select(&items, 3, 0.5, 0), [0, 2, 1]);
    }

    #[test]
    fn mmr_caps_items_per_group() {
        let v = [[1.0, 0.0], [0.9, 0.1], [0.0, 1.0]];
        let items = [
            item(1.0, &v[0], "same.com"),
            item(0.9, &v[1], "same.com"),
            item(0.5, &v[2], "other.com"),
        ];
        assert_eq!(mmr_select(&items, 3, 0.0, 1), [0, 2]);
        assert_eq!(mmr_select(&items, 3, 0.0, 2), [0, 1, 2]);
    }

    #[test]
    fn mmr_handles_edges() {
        assert!(mmr_select(&[], 5, 0.5, 0).is_empty());
        let v = [1.0, 0.0];
        let items = [item(1.0, &v, "a")];
        assert!(mmr_select(&items, 0, 0.5, 0).is_empty());
        assert_eq!(mmr_select(&items, 9, f32::NAN, 0), [0]);
        // Items without vectors are never penalised.
        let bare = [
            MmrItem {
                relevance: 1.0,
                vector: None,
                group: None,
            },
            MmrItem {
                relevance: 0.5,
                vector: None,
                group: None,
            },
        ];
        assert_eq!(mmr_select(&bare, 2, 1.0, 0).len(), 2);
    }

    /// Property-style checks over pseudo-random inputs (no extra dependency in this crate).
    #[test]
    fn mmr_invariants_hold_on_random_inputs() {
        let mut s = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 11) as f32 / (1u64 << 53) as f32
        };
        for _ in 0..200 {
            let n = 1 + (next() * 20.0) as usize;
            let vecs: Vec<[f32; 3]> = (0..n).map(|_| [next(), next(), next()]).collect();
            let groups: Vec<String> = (0..n)
                .map(|_| format!("g{}", (next() * 4.0) as u32))
                .collect();
            let mut rel: Vec<f32> = (0..n).map(|_| next()).collect();
            rel.sort_by(|a, b| b.total_cmp(a));
            let items: Vec<MmrItem> = (0..n)
                .map(|i| MmrItem {
                    relevance: rel[i],
                    vector: Some(&vecs[i]),
                    group: Some(&groups[i]),
                })
                .collect();
            let k = (next() * 10.0) as usize;
            let div = next();
            let cap = (next() * 3.0) as usize;
            let got = mmr_select(&items, k, div, cap);
            assert!(got.len() <= k.min(n));
            let mut uniq = got.clone();
            uniq.sort_unstable();
            uniq.dedup();
            assert_eq!(uniq.len(), got.len(), "no repeats");
            if cap > 0 {
                for g in &groups {
                    assert!(got.iter().filter(|i| &groups[**i] == g).count() <= cap);
                }
            }
            if cap == 0 {
                assert_eq!(got.len(), k.min(n), "fills k when uncapped");
                // Zero diversity is plain relevance order.
                assert_eq!(
                    mmr_select(&items, k, 0.0, 0),
                    (0..k.min(n)).collect::<Vec<_>>()
                );
            }
        }
    }

    #[test]
    fn rrf_fuse_is_order_stable_across_runs() {
        let a: Vec<u32> = (0..50).collect();
        let b: Vec<u32> = (0..50).rev().collect();
        let first = rrf_fuse(&[(&a[..], 1.0), (&b[..], 1.0)], 60.0);
        for _ in 0..5 {
            assert_eq!(first, rrf_fuse(&[(&a[..], 1.0), (&b[..], 1.0)], 60.0));
        }
    }
}
