//! Heading- and paragraph-aware chunking with stable ids.
//!
//! Every chunk is a contiguous slice of the source text (`start..end` in
//! character offsets), so offsets always slice back to the chunk text.
//! Markdown tables and fenced code blocks are kept whole when they fit in the
//! target size; larger ones are split at line boundaries. Chunking is
//! deterministic.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Chunking parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkOptions {
    /// Soft target size in characters. A chunk may exceed it slightly (a
    /// heading kept with its first block, plus overlap). Clamped to at least 50.
    pub target_chars: usize,
    /// Characters repeated from the previous chunk's tail. Clamped to half the
    /// target. Skipped when it would start inside a table or code block, or
    /// when the chunk starts at a heading.
    pub overlap_chars: usize,
}

impl Default for ChunkOptions {
    fn default() -> Self {
        Self {
            target_chars: 1200,
            overlap_chars: 150,
        }
    }
}

/// One chunk of a page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chunk {
    /// Stable id: first 16 bytes (32 hex chars) of SHA-256 over url, start offset and text.
    pub id: String,
    /// Start offset (characters, inclusive) in the source text.
    pub start: usize,
    /// End offset (characters, exclusive) in the source text.
    pub end: usize,
    /// Markdown heading path in effect at the start of the chunk (outermost first).
    pub heading_path: Vec<String>,
    /// The chunk text, equal to the source slice `start..end`.
    pub text: String,
    /// Rough token estimate, see [`estimate_tokens`].
    pub token_estimate: usize,
}

/// Rough token estimate: about 4 characters per token for alphabetic text and
/// one token per CJK character. A heuristic, not a real tokenizer.
pub fn estimate_tokens(text: &str) -> usize {
    let mut cjk = 0usize;
    let mut other = 0usize;
    for c in text.chars() {
        if is_cjk(c) {
            cjk += 1;
        } else {
            other += 1;
        }
    }
    other.div_ceil(4) + cjk
}

pub(crate) fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF
        | 0x20000..=0x2FFFF)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Heading,
    Paragraph,
    Table,
    Code,
}

#[derive(Clone, Copy, Debug)]
struct Block {
    start: usize,
    end: usize,
    kind: Kind,
    path: usize,
}

fn is_ws(c: char) -> bool {
    c.is_whitespace()
}

/// Split `text` (a page's text or markdown) into chunks. `url` only feeds the id hash.
pub fn chunk_text(url: &str, text: &str, opts: &ChunkOptions) -> Vec<Chunk> {
    let chars: Vec<char> = text.chars().collect();
    let target = opts.target_chars.max(50);
    let overlap = opts.overlap_chars.min(target / 2);

    let (blocks, paths, atomic, headings) = parse_blocks(&chars);

    // Split oversized blocks into pieces.
    let mut pieces: Vec<Block> = Vec::new();
    for b in &blocks {
        if b.end - b.start <= target {
            pieces.push(*b);
        } else {
            let sentence = matches!(b.kind, Kind::Paragraph | Kind::Heading);
            for (s, e) in split_range(&chars, b.start, b.end, target, sentence) {
                pieces.push(Block {
                    start: s,
                    end: e,
                    kind: if b.kind == Kind::Heading {
                        Kind::Paragraph
                    } else {
                        b.kind
                    },
                    path: b.path,
                });
            }
        }
    }

    // Greedy packing. Each raw chunk: (start, end, path, begins_with_heading).
    let mut raw: Vec<(usize, usize, usize, bool)> = Vec::new();
    let mut cur: Option<(usize, usize, usize, bool, bool)> = None; // + only_headings
    for p in &pieces {
        let is_heading = p.kind == Kind::Heading;
        if let Some(c) = cur {
            let overflow = p.end - c.0 > target;
            if (is_heading && !c.4) || (overflow && !c.4) {
                raw.push((c.0, c.1, c.2, c.3));
                cur = None;
            }
        }
        cur = Some(match cur {
            None => (p.start, p.end, p.path, is_heading, is_heading),
            Some(c) => (c.0, p.end, c.2, c.3, c.4 && is_heading),
        });
    }
    if let Some(c) = cur {
        raw.push((c.0, c.1, c.2, c.3));
    }

    let mut out = Vec::with_capacity(raw.len());
    let mut prev_start = 0usize;
    for (i, &(rs, re, path, heading_start)) in raw.iter().enumerate() {
        let mut start = rs;
        if i > 0 && overlap > 0 && !heading_start {
            let mut cand = rs.saturating_sub(overlap).max(prev_start);
            while cand < rs && cand > 0 && !(is_ws(chars[cand - 1]) && !is_ws(chars[cand])) {
                cand += 1;
            }
            let in_atomic = atomic.iter().any(|&(s, e)| cand >= s && cand < e);
            let has_heading = headings.iter().any(|&h| h >= cand && h < rs);
            if cand < rs && !in_atomic && !has_heading {
                start = cand;
            }
        }
        prev_start = rs;
        let body: String = chars[start..re].iter().collect();
        out.push(Chunk {
            id: chunk_id(url, start, &body),
            start,
            end: re,
            heading_path: paths[path].clone(),
            token_estimate: estimate_tokens(&body),
            text: body,
        });
    }
    out
}

fn chunk_id(url: &str, start: usize, text: &str) -> String {
    let mut h = Sha256::new();
    h.update(url.as_bytes());
    h.update([0]);
    h.update(start.to_string().as_bytes());
    h.update([0]);
    h.update(text.as_bytes());
    let d = h.finalize();
    d.iter().take(16).map(|b| format!("{b:02x}")).collect()
}

type Parsed = (
    Vec<Block>,
    Vec<Vec<String>>,
    Vec<(usize, usize)>,
    Vec<usize>,
);

fn parse_blocks(chars: &[char]) -> Parsed {
    // Line ranges (start, end excluding '\n'), then trimmed bounds.
    let mut lines: Vec<(usize, usize, usize, usize)> = Vec::new(); // (ls, le, ts, te)
    let mut ls = 0usize;
    let n = chars.len();
    let mut i = 0usize;
    while i <= n {
        if i == n || chars[i] == '\n' {
            let mut ts = ls;
            while ts < i && is_ws(chars[ts]) {
                ts += 1;
            }
            let mut te = i;
            while te > ts && is_ws(chars[te - 1]) {
                te -= 1;
            }
            lines.push((ls, i, ts, te));
            ls = i + 1;
        }
        i += 1;
    }

    let mut blocks = Vec::new();
    let mut paths: Vec<Vec<String>> = vec![Vec::new()];
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut cur_path = 0usize;
    let mut atomic = Vec::new();
    let mut headings = Vec::new();
    let blank = |l: &(usize, usize, usize, usize)| l.2 == l.3;
    let mut li = 0usize;
    while li < lines.len() {
        let l = lines[li];
        if blank(&l) {
            li += 1;
            continue;
        }
        let txt = &chars[l.2..l.3];
        if let Some((fc, flen)) = fence_open(txt) {
            let mut j = li + 1;
            while j < lines.len() {
                let lj = lines[j];
                j += 1;
                if !blank(&lj) && fence_close(&chars[lj.2..lj.3], fc, flen) {
                    break;
                }
            }
            let mut last = j - 1;
            while last > li && blank(&lines[last]) {
                last -= 1;
            }
            let b = Block {
                start: l.2,
                end: lines[last].3,
                kind: Kind::Code,
                path: cur_path,
            };
            atomic.push((b.start, b.end));
            blocks.push(b);
            li = j;
        } else if let Some((level, title)) = heading(chars, &l) {
            while stack.last().is_some_and(|(lv, _)| *lv >= level) {
                stack.pop();
            }
            stack.push((level, title));
            paths.push(stack.iter().map(|(_, t)| t.clone()).collect());
            cur_path = paths.len() - 1;
            headings.push(l.2);
            blocks.push(Block {
                start: l.2,
                end: l.3,
                kind: Kind::Heading,
                path: cur_path,
            });
            li += 1;
        } else if txt[0] == '|' {
            let mut j = li;
            while j < lines.len() && !blank(&lines[j]) && chars[lines[j].2] == '|' {
                j += 1;
            }
            let b = Block {
                start: l.2,
                end: lines[j - 1].3,
                kind: Kind::Table,
                path: cur_path,
            };
            atomic.push((b.start, b.end));
            blocks.push(b);
            li = j;
        } else {
            let mut j = li + 1;
            while j < lines.len() && !blank(&lines[j]) && !starts_other_block(chars, &lines[j]) {
                j += 1;
            }
            blocks.push(Block {
                start: l.2,
                end: lines[j - 1].3,
                kind: Kind::Paragraph,
                path: cur_path,
            });
            li = j;
        }
    }
    (blocks, paths, atomic, headings)
}

fn starts_other_block(chars: &[char], l: &(usize, usize, usize, usize)) -> bool {
    let txt = &chars[l.2..l.3];
    fence_open(txt).is_some() || heading(chars, l).is_some() || txt[0] == '|'
}

fn fence_open(txt: &[char]) -> Option<(char, usize)> {
    let c = *txt.first()?;
    if c != '`' && c != '~' {
        return None;
    }
    let run = txt.iter().take_while(|&&x| x == c).count();
    (run >= 3).then_some((c, run))
}

fn fence_close(txt: &[char], c: char, len: usize) -> bool {
    txt.len() >= len && txt.iter().all(|&x| x == c)
}

fn heading(chars: &[char], l: &(usize, usize, usize, usize)) -> Option<(usize, String)> {
    if l.2 - l.0 > 3 {
        return None;
    }
    let txt = &chars[l.2..l.3];
    let hashes = txt.iter().take_while(|&&c| c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    if hashes < txt.len() && txt[hashes] != ' ' && txt[hashes] != '\t' {
        return None;
    }
    let s: String = txt[hashes..].iter().collect();
    let title = s.trim().trim_end_matches('#').trim().to_string();
    Some((hashes, title))
}

/// Greedy split of `start..end` into pieces of at most `target` chars,
/// preferring line breaks, then sentence ends, then whitespace, then a hard cut.
fn split_range(
    chars: &[char],
    start: usize,
    end: usize,
    target: usize,
    sentence: bool,
) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut pos = start;
    loop {
        while pos < end && is_ws(chars[pos]) {
            pos += 1;
        }
        if pos >= end {
            break;
        }
        if end - pos <= target {
            out.push((pos, trim_end(chars, pos, end)));
            break;
        }
        let limit = pos + target;
        let lo = pos + (target / 4).max(1);
        let mut best = None;
        for k in (lo..=limit).rev() {
            if chars[k - 1] == '\n' {
                best = Some(k);
                break;
            }
        }
        if best.is_none() && sentence {
            for k in (lo..=limit).rev() {
                if matches!(chars[k - 1], '.' | '!' | '?') && is_ws(chars[k]) {
                    best = Some(k);
                    break;
                }
            }
        }
        if best.is_none() {
            for k in (lo..=limit).rev() {
                if is_ws(chars[k]) {
                    best = Some(k);
                    break;
                }
            }
        }
        let b = best.unwrap_or(limit);
        out.push((pos, trim_end(chars, pos, b)));
        pos = b;
    }
    out
}

fn trim_end(chars: &[char], start: usize, mut end: usize) -> usize {
    while end > start && is_ws(chars[end - 1]) {
        end -= 1;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn opts(t: usize, o: usize) -> ChunkOptions {
        ChunkOptions {
            target_chars: t,
            overlap_chars: o,
        }
    }

    #[test]
    fn empty_and_blank() {
        assert!(chunk_text("u", "", &ChunkOptions::default()).is_empty());
        assert!(chunk_text("u", "  \n\n \t", &ChunkOptions::default()).is_empty());
    }

    #[test]
    fn single_paragraph_one_chunk() {
        let c = chunk_text("u", "Hello world.", &ChunkOptions::default());
        assert_eq!(c.len(), 1);
        assert_eq!((c[0].start, c[0].end), (0, 12));
        assert_eq!(c[0].text, "Hello world.");
        assert_eq!(c[0].id.len(), 32);
        assert_eq!(c[0].token_estimate, 3);
    }

    #[test]
    fn heading_paths_and_split() {
        let t = "# A\n\nintro\n\n## B\n\nbody b\n\n## C\n\nbody c\n\n# D\n\ntail";
        let c = chunk_text("u", t, &opts(1000, 0));
        let paths: Vec<_> = c.iter().map(|c| c.heading_path.join(">")).collect();
        assert_eq!(paths, vec!["A", "A>B", "A>C", "D"]);
        assert!(c[1].text.starts_with("## B"));
    }

    #[test]
    fn table_kept_whole() {
        let table = "| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |";
        let t = format!("{}\n\n{table}\n\n{}", "x ".repeat(40), "y ".repeat(40));
        let c = chunk_text("u", &t, &opts(150, 0));
        assert!(c.iter().any(|c| c.text.contains(table)));
    }

    #[test]
    fn code_fence_kept_whole_and_not_overlapped_into() {
        let code = "```rust\nfn main() {\n\n    println!(\"hi\");\n}\n```";
        let t = format!(
            "{}\n\n{code}\n\n{}",
            "x ".repeat(30),
            "after text ".repeat(20)
        );
        let c = chunk_text("u", &t, &opts(120, 40));
        assert!(c.iter().any(|c| c.text.contains(code)));
        let last = c.last().unwrap();
        assert!(!last.text.contains("```") || last.text.contains(code));
    }

    #[test]
    fn long_table_splits_on_rows() {
        let rows: Vec<String> = (0..40).map(|i| format!("| row{i} | value{i} |")).collect();
        let t = rows.join("\n");
        let c = chunk_text("u", &t, &opts(200, 0));
        assert!(c.len() > 1);
        for ch in &c {
            for line in ch.text.lines() {
                assert!(
                    line.starts_with("| row") && line.ends_with(" |"),
                    "{line:?}"
                );
            }
        }
    }

    #[test]
    fn overlap_applied_and_word_aligned() {
        let t = "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima mike november oscar papa quebec romeo sierra tango";
        let c = chunk_text("u", t, &opts(60, 20));
        assert!(c.len() > 1);
        assert!(c[1].start < c[0].end);
        assert!(c[1].text.chars().next().is_some_and(|c| !c.is_whitespace()));
    }

    #[test]
    fn deterministic_and_ids_depend_on_url() {
        let t = "# H\n\npara one.\n\npara two.";
        let a = chunk_text("u1", t, &ChunkOptions::default());
        let b = chunk_text("u1", t, &ChunkOptions::default());
        let c = chunk_text("u2", t, &ChunkOptions::default());
        assert_eq!(a, b);
        assert_ne!(a[0].id, c[0].id);
    }

    #[test]
    fn unicode_offsets_are_chars() {
        let t = "héllo wörld 日本語のテキスト";
        let c = chunk_text("u", t, &ChunkOptions::default());
        assert_eq!(c[0].end, t.chars().count());
        assert!(c[0].token_estimate >= 7);
    }

    #[test]
    fn hard_cut_without_whitespace() {
        let t = "x".repeat(500);
        let c = chunk_text("u", &t, &opts(100, 0));
        assert!(c.len() >= 5);
        assert!(c.iter().all(|c| c.text.chars().count() <= 100));
    }

    fn check_invariants(t: &str, o: &ChunkOptions) {
        let chars: Vec<char> = t.chars().collect();
        let chunks = chunk_text("u", t, o);
        let mut covered = vec![false; chars.len()];
        let mut prev_end = 0;
        for c in &chunks {
            let s: String = chars[c.start..c.end].iter().collect();
            assert_eq!(s, c.text);
            assert!(c.end >= prev_end, "chunks must be ordered");
            prev_end = c.end;
            for f in covered.iter_mut().take(c.end).skip(c.start) {
                *f = true;
            }
        }
        for (i, ch) in chars.iter().enumerate() {
            assert!(covered[i] || ch.is_whitespace(), "uncovered {ch:?} at {i}");
        }
    }

    proptest! {
        #[test]
        fn offsets_slice_back_and_cover(
            t in "([a-zA-Z0-9 .,|`#~\\-\n\u{e9}\u{65e5}]{0,400})",
            target in 50usize..300,
            overlap in 0usize..100,
        ) {
            check_invariants(&t, &opts(target, overlap));
        }

        #[test]
        fn structured_docs(
            parts in proptest::collection::vec(
                prop_oneof![
                    Just("# Title".to_string()),
                    Just("## Sub heading".to_string()),
                    Just("| a | b |\n|---|---|\n| 1 | 2 |".to_string()),
                    Just("```\ncode\n\nmore code\n```".to_string()),
                    "[a-z ]{1,200}",
                ], 0..12),
            target in 50usize..200,
        ) {
            check_invariants(&parts.join("\n\n"), &opts(target, 30));
        }
    }
}
