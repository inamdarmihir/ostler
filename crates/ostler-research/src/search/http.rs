//! Shared hardened HTTP helpers for search providers.

use std::time::Duration;

use reqwest::redirect::Policy;
use url::Url;

use super::{HttpOptions, SearchError};

fn origin(u: &Url) -> (String, Option<String>, Option<u16>) {
    (
        u.scheme().to_owned(),
        u.host_str().map(str::to_ascii_lowercase),
        u.port_or_known_default(),
    )
}

/// Client that follows at most 3 redirects, and only within the origin of `base`.
pub(crate) fn client(base: &Url, opts: &HttpOptions) -> Result<reqwest::Client, SearchError> {
    let base_origin = origin(base);
    let policy = Policy::custom(move |attempt| {
        if attempt.previous().len() >= 3 {
            attempt.error("too many redirects")
        } else if origin(attempt.url()) == base_origin {
            attempt.follow()
        } else {
            attempt.stop()
        }
    });
    reqwest::Client::builder()
        .timeout(opts.timeout)
        .redirect(policy)
        .user_agent("ostler-research/0.2 (+https://github.com/inamdarmihir/ostler)")
        .build()
        .map_err(|e| {
            SearchError::Config(format!("could not build HTTP client: {}", e.without_url()))
        })
}

/// Map a transport error to [`SearchError`] without leaking URLs.
pub(crate) fn map_err(e: reqwest::Error, timeout: Duration) -> SearchError {
    let e = e.without_url();
    if e.is_timeout() {
        SearchError::Timeout(timeout)
    } else {
        SearchError::Network(e.to_string())
    }
}

/// Read the body, failing with [`SearchError::TooLarge`] if it exceeds `cap` bytes.
pub(crate) async fn read_capped(
    mut resp: reqwest::Response,
    cap: usize,
    timeout: Duration,
) -> Result<Vec<u8>, SearchError> {
    if resp.content_length().is_some_and(|l| l > cap as u64) {
        return Err(SearchError::TooLarge(cap));
    }
    let mut buf = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| map_err(e, timeout))? {
        if buf.len() + chunk.len() > cap {
            return Err(SearchError::TooLarge(cap));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

/// True for a plain-http URL whose host is not loopback.
pub(crate) fn is_cleartext_remote(u: &Url) -> bool {
    if u.scheme() != "http" {
        return false;
    }
    let host = u
        .host_str()
        .unwrap_or("")
        .trim_matches(|c| c == '[' || c == ']');
    !(host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback()))
}
