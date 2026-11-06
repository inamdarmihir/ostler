//! Reduce the markdown that `read` produces to the plain words a person sees.
//!
//! Quote verification compares words, not markup. A reader who copies `designed by **Marta Ilves**` or
//! `[Marta Ilves](https://x/y)` out of a rendered passage must still verify, and so must a reader who
//! types the bare sentence. Both the stored page and the quote therefore go through [`plain_text`] first.
//!
//! The function is deterministic and total: it never panics and never adds words. It understands exactly
//! the dialect the browser's `read` emits (ATX headings, `-`/`1.` lists, `>` quotes, pipe tables, fenced
//! code, `[text](url)` links with `\[`/`\]`/`\\` escapes, `**`, `*`, `~~` and backtick spans). Text that
//! merely looks like markdown inside code fences is kept as written.

/// Plain text for `md`: markup removed, one blank line between blocks, whitespace inside lines collapsed.
pub fn plain_text(md: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    for raw in md.lines() {
        let line = raw.trim_end();
        if let Some((ch, n)) = fence {
            if is_fence_close(line, ch, n) {
                fence = None;
            } else {
                out.push(line.to_string());
            }
            continue;
        }
        if let Some((ch, n)) = fence_open(line) {
            fence = Some((ch, n));
            continue;
        }
        let t = line.trim_start();
        if t.is_empty() {
            if out.last().is_some_and(|l| !l.is_empty()) {
                out.push(String::new());
            }
            continue;
        }
        if is_rule(t) || is_table_separator(t) {
            continue;
        }
        let body = strip_block_prefix(t);
        let body = if body.starts_with('|') {
            table_row(body)
        } else {
            body.to_string()
        };
        let inline = collapse(&strip_inline(&body));
        if !inline.is_empty() {
            out.push(inline);
        }
    }
    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out.join("\n")
}

fn fence_open(line: &str) -> Option<(char, usize)> {
    let t = line.trim_start();
    let ch = t.chars().next()?;
    if ch != '`' && ch != '~' {
        return None;
    }
    let n = t.chars().take_while(|&c| c == ch).count();
    (n >= 3).then_some((ch, n))
}

fn is_fence_close(line: &str, ch: char, n: usize) -> bool {
    let t = line.trim();
    t.chars().count() >= n && t.chars().all(|c| c == ch)
}

fn is_rule(t: &str) -> bool {
    let s: Vec<char> = t.chars().filter(|c| !c.is_whitespace()).collect();
    s.len() >= 3 && matches!(s[0], '-' | '*' | '_') && s.iter().all(|&c| c == s[0])
}

fn is_table_separator(t: &str) -> bool {
    t.starts_with('|')
        && t.contains('-')
        && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t'))
}

/// Remove leading `>`, list markers and heading hashes (repeatedly: `> - item`, `- 1. x`).
fn strip_block_prefix(mut t: &str) -> &str {
    loop {
        let before = t;
        t = t.trim_start();
        if let Some(r) = t.strip_prefix('>') {
            t = r;
            continue;
        }
        let hashes = t.chars().take_while(|&c| c == '#').count();
        if (1..=6).contains(&hashes) && t[hashes..].starts_with(' ') {
            t = &t[hashes + 1..];
            continue;
        }
        if let Some(r) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
            t = r;
            continue;
        }
        let digits = t.chars().take_while(char::is_ascii_digit).count();
        if digits > 0 && digits <= 9 && t[digits..].starts_with(". ") {
            t = &t[digits + 2..];
            continue;
        }
        if t == before {
            return t;
        }
    }
}

/// `| a | b \| c |` -> `a b | c` (cells joined by a space; escaped pipes become literal pipes).
fn table_row(row: &str) -> String {
    let mut cells: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut chars = row.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                cur.push('\\');
                cur.push('|');
                chars.next();
            }
            '|' => {
                cells.push(std::mem::take(&mut cur));
            }
            _ => cur.push(c),
        }
    }
    cells.push(cur);
    cells
        .into_iter()
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Links, emphasis, code spans and backslash escapes.
fn strip_inline(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < c.len() {
        match c[i] {
            '\\' if i + 1 < c.len() => {
                out.push(c[i + 1]);
                i += 2;
            }
            '[' => {
                if let Some((text_end, after)) = link_at(&c, i) {
                    out.push_str(&strip_inline(
                        &c[i + 1..text_end].iter().collect::<String>(),
                    ));
                    i = after;
                } else {
                    out.push('[');
                    i += 1;
                }
            }
            '*' | '~' | '`' => {
                // Emphasis, strike and code markers carry no words. A lone `~` or `*` the page really
                // wrote is escaped by the browser (`*` never is, which is why `*` is always syntax here);
                // `~` is dropped only when doubled.
                let ch = c[i];
                let run = c[i..].iter().take_while(|&&x| x == ch).count();
                if ch == '~' && run < 2 {
                    out.push('~');
                }
                i += run;
            }
            ch => {
                out.push(ch);
                i += 1;
            }
        }
    }
    out
}

/// For `[` at `open`: the index of the matching `]` and the index after `(url)`, when it is a link.
fn link_at(c: &[char], open: usize) -> Option<(usize, usize)> {
    let mut depth = 0usize;
    let mut j = open;
    while j < c.len() {
        match c[j] {
            '\\' => j += 1,
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        j += 1;
    }
    if j >= c.len() || c.get(j + 1) != Some(&'(') {
        return None;
    }
    let close = c[j + 2..].iter().position(|&x| x == ')')? + j + 2;
    // A URL has no spaces (the browser percent-encodes them); this keeps `[a] (b)`-style prose intact.
    if c[j + 2..close].iter().any(|x| x.is_whitespace()) {
        return None;
    }
    Some((j, close + 1))
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn links_and_emphasis_reduce_to_their_words() {
        assert_eq!(
            plain_text("It was designed by [**Marta Ilves**](https://x.test/m?a=1&b=2) in *1887*."),
            "It was designed by Marta Ilves in 1887."
        );
    }

    #[test]
    fn escapes_are_undone_and_brackets_that_are_not_links_survive() {
        assert_eq!(
            plain_text(r"[a \[b\] c](https://x.test/p) and [not a link] (nor this)"),
            "a [b] c and [not a link] (nor this)"
        );
    }

    #[test]
    fn block_markers_go_but_words_stay() {
        let md = "# Title\n\n> quoted *text*\n\n- one\n- two\n\n1. first\n2. second\n\n---\n\nend";
        assert_eq!(
            plain_text(md),
            "Title\n\nquoted text\n\none\ntwo\n\nfirst\nsecond\n\nend"
        );
    }

    #[test]
    fn tables_keep_cells_in_order_and_lose_the_separator_row() {
        let md = "| Region | Output |\n| --- | --- |\n| Delta | 71 |\n| Ridge | 40 \\| est |";
        assert_eq!(plain_text(md), "Region Output\nDelta 71\nRidge 40 | est");
    }

    #[test]
    fn fenced_code_is_kept_verbatim() {
        let md = "before\n\n```rust\nlet x = **y**;\n```\n\nafter";
        assert_eq!(plain_text(md), "before\n\nlet x = **y**;\n\nafter");
    }

    #[test]
    fn a_quote_copied_from_the_markdown_and_one_typed_plain_agree() {
        let page = plain_text("The lock opened in **1887** and was designed by [Marta Ilves](/m).");
        let copied = plain_text("opened in **1887** and was designed by [Marta Ilves](/m)");
        let typed = plain_text("opened in 1887 and was designed by Marta Ilves");
        assert_eq!(copied, typed);
        assert!(page.contains(&typed));
    }

    #[test]
    fn single_tilde_is_text() {
        assert_eq!(
            plain_text("about ~5 km, ~~old~~ new"),
            "about ~5 km, old new"
        );
    }

    proptest! {
        #[test]
        fn never_panics_and_never_grows(s in "\\PC{0,200}") {
            prop_assert!(plain_text(&s).chars().count() <= s.chars().count());
        }
    }
}
