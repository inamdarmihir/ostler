//! Web search behind one trait.
//!
//! Providers: [`SearxngProvider`] (self-hosted, no key) and [`BraveProvider`] (commercial API).
//! Shared hardening: request timeout, response size cap, same-origin-only redirects, only
//! http/https result URLs kept, results deduplicated by normalised URL and sanitised (no control
//! characters, length caps). API keys are never part of `Debug` output or error text.
//!
//! Environment configuration (see [`ProviderConfig::from_env`]):
//!
//! | variable | meaning |
//! |---|---|
//! | `OSTLER_SEARCH_PROVIDER` | `searxng`, `brave` or `none`. Unset means: SearXNG if its URL is set, else Brave if its key is set, else no provider |
//! | `OSTLER_SEARXNG_URL` | base URL of a SearXNG instance, e.g. `http://localhost:8080` |
//! | `OSTLER_BRAVE_API_KEY` | Brave Search API subscription token |

mod brave;
mod http;
mod searxng;

use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use url::Url;

pub use brave::BraveProvider;
pub use searxng::SearxngProvider;

use crate::sanitise_text;

/// Errors from a search provider. Messages never contain API keys.
#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    /// Bad configuration or query (empty text, bad URL, missing key).
    #[error("search configuration error: {0}")]
    Config(String),
    /// The request could not be completed (DNS, connect, TLS, reset).
    #[error("search request failed: {0}")]
    Network(String),
    /// The provider did not answer within the timeout.
    #[error("search request timed out after {0:?}")]
    Timeout(Duration),
    /// The provider answered with an unexpected HTTP status.
    #[error("search provider returned HTTP {status}: {hint}")]
    Status {
        /// HTTP status code.
        status: u16,
        /// Short human-readable explanation.
        hint: String,
    },
    /// SearXNG refused JSON output. Actionable: the operator must enable it.
    #[error("{0}")]
    JsonDisabled(String),
    /// The provider rejected our credentials.
    #[error("search provider rejected the API key (HTTP {0}); check OSTLER_BRAVE_API_KEY")]
    Unauthorized(u16),
    /// The provider rate-limited us (HTTP 429).
    #[error("search provider rate limit hit (HTTP 429); retry later")]
    RateLimited,
    /// The response body exceeded the size cap.
    #[error("search response larger than {0} bytes; refusing to read it")]
    TooLarge(usize),
    /// The response was not the expected JSON.
    #[error("search response was not valid: {0}")]
    Malformed(String),
    /// The provider redirected to a different origin; not followed.
    #[error("search provider redirected to another origin; refusing to follow")]
    OffOriginRedirect,
}

/// How recent results must be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recency {
    /// Past day.
    Day,
    /// Past week.
    Week,
    /// Past month.
    Month,
    /// Past year.
    Year,
}

/// A search request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    /// Query text (1 to 400 characters).
    pub text: String,
    /// Maximum results wanted (clamped to 1..=50 by providers).
    pub limit: usize,
    /// Language code such as `en` or `de`.
    pub language: Option<String>,
    /// Restrict to recent results.
    pub recency: Option<Recency>,
    /// Restrict to a site (host name only), applied as `site:host`.
    pub site: Option<String>,
}

impl SearchQuery {
    /// Query with default limit 10 and no filters.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            limit: 10,
            language: None,
            recency: None,
            site: None,
        }
    }

    /// Validate and return the final query string (with `site:` applied).
    pub(crate) fn effective_text(&self) -> Result<String, SearchError> {
        let base = sanitise_text(&self.text, 400);
        if base.is_empty() {
            return Err(SearchError::Config("query text is empty".into()));
        }
        match &self.site {
            None => Ok(base),
            Some(s) => {
                let host = s.trim().to_ascii_lowercase();
                let ok = !host.is_empty()
                    && host.len() <= 253
                    && host
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
                if !ok {
                    return Err(SearchError::Config(format!(
                        "site filter {s:?} is not a plain host name"
                    )));
                }
                Ok(format!("{base} site:{host}"))
            }
        }
    }

    pub(crate) fn clamped_limit(&self, max: usize) -> usize {
        self.limit.clamp(1, max)
    }

    pub(crate) fn lang(&self) -> Option<String> {
        self.language
            .as_deref()
            .map(str::trim)
            .filter(|l| {
                !l.is_empty()
                    && l.len() <= 16
                    && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            })
            .map(str::to_owned)
    }
}

/// One search hit. All text is sanitised and untrusted: it came from a third party.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    /// Page title.
    pub title: String,
    /// Normalised http(s) URL (see [`normalise_url`]).
    pub url: String,
    /// Snippet text.
    pub snippet: String,
    /// Engine that produced the hit (provider name or SearXNG engine).
    pub engine: String,
    /// Publication date or age as reported by the provider, unparsed.
    pub published: Option<String>,
}

/// A source of web search results.
#[async_trait]
pub trait SearchProvider: Send + Sync {
    /// Run a query. Results are already sanitised, filtered to http(s) and deduplicated.
    async fn search(&self, q: &SearchQuery) -> Result<Vec<SearchResult>, SearchError>;
    /// Short provider name, e.g. `searxng`.
    fn name(&self) -> &str;
}

/// An API key whose `Debug` output is redacted.
#[derive(Clone)]
pub struct ApiKey(String);

impl ApiKey {
    /// Wrap a key. Rejects empty keys and keys with control characters (header injection).
    pub fn new(key: impl Into<String>) -> Result<Self, SearchError> {
        let k = key.into().trim().to_owned();
        if k.is_empty() || k.chars().any(|c| c.is_control()) {
            return Err(SearchError::Config(
                "API key is empty or contains control characters".into(),
            ));
        }
        Ok(Self(k))
    }
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(<redacted>)")
    }
}

/// Transport limits shared by all providers.
#[derive(Debug, Clone, Copy)]
pub struct HttpOptions {
    /// Total request timeout including reading the body.
    pub timeout: Duration,
    /// Maximum response body size in bytes.
    pub max_body_bytes: usize,
}

impl Default for HttpOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            max_body_bytes: 2 * 1024 * 1024,
        }
    }
}

/// Which provider to use and how to reach it. `Debug` never prints the key.
#[derive(Debug, Clone)]
pub enum ProviderConfig {
    /// A SearXNG instance.
    Searxng {
        /// Base URL, e.g. `http://localhost:8080`.
        base_url: String,
    },
    /// Brave Search API.
    Brave {
        /// Subscription token.
        api_key: ApiKey,
    },
}

impl ProviderConfig {
    /// Read the configuration from the process environment. `Ok(None)` means search is not configured.
    pub fn from_env() -> Result<Option<Self>, SearchError> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Like [`from_env`](Self::from_env) with an injectable variable lookup (for tests).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Option<Self>, SearchError> {
        let nonempty = |k: &str| {
            get(k)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let searx = || -> Result<Self, SearchError> {
            let url = nonempty("OSTLER_SEARXNG_URL").ok_or_else(|| {
                SearchError::Config(
                    "OSTLER_SEARCH_PROVIDER=searxng needs OSTLER_SEARXNG_URL".into(),
                )
            })?;
            Ok(Self::Searxng { base_url: url })
        };
        let brave = || -> Result<Self, SearchError> {
            let key = nonempty("OSTLER_BRAVE_API_KEY").ok_or_else(|| {
                SearchError::Config(
                    "OSTLER_SEARCH_PROVIDER=brave needs OSTLER_BRAVE_API_KEY".into(),
                )
            })?;
            Ok(Self::Brave {
                api_key: ApiKey::new(key)?,
            })
        };
        match nonempty("OSTLER_SEARCH_PROVIDER")
            .map(|v| v.to_ascii_lowercase())
            .as_deref()
        {
            Some("none") | Some("off") => Ok(None),
            Some("searxng") => searx().map(Some),
            Some("brave") => brave().map(Some),
            Some(other) => Err(SearchError::Config(format!(
                "unknown OSTLER_SEARCH_PROVIDER {other:?}; use searxng, brave or none"
            ))),
            None => {
                if nonempty("OSTLER_SEARXNG_URL").is_some() {
                    searx().map(Some)
                } else if nonempty("OSTLER_BRAVE_API_KEY").is_some() {
                    brave().map(Some)
                } else {
                    Ok(None)
                }
            }
        }
    }

    /// Build the provider with default [`HttpOptions`].
    pub fn build(self) -> Result<Arc<dyn SearchProvider>, SearchError> {
        self.build_with(HttpOptions::default())
    }

    /// Build the provider with explicit transport limits.
    pub fn build_with(self, opts: HttpOptions) -> Result<Arc<dyn SearchProvider>, SearchError> {
        Ok(match self {
            Self::Searxng { base_url } => Arc::new(SearxngProvider::new(&base_url, opts)?),
            Self::Brave { api_key } => Arc::new(BraveProvider::new(api_key, opts)?),
        })
    }
}

const TRACKING_PARAMS: &[&str] = &[
    "fbclid", "gclid", "dclid", "msclkid", "mc_cid", "mc_eid", "igshid", "_ga", "yclid",
];

/// Normalise a URL for deduplication: http/https only, host lowercased, default port and fragment
/// removed, tracking parameters (`utm_*`, `fbclid`, `gclid`, ...) dropped, credentials rejected.
/// Returns `None` for anything else.
pub fn normalise_url(raw: &str) -> Option<String> {
    let mut u = Url::parse(raw.trim()).ok()?;
    if !matches!(u.scheme(), "http" | "https") || u.host_str().is_none() {
        return None;
    }
    if !u.username().is_empty() || u.password().is_some() {
        return None;
    }
    u.set_fragment(None);
    let kept: Vec<(String, String)> = u
        .query_pairs()
        .filter(|(k, _)| {
            let k = k.to_ascii_lowercase();
            !(k.starts_with("utm_") || TRACKING_PARAMS.contains(&k.as_str()))
        })
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if kept.is_empty() {
        u.set_query(None);
    } else {
        u.query_pairs_mut().clear().extend_pairs(kept);
    }
    Some(u.to_string())
}

/// Sanitise, filter, deduplicate and truncate raw provider results.
pub(crate) fn finish(raw: Vec<SearchResult>, limit: usize) -> Vec<SearchResult> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for r in raw {
        let Some(url) = normalise_url(&r.url) else {
            continue;
        };
        if url.len() > 2048 || !seen.insert(url.clone()) {
            continue;
        }
        out.push(SearchResult {
            title: sanitise_text(&r.title, 300),
            url,
            snippet: sanitise_text(&r.snippet, 1000),
            engine: sanitise_text(&r.engine, 64),
            published: r
                .published
                .map(|p| sanitise_text(&p, 64))
                .filter(|p| !p.is_empty()),
        });
        if out.len() >= limit {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalise_strips_tracking_fragment_and_lowercases_host() {
        assert_eq!(
            normalise_url("HTTPS://Example.COM:443/a?utm_source=x&b=1&fbclid=z#frag").as_deref(),
            Some("https://example.com/a?b=1")
        );
        assert_eq!(
            normalise_url("http://example.com/?utm_medium=a").as_deref(),
            Some("http://example.com/")
        );
    }

    #[test]
    fn normalise_rejects_non_http_and_credentials() {
        assert!(normalise_url("javascript:alert(1)").is_none());
        assert!(normalise_url("file:///etc/passwd").is_none());
        assert!(normalise_url("ftp://example.com/").is_none());
        assert!(normalise_url("https://user:pw@example.com/").is_none());
        assert!(normalise_url("not a url").is_none());
    }

    #[test]
    fn finish_dedupes_and_sanitises() {
        let mk = |u: &str, t: &str| SearchResult {
            title: t.into(),
            url: u.into(),
            snippet: "s\u{0}n\nip".into(),
            engine: "e".into(),
            published: Some("  ".into()),
        };
        let out = finish(
            vec![
                mk("https://a.com/x?utm_a=1", "one\u{7}"),
                mk("https://A.com/x#top", "dup"),
                mk("javascript:x", "bad"),
                mk("https://b.com/", "two"),
            ],
            10,
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].title, "one");
        assert_eq!(out[0].snippet, "sn ip");
        assert_eq!(out[0].published, None);
        assert_eq!(
            finish(
                vec![mk("https://a.com/", "a"), mk("https://b.com/", "b")],
                1
            )
            .len(),
            1
        );
    }

    #[test]
    fn query_site_and_empty() {
        let mut q = SearchQuery::new("rust");
        q.site = Some("Docs.rs".into());
        assert_eq!(q.effective_text().unwrap(), "rust site:docs.rs");
        q.site = Some("a b; c".into());
        assert!(q.effective_text().is_err());
        assert!(SearchQuery::new("  \n ").effective_text().is_err());
    }

    #[test]
    fn config_selection() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        assert!(ProviderConfig::from_lookup(env(&[])).unwrap().is_none());
        assert!(matches!(
            ProviderConfig::from_lookup(env(&[("OSTLER_SEARXNG_URL", "http://x")])).unwrap(),
            Some(ProviderConfig::Searxng { .. })
        ));
        assert!(matches!(
            ProviderConfig::from_lookup(env(&[("OSTLER_BRAVE_API_KEY", "k")])).unwrap(),
            Some(ProviderConfig::Brave { .. })
        ));
        assert!(ProviderConfig::from_lookup(env(&[("OSTLER_SEARCH_PROVIDER", "brave")])).is_err());
        assert!(ProviderConfig::from_lookup(env(&[("OSTLER_SEARCH_PROVIDER", "bing")])).is_err());
        assert!(ProviderConfig::from_lookup(env(&[
            ("OSTLER_SEARCH_PROVIDER", "none"),
            ("OSTLER_SEARXNG_URL", "http://x")
        ]))
        .unwrap()
        .is_none());
    }

    #[test]
    fn debug_never_prints_key() {
        let cfg = ProviderConfig::Brave {
            api_key: ApiKey::new("SECRET-KEY-123").unwrap(),
        };
        assert!(!format!("{cfg:?}").contains("SECRET"));
        assert!(!format!("{:?}", ApiKey::new("SECRET-KEY-123").unwrap()).contains("SECRET"));
        assert!(ApiKey::new("bad\nkey").is_err());
    }
}
