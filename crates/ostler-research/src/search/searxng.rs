//! SearXNG provider.
//!
//! Request: `GET {base}/search?q=...&format=json`. Many instances ship with JSON output disabled
//! (they answer 403, or return an HTML page); that is reported as [`SearchError::JsonDisabled`]
//! with the fix. Response fields used: `results[].{title,url,content,engine,publishedDate}`, as
//! documented for SearXNG's search API. Tested against a local mock only, not a live instance.

use async_trait::async_trait;
use serde::Deserialize;
use url::Url;

use super::http::{client, map_err, read_capped};
use super::{finish, HttpOptions, Recency, SearchError, SearchProvider, SearchQuery, SearchResult};

/// SearXNG search provider.
#[derive(Debug, Clone)]
pub struct SearxngProvider {
    base: Url,
    client: reqwest::Client,
    opts: HttpOptions,
}

#[derive(Deserialize)]
struct Resp {
    #[serde(default)]
    results: Vec<Item>,
}

#[derive(Deserialize)]
struct Item {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    engine: Option<String>,
    #[serde(default, rename = "publishedDate")]
    published_date: Option<String>,
}

const JSON_HINT: &str = "SearXNG refused JSON output. Add `json` to `search.formats` in the instance's settings.yml (formats: [html, json]) and restart it, or point OSTLER_SEARXNG_URL at an instance that allows it";

impl SearxngProvider {
    /// Create a provider for the instance at `base_url` (http or https).
    pub fn new(base_url: &str, opts: HttpOptions) -> Result<Self, SearchError> {
        let base = Url::parse(base_url.trim())
            .map_err(|e| SearchError::Config(format!("SearXNG URL is not valid: {e}")))?;
        if !matches!(base.scheme(), "http" | "https") || base.host_str().is_none() {
            return Err(SearchError::Config(
                "SearXNG URL must be http or https".into(),
            ));
        }
        let client = client(&base, &opts)?;
        Ok(Self { base, client, opts })
    }

    fn endpoint(&self, q: &SearchQuery) -> Result<Url, SearchError> {
        let mut u = self.base.clone();
        let path = format!("{}/search", u.path().trim_end_matches('/'));
        u.set_path(&path);
        u.set_query(None);
        {
            let mut p = u.query_pairs_mut();
            p.append_pair("q", &q.effective_text()?);
            p.append_pair("format", "json");
            if let Some(l) = q.lang() {
                p.append_pair("language", &l);
            }
            if let Some(r) = q.recency {
                // SearXNG documents day, month and year. Week is widened to month (unverified
                // whether newer versions accept `week`), and results are not re-filtered.
                let v = match r {
                    Recency::Day => "day",
                    Recency::Week | Recency::Month => "month",
                    Recency::Year => "year",
                };
                p.append_pair("time_range", v);
            }
        }
        Ok(u)
    }
}

#[async_trait]
impl SearchProvider for SearxngProvider {
    async fn search(&self, q: &SearchQuery) -> Result<Vec<SearchResult>, SearchError> {
        let url = self.endpoint(q)?;
        let resp = self
            .client
            .get(url)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| map_err(e, self.opts.timeout))?;
        let status = resp.status().as_u16();
        match status {
            200..=299 => {}
            300..=399 => return Err(SearchError::OffOriginRedirect),
            403 | 406 => return Err(SearchError::JsonDisabled(JSON_HINT.into())),
            429 => return Err(SearchError::RateLimited),
            s => {
                return Err(SearchError::Status {
                    status: s,
                    hint: "unexpected response from SearXNG".into(),
                })
            }
        }
        let body = read_capped(resp, self.opts.max_body_bytes, self.opts.timeout).await?;
        let first = body.iter().copied().find(|b| !b.is_ascii_whitespace());
        if first == Some(b'<') {
            return Err(SearchError::JsonDisabled(JSON_HINT.into()));
        }
        let parsed: Resp = serde_json::from_slice(&body)
            .map_err(|e| SearchError::Malformed(format!("SearXNG JSON: {e}")))?;
        let raw = parsed
            .results
            .into_iter()
            .filter_map(|i| {
                Some(SearchResult {
                    title: i.title.unwrap_or_default(),
                    url: i.url?,
                    snippet: i.content.unwrap_or_default(),
                    engine: i.engine.unwrap_or_else(|| "searxng".into()),
                    published: i.published_date,
                })
            })
            .collect();
        Ok(finish(raw, q.clamped_limit(50)))
    }

    fn name(&self) -> &str {
        "searxng"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{serve, Resp as R};
    use std::time::Duration;

    fn provider(base: &str, opts: HttpOptions) -> SearxngProvider {
        SearxngProvider::new(base, opts).unwrap()
    }

    #[tokio::test]
    async fn parses_results_and_builds_query() {
        let m = serve(|_| {
            R::json(
                r#"{"results":[
                {"title":"A","url":"https://a.com/?utm_x=1","content":"snip","engine":"duckduckgo","publishedDate":"2026-01-02T00:00:00"},
                {"title":"dup","url":"https://a.com/"},
                {"title":"bad","url":"javascript:alert(1)"},
                {"title":"nourl"},
                {"title":"B","url":"https://b.com/p","content":"x"}]}"#,
            )
        });
        let p = provider(&m.base, HttpOptions::default());
        let mut q = SearchQuery::new("rust async");
        q.language = Some("en".into());
        q.recency = Some(Recency::Day);
        q.site = Some("a.com".into());
        let r = p.search(&q).await.unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].url, "https://a.com/");
        assert_eq!(r[0].engine, "duckduckgo");
        assert_eq!(r[0].published.as_deref(), Some("2026-01-02T00:00:00"));
        assert_eq!(r[1].engine, "searxng");
        let seen = m.requests();
        let u = &seen[0].url;
        assert!(u.starts_with("/search?"), "{u}");
        assert!(
            u.contains("format=json") && u.contains("time_range=day") && u.contains("language=en")
        );
        assert!(u.contains("site%3Aa.com"), "{u}");
    }

    #[tokio::test]
    async fn json_disabled_is_actionable() {
        let m = serve(|_| R::new(403, "Forbidden"));
        let e = provider(&m.base, HttpOptions::default())
            .search(&SearchQuery::new("x"))
            .await
            .unwrap_err();
        assert!(matches!(e, SearchError::JsonDisabled(_)));
        assert!(e.to_string().contains("settings.yml"));
        let m = serve(|_| R::new(200, "<html>results</html>").header("Content-Type", "text/html"));
        let e = provider(&m.base, HttpOptions::default())
            .search(&SearchQuery::new("x"))
            .await
            .unwrap_err();
        assert!(matches!(e, SearchError::JsonDisabled(_)));
    }

    #[tokio::test]
    async fn malformed_json_and_status_errors() {
        let m = serve(|_| R::json("{not json"));
        let e = provider(&m.base, HttpOptions::default())
            .search(&SearchQuery::new("x"))
            .await
            .unwrap_err();
        assert!(matches!(e, SearchError::Malformed(_)));
        let m = serve(|_| R::new(500, "boom"));
        let e = provider(&m.base, HttpOptions::default())
            .search(&SearchQuery::new("x"))
            .await
            .unwrap_err();
        assert!(matches!(e, SearchError::Status { status: 500, .. }));
        let m = serve(|_| R::new(429, ""));
        let e = provider(&m.base, HttpOptions::default())
            .search(&SearchQuery::new("x"))
            .await
            .unwrap_err();
        assert!(matches!(e, SearchError::RateLimited));
    }

    #[tokio::test]
    async fn timeout_and_oversize() {
        let m = serve(|_| R::json(r#"{"results":[]}"#).delay(600));
        let opts = HttpOptions {
            timeout: Duration::from_millis(150),
            ..HttpOptions::default()
        };
        let e = provider(&m.base, opts)
            .search(&SearchQuery::new("x"))
            .await
            .unwrap_err();
        assert!(matches!(e, SearchError::Timeout(_)), "{e:?}");

        let big = format!(
            r#"{{"results":[{{"title":"{}","url":"https://a.com"}}]}}"#,
            "x".repeat(5000)
        );
        let m = serve(move |_| R::json(&big));
        let opts = HttpOptions {
            max_body_bytes: 1000,
            ..HttpOptions::default()
        };
        let e = provider(&m.base, opts)
            .search(&SearchQuery::new("x"))
            .await
            .unwrap_err();
        assert!(matches!(e, SearchError::TooLarge(1000)), "{e:?}");
    }

    #[tokio::test]
    async fn off_origin_redirect_is_refused_same_origin_followed() {
        let target = serve(|_| R::json(r#"{"results":[{"title":"leak","url":"https://x.com"}]}"#));
        let t = target.base.clone();
        let m = serve(move |_| {
            R::new(302, "").header("Location", &format!("{t}/search?q=x&format=json"))
        });
        let e = provider(&m.base, HttpOptions::default())
            .search(&SearchQuery::new("x"))
            .await
            .unwrap_err();
        assert!(matches!(e, SearchError::OffOriginRedirect), "{e:?}");
        assert!(target.requests().is_empty());

        let m = serve(|u| {
            if u.starts_with("/search") {
                R::new(302, "").header("Location", "/real?format=json")
            } else {
                R::json(r#"{"results":[{"title":"ok","url":"https://x.com"}]}"#)
            }
        });
        let r = provider(&m.base, HttpOptions::default())
            .search(&SearchQuery::new("x"))
            .await
            .unwrap();
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn rejects_bad_base_url() {
        assert!(SearxngProvider::new("ftp://x", HttpOptions::default()).is_err());
        assert!(SearxngProvider::new("nonsense", HttpOptions::default()).is_err());
    }
}
