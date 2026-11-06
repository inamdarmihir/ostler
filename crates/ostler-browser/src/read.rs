//! Reading and extraction: main content as markdown, tables, repeated structures, links, pagination hints
//! and a plain observation for `expect`. The work happens in `runtime.js`; this module owns the typed
//! results and the size limits.
//!
//! Every string in these types comes from the page and is untrusted: a caller that shows it to a model must
//! fence it like any other page text. Limits that are not hidden: closed shadow roots and cross-origin
//! iframes are invisible; "main content" is a heuristic and `kind` says when it did not find one; PDFs have
//! no text layer here (no PDF parser).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::page::{BrowserError, Page, Result};

/// Default `max_chars` for [`Page::read`].
pub const DEFAULT_READ_CHARS: usize = 20_000;

/// Options for [`Page::read`].
#[derive(Debug, Clone)]
pub struct ReadOptions {
    /// Cap on the markdown, in characters (default `DEFAULT_READ_CHARS`, 20 000).
    pub max_chars: Option<usize>,
    /// Render links as `[text](absolute-url)` and list them in `meta.links`; false gives plain text.
    pub include_links: bool,
    /// Read only this element (a ref from a snapshot). Boilerplate heuristics are off for an explicit choice.
    pub selector_ref: Option<String>,
}

impl Default for ReadOptions {
    fn default() -> Self {
        Self {
            max_chars: None,
            include_links: true,
            selector_ref: None,
        }
    }
}

/// What kind of page `read` found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadKind {
    /// A main-content block was found and extracted.
    Article,
    /// A short, simple page with no dominant block; the whole body minus boilerplate.
    Page,
    /// Interactive or list-like page with no article: `markdown` is only a heading outline. Use
    /// `snapshot`, `extract_lists` or `extract_tables`.
    App,
    /// A non-HTML text document (`text/plain` and similar), returned as is.
    Text,
    /// The caller chose an element with `selector_ref`.
    Selection,
    /// A document with no readable text (for example a PDF).
    Empty,
}

/// A link found in the extracted content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadLink {
    pub text: String,
    pub url: String,
}

/// Page metadata. All fields are page-supplied.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReadMeta {
    pub title: String,
    pub byline: String,
    /// As written by the page (usually ISO 8601); not validated.
    pub published: String,
    pub canonical_url: String,
    pub language: String,
    pub site_name: String,
    /// Parsed `application/ld+json` blocks, at most 10, each under 60 KB.
    pub json_ld: Vec<Value>,
    /// Blocks left out because they were too many, too large or not valid JSON.
    pub json_ld_skipped: u32,
    /// Links inside the extracted content (empty when `include_links` is false).
    pub links: Vec<ReadLink>,
}

/// Result of [`Page::read`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReadResult {
    pub kind: ReadKind,
    pub url: String,
    pub markdown: String,
    pub meta: ReadMeta,
    /// True when `markdown` was cut at `max_chars`.
    pub truncated: bool,
    /// Characters in the full markdown (a lower bound when the page was larger than the extraction cap).
    pub chars_total: usize,
    /// Plain observations (invisible text omitted, why the page is `app`, no PDF text layer).
    pub notes: Vec<String>,
}

/// A table with colspan and rowspan flattened (a spanned cell's text repeats in every slot it covers).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Table {
    /// Runtime ref of the table element (usable with `snapshot`).
    pub r#ref: String,
    pub caption: String,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// Rows in the table body before the row cap.
    pub row_count: usize,
    pub truncated: bool,
}

/// A link in a row or in [`Page::links`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowLink {
    pub text: String,
    pub url: String,
}

/// One item of a repeated structure (a result card, a list item).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListRow {
    pub r#ref: String,
    pub text: String,
    pub links: Vec<RowLink>,
    /// Heuristic named parts (title, price, date, or a class/itemprop name shared by most rows).
    pub fields: BTreeMap<String, String>,
}

/// A repeated sibling structure with at least three members.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListBlock {
    /// Ref of the parent element holding the items.
    pub r#ref: String,
    /// `TAG.class.names` shared by the items.
    pub signature: String,
    pub item_count: usize,
    pub rows: Vec<ListRow>,
    pub truncated: bool,
}

/// A "next page" control.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NextControl {
    /// `link`, `button` or `link-rel`.
    pub kind: String,
    pub text: String,
    pub url: Option<String>,
    pub r#ref: Option<String>,
    /// True when the control is disabled (often the last page).
    pub disabled: bool,
}

/// Pagination evidence, verbatim from the page. Nothing here says whether a result set is complete.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaginationHints {
    pub next: Vec<NextControl>,
    /// Text such as "Page 2 of 7".
    pub page_x_of_y: Vec<String>,
    /// Text such as "Showing 1-10 of 57".
    pub showing: Vec<String>,
    /// Text such as "About 1,234 results".
    pub totals: Vec<String>,
}

/// Result of [`Page::extract_lists`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExtractedLists {
    /// Best first (item count times typical text length).
    pub lists: Vec<ListBlock>,
    pub hints: PaginationHints,
}

/// Filter for [`Page::links`]; all conditions combine.
#[derive(Debug, Clone, Default)]
pub struct LinkFilter {
    /// Only links under this element ref.
    pub scope_ref: Option<String>,
    pub same_origin: Option<bool>,
    /// Case-insensitive substring of the anchor text.
    pub text_contains: Option<String>,
    /// Case-insensitive substring of the absolute URL.
    pub href_contains: Option<String>,
    /// Maximum links returned (default 200, at most 1000).
    pub limit: Option<usize>,
}

/// A link on the page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkInfo {
    pub text: String,
    pub url: String,
    pub rel: String,
    pub same_origin: bool,
    /// About 120 characters of text around the link.
    pub context: String,
    pub r#ref: String,
}

/// Result of [`Page::links`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinksResult {
    pub links: Vec<LinkInfo>,
    /// Links matching the filter before `limit`.
    pub total: usize,
}

/// An element in an [`ExpectObservation`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpectElement {
    pub role: String,
    pub name: String,
}

/// What an `expect` check can look at, as plain data.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpectObservation {
    pub url: String,
    pub title: String,
    /// Visible text (invisible text omitted), capped.
    pub text: String,
    pub text_truncated: bool,
    /// Interactive elements and headings with a name, capped.
    pub elements: Vec<ExpectElement>,
}

fn decode<T: serde::de::DeserializeOwned>(v: Value, what: &str) -> Result<T> {
    serde_json::from_value(v)
        .map_err(|e| BrowserError::Other(format!("could not read {what}: {e}")))
}

fn truncate_chars(s: &str, max: usize) -> (String, bool) {
    if s.chars().count() <= max {
        return (s.to_string(), false);
    }
    let cut: String = s.chars().take(max).collect();
    // Prefer a paragraph, then a line, then a word boundary in the last tenth.
    let floor = cut.len() - cut.len() / 10;
    let at = ["\n\n", "\n", " "]
        .iter()
        .filter_map(|sep| cut.rfind(sep))
        .find(|&i| i >= floor);
    let cut = match at {
        Some(i) => cut[..i].to_string(),
        None => cut,
    };
    (cut, true)
}

impl Page {
    /// Main content of the page as markdown (headings, lists, tables, code blocks, links as
    /// `[text](url)`, images as alt text) with metadata. Passes through open shadow roots and same-origin
    /// iframes; boilerplate and invisible text are left out.
    pub async fn read(&self, opts: ReadOptions) -> Result<ReadResult> {
        self.guard().await?;
        let max = opts.max_chars.unwrap_or(DEFAULT_READ_CHARS).max(1);
        let v = self
            .rt(
                "read",
                &[json!({
                    "maxChars": max,
                    "includeLinks": opts.include_links,
                    "ref": opts.selector_ref,
                })],
            )
            .await?;
        #[derive(Deserialize)]
        struct Raw {
            kind: ReadKind,
            url: String,
            markdown: String,
            #[serde(default)]
            meta: ReadMeta,
            #[serde(default)]
            links: Vec<ReadLink>,
            #[serde(default)]
            notes: Vec<String>,
            #[serde(default)]
            capped: bool,
        }
        let mut raw: Raw = decode(v, "page content")?;
        let chars_total = raw.markdown.chars().count();
        let (markdown, mut truncated) = truncate_chars(&raw.markdown, max);
        if truncated {
            raw.notes.push(format!(
                "cut at {max} of {} characters; raise max_chars or read a section by ref",
                if raw.capped {
                    format!("at least {chars_total}")
                } else {
                    chars_total.to_string()
                }
            ));
        }
        truncated |= raw.capped;
        raw.meta.links = if opts.include_links {
            raw.links
        } else {
            vec![]
        };
        Ok(ReadResult {
            kind: raw.kind,
            url: raw.url,
            markdown,
            meta: raw.meta,
            truncated,
            chars_total,
            notes: raw.notes,
        })
    }

    /// Every HTML table and ARIA `table`/`grid` on the page, headers separated, spans flattened, cell text
    /// trimmed. At most 50 tables, 500 rows each, 300 characters per cell.
    pub async fn extract_tables(&self) -> Result<Vec<Table>> {
        self.guard().await?;
        decode(self.rt("tables", &[json!({})]).await?, "tables")
    }

    /// Repeated sibling structures (search-result cards, product tiles, list items; three or more alike)
    /// as rows of text, links and heuristic fields, plus pagination hints. At most 8 lists of 100 rows.
    pub async fn extract_lists(&self) -> Result<ExtractedLists> {
        self.guard().await?;
        decode(self.rt("lists", &[json!({})]).await?, "lists")
    }

    /// "Next page" controls and "Page X of Y" / "Showing a-b of N" text, verbatim.
    pub async fn pagination_hints(&self) -> Result<PaginationHints> {
        self.guard().await?;
        decode(self.rt("hints", &[]).await?, "pagination hints")
    }

    /// Links with anchor text, absolute URL, `rel`, same-origin flag and about 120 characters of context.
    pub async fn links(&self, filter: LinkFilter) -> Result<LinksResult> {
        self.guard().await?;
        let v = self
            .rt(
                "links",
                &[json!({
                    "ref": filter.scope_ref,
                    "sameOrigin": filter.same_origin,
                    "textContains": filter.text_contains,
                    "hrefContains": filter.href_contains,
                    "limit": filter.limit,
                })],
            )
            .await?;
        decode(v, "links")
    }

    /// Plain snapshot of what an `expect` condition can match: URL, title, visible text (capped at 6000
    /// characters) and up to 150 named interactive elements and headings.
    pub async fn expect_observation(&self) -> Result<ExpectObservation> {
        let v = self.rt("observe", &[json!({})]).await?;
        decode(v, "page observation")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_prefers_paragraph_boundaries() {
        let text = format!("{}\n\n{}", "a".repeat(95), "b".repeat(50));
        let (t, cut) = truncate_chars(&text, 100);
        assert!(cut);
        assert_eq!(t, "a".repeat(95));
        let (t, cut) = truncate_chars("short", 100);
        assert!(!cut);
        assert_eq!(t, "short");
    }
}
