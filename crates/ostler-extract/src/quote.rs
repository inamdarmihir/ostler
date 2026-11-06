//! Verify that a quote appears in stored page text.
//!
//! Both sides are normalised before comparison: case-folded, whitespace runs
//! collapsed, curly quotes straightened, dash variants mapped to `-`, soft
//! hyphens and zero-width characters dropped. A quote may contain `...` or
//! `…` meaning "something omitted here": the pieces must then appear in order.
//! Offsets returned are **character** offsets into the *original* text.
//!
//! This proves only that the words occur in the text; it says nothing about
//! whether the claim they support is true.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// Minimum normalised length (all segments together) of a verifiable quote, in characters.
pub const MIN_QUOTE_CHARS: usize = 12;
/// Minimum normalised length of each segment between ellipsis markers.
pub const MIN_SEGMENT_CHARS: usize = 4;
/// Maximum number of normalised characters scanned when looking for the nearest match.
pub const NEAREST_SCAN_CHARS: usize = 2_000_000;
const NEAREST_MIN_SIMILARITY: f32 = 0.35;
const MAX_ANCHOR_CANDIDATES: usize = 64;

/// Result of [`verify_quote`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum QuoteVerdict {
    /// Found. `start..end` are character offsets in the original text (first
    /// occurrence; with gap markers, from the first segment's start to the
    /// last segment's end).
    Verified {
        /// Start offset, inclusive.
        start: usize,
        /// End offset, exclusive.
        end: usize,
    },
    /// Not found. `nearest` is the most similar passage (original text) and a
    /// trigram Dice similarity in `0..=1`, when anything is reasonably close.
    /// The similarity is a cheap heuristic over at most
    /// [`NEAREST_SCAN_CHARS`] characters.
    NotFound {
        /// Closest passage and its similarity.
        nearest: Option<(String, f32)>,
    },
    /// Quote shorter than [`MIN_QUOTE_CHARS`] (or a segment shorter than
    /// [`MIN_SEGMENT_CHARS`]) after normalisation: too weak to count as evidence.
    TooShort,
}

/// Slice `text` by character offsets (clamped to the text length).
pub fn char_slice(text: &str, start: usize, end: usize) -> String {
    text.chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

struct Norm {
    text: String,
    /// Byte offset of each normalised char, plus the total length.
    byte_off: Vec<usize>,
    /// Original `(start, end)` char range of each normalised char.
    orig: Vec<(usize, usize)>,
}

impl Norm {
    fn char_count(&self) -> usize {
        self.orig.len()
    }
}

fn is_dropped(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}' | '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{2060}' | '\u{FEFF}' | '\u{180E}'
    )
}

fn fold_char(c: char) -> char {
    match c {
        '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' | '\u{2032}' => '\'',
        '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' | '\u{2033}' => '"',
        '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
        _ => c,
    }
}

fn normalise(text: &str) -> Norm {
    let mut chars_out: Vec<char> = Vec::new();
    let mut orig: Vec<(usize, usize)> = Vec::new();
    let mut in_ws = false;
    for (i, c) in text.chars().enumerate() {
        if is_dropped(c) {
            continue;
        }
        if c.is_whitespace() {
            if in_ws {
                if let Some(l) = orig.last_mut() {
                    l.1 = i + 1;
                }
            } else if !chars_out.is_empty() {
                chars_out.push(' ');
                orig.push((i, i + 1));
                in_ws = true;
            }
            continue;
        }
        in_ws = false;
        for l in fold_char(c).to_lowercase() {
            chars_out.push(l);
            orig.push((i, i + 1));
        }
    }
    if chars_out.last() == Some(&' ') {
        chars_out.pop();
        orig.pop();
    }
    let mut byte_off = Vec::with_capacity(chars_out.len() + 1);
    let mut s = String::new();
    for &c in &chars_out {
        byte_off.push(s.len());
        s.push(c);
    }
    byte_off.push(s.len());
    Norm {
        text: s,
        byte_off,
        orig,
    }
}

/// Split a raw quote on `...`/`…` gap markers, dropping `[`/`]` around them.
fn split_segments(quote: &str, strip_outer_quotes: bool) -> Vec<String> {
    let chars: Vec<char> = quote.chars().collect();
    let mut segs: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\u{2026}' {
            segs.push(std::mem::take(&mut cur));
            i += 1;
        } else if chars[i] == '.'
            && chars.get(i + 1) == Some(&'.')
            && chars.get(i + 2) == Some(&'.')
        {
            segs.push(std::mem::take(&mut cur));
            while chars.get(i) == Some(&'.') {
                i += 1;
            }
        } else {
            cur.push(chars[i]);
            i += 1;
        }
    }
    segs.push(cur);
    let n = segs.len();
    for (k, s) in segs.iter_mut().enumerate() {
        let mut t = s.trim().to_string();
        if k + 1 < n {
            if let Some(x) = t.strip_suffix('[') {
                t = x.to_string();
            }
        }
        if k > 0 {
            if let Some(x) = t.strip_prefix(']') {
                t = x.to_string();
            }
        }
        *s = t;
    }
    if strip_outer_quotes {
        let q = |c: char| matches!(c, '"' | '\u{201C}' | '\u{201D}' | '\u{201E}');
        if let Some(f) = segs.first_mut() {
            *f = f.trim_start_matches(q).to_string();
        }
        if let Some(l) = segs.last_mut() {
            *l = l.trim_end_matches(q).to_string();
        }
    }
    segs.into_iter()
        .map(|s| normalise(&s).text)
        .filter(|s| !s.is_empty())
        .collect()
}

fn find_in_order(hay: &Norm, segs: &[String]) -> Option<(usize, usize)> {
    let first = segs.first()?;
    for (bpos, _) in hay
        .text
        .match_indices(first.as_str())
        .take(MAX_ANCHOR_CANDIDATES)
    {
        let mut end = bpos + first.len();
        let mut ok = true;
        for seg in &segs[1..] {
            match hay.text[end..].find(seg.as_str()) {
                Some(p) => end += p + seg.len(),
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            let cs = hay.byte_off.binary_search(&bpos).ok()?;
            let ce = hay.byte_off.binary_search(&end).ok()?;
            return Some((cs, ce));
        }
    }
    None
}

/// Verify `quote` against `page_text`. See the module docs for normalisation rules.
pub fn verify_quote(page_text: &str, quote: &str) -> QuoteVerdict {
    let mut segs = split_segments(quote, false);
    let total: usize = segs.iter().map(|s| s.chars().count()).sum();
    if segs.is_empty()
        || total < MIN_QUOTE_CHARS
        || segs.iter().any(|s| s.chars().count() < MIN_SEGMENT_CHARS)
    {
        return QuoteVerdict::TooShort;
    }
    let hay = normalise(page_text);
    let mut found = find_in_order(&hay, &segs);
    if found.is_none() {
        // The agent may have wrapped the quote in quotation marks.
        let stripped = split_segments(quote, true);
        if stripped != segs && !stripped.is_empty() {
            let t: usize = stripped.iter().map(|s| s.chars().count()).sum();
            if t >= MIN_QUOTE_CHARS {
                found = find_in_order(&hay, &stripped);
                if found.is_some() {
                    segs = stripped;
                }
            }
        }
    }
    if let Some((cs, ce)) = found {
        if ce > cs {
            return QuoteVerdict::Verified {
                start: hay.orig[cs].0,
                end: hay.orig[ce - 1].1,
            };
        }
    }
    QuoteVerdict::NotFound {
        nearest: nearest(page_text, &hay, &segs.join(" ")),
    }
}

fn trigrams(c: &[char]) -> HashSet<u64> {
    c.windows(3)
        .map(|w| {
            ((u64::from(w[0] as u32) * 1_000_003 + u64::from(w[1] as u32)) * 1_000_003)
                + u64::from(w[2] as u32)
        })
        .collect()
}

fn nearest(original: &str, hay: &Norm, q: &str) -> Option<(String, f32)> {
    let qc: Vec<char> = q.chars().take(512).collect();
    if qc.len() < 3 || hay.char_count() < 3 {
        return None;
    }
    let qg = trigrams(&qc);
    let hc: Vec<char> = hay.text.chars().take(NEAREST_SCAN_CHARS).collect();
    let l = qc.len().min(hc.len());
    let step = (l / 2).max(1);
    let mut best: Option<(f32, usize, usize)> = None;
    let mut s = 0usize;
    loop {
        let e = (s + l).min(hc.len());
        let wg = trigrams(&hc[s..e]);
        let inter = qg.intersection(&wg).count();
        let denom = qg.len() + wg.len();
        let sim = if denom == 0 {
            0.0
        } else {
            2.0 * inter as f32 / denom as f32
        };
        if best.is_none_or(|b| sim > b.0) {
            best = Some((sim, s, e));
        }
        if e >= hc.len() {
            break;
        }
        s += step;
    }
    let (sim, s, e) = best?;
    if sim < NEAREST_MIN_SIMILARITY || e <= s {
        return None;
    }
    let (os, oe) = (hay.orig[s].0, hay.orig[e - 1].1);
    let mut snippet = char_slice(original, os, oe);
    if snippet.chars().count() > 240 {
        snippet = snippet.chars().take(240).collect::<String>() + "\u{2026}";
    }
    Some((snippet, sim))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const PAGE: &str = "The  quick\u{00A0}brown fox\njumps over\u{200B} the lazy dog. \u{201C}It\u{2019}s fine\u{201D} \u{2014} really, said Dr. Smith. Row total: 1,234 items.";

    fn v(q: &str) -> Option<String> {
        match verify_quote(PAGE, q) {
            QuoteVerdict::Verified { start, end } => Some(char_slice(PAGE, start, end)),
            _ => None,
        }
    }

    #[test]
    fn verifies_with_whitespace_case_and_zero_width() {
        assert_eq!(
            v("quick brown FOX jumps over the lazy dog").unwrap(),
            "quick\u{00A0}brown fox\njumps over\u{200B} the lazy dog"
        );
    }

    #[test]
    fn curly_and_straight_quotes_and_dashes() {
        assert!(v("\"It's fine\" - really, said Dr. Smith").is_some());
        assert!(v("It\u{2019}s fine\u{201D} \u{2013} really").is_some());
    }

    #[test]
    fn surrounding_quote_marks_are_tolerated() {
        assert!(v("\"the lazy dog. \u{201C}It's fine\"").is_some());
        assert!(v("\u{201C}quick brown fox jumps over\u{201D}").is_some());
    }

    #[test]
    fn ellipsis_gaps() {
        let r = v("The quick brown ... lazy dog").unwrap();
        assert!(r.starts_with("The") && r.ends_with("lazy dog"));
        assert!(v("The quick brown \u{2026} lazy dog").is_some());
        assert!(v("The quick brown [...] lazy dog").is_some());
        // Order matters.
        assert!(v("lazy dog ... The quick brown").is_none());
    }

    #[test]
    fn offsets_are_original_chars() {
        let text = "héllo wörld — the answer is forty-two, truly.";
        match verify_quote(text, "the answer is forty-two") {
            QuoteVerdict::Verified { start, end } => {
                assert_eq!(char_slice(text, start, end), "the answer is forty-two");
                assert_eq!(start, 14);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn too_short() {
        assert_eq!(verify_quote(PAGE, "the lazy"), QuoteVerdict::TooShort);
        assert_eq!(verify_quote(PAGE, ""), QuoteVerdict::TooShort);
        assert_eq!(verify_quote(PAGE, "...  ..."), QuoteVerdict::TooShort);
        assert_eq!(
            verify_quote(PAGE, "the quick brown fox ... dog"),
            QuoteVerdict::TooShort
        );
    }

    #[test]
    fn not_found_with_nearest() {
        match verify_quote(PAGE, "The quick brown fox leaps over the lazy dog") {
            QuoteVerdict::NotFound {
                nearest: Some((s, sim)),
            } => {
                assert!(s.contains("quick"), "{s}");
                assert!(sim > 0.5 && sim <= 1.0);
            }
            other => panic!("{other:?}"),
        }
        match verify_quote(PAGE, "completely unrelated zebra statement") {
            QuoteVerdict::NotFound { nearest } => assert!(nearest.is_none()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn large_input_is_bounded() {
        let big = "lorem ipsum dolor sit amet ".repeat(40_000);
        let r = verify_quote(&big, "lorem ipsum dolor sat amet consectetur");
        assert!(matches!(r, QuoteVerdict::NotFound { .. }));
    }

    #[test]
    fn repeated_anchor_tries_later_occurrence() {
        let t = "alpha beta gamma. alpha beta delta epsilon.";
        assert!(matches!(
            verify_quote(t, "alpha beta ... delta epsilon"),
            QuoteVerdict::Verified { .. }
        ));
    }

    proptest! {
        #[test]
        fn cut_quote_always_verifies(
            words in proptest::collection::vec("[a-zA-Z0-9\u{e9}\u{f6}]{1,9}", 6..40),
            a in 0usize..20, len in 3usize..12,
            seps in proptest::collection::vec(prop_oneof![Just(" "), Just("  "), Just("\n"), Just("\t "), Just("\u{00A0}")], 40),
        ) {
            let mut text = String::new();
            let mut spans = Vec::new();
            for (i, w) in words.iter().enumerate() {
                let s = text.chars().count();
                text.push_str(w);
                spans.push((s, text.chars().count()));
                text.push_str(seps[i % seps.len()]);
            }
            let a = a % words.len();
            let b = (a + len).min(words.len());
            let (qs, qe) = (spans[a].0, spans[b - 1].1);
            let quote = char_slice(&text, qs, qe);
            match verify_quote(&text, &quote) {
                QuoteVerdict::Verified { start, end } => {
                    prop_assert!(start <= qs + text.chars().count());
                    prop_assert!(end > start);
                }
                QuoteVerdict::TooShort => {
                    let n: usize = quote.split_whitespace().map(|w| w.chars().count()).sum::<usize>()
                        + quote.split_whitespace().count().saturating_sub(1);
                    prop_assert!(n < MIN_QUOTE_CHARS);
                }
                other => prop_assert!(false, "not verified: {:?} in {:?}", other, text),
            }
            // Whitespace mutation of the quote must not matter.
            let mutated = quote.split_whitespace().collect::<Vec<_>>().join(" \n  ");
            prop_assert_eq!(verify_quote(&text, &quote), verify_quote(&text, &mutated));
            let upper = quote.to_uppercase();
            prop_assert_eq!(verify_quote(&text, &quote), verify_quote(&text, &upper));
        }
    }
}
