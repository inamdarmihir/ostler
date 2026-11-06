//! URL text fragments (`#:~:text=`) for verified quotes.
//!
//! Browsers match text fragments within the rendered page, so a fragment built
//! from stored text can still fail to highlight (for example when the stored
//! text came from a reader-mode transform). Treat the link as a best-effort
//! locator, not as proof.

use std::fmt;

use crate::quote::char_slice;

/// Fragment building options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FragmentOptions {
    /// Quotes longer than this many characters use the `start,end` range form. Default 120.
    pub range_threshold: usize,
    /// Approximate length of each end of the range form. Default 40.
    pub edge_chars: usize,
    /// For [`fragment_from_verified`]: characters of prefix/suffix context taken
    /// from the page (0 disables). Default 0.
    pub context_chars: usize,
}

impl Default for FragmentOptions {
    fn default() -> Self {
        Self {
            range_threshold: 120,
            edge_chars: 40,
            context_chars: 0,
        }
    }
}

/// Why a fragment URL could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FragmentError {
    /// Only `http` and `https` URLs are accepted.
    NotHttp,
    /// The quote is empty after whitespace normalisation.
    EmptyQuote,
}

impl fmt::Display for FragmentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotHttp => f.write_str("text fragments are only built for http(s) URLs"),
            Self::EmptyQuote => f.write_str("quote is empty"),
        }
    }
}

impl std::error::Error for FragmentError {}

/// Percent-encode everything except ASCII alphanumerics and `_ . ~`.
/// That includes `- , & #` (reserved by the text-fragment syntax) and all
/// non-ASCII bytes (UTF-8).
fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// First `n`-ish characters of `words`, cut at a word boundary (at least one word).
fn head_words(s: &str, n: usize) -> String {
    let mut out = String::new();
    for w in s.split(' ') {
        if !out.is_empty() && out.chars().count() + 1 + w.chars().count() > n {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    out
}

fn tail_words(s: &str, n: usize) -> String {
    let rev: Vec<&str> = s.split(' ').rev().collect();
    let mut out: Vec<&str> = Vec::new();
    let mut len = 0usize;
    for w in rev {
        let add = w.chars().count() + usize::from(!out.is_empty());
        if !out.is_empty() && len + add > n {
            break;
        }
        len += add;
        out.push(w);
    }
    out.reverse();
    out.join(" ")
}

/// Build `url#:~:text=...` for `quote`. Any existing fragment is dropped.
///
/// Short quotes use the exact form `text=quote`; quotes longer than
/// `range_threshold` use `text=start,end` built from the first and last words
/// (this also works across block boundaries). `prefix` and `suffix` add
/// disambiguating context (`prefix-,` and `,-suffix`).
pub fn text_fragment_url(
    url: &str,
    quote: &str,
    prefix: Option<&str>,
    suffix: Option<&str>,
    opts: &FragmentOptions,
) -> Result<String, FragmentError> {
    let lower = url.trim().to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err(FragmentError::NotHttp);
    }
    let base = url.trim().split('#').next().unwrap_or("");
    let scheme_len = if lower.starts_with("https://") { 8 } else { 7 };
    if base.len() <= scheme_len {
        return Err(FragmentError::NotHttp);
    }
    let q = collapse(quote);
    if q.is_empty() {
        return Err(FragmentError::EmptyQuote);
    }
    let mut d = String::from("text=");
    if let Some(p) = prefix.map(collapse).filter(|p| !p.is_empty()) {
        d.push_str(&enc(&p));
        d.push_str("-,");
    }
    if q.chars().count() > opts.range_threshold {
        let start = head_words(&q, opts.edge_chars);
        let end = tail_words(&q, opts.edge_chars);
        d.push_str(&enc(&start));
        d.push(',');
        d.push_str(&enc(&end));
    } else {
        d.push_str(&enc(&q));
    }
    if let Some(s) = suffix.map(collapse).filter(|s| !s.is_empty()) {
        d.push_str(",-");
        d.push_str(&enc(&s));
    }
    Ok(format!("{base}#:~:{d}"))
}

/// Build a fragment URL for a quote verified at char offsets `start..end` of
/// `page_text` (as returned by [`crate::quote::verify_quote`]). With
/// `opts.context_chars > 0`, whole words of context before and after the
/// match are added as prefix and suffix.
pub fn fragment_from_verified(
    url: &str,
    page_text: &str,
    start: usize,
    end: usize,
    opts: &FragmentOptions,
) -> Result<String, FragmentError> {
    let quote = char_slice(page_text, start, end);
    let (mut prefix, mut suffix) = (None, None);
    if opts.context_chars > 0 {
        let before: String = {
            let from = start.saturating_sub(opts.context_chars);
            let s = collapse(&char_slice(page_text, from, start));
            // Drop a possibly-cut first word unless it begins at a boundary.
            if from > 0 && !s.is_empty() {
                s.split_once(' ').map_or(String::new(), |x| x.1.to_string())
            } else {
                s
            }
        };
        let after: String = {
            let s = collapse(&char_slice(page_text, end, end + opts.context_chars));
            match s.rsplit_once(' ') {
                Some((x, _)) => x.to_string(),
                None => String::new(),
            }
        };
        prefix = Some(before).filter(|s| !s.is_empty());
        suffix = Some(after).filter(|s| !s.is_empty());
    }
    text_fragment_url(url, &quote, prefix.as_deref(), suffix.as_deref(), opts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn o() -> FragmentOptions {
        FragmentOptions::default()
    }

    #[test]
    fn exact_form_and_encoding() {
        let u = text_fragment_url(
            "https://ex.com/a?x=1",
            "Rock-n-roll, R&D #1 \u{e9}",
            None,
            None,
            &o(),
        )
        .unwrap();
        assert_eq!(
            u,
            "https://ex.com/a?x=1#:~:text=Rock%2Dn%2Droll%2C%20R%26D%20%231%20%C3%A9"
        );
    }

    #[test]
    fn strips_existing_fragment() {
        let u = text_fragment_url(
            "http://ex.com/p#sec:~:text=old",
            "hello there",
            None,
            None,
            &o(),
        )
        .unwrap();
        assert_eq!(u, "http://ex.com/p#:~:text=hello%20there");
    }

    #[test]
    fn range_form_for_long_quotes() {
        let q = "word ".repeat(60);
        let u = text_fragment_url("https://ex.com/", &q, None, None, &o()).unwrap();
        let frag = u.split("text=").nth(1).unwrap();
        assert_eq!(frag.matches(',').count(), 1, "{frag}");
        let (a, b) = frag.split_once(',').unwrap();
        assert!(a.len() < 80 && b.len() < 80);
    }

    #[test]
    fn prefix_suffix() {
        let u = text_fragment_url(
            "https://ex.com/",
            "target text",
            Some("before"),
            Some("after, x"),
            &o(),
        )
        .unwrap();
        assert_eq!(
            u,
            "https://ex.com/#:~:text=before-,target%20text,-after%2C%20x"
        );
    }

    #[test]
    fn refuses_bad_urls_and_empty() {
        for bad in [
            "javascript:alert(1)",
            "ftp://x/y",
            "file:///etc/passwd",
            "data:text/html,hi",
            "",
            "https://",
        ] {
            assert_eq!(
                text_fragment_url(bad, "some quote", None, None, &o()),
                Err(FragmentError::NotHttp),
                "{bad}"
            );
        }
        assert_eq!(
            text_fragment_url("https://a.b/", "  \n ", None, None, &o()),
            Err(FragmentError::EmptyQuote)
        );
        assert!(text_fragment_url("HTTPS://A.B/", "quote text", None, None, &o()).is_ok());
    }

    #[test]
    fn from_verified_with_context() {
        let page =
            "Intro words here. The needle phrase sits right here, then more trailing words follow.";
        let s = page.find("needle").unwrap();
        let e = s + "needle phrase".len();
        let opts = FragmentOptions {
            context_chars: 12,
            ..o()
        };
        let u = fragment_from_verified("https://ex.com/", page, s, e, &opts).unwrap();
        assert!(u.contains("text=") && u.contains("needle%20phrase"), "{u}");
        assert!(u.contains("-,") && u.contains(",-"), "{u}");
        let plain = fragment_from_verified("https://ex.com/", page, s, e, &o()).unwrap();
        assert_eq!(plain, "https://ex.com/#:~:text=needle%20phrase");
    }
}
