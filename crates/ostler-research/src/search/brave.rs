//! Brave Search API provider.
//!
//! Verification status (honest): the endpoint `GET https://api.search.brave.com/res/v1/web/search`,
//! the `X-Subscription-Token` auth header, the parameters `q`, `count`, `country`, `search_lang`,
//! `freshness` (`pd`, `pw`, `pm`, `py`) and `offset`, and the response shape `web.results[]` with
//! `title`, `url`, `description` were confirmed from Brave's documentation excerpts and several
//! third-party examples seen through a search tool. The documentation site itself could not be
//! fetched in the build environment, so the `age` / `page_age` fields, the `count` maximum of 20
//! and the 401/403/422/429 meanings are **unverified**. Never run against the real service in
//! tests: all tests use a local mock server.

use async_trait::async_trait;
use serde::Deserialize;
use url::Url;

use super::http::{client, is_cleartext_remote, map_err, read_capped};
use super::{
    finish, ApiKey, HttpOptions, Recency, SearchError, SearchProvider, SearchQuery, SearchResult,
};

const DEFAULT_BASE: &str = "https://api.search.brave.com";
/// Documented per-request maximum for `count` (unverified, see module docs).
const MAX_COUNT: usize = 20;

/// Brave Search API provider. `Debug` redacts the key.
#[derive(Debug, Clone)]
pub struct BraveProvider {
    base: Url,
    key: ApiKey,
    client: reqwest::Client,
    opts: HttpOptions,
}

#[derive(Deserialize)]
struct Resp {
    #[serde(default)]
    web: Option<Web>,
}

#[derive(Deserialize)]
struct Web {
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
    description: Option<String>,
    #[serde(default)]
    page_age: Option<String>,
    #[serde(default)]
    age: Option<String>,
}

impl BraveProvider {
    /// Provider for the real Brave endpoint.
    pub fn new(key: ApiKey, opts: HttpOptions) -> Result<Self, SearchError> {
        Self::with_base_url(DEFAULT_BASE, key, opts)
    }

    /// Provider for another base URL (tests, proxies). The key is only sent over https or to loopback.
    pub fn with_base_url(base: &str, key: ApiKey, opts: HttpOptions) -> Result<Self, SearchError> {
        let base = Url::parse(base.trim())
            .map_err(|e| SearchError::Config(format!("Brave base URL is not valid: {e}")))?;
        if !matches!(base.scheme(), "http" | "https") || base.host_str().is_none() {
            return Err(SearchError::Config(
                "Brave base URL must be http or https".into(),
            ));
        }
        if is_cleartext_remote(&base) {
            return Err(SearchError::Config(
                "refusing to send the Brave API key over plain http to a non-loopback host".into(),
            ));
        }
        let client = client(&base, &opts)?;
        Ok(Self {
            base,
            key,
            client,
            opts,
        })
    }

    fn endpoint(&self, q: &SearchQuery) -> Result<Url, SearchError> {
        let mut u = self.base.clone();
        u.set_path("/res/v1/web/search");
        u.set_query(None);
        {
            let mut p = u.query_pairs_mut();
            p.append_pair("q", &q.effective_text()?);
            p.append_pair("count", &q.clamped_limit(MAX_COUNT).to_string());
            if let Some(l) = q.lang() {
                p.append_pair("search_lang", &l);
            }
            if let Some(r) = q.recency {
                p.append_pair(
                    "freshness",
                    match r {
                        Recency::Day => "pd",
                        Recency::Week => "pw",
                        Recency::Month => "pm",
                        Recency::Year => "py",
                    },
                );
            }
        }
        Ok(u)
    }
}

/// Brave wraps matched terms in `<strong>` and escapes entities; reduce to plain text.
fn strip_markup(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

#[async_trait]
impl SearchProvider for BraveProvider {
    async fn search(&self, q: &SearchQuery) -> Result<Vec<SearchResult>, SearchError> {
        let url = self.endpoint(q)?;
        let resp = self
            .client
            .get(url)
            .header("Accept", "application/json")
            .header("X-Subscription-Token", self.key.expose())
            .send()
            .await
            .map_err(|e| map_err(e, self.opts.timeout))?;
        match resp.status().as_u16() {
            200..=299 => {}
            300..=399 => return Err(SearchError::OffOriginRedirect),
            s @ (401 | 403) => return Err(SearchError::Unauthorized(s)),
            429 => return Err(SearchError::RateLimited),
            s => {
                return Err(SearchError::Status {
                    status: s,
                    hint: "unexpected response from Brave Search API".into(),
                })
            }
        }
        let body = read_capped(resp, self.opts.max_body_bytes, self.opts.timeout).await?;
        let parsed: Resp = serde_json::from_slice(&body)
            .map_err(|e| SearchError::Malformed(format!("Brave JSON: {e}")))?;
        let raw = parsed
            .web
            .map(|w| w.results)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|i| {
                Some(SearchResult {
                    title: strip_markup(&i.title.unwrap_or_default()),
                    url: i.url?,
                    snippet: strip_markup(&i.description.unwrap_or_default()),
                    engine: "brave".into(),
                    published: i.page_age.or(i.age),
                })
            })
            .collect();
        Ok(finish(raw, q.clamped_limit(MAX_COUNT)))
    }

    fn name(&self) -> &str {
        "brave"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{serve, Resp as R};

    const KEY: &str = "BSA-SECRET-KEY-42";

    fn provider(base: &str) -> BraveProvider {
        BraveProvider::with_base_url(base, ApiKey::new(KEY).unwrap(), HttpOptions::default())
            .unwrap()
    }

    #[tokio::test]
    async fn sends_token_and_parses() {
        let m = serve(|_| {
            R::json(
                r#"{"web":{"results":[
                {"title":"A <strong>rust</strong>","url":"https://a.com/x#f","description":"fast &amp; safe","page_age":"2026-02-01T00:00:00","age":"1 week ago"},
                {"title":"B","url":"https://b.com","description":"d","age":"2 days ago"},
                {"title":"bad","url":"data:text/html,x"}]}}"#,
            )
        });
        let mut q = SearchQuery::new("rust");
        q.recency = Some(Recency::Week);
        q.language = Some("en".into());
        q.limit = 100;
        let r = provider(&m.base).search(&q).await.unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].title, "A rust");
        assert_eq!(r[0].snippet, "fast & safe");
        assert_eq!(r[0].engine, "brave");
        assert_eq!(r[0].published.as_deref(), Some("2026-02-01T00:00:00"));
        assert_eq!(r[1].published.as_deref(), Some("2 days ago"));
        let seen = &m.requests()[0];
        assert_eq!(seen.header("x-subscription-token"), Some(KEY));
        assert!(seen.url.starts_with("/res/v1/web/search?"));
        assert!(
            seen.url.contains("count=20")
                && seen.url.contains("freshness=pw")
                && seen.url.contains("search_lang=en")
        );
        assert!(!seen.url.contains(KEY));
    }

    #[tokio::test]
    async fn errors_never_contain_key() {
        for (status, want) in [
            (401u16, "API key"),
            (403, "API key"),
            (429, "rate limit"),
            (500, "HTTP 500"),
        ] {
            let m = serve(move |_| R::new(status, KEY));
            let e = provider(&m.base)
                .search(&SearchQuery::new("x"))
                .await
                .unwrap_err();
            let text = format!("{e} {e:?}");
            assert!(text.contains(want), "{text}");
            assert!(!text.contains(KEY), "{text}");
        }
    }

    #[tokio::test]
    async fn malformed_and_empty() {
        let m = serve(|_| R::json("[1,2"));
        assert!(matches!(
            provider(&m.base).search(&SearchQuery::new("x")).await,
            Err(SearchError::Malformed(_))
        ));
        let m = serve(|_| R::json("{}"));
        assert!(provider(&m.base)
            .search(&SearchQuery::new("x"))
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn redirect_is_not_followed_and_key_not_leaked() {
        let target = serve(|_| R::json("{}"));
        let t = target.base.clone();
        let m = serve(move |_| R::new(307, "").header("Location", &format!("{t}/steal")));
        let e = provider(&m.base)
            .search(&SearchQuery::new("x"))
            .await
            .unwrap_err();
        assert!(matches!(e, SearchError::OffOriginRedirect));
        assert!(target.requests().is_empty());
    }

    #[test]
    fn debug_redacts_and_cleartext_refused() {
        let p = provider("http://127.0.0.1:1");
        assert!(!format!("{p:?}").contains(KEY));
        let e = BraveProvider::with_base_url(
            "http://example.com",
            ApiKey::new(KEY).unwrap(),
            HttpOptions::default(),
        );
        assert!(e.is_err());
        assert!(!format!("{e:?}").contains(KEY));
    }
}
