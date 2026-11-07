//! Server-side result filtering (`where`, `select`).
//!
//! Large tool results should not have to pass through the model just to be thrown away. The
//! list-shaped tools (`extract`, `links`, `search`) accept `where` (keep rows containing any of
//! `a|b|c`, case-insensitive) and `select` (table columns by header name or 0-based index), and the
//! filtering happens here, before the text is returned. Only the rows inside the untrusted fence are
//! touched; headers, notices and the fence itself are never altered.

use serde_json::Value;

const OPEN: &str = "<page_content untrusted=\"true\">\n";
const CLOSE: &str = "\n</page_content>";

#[derive(Debug, Default, PartialEq)]
pub struct Filter {
    terms: Vec<String>,
    select: Vec<String>,
}

impl Filter {
    /// `None` when the call asked for no filtering.
    pub fn from_args(a: &Value) -> Option<Self> {
        let terms: Vec<String> = a
            .get("where")
            .and_then(Value::as_str)
            .map(|s| {
                s.split('|')
                    .map(|t| t.trim().to_lowercase())
                    .filter(|t| !t.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let select: Vec<String> = match a.get("select") {
            Some(Value::Array(v)) => v
                .iter()
                .filter_map(|x| match x {
                    Value::String(s) => Some(s.trim().to_lowercase()),
                    Value::Number(n) => Some(n.to_string()),
                    _ => None,
                })
                .filter(|s| !s.is_empty())
                .collect(),
            _ => Vec::new(),
        };
        if terms.is_empty() && select.is_empty() {
            None
        } else {
            Some(Self { terms, select })
        }
    }
}

fn cells(line: &str) -> Vec<&str> {
    line.trim()
        .trim_start_matches('|')
        .trim_end_matches('|')
        .split(" | ")
        .map(str::trim)
        .collect()
}

/// Indexes of the selected columns, in the order asked for, and the names that matched nothing.
fn pick(header: &[&str], select: &[String]) -> (Vec<usize>, Vec<String>) {
    let mut idx = Vec::new();
    let mut missing = Vec::new();
    for s in select {
        let by_index = s.parse::<usize>().ok().filter(|i| *i < header.len());
        let by_name = header
            .iter()
            .position(|h| h.to_lowercase() == *s)
            .or_else(|| {
                header
                    .iter()
                    .position(|h| h.to_lowercase().contains(s.as_str()))
            });
        match by_index.or(by_name) {
            Some(i) if !idx.contains(&i) => idx.push(i),
            Some(_) => {}
            None => missing.push(s.clone()),
        }
    }
    (idx, missing)
}

/// Apply `f` to the rows of `text`. Text without a fence is returned unchanged.
pub fn apply(text: &str, f: &Filter) -> String {
    let Some(start) = text.find(OPEN) else {
        return text.to_string();
    };
    let body_at = start + OPEN.len();
    let Some(end_rel) = text[body_at..].rfind(CLOSE) else {
        return text.to_string();
    };
    let (head, body, tail) = (
        &text[..body_at],
        &text[body_at..body_at + end_rel],
        &text[body_at + end_rel..],
    );

    let (mut kept, mut total) = (0usize, 0usize);
    let mut missing: Vec<String> = Vec::new();
    let mut cols: Option<Vec<usize>> = None;
    let mut after_title = false;
    let mut out: Vec<String> = Vec::new();

    for line in body.lines() {
        if line.starts_with("Table ") {
            after_title = true;
            cols = None;
            out.push(line.to_string());
            continue;
        }
        let is_row = line.starts_with("| ");
        if is_row && after_title {
            // The first row of a table is its header (or, if the page gave none, its first row: it
            // is kept as the column guide either way).
            after_title = false;
            let h = cells(line);
            if f.select.is_empty() {
                out.push(line.to_string());
            } else {
                let (idx, miss) = pick(&h, &f.select);
                for m in miss {
                    if !missing.contains(&m) {
                        missing.push(m);
                    }
                }
                out.push(project(&h, &idx));
                cols = Some(idx);
            }
            continue;
        }
        if line.trim().is_empty() {
            out.push(line.to_string());
            continue;
        }
        total += 1;
        let hit = f.terms.is_empty() || {
            let l = line.to_lowercase();
            f.terms.iter().any(|t| l.contains(t.as_str()))
        };
        if !hit {
            continue;
        }
        kept += 1;
        match (&cols, is_row) {
            (Some(idx), true) => out.push(project(&cells(line), idx)),
            _ => out.push(line.to_string()),
        }
    }

    let mut note = format!("Filtered: {kept} of {total} row(s) kept");
    if !f.terms.is_empty() {
        note.push_str(&format!(" (where {})", f.terms.join(" | ")));
    }
    if !missing.is_empty() {
        note.push_str(&format!(". No such column: {}", missing.join(", ")));
    }
    note.push('.');
    format!("{head}{}{tail}\n{note}", out.join("\n").trim_end())
}

fn project(row: &[&str], idx: &[usize]) -> String {
    if idx.is_empty() {
        return "|  |".to_string();
    }
    let v: Vec<&str> = idx
        .iter()
        .map(|i| row.get(*i).copied().unwrap_or(""))
        .collect();
    format!("| {} |", v.join(" | "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const TABLE: &str = "1 table(s).\n<page_content untrusted=\"true\">\nTable 1 \"Prices\" (3 row(s))\n| name | price | stock |\n| apple | 1 | 10 |\n| pear | 2 | 0 |\n| plum | 3 | 7 |\n</page_content>";

    #[test]
    fn no_filter_arguments_means_no_filter() {
        assert!(Filter::from_args(&json!({"kind":"tables"})).is_none());
        assert!(Filter::from_args(&json!({"where":"  | "})).is_none());
    }

    #[test]
    fn where_keeps_matching_rows_and_the_header() {
        let f = Filter::from_args(&json!({"where":"apple|plum"})).unwrap();
        let t = apply(TABLE, &f);
        assert!(t.contains("| name | price | stock |"));
        assert!(t.contains("apple") && t.contains("plum") && !t.contains("pear"));
        assert!(
            t.contains("Filtered: 2 of 3 row(s) kept (where apple | plum)."),
            "{t}"
        );
        assert!(t.starts_with("1 table(s)."));
        assert!(t.contains("</page_content>"));
    }

    #[test]
    fn select_picks_columns_by_name_or_index_in_order() {
        let f = Filter::from_args(&json!({"select":["price", 0]})).unwrap();
        let t = apply(TABLE, &f);
        assert!(t.contains("| price | name |"), "{t}");
        assert!(t.contains("| 2 | pear |"));
        assert!(!t.contains("stock"));
    }

    #[test]
    fn an_unknown_column_is_reported_not_silently_dropped() {
        let f = Filter::from_args(&json!({"select":["colour"]})).unwrap();
        assert!(apply(TABLE, &f).contains("No such column: colour"));
    }

    #[test]
    fn link_lines_are_filtered_by_substring() {
        let t = "2 of 9 link(s):\n<page_content untrusted=\"true\">\nDocs → http://a/docs\nBlog → http://a/blog\n</page_content>";
        let f = Filter::from_args(&json!({"where":"/docs"})).unwrap();
        let o = apply(t, &f);
        assert!(o.contains("Docs →") && !o.contains("Blog"));
    }

    #[test]
    fn text_without_a_fence_is_untouched() {
        let f = Filter::from_args(&json!({"where":"x"})).unwrap();
        assert_eq!(
            apply("No tables on this page.", &f),
            "No tables on this page."
        );
    }
}
