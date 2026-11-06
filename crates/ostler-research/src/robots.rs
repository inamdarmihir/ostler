//! robots.txt per RFC 9309: parser, matcher, fetch policy and cache.
//!
//! * Parsing: groups with several `User-agent` lines, case-insensitive directive names, comments,
//!   BOM, CR/LF/CRLF line ends, at most [`MAX_ROBOTS_BYTES`] parsed (a partial last line is dropped).
//! * Matching: the group whose product token equals ours (case-insensitive) is used, otherwise `*`,
//!   otherwise everything is allowed. Several groups for the same agent are merged. The longest
//!   matching pattern wins; on equal length `Allow` wins. `*` matches any run of characters, a
//!   trailing `$` anchors the end. Paths are percent-encoding normalised (unreserved escapes are
//!   decoded, others upper-cased, non-ASCII encoded).
//! * Fetch policy ([`RobotsClient`]): 2xx parse; 4xx (except 429) means "unavailable", allow all;
//!   429, 5xx and network errors mean "unreachable", which is reported as [`Decision::Unreachable`]
//!   (callers must treat it as disallow-all) after `retries` retries; more than 5 redirects count
//!   as unavailable (RFC 9309 section 2.3.1.2 permits this). If a refresh fails but a stale copy
//!   exists, the stale copy is used (RFC 9309 section 2.4).
//! * `Crawl-delay` is parsed and exposed ([`Robots::crawl_delay`]) but is advisory only.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::time::Instant;
use url::Url;

/// RFC 9309 requires parsing at least 500 KiB; anything past this is ignored.
pub const MAX_ROBOTS_BYTES: usize = 500 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Rule {
    allow: bool,
    /// Normalised pattern.
    pattern: String,
}

#[derive(Debug, Clone, Default)]
struct Group {
    agents: Vec<String>,
    rules: Vec<Rule>,
    crawl_delay: Option<f64>,
}

/// Result of matching one path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    /// Whether crawling is allowed.
    pub allowed: bool,
    /// The deciding rule as text (`Disallow: /x`), `None` when no rule matched.
    pub rule: Option<String>,
}

/// A parsed robots.txt.
#[derive(Debug, Clone, Default)]
pub struct Robots {
    groups: Vec<Group>,
}

/// Product token of a user agent string: `ostler/0.2 (x)` -> `ostler`.
pub fn product_token(ua: &str) -> String {
    ua.trim()
        .split(|c: char| c == '/' || c.is_whitespace())
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Normalise a path (and optional query) or pattern: decode percent-escapes of unreserved
/// characters, upper-case other escapes, percent-encode non-ASCII, spaces and control bytes.
pub fn normalise_path(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'%'
            && i + 2 < b.len()
            && b[i + 1].is_ascii_hexdigit()
            && b[i + 2].is_ascii_hexdigit()
        {
            let v = u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap_or(0);
            if v.is_ascii_alphanumeric() || matches!(v, b'-' | b'.' | b'_' | b'~') {
                out.push(v as char);
            } else {
                out.push_str(&format!("%{v:02X}"));
            }
            i += 3;
        } else if c >= 0x80 || c <= 0x20 || c == 0x7f {
            out.push_str(&format!("%{c:02X}"));
            i += 1;
        } else {
            out.push(c as char);
            i += 1;
        }
    }
    out
}

fn pattern_matches(pattern: &str, path: &str) -> bool {
    let (pat, anchored) = match pattern.strip_suffix('$') {
        Some(p) => (p, true),
        None => (pattern, false),
    };
    let segs: Vec<&str> = pat.split('*').collect();
    if !path.starts_with(segs[0]) {
        return false;
    }
    let mut pos = segs[0].len();
    if segs.len() == 1 {
        return !anchored || path.len() == pos;
    }
    let last = segs.len() - 1;
    for seg in &segs[1..last] {
        match path[pos..].find(seg) {
            Some(i) => pos += i + seg.len(),
            None => return false,
        }
    }
    let seg = segs[last];
    if anchored {
        path[pos..].ends_with(seg)
    } else {
        path[pos..].contains(seg)
    }
}

impl Robots {
    /// Everything allowed (no rules).
    pub fn allow_all() -> Self {
        Self::default()
    }

    /// Everything disallowed.
    pub fn disallow_all() -> Self {
        Self {
            groups: vec![Group {
                agents: vec!["*".into()],
                rules: vec![Rule {
                    allow: false,
                    pattern: "/".into(),
                }],
                crawl_delay: None,
            }],
        }
    }

    /// Parse robots.txt bytes. Never fails; unknown lines are ignored.
    pub fn parse(bytes: &[u8]) -> Self {
        let mut data = bytes;
        let mut truncated = false;
        if data.len() > MAX_ROBOTS_BYTES {
            data = &data[..MAX_ROBOTS_BYTES];
            truncated = true;
        }
        let mut text = String::from_utf8_lossy(data).into_owned();
        if truncated {
            // Drop the possibly cut-off last line.
            if let Some(i) = text.rfind(['\n', '\r']) {
                text.truncate(i);
            }
        }
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);

        let mut groups: Vec<Group> = Vec::new();
        let mut collecting_agents = false;
        for raw in text.split(['\n', '\r']) {
            let line = raw.split('#').next().unwrap_or("").trim();
            let Some((name, value)) = line.split_once(':') else {
                continue;
            };
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim();
            match name.as_str() {
                "user-agent" => {
                    if !collecting_agents {
                        groups.push(Group::default());
                    }
                    collecting_agents = true;
                    if let Some(g) = groups.last_mut() {
                        g.agents.push(if value == "*" {
                            "*".into()
                        } else {
                            product_token(value)
                        });
                    }
                }
                "allow" | "disallow" => {
                    collecting_agents = false;
                    if let Some(g) = groups.last_mut() {
                        if !value.is_empty() {
                            g.rules.push(Rule {
                                allow: name == "allow",
                                pattern: normalise_path(value),
                            });
                        }
                    }
                }
                "crawl-delay" => {
                    collecting_agents = false;
                    if let (Some(g), Ok(d)) = (groups.last_mut(), value.parse::<f64>()) {
                        if d.is_finite() && d >= 0.0 {
                            g.crawl_delay = Some(d);
                        }
                    }
                }
                _ => {
                    // sitemap and unknown directives do not end a group's agent list per RFC 9309.
                }
            }
        }
        Self { groups }
    }

    fn matching_groups(&self, token: &str) -> Vec<&Group> {
        let exact: Vec<&Group> = self
            .groups
            .iter()
            .filter(|g| !token.is_empty() && g.agents.iter().any(|a| a == token))
            .collect();
        if !exact.is_empty() {
            return exact;
        }
        self.groups
            .iter()
            .filter(|g| g.agents.iter().any(|a| a == "*"))
            .collect()
    }

    /// Match `path_and_query` (starting with `/`) for the given user agent string or product token.
    pub fn evaluate(&self, user_agent: &str, path_and_query: &str) -> Verdict {
        let token = product_token(user_agent);
        let path = normalise_path(path_and_query);
        let mut best: Option<&Rule> = None;
        for g in self.matching_groups(&token) {
            for r in &g.rules {
                if !pattern_matches(&r.pattern, &path) {
                    continue;
                }
                best = match best {
                    None => Some(r),
                    Some(b) if r.pattern.len() > b.pattern.len() => Some(r),
                    Some(b) if r.pattern.len() == b.pattern.len() && r.allow && !b.allow => Some(r),
                    other => other,
                };
            }
        }
        match best {
            None => Verdict {
                allowed: true,
                rule: None,
            },
            Some(r) => Verdict {
                allowed: r.allow,
                rule: Some(format!(
                    "{}: {}",
                    if r.allow { "Allow" } else { "Disallow" },
                    r.pattern
                )),
            },
        }
    }

    /// Advisory `Crawl-delay` for this user agent, if any (largest among matching groups).
    pub fn crawl_delay(&self, user_agent: &str) -> Option<Duration> {
        let token = product_token(user_agent);
        self.matching_groups(&token)
            .iter()
            .filter_map(|g| g.crawl_delay)
            .fold(None, |a: Option<f64>, d| Some(a.map_or(d, |x| x.max(d))))
            .map(|s| Duration::from_secs_f64(s.min(3600.0)))
    }
}

/// What the robots.txt request produced.
#[derive(Debug, Clone)]
pub enum FetchOutcome {
    /// Final HTTP status and (capped) body after following redirects.
    Response {
        /// HTTP status.
        status: u16,
        /// Body, at most [`MAX_ROBOTS_BYTES`] + 1 bytes.
        body: Vec<u8>,
    },
    /// More than 5 consecutive redirects.
    TooManyRedirects,
    /// DNS, connect, TLS or timeout failure.
    NetworkError(String),
}

/// Fetches `/robots.txt`. Injectable so tests need no network.
#[async_trait]
pub trait RobotsFetcher: Send + Sync {
    /// Fetch `robots_url` and report the outcome.
    async fn fetch(&self, robots_url: &Url, user_agent: &str) -> FetchOutcome;
}

/// [`RobotsFetcher`] over reqwest: follows up to 5 redirects, 10 s timeout, body capped.
#[derive(Debug, Clone)]
pub struct HttpRobotsFetcher {
    client: reqwest::Client,
}

impl HttpRobotsFetcher {
    /// Build the fetcher with the given total timeout.
    pub fn new(timeout: Duration) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::custom(|a| {
                if a.previous().len() >= 5 {
                    a.stop()
                } else {
                    a.follow()
                }
            }))
            .build()
            .map_err(|e| e.without_url().to_string())?;
        Ok(Self { client })
    }
}

#[async_trait]
impl RobotsFetcher for HttpRobotsFetcher {
    async fn fetch(&self, robots_url: &Url, user_agent: &str) -> FetchOutcome {
        let resp = match self
            .client
            .get(robots_url.clone())
            .header("User-Agent", user_agent)
            .header("Accept", "text/plain, */*;q=0.5")
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return FetchOutcome::NetworkError(e.without_url().to_string()),
        };
        let status = resp.status().as_u16();
        if (300..400).contains(&status) {
            return FetchOutcome::TooManyRedirects;
        }
        let mut resp = resp;
        let mut body = Vec::new();
        loop {
            match resp.chunk().await {
                Ok(Some(c)) => {
                    body.extend_from_slice(&c);
                    if body.len() > MAX_ROBOTS_BYTES {
                        body.truncate(MAX_ROBOTS_BYTES + 1);
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => return FetchOutcome::NetworkError(e.without_url().to_string()),
            }
        }
        FetchOutcome::Response { status, body }
    }
}

/// Answer to "may I fetch this URL?".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Allowed (including when robots.txt is unavailable per RFC, or has no matching rule).
    Allowed,
    /// Disallowed by this rule.
    Disallowed {
        /// The deciding rule, e.g. `Disallow: /private`.
        rule: String,
    },
    /// robots.txt could not be fetched (5xx, 429, network error). Treat as disallowed and report as a gap.
    Unreachable,
}

/// Cache and policy settings for [`RobotsClient`].
#[derive(Debug, Clone)]
pub struct RobotsOptions {
    /// How long a fetched robots.txt is reused (RFC 9309 recommends at most 24 h).
    pub ttl: Duration,
    /// How long an "unreachable" result is remembered before trying again.
    pub unreachable_ttl: Duration,
    /// Extra attempts after a 5xx/429/network failure.
    pub retries: u32,
    /// Pause before each retry.
    pub retry_delay: Duration,
}

impl Default for RobotsOptions {
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(24 * 3600),
            unreachable_ttl: Duration::from_secs(60),
            retries: 1,
            retry_delay: Duration::from_millis(500),
        }
    }
}

struct Entry {
    /// `None` = unreachable.
    robots: Option<Arc<Robots>>,
    fetched: Instant,
}

type Slot = Arc<tokio::sync::Mutex<Option<Entry>>>;

/// robots.txt checker with an in-memory per-origin cache.
pub struct RobotsClient {
    fetcher: Arc<dyn RobotsFetcher>,
    opts: RobotsOptions,
    fetch_ua: String,
    slots: Mutex<HashMap<String, Slot>>,
}

impl std::fmt::Debug for RobotsClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RobotsClient")
            .field("opts", &self.opts)
            .finish_non_exhaustive()
    }
}

impl RobotsClient {
    /// Client with an explicit fetcher. `fetch_ua` is the User-Agent header sent when fetching robots.txt.
    pub fn new(
        fetcher: Arc<dyn RobotsFetcher>,
        fetch_ua: impl Into<String>,
        opts: RobotsOptions,
    ) -> Self {
        Self {
            fetcher,
            opts,
            fetch_ua: fetch_ua.into(),
            slots: Mutex::new(HashMap::new()),
        }
    }

    /// May `user_agent` fetch `url`?
    pub async fn allowed(&self, url: &str, user_agent: &str) -> Decision {
        let Ok(u) = Url::parse(url) else {
            return Decision::Disallowed {
                rule: "invalid URL".into(),
            };
        };
        if !matches!(u.scheme(), "http" | "https") || u.host_str().is_none() {
            return Decision::Disallowed {
                rule: "not an http(s) URL".into(),
            };
        }
        let Some(robots) = self.robots_for(&u).await else {
            return Decision::Unreachable;
        };
        let mut pq = u.path().to_owned();
        if let Some(q) = u.query() {
            pq.push('?');
            pq.push_str(q);
        }
        let v = robots.evaluate(user_agent, &pq);
        if v.allowed {
            Decision::Allowed
        } else {
            Decision::Disallowed {
                rule: v.rule.unwrap_or_default(),
            }
        }
    }

    /// Advisory crawl delay for the origin of `url`, if robots.txt names one. Never enforced here.
    pub async fn crawl_delay(&self, url: &str, user_agent: &str) -> Option<Duration> {
        let u = Url::parse(url).ok()?;
        self.robots_for(&u).await?.crawl_delay(user_agent)
    }

    fn origin_key(u: &Url) -> String {
        format!(
            "{}://{}:{}",
            u.scheme(),
            u.host_str().unwrap_or("").to_ascii_lowercase(),
            u.port_or_known_default().unwrap_or(0)
        )
    }

    async fn robots_for(&self, u: &Url) -> Option<Arc<Robots>> {
        let key = Self::origin_key(u);
        let slot = {
            let mut m = self.slots.lock().ok()?;
            m.entry(key).or_default().clone()
        };
        let mut guard = slot.lock().await;
        if let Some(e) = guard.as_ref() {
            let ttl = if e.robots.is_some() {
                self.opts.ttl
            } else {
                self.opts.unreachable_ttl
            };
            if e.fetched.elapsed() < ttl {
                return e.robots.clone();
            }
        }
        let mut robots_url = u.clone();
        robots_url.set_path("/robots.txt");
        robots_url.set_query(None);
        robots_url.set_fragment(None);
        let _ = robots_url.set_username("");
        let _ = robots_url.set_password(None);
        let fresh = self.fetch_policy(&robots_url).await;
        let robots = match (fresh, guard.as_ref()) {
            (Some(r), _) => Some(Arc::new(r)),
            // Refresh failed: keep serving the stale copy (RFC 9309 section 2.4).
            (
                None,
                Some(Entry {
                    robots: Some(old), ..
                }),
            ) => Some(old.clone()),
            (None, _) => None,
        };
        *guard = Some(Entry {
            robots: robots.clone(),
            fetched: Instant::now(),
        });
        robots
    }

    /// `Some(rules)` when the policy yields rules (including allow-all), `None` when unreachable.
    async fn fetch_policy(&self, robots_url: &Url) -> Option<Robots> {
        for attempt in 0..=self.opts.retries {
            if attempt > 0 {
                tokio::time::sleep(self.opts.retry_delay).await;
            }
            match self.fetcher.fetch(robots_url, &self.fetch_ua).await {
                FetchOutcome::Response { status, body } => match status {
                    200..=299 => return Some(Robots::parse(&body)),
                    429 | 500..=599 => continue,
                    400..=499 => return Some(Robots::allow_all()),
                    _ => continue,
                },
                FetchOutcome::TooManyRedirects => return Some(Robots::allow_all()),
                FetchOutcome::NetworkError(_) => continue,
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const UA: &str = "ostler/0.2 (+https://example.org)";

    fn ok(rules: &str, ua: &str, path: &str) -> bool {
        Robots::parse(rules.as_bytes()).evaluate(ua, path).allowed
    }

    #[test]
    fn rfc_example_groups_and_agents() {
        let r = "User-Agent: *\nDisallow: *.gif$\nDisallow: /example/\nDisallow: /publications/\n\nUser-Agent: foobot\nDisallow:/\nAllow:/example/page.html\nAllow:/example/allowed.gif\n\nUser-Agent: barbot\nUser-Agent: bazbot\nDisallow: /example/page.html\n\nUser-Agent: quxbot\n\n";
        assert!(!ok(r, "foobot", "/"));
        assert!(ok(r, "foobot", "/example/page.html"));
        assert!(!ok(r, "foobot", "/example/other.html"));
        assert!(!ok(r, "barbot", "/example/page.html"));
        assert!(!ok(r, "bazbot", "/example/page.html"));
        assert!(ok(r, "barbot", "/example/other.html"));
        assert!(ok(r, "quxbot", "/anything"));
        // `*` group applies to unknown bots
        assert!(!ok(r, "other", "/example/x"));
        assert!(!ok(r, "other", "/a/b.gif"));
        assert!(ok(r, "other", "/a/b.gif?x=1"));
    }

    #[test]
    fn rfc_longest_match_and_allow_wins_ties() {
        let r = "User-agent: *\nAllow: /p\nDisallow: /\n";
        assert!(ok(r, "x", "/page"));
        assert!(!ok(r, "x", "/other"));
        let r = "User-agent: *\nAllow: /folder\nDisallow: /folder\n";
        assert!(ok(r, "x", "/folder/page"));
        let r = "User-agent: *\nAllow: /page\nDisallow: /*.htm\n";
        assert!(!ok(r, "x", "/page.htm"), "longer disallow wins");
        let r = "User-agent: *\nAllow: /$\nDisallow: /\n";
        assert!(ok(r, "x", "/"));
        assert!(!ok(r, "x", "/page.htm"));
        let v = Robots::parse(b"User-agent: *\nDisallow: /private\n").evaluate("x", "/private/a");
        assert_eq!(v.rule.as_deref(), Some("Disallow: /private"));
    }

    #[test]
    fn wildcards_and_anchor() {
        let r = "User-agent: *\nDisallow: /a*b$\nDisallow: /x*y*z\n";
        assert!(!ok(r, "x", "/axxb"));
        assert!(ok(r, "x", "/axxbc"));
        assert!(!ok(r, "x", "/x1y2z3"));
        assert!(ok(r, "x", "/x1z2y"));
        assert!(!ok(r, "x", "/ab"));
    }

    #[test]
    fn case_insensitive_names_comments_bom_and_line_ends() {
        let r = "\u{feff}USER-AGENT: OstlerBot # me\r\nDISALLOW: /secret # c\rAllow: /secret/ok\n";
        assert!(!ok(r, "ostlerbot/1", "/secret/x"));
        assert!(ok(r, "ostlerbot", "/secret/ok"));
        assert!(
            ok(r, "someoneelse", "/secret/x"),
            "no * group means allowed"
        );
        // path matching itself is case-sensitive
        assert!(ok("User-agent: *\nDisallow: /Secret\n", "x", "/secret"));
    }

    #[test]
    fn percent_encoding_normalisation() {
        let r = "User-agent: *\nDisallow: /a%2fb\nDisallow: /caf\u{e9}\nDisallow: /%7Euser\nDisallow: /sp ace\n";
        assert!(!ok(r, "x", "/a%2Fb"));
        assert!(ok(r, "x", "/a/b"), "%2F is not the same as /");
        assert!(!ok(r, "x", "/caf%C3%A9"));
        assert!(!ok(r, "x", "/caf\u{e9}"));
        assert!(!ok(r, "x", "/~user/page"));
        assert!(!ok(r, "x", "/sp%20ace"));
    }

    #[test]
    fn size_cap_drops_partial_line() {
        let mut s = String::from("User-agent: *\nDisallow: /early\n");
        while s.len() < MAX_ROBOTS_BYTES - 20 {
            s.push_str("# padding padding padding\n");
        }
        let pad_end = s.len();
        s.push_str("Disallow: /cut-off-here-and-more\n");
        s.push_str("Disallow: /late\n");
        let cut = MAX_ROBOTS_BYTES;
        assert!(s.len() > cut && pad_end < cut);
        let r = Robots::parse(s.as_bytes());
        assert!(!r.evaluate("x", "/early").allowed);
        assert!(r.evaluate("x", "/late").allowed);
        assert!(r.evaluate("x", "/cut").allowed, "partial line dropped");
    }

    #[test]
    fn crawl_delay_is_parsed_and_merged() {
        let r =
            Robots::parse(b"User-agent: bot\nCrawl-delay: 2.5\n\nUser-agent: *\nCrawl-delay: 10\n");
        assert_eq!(r.crawl_delay("bot/1"), Some(Duration::from_millis(2500)));
        assert_eq!(r.crawl_delay("zzz"), Some(Duration::from_secs(10)));
        assert_eq!(Robots::allow_all().crawl_delay("x"), None);
    }

    #[test]
    fn empty_disallow_and_rules_before_agent() {
        assert!(ok("Disallow: /\nUser-agent: *\nDisallow:\n", "x", "/a"));
        assert!(!Robots::disallow_all().evaluate("x", "/a").allowed);
    }

    struct Fake {
        calls: AtomicUsize,
        outcomes: Mutex<Vec<FetchOutcome>>,
        last_ua: Mutex<String>,
    }
    impl Fake {
        fn new(outcomes: Vec<FetchOutcome>) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                outcomes: Mutex::new(outcomes),
                last_ua: Mutex::new(String::new()),
            })
        }
    }
    #[async_trait]
    impl RobotsFetcher for Fake {
        async fn fetch(&self, _u: &Url, ua: &str) -> FetchOutcome {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last_ua.lock().unwrap() = ua.to_owned();
            let mut o = self.outcomes.lock().unwrap();
            if o.len() > 1 {
                o.remove(0)
            } else {
                o[0].clone()
            }
        }
    }
    fn resp(status: u16, body: &str) -> FetchOutcome {
        FetchOutcome::Response {
            status,
            body: body.as_bytes().to_vec(),
        }
    }
    fn client(f: Arc<Fake>) -> RobotsClient {
        RobotsClient::new(
            f,
            "ostler-fetch",
            RobotsOptions {
                retry_delay: Duration::from_millis(1),
                ..RobotsOptions::default()
            },
        )
    }

    #[tokio::test]
    async fn policy_2xx_parses_and_caches_per_origin() {
        let f = Fake::new(vec![resp(200, "User-agent: *\nDisallow: /private\n")]);
        let c = client(f.clone());
        assert_eq!(
            c.allowed("https://a.com/public", UA).await,
            Decision::Allowed
        );
        assert_eq!(
            c.allowed("https://a.com/private/x?y=1", UA).await,
            Decision::Disallowed {
                rule: "Disallow: /private".into()
            }
        );
        assert_eq!(f.calls.load(Ordering::SeqCst), 1, "cached");
        assert_eq!(
            c.allowed("https://a.com:8443/x", UA).await,
            Decision::Allowed
        );
        assert_eq!(
            f.calls.load(Ordering::SeqCst),
            2,
            "port is a different origin"
        );
        assert_eq!(*f.last_ua.lock().unwrap(), "ostler-fetch");
    }

    #[tokio::test]
    async fn policy_4xx_allows_all_and_5xx_is_unreachable() {
        for s in [404u16, 401, 403, 410] {
            let c = client(Fake::new(vec![resp(s, "User-agent: *\nDisallow: /\n")]));
            assert_eq!(
                c.allowed("https://a.com/x", UA).await,
                Decision::Allowed,
                "{s}"
            );
        }
        for s in [500u16, 503, 429] {
            let f = Fake::new(vec![resp(s, "")]);
            let c = client(f.clone());
            assert_eq!(
                c.allowed("https://a.com/x", UA).await,
                Decision::Unreachable,
                "{s}"
            );
            assert_eq!(f.calls.load(Ordering::SeqCst), 2, "one retry");
        }
        let c = client(Fake::new(vec![FetchOutcome::NetworkError("reset".into())]));
        assert_eq!(
            c.allowed("https://a.com/x", UA).await,
            Decision::Unreachable
        );
        let c = client(Fake::new(vec![FetchOutcome::TooManyRedirects]));
        assert_eq!(c.allowed("https://a.com/x", UA).await, Decision::Allowed);
    }

    #[tokio::test]
    async fn retry_recovers() {
        let f = Fake::new(vec![
            resp(503, ""),
            resp(200, "User-agent: *\nDisallow: /no\n"),
        ]);
        let c = client(f);
        assert!(matches!(
            c.allowed("https://a.com/no", UA).await,
            Decision::Disallowed { .. }
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn ttl_expiry_refetches_and_stale_copy_survives_failure() {
        let f = Fake::new(vec![
            resp(200, "User-agent: *\nDisallow: /no\n"),
            resp(500, ""),
        ]);
        let c = client(f.clone());
        assert!(matches!(
            c.allowed("https://a.com/no", UA).await,
            Decision::Disallowed { .. }
        ));
        tokio::time::advance(Duration::from_secs(25 * 3600)).await;
        // refresh fails (500 twice) -> stale copy still enforced
        assert!(matches!(
            c.allowed("https://a.com/no", UA).await,
            Decision::Disallowed { .. }
        ));
        assert_eq!(c.allowed("https://a.com/yes", UA).await, Decision::Allowed);
        assert_eq!(f.calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn unreachable_is_retried_after_short_ttl() {
        let f = Fake::new(vec![resp(500, ""), resp(500, ""), resp(200, "")]);
        let c = client(f);
        assert_eq!(
            c.allowed("https://a.com/x", UA).await,
            Decision::Unreachable
        );
        assert_eq!(
            c.allowed("https://a.com/x", UA).await,
            Decision::Unreachable,
            "cached for 60 s"
        );
        tokio::time::advance(Duration::from_secs(61)).await;
        assert_eq!(c.allowed("https://a.com/x", UA).await, Decision::Allowed);
    }

    #[tokio::test]
    async fn non_http_and_invalid_urls_are_disallowed() {
        let c = client(Fake::new(vec![resp(200, "")]));
        assert!(matches!(
            c.allowed("file:///etc/passwd", UA).await,
            Decision::Disallowed { .. }
        ));
        assert!(matches!(
            c.allowed("nonsense", UA).await,
            Decision::Disallowed { .. }
        ));
    }

    #[tokio::test]
    async fn crawl_delay_via_client() {
        let c = client(Fake::new(vec![resp(
            200,
            "User-agent: *\nCrawl-delay: 3\n",
        )]));
        assert_eq!(
            c.crawl_delay("https://a.com/x", UA).await,
            Some(Duration::from_secs(3))
        );
    }

    // Real HTTP fetcher against a mock server.
    use crate::testutil::{serve, Resp as R};

    #[tokio::test]
    async fn http_fetcher_follows_redirects_and_caps_body() {
        let m = serve(|u| match u {
            "/robots.txt" => R::new(301, "").header("Location", "/real-robots"),
            "/real-robots" => R::new(200, "User-agent: *\nDisallow: /x\n"),
            _ => R::new(404, ""),
        });
        let f = HttpRobotsFetcher::new(Duration::from_secs(5)).unwrap();
        let url = Url::parse(&format!("{}/robots.txt", m.base)).unwrap();
        match f.fetch(&url, "t").await {
            FetchOutcome::Response { status: 200, body } => {
                assert!(body.starts_with(b"User-agent"))
            }
            o => panic!("{o:?}"),
        }
        let m = serve(|_| R::new(200, vec![b'#'; MAX_ROBOTS_BYTES * 2]));
        let url = Url::parse(&format!("{}/robots.txt", m.base)).unwrap();
        match f.fetch(&url, "t").await {
            FetchOutcome::Response { body, .. } => assert!(body.len() <= MAX_ROBOTS_BYTES + 1),
            o => panic!("{o:?}"),
        }
        let m = serve(|_| R::new(302, "").header("Location", "/robots.txt"));
        let url = Url::parse(&format!("{}/robots.txt", m.base)).unwrap();
        assert!(matches!(
            f.fetch(&url, "t").await,
            FetchOutcome::TooManyRedirects
        ));
    }
}
