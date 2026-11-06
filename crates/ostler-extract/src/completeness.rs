//! Compare an extraction's row count with totals the page states about itself.
//!
//! Only English phrasing is recognised ("Showing 1-20 of 1,134 results",
//! "Page 2 of 7", "About 3,400 results", "20 per page", "No results").
//! Thousands separators `,`, `'`, U+2019, NBSP and narrow NBSP are accepted;
//! `.` is not treated as a thousands separator (it is ambiguous). Everything
//! here is heuristic: a page can state a total that differs from what is
//! actually reachable, and absence of a hint proves nothing.

use serde::{Deserialize, Serialize};

/// What a page says about its own result set.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Hints {
    /// "Showing 21-40 of 134": the displayed range, 1-based inclusive.
    pub range: Option<(u64, u64)>,
    /// Total number of results/items the page states.
    pub total: Option<u64>,
    /// "Page 2 of 7": `(current, last)`.
    pub page: Option<(u64, u64)>,
    /// "20 per page".
    pub per_page: Option<u64>,
    /// Two different totals were found; the first is kept in `total`.
    pub conflicting_totals: bool,
}

/// Completeness verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Completeness {
    /// The row count reaches the page-stated total.
    Complete,
    /// Fewer rows than the page-stated total.
    Partial {
        /// Rows extracted.
        have: u64,
        /// Rows the page says exist.
        expected: u64,
    },
    /// Not enough information.
    Unknown,
}

/// Verdict plus the evidence behind it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assessment {
    /// The verdict.
    pub completeness: Completeness,
    /// Human-readable reasons.
    pub reasons: Vec<String>,
    /// Parsed hints.
    pub hints: Hints,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(u64),
    Word(String),
    Dash,
    Break,
}

fn tokenize(text: &str) -> Vec<Tok> {
    let c: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let is_sep = |x: char| matches!(x, ',' | '\'' | '\u{2019}' | '\u{00A0}' | '\u{202F}');
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch.is_ascii_digit() {
            let mut s = String::new();
            while i < c.len() {
                if c[i].is_ascii_digit() {
                    s.push(c[i]);
                    i += 1;
                } else if is_sep(c[i])
                    && c.get(i + 1..i + 4)
                        .is_some_and(|g| g.iter().all(char::is_ascii_digit))
                    && !c.get(i + 4).is_some_and(char::is_ascii_digit)
                {
                    i += 1;
                } else {
                    break;
                }
            }
            // A decimal (3.5) is not a count.
            if c.get(i) == Some(&'.') && c.get(i + 1).is_some_and(char::is_ascii_digit) {
                i += 1;
                while c.get(i).is_some_and(char::is_ascii_digit) {
                    i += 1;
                }
                out.push(Tok::Break);
            } else {
                match s.parse::<u64>() {
                    Ok(n) => out.push(Tok::Num(n)),
                    Err(_) => out.push(Tok::Break),
                }
            }
        } else if ch.is_alphabetic() {
            let mut s = String::new();
            while i < c.len() && c[i].is_alphabetic() {
                s.push(c[i]);
                i += 1;
            }
            out.push(Tok::Word(s));
        } else if matches!(ch, '-' | '\u{2010}'..='\u{2015}' | '\u{2212}') {
            out.push(Tok::Dash);
            i += 1;
        } else if ch == '/' {
            out.push(Tok::Word("of".into()));
            i += 1;
        } else if ch == '\n' {
            out.push(Tok::Break);
            i += 1;
        } else {
            i += 1;
        }
    }
    out
}

const NOUNS: &[&str] = &[
    "results",
    "result",
    "items",
    "item",
    "products",
    "product",
    "entries",
    "entry",
    "records",
    "record",
    "rows",
    "row",
    "matches",
    "match",
    "hits",
    "listings",
    "listing",
    "jobs",
    "posts",
    "articles",
    "reviews",
    "comments",
    "users",
    "properties",
    "homes",
    "cars",
    "photos",
    "images",
];

fn w(t: &Tok) -> Option<&str> {
    match t {
        Tok::Word(s) => Some(s),
        _ => None,
    }
}

/// Parse page-stated range/total/page hints from `text`.
pub fn parse_hints(text: &str) -> Hints {
    let t = tokenize(text);
    let mut h = Hints::default();
    let set_total = |h: &mut Hints, n: u64| match h.total {
        None => h.total = Some(n),
        Some(old) if old != n => h.conflicting_totals = true,
        _ => {}
    };
    let word_at = |i: usize| t.get(i).and_then(w);
    for i in 0..t.len() {
        let prev = i.checked_sub(1).and_then(|p| t.get(p)).and_then(w);
        match &t[i] {
            Tok::Num(a) => {
                // "1-20 of 134", "1 to 20 of 134"
                if let (Some(Tok::Num(b)), true, Some(Tok::Num(tot))) = (
                    t.get(i + 2),
                    matches!(t.get(i + 1), Some(Tok::Dash))
                        || word_at(i + 1) == Some("to")
                        || word_at(i + 1) == Some("through"),
                    t.get(i + 4),
                ) {
                    if word_at(i + 3) == Some("of") && a <= b {
                        h.range.get_or_insert((*a, *b));
                        set_total(&mut h, *tot);
                        continue;
                    }
                }
                // "page 2 of 7"
                if matches!(prev, Some("page" | "pages")) && word_at(i + 1) == Some("of") {
                    if let Some(Tok::Num(m)) = t.get(i + 2) {
                        h.page.get_or_insert((*a, *m));
                        continue;
                    }
                }
                // "20 of 134 results" / "showing 20 of 134"
                if word_at(i + 1) == Some("of") {
                    if let Some(Tok::Num(tot)) = t.get(i + 2) {
                        let ctx = matches!(
                            prev,
                            Some(
                                "showing"
                                    | "viewing"
                                    | "displaying"
                                    | "results"
                                    | "items"
                                    | "entries"
                                    | "rows"
                                    | "records"
                            )
                        ) || word_at(i + 3).is_some_and(|x| NOUNS.contains(&x));
                        if ctx && a <= tot {
                            set_total(&mut h, *tot);
                        }
                        continue;
                    }
                }
                // "20 per page", "20 results per page", "show 20 per page"
                if word_at(i + 1) == Some("per") && matches!(word_at(i + 2), Some("page" | "load"))
                {
                    h.per_page.get_or_insert(*a);
                    continue;
                }
                if word_at(i + 1).is_some_and(|x| NOUNS.contains(&x)) {
                    if word_at(i + 2) == Some("per") && word_at(i + 3) == Some("page") {
                        h.per_page.get_or_insert(*a);
                        continue;
                    }
                    let before = matches!(
                        prev,
                        Some(
                            "about"
                                | "approximately"
                                | "approx"
                                | "around"
                                | "over"
                                | "nearly"
                                | "of"
                                | "total"
                                | "found"
                                | "all"
                                | "roughly"
                                | "than"
                        )
                    );
                    let after = matches!(
                        word_at(i + 2),
                        Some(
                            "found"
                                | "total"
                                | "available"
                                | "matching"
                                | "matches"
                                | "match"
                                | "overall"
                        )
                    ) || (word_at(i + 2) == Some("in")
                        && word_at(i + 3) == Some("total"));
                    if before || after {
                        set_total(&mut h, *a);
                    }
                }
            }
            Tok::Word(x) => match x.as_str() {
                "total" => {
                    let mut k = i + 1;
                    while matches!(t.get(k), Some(Tok::Word(s)) if s == "of" || s == "count" || NOUNS.contains(&s.as_str()))
                    {
                        k += 1;
                    }
                    if let Some(Tok::Num(n)) = t.get(k) {
                        set_total(&mut h, *n);
                    }
                }
                "no" => {
                    if word_at(i + 1).is_some_and(|n| NOUNS.contains(&n)) {
                        set_total(&mut h, 0);
                    }
                }
                "per" if word_at(i + 1) == Some("page") => {
                    if let Some(Tok::Num(n)) = t.get(i + 2) {
                        h.per_page.get_or_insert(*n);
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    h
}

/// Assess `rows` extracted rows (cumulative across pages, if the caller paged)
/// against hints parsed from `page_text`.
pub fn assess(rows: usize, page_text: Option<&str>) -> Assessment {
    let hints = page_text.map(parse_hints).unwrap_or_default();
    assess_hints(rows, hints)
}

/// Like [`assess`] with already-parsed hints.
pub fn assess_hints(rows: usize, hints: Hints) -> Assessment {
    let have = rows as u64;
    let mut reasons = Vec::new();
    let completeness = if let Some(total) = hints.total {
        if hints.conflicting_totals {
            reasons.push("page states more than one total; using the first".to_string());
        }
        if let Some((a, b)) = hints.range {
            let shown = b - a + 1;
            reasons.push(format!("page shows items {a}-{b} of {total}"));
            if shown != have && have < total {
                reasons.push(format!(
                    "extracted {have} rows but the page range covers {shown}"
                ));
            }
        }
        if let Some((p, m)) = hints.page {
            if p < m {
                reasons.push(format!("page {p} of {m}: more pages remain"));
            }
        }
        if have >= total {
            reasons.push(format!("{have} rows extracted, page states {total}"));
            if have > total {
                reasons.push(
                    "more rows than the page-stated total (duplicates or a wrong total?)"
                        .to_string(),
                );
            }
            Completeness::Complete
        } else {
            reasons.push(format!("{have} of {total} rows extracted"));
            Completeness::Partial {
                have,
                expected: total,
            }
        }
    } else {
        if let Some((p, m)) = hints.page {
            if p < m {
                reasons.push(format!("page {p} of {m}: more pages remain, total unknown"));
            } else {
                reasons.push(format!(
                    "page {p} of {m}, but no cumulative total is stated"
                ));
            }
        }
        if let Some(pp) = hints.per_page {
            if have >= pp && pp > 0 {
                reasons.push(format!(
                    "row count ({have}) reaches the page size ({pp}); may be truncated"
                ));
            }
        }
        if reasons.is_empty() {
            reasons.push("no page-stated total found".to_string());
        }
        Completeness::Unknown
    };
    Assessment {
        completeness,
        reasons,
        hints,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_of_total() {
        let h = parse_hints("Showing 1-20 of 134 results");
        assert_eq!(h.range, Some((1, 20)));
        assert_eq!(h.total, Some(134));
        let h = parse_hints("Results 21 \u{2013} 40 of 1,134");
        assert_eq!((h.range, h.total), (Some((21, 40)), Some(1134)));
        let h = parse_hints("Showing 1 to 10 of 2\u{00A0}500 entries");
        assert_eq!((h.range, h.total), (Some((1, 10)), Some(2500)));
    }

    #[test]
    fn page_of_pages() {
        let h = parse_hints("Page 2 of 7");
        assert_eq!(h.page, Some((2, 7)));
        assert_eq!(h.total, None);
        assert_eq!(parse_hints("page 3 / 9").page, Some((3, 9)));
    }

    #[test]
    fn totals_phrasing() {
        assert_eq!(
            parse_hints("About 3,400 results (0.4 seconds)").total,
            Some(3400)
        );
        assert_eq!(parse_hints("134 results found").total, Some(134));
        assert_eq!(parse_hints("Total: 87").total, Some(87));
        assert_eq!(parse_hints("Total results: 1,001").total, Some(1001));
        assert_eq!(parse_hints("Showing 20 of 134 products").total, Some(134));
        assert_eq!(parse_hints("No results found").total, Some(0));
        assert_eq!(parse_hints("12 items in total").total, Some(12));
    }

    #[test]
    fn per_page_is_not_total() {
        let h = parse_hints("Show 25 per page");
        assert_eq!((h.per_page, h.total), (Some(25), None));
        let h = parse_hints("20 results per page");
        assert_eq!((h.per_page, h.total), (Some(20), None));
        assert_eq!(parse_hints("Results per page: 50").per_page, Some(50));
    }

    #[test]
    fn ignores_decimals_and_bare_numbers() {
        let h = parse_hints("Rated 4.5 stars by 20 reviewers; price 1,299.99");
        assert_eq!(h, Hints::default());
    }

    #[test]
    fn conflicting_totals_flagged() {
        let h = parse_hints("Showing 1-10 of 50 results\nTotal: 60");
        assert_eq!(h.total, Some(50));
        assert!(h.conflicting_totals);
    }

    #[test]
    fn assess_partial_complete_unknown() {
        let a = assess(20, Some("Showing 1-20 of 134 results"));
        assert_eq!(
            a.completeness,
            Completeness::Partial {
                have: 20,
                expected: 134
            }
        );
        assert!(a.reasons.iter().any(|r| r.contains("1-20 of 134")));
        let a = assess(134, Some("Showing 1-20 of 134 results"));
        assert_eq!(a.completeness, Completeness::Complete);
        let a = assess(7, Some("Page 2 of 7"));
        assert_eq!(a.completeness, Completeness::Unknown);
        assert!(a.reasons[0].contains("more pages remain"));
        let a = assess(5, None);
        assert_eq!(a.completeness, Completeness::Unknown);
        assert!(a.reasons[0].contains("no page-stated total"));
        let a = assess(0, Some("No results found"));
        assert_eq!(a.completeness, Completeness::Complete);
    }

    #[test]
    fn page_size_truncation_reason() {
        let a = assess(20, Some("20 per page"));
        assert_eq!(a.completeness, Completeness::Unknown);
        assert!(a.reasons[0].contains("page size"));
    }

    #[test]
    fn mismatch_with_range_noted() {
        let a = assess(15, Some("Showing 1-20 of 134"));
        assert!(a.reasons.iter().any(|r| r.contains("page range covers 20")));
    }

    #[test]
    fn more_rows_than_total_noted() {
        let a = assess(12, Some("Total: 10"));
        assert_eq!(a.completeness, Completeness::Complete);
        assert!(a.reasons.iter().any(|r| r.contains("more rows")));
    }

    #[test]
    fn serde_shape() {
        let j = serde_json::to_value(Completeness::Partial {
            have: 1,
            expected: 2,
        })
        .unwrap();
        assert_eq!(j["status"], "partial");
    }
}
