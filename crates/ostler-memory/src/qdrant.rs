//! Qdrant over REST. Two ways to get vectors:
//!
//! * [`VectorSource::CloudInference`]: send `{text, model}` and let Qdrant Cloud embed
//!   server-side (no client model, one fewer network hop for us).
//! * [`VectorSource::Client`]: embed locally and send raw vectors (any Qdrant, incl. OSS).

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::store::{Filter, Hit, Kind, Rank, Store};
use crate::{Embedder, MemoryError, Result};

#[derive(Clone)]
pub enum VectorSource {
    CloudInference { model: String, dim: usize },
    Client(Arc<dyn Embedder>),
}

impl VectorSource {
    /// Qdrant-hosted free model, 384 dimensions.
    pub fn cloud_default() -> Self {
        Self::CloudInference {
            model: "sentence-transformers/all-minilm-l6-v2".into(),
            dim: 384,
        }
    }
    fn dim(&self) -> usize {
        match self {
            Self::CloudInference { dim, .. } => *dim,
            Self::Client(e) => e.dim(),
        }
    }
    /// Name of the embedding model, recorded in the store so a later switch to another model is caught.
    fn model_id(&self) -> String {
        match self {
            Self::CloudInference { model, .. } => model.clone(),
            Self::Client(e) => e.model_id(),
        }
    }
}

/// Reserved point (in the profile collection) recording which embedding model built the vectors.
/// It carries no `tenant`, so no tenant-scoped query, count or wipe ever touches it.
const META_ID: &str = "00000000-0000-4000-8000-6f73746c6572";

#[derive(Clone)]
pub struct QdrantConfig {
    pub url: String,
    pub api_key: Option<String>,
    pub vectors: VectorSource,
    pub timeout: Duration,
    /// Keep vectors (and the HNSW graph) on disk, memory-mapped, instead of in RAM.
    /// Trades a little latency for far lower memory per user. Applies when collections are created.
    pub on_disk: bool,
    /// Allow a plain `http://` URL to a non-loopback host. Off by default: the API key and every
    /// stored fact would cross the network in cleartext.
    pub allow_insecure: bool,
    /// Prepended to every collection name. With [`tenant_prefix`](crate::tenant_prefix) each tenant gets its own collections,
    /// so a token scoped to them (see [`crate::mint_tenant_token`]) is enforced by the server.
    pub collection_prefix: String,
}

/// Refuse to send the API key and memory over plaintext HTTP to anything but this machine.
pub(crate) fn check_transport(url: &str, allow_insecure: bool) -> Result<()> {
    let u = reqwest::Url::parse(url)
        .map_err(|e| MemoryError::Invalid(format!("qdrant url {url:?} is not valid: {e}")))?;
    match u.scheme() {
        "https" => Ok(()),
        "http" => {
            let host = u
                .host_str()
                .unwrap_or("")
                .trim_matches(|c| c == '[' || c == ']');
            let loopback = host.eq_ignore_ascii_case("localhost")
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback());
            if loopback || allow_insecure {
                Ok(())
            } else {
                Err(MemoryError::Invalid(format!(
                    "refusing plain http:// to {host}: the API key and your saved facts would cross the network unencrypted. \
                     Use https://, or pass --allow-insecure-qdrant if the link is already protected (VPN, SSH tunnel)."
                )))
            }
        }
        other => Err(MemoryError::Invalid(format!(
            "qdrant url must be http(s), got {other:?}"
        ))),
    }
}

/// Fails fast once Qdrant has failed repeatedly, so a down or slow cluster costs one short timeout
/// instead of a full timeout on every lookup of every field.
///
/// Closed -> (two failures) -> open for the cooldown -> half-open: exactly **one** request is let through as
/// the trial while every other request keeps failing fast; the trial's outcome closes or reopens the breaker.
#[derive(Default)]
struct Breaker {
    state: std::sync::Mutex<BreakerState>,
}

#[derive(Default)]
struct BreakerState {
    failures: u32,
    open_until: Option<std::time::Instant>,
    /// A trial request is out and has not reported back yet.
    trial_in_flight: bool,
    last_error: String,
}

/// Consecutive failures that open the breaker.
const BREAKER_THRESHOLD: u32 = 2;

/// Permission to make one request. Report the outcome with [`Permit::success`] or [`Permit::failure`]; if it is
/// dropped without either (the caller was cancelled or timed out) a trial is released, not lost, so the
/// breaker cannot get stuck half-open.
struct Permit<'a> {
    breaker: &'a Breaker,
    trial: bool,
    done: bool,
}

impl Breaker {
    fn lock(&self) -> std::sync::MutexGuard<'_, BreakerState> {
        // A panic elsewhere must not turn the breaker into a second panic.
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// `Err` while open, or while another request is the trial.
    fn acquire(&self) -> Result<Permit<'_>> {
        let mut s = self.lock();
        match s.open_until {
            None => Ok(Permit {
                breaker: self,
                trial: false,
                done: false,
            }),
            Some(t) if std::time::Instant::now() < t => {
                Err(MemoryError::Unavailable(s.last_error.clone()))
            }
            Some(_) if s.trial_in_flight => Err(MemoryError::Unavailable(s.last_error.clone())),
            Some(_) => {
                s.trial_in_flight = true;
                Ok(Permit {
                    breaker: self,
                    trial: true,
                    done: false,
                })
            }
        }
    }

    /// Fail fast while open, without taking the trial (for work that precedes the real request).
    fn peek(&self) -> Result<()> {
        let s = self.lock();
        match s.open_until {
            Some(t) if std::time::Instant::now() < t || s.trial_in_flight => {
                Err(MemoryError::Unavailable(s.last_error.clone()))
            }
            _ => Ok(()),
        }
    }

    /// A failure that did not come through a permit (the local embedder).
    fn note_failure(&self, why: &str, cooldown: Duration) {
        let mut s = self.lock();
        Self::record_failure(&mut s, why, cooldown, false);
    }

    fn record_failure(s: &mut BreakerState, why: &str, cooldown: Duration, trial: bool) {
        s.failures += 1;
        s.last_error = clip(why, 200);
        if trial || s.failures >= BREAKER_THRESHOLD {
            s.open_until = Some(std::time::Instant::now() + cooldown);
        }
    }

    fn degraded(&self) -> Option<String> {
        let s = self.lock();
        let open = s
            .open_until
            .is_some_and(|t| std::time::Instant::now() < t || s.trial_in_flight);
        open.then(|| s.last_error.clone())
    }
}

impl Permit<'_> {
    fn success(mut self) {
        self.done = true;
        let mut s = self.breaker.lock();
        // A straggler that started before the breaker opened says nothing about the cluster now.
        if self.trial || s.open_until.is_none() {
            s.failures = 0;
            s.open_until = None;
            s.trial_in_flight = false;
        }
    }

    fn failure(mut self, why: &str, cooldown: Duration) {
        self.done = true;
        let mut s = self.breaker.lock();
        if self.trial {
            s.trial_in_flight = false;
        }
        Breaker::record_failure(&mut s, why, cooldown, self.trial);
    }
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        if self.trial && !self.done {
            self.breaker.lock().trial_in_flight = false;
        }
    }
}

/// How long the breaker stays open before one trial request is let through.
const BREAKER_COOLDOWN: Duration = Duration::from_secs(30);

/// Make `s` safe to show to the user and the agent: no control characters or line breaks, one run of
/// whitespace collapsed to a space, at most `max` characters (an ellipsis marks a cut).
pub(crate) fn clip(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut last_space = true;
    let mut n = 0;
    for c in s.chars() {
        let c = if c.is_control() || c.is_whitespace() {
            ' '
        } else {
            c
        };
        if c == ' ' {
            if last_space {
                continue;
            }
            last_space = true;
        } else {
            last_space = false;
        }
        if n == max {
            out.push('\u{2026}');
            return out;
        }
        out.push(c);
        n += 1;
    }
    out.trim_end().to_string()
}

pub struct QdrantStore {
    cfg: QdrantConfig,
    http: reqwest::Client,
    breaker: Breaker,
    cooldown: Duration,
    retry: Retry,
    /// Whether the server can score with a formula (Qdrant 1.14+): see [`FORMULA_UNKNOWN`].
    formula: AtomicU8,
}

/// Minimum Qdrant server version for server-side ranked search (the Query API `formula`).
pub const MIN_FORMULA_VERSION: (u32, u32) = (1, 14);
const FORMULA_UNKNOWN: u8 = 0;
const FORMULA_YES: u8 = 1;
const FORMULA_NO: u8 = 2;

/// Parse `major.minor` out of a Qdrant version string such as `1.12.6` or `v1.14.0-rc1`.
pub fn parse_version(s: &str) -> Option<(u32, u32)> {
    let mut it = s.trim().trim_start_matches('v').split(['.', '-', '+']);
    Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
}

/// Bounded retry of transient failures, for idempotent requests only (reads and upserts under a fixed id).
/// Exponential backoff with jitter; the whole sequence is also bounded by the request timeout, so retrying
/// can never make a lookup slower than one timeout plus a small delay.
#[derive(Clone, Copy)]
struct Retry {
    /// Extra attempts after the first.
    max: u32,
    base: Duration,
    cap: Duration,
}

impl Retry {
    const DEFAULT: Retry = Retry {
        max: 2,
        base: Duration::from_millis(100),
        cap: Duration::from_secs(1),
    };

    /// Delay before retry number `attempt` (0-based): half of `min(cap, base * 2^attempt)` plus a random
    /// share of the other half, so concurrent callers do not retry in lockstep.
    fn delay(&self, attempt: u32) -> Duration {
        let ceil = self
            .base
            .saturating_mul(1u32.checked_shl(attempt).unwrap_or(u32::MAX))
            .min(self.cap);
        let mut b = [0u8; 4];
        let r = if getrandom::fill(&mut b).is_ok() {
            u32::from_le_bytes(b) as f64 / u32::MAX as f64
        } else {
            0.5
        };
        ceil.mul_f64(0.5 + 0.5 * r)
    }
}

/// Failures worth retrying: the request never completed, or the server said "try again" (not 500, which
/// usually means a bug that will repeat).
fn transient(r: &Result<(u16, Value)>) -> bool {
    match r {
        Err(_) => true,
        Ok((status, _)) => matches!(*status, 429 | 502 | 503 | 504),
    }
}

impl QdrantStore {
    pub fn new(cfg: QdrantConfig) -> Result<Self> {
        check_transport(&cfg.url, cfg.allow_insecure)?;
        let http = reqwest::Client::builder()
            .timeout(cfg.timeout)
            // Never follow a redirect: it would resend the api-key and body wherever it points (even http),
            // and never use an ambient proxy: it would see the key and your facts in clear for loopback URLs.
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .pool_idle_timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| MemoryError::Http(e.to_string()))?;
        Ok(Self {
            cfg,
            http,
            breaker: Breaker::default(),
            cooldown: BREAKER_COOLDOWN,
            retry: Retry::DEFAULT,
            formula: AtomicU8::new(FORMULA_UNKNOWN),
        })
    }

    /// Shorten the breaker's cooldown (tests only).
    #[cfg(test)]
    fn with_cooldown(mut self, d: Duration) -> Self {
        self.cooldown = d;
        self
    }

    /// Set the retry policy (tests only; production uses the defaults).
    #[cfg(test)]
    fn with_retry(mut self, max: u32, base: Duration) -> Self {
        self.retry = Retry {
            max,
            base,
            cap: base.saturating_mul(8),
        };
        self
    }

    fn coll(&self, kind: Kind) -> String {
        format!("{}{}", self.cfg.collection_prefix, kind.collection())
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.cfg.url.trim_end_matches('/'), path)
    }

    /// Remove from `s` anything that must not reach the user or the agent: the API key, and credentials or
    /// the address embedded in the Qdrant URL.
    fn scrub(&self, s: &str) -> String {
        let mut out = s.to_string();
        if let Some(k) = self.cfg.api_key.as_deref().filter(|k| !k.is_empty()) {
            out = out.replace(k, "[redacted]");
        }
        if let Ok(u) = reqwest::Url::parse(&self.cfg.url) {
            if !u.username().is_empty() {
                out = out.replace(u.username(), "[redacted]");
            }
            if let Some(pw) = u.password().filter(|p| !p.is_empty()) {
                out = out.replace(pw, "[redacted]");
            }
        }
        out
    }

    /// A bounded, single-line, secret-free rendering of an error body.
    fn body_text(&self, v: &Value) -> String {
        let raw = match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        // Scrub before clipping so a key cut in half by the limit cannot slip through.
        clip(&self.scrub(&raw), 200)
    }

    fn status_error(&self, status: u16, body: &Value) -> MemoryError {
        MemoryError::Status {
            status,
            body: self.body_text(body),
        }
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<(u16, Value)> {
        self.send_with(method, path, body, false).await
    }

    /// `idempotent` requests may be repeated: transient failures are retried with backoff. The sequence
    /// counts as one outcome for the breaker, so a retried blip does not open it. Never pass `true` for
    /// anything that is not safe to apply twice.
    async fn send_with(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        idempotent: bool,
    ) -> Result<(u16, Value)> {
        let permit = self.breaker.acquire()?;
        let started = std::time::Instant::now();
        let mut attempt = 0;
        let r = loop {
            let r = self.send_once(method.clone(), path, body.clone()).await;
            if !idempotent || attempt >= self.retry.max || !transient(&r) {
                break r;
            }
            let wait = self.retry.delay(attempt);
            if started.elapsed() + wait >= self.cfg.timeout {
                break r;
            }
            tokio::time::sleep(wait).await;
            attempt += 1;
        };
        match &r {
            // Server-side trouble counts; a 4xx is the caller's mistake and says the server is up.
            Ok((status, _)) if *status >= 500 => {
                permit.failure(&format!("HTTP {status}"), self.cooldown)
            }
            Ok(_) => permit.success(),
            Err(e) => permit.failure(&e.to_string(), self.cooldown),
        }
        r
    }

    fn transport_error(&self, e: reqwest::Error) -> MemoryError {
        use std::error::Error;
        let mut causes = String::new();
        let mut src = e.source();
        while let Some(s) = src {
            causes.push_str(": ");
            causes.push_str(&s.to_string());
            src = s.source();
        }
        // `without_url` drops the address (and any credentials in it) from the top-level message.
        let msg = format!("{}{causes}", e.without_url());
        MemoryError::Http(clip(&self.scrub(&msg), 200))
    }

    async fn send_once(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<(u16, Value)> {
        let mut req = self.http.request(method, self.url(path));
        if let Some(k) = &self.cfg.api_key {
            req = req.header("api-key", k);
        }
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req.send().await.map_err(|e| self.transport_error(e))?;
        let status = resp.status().as_u16();
        let text = resp.text().await.map_err(|e| self.transport_error(e))?;
        Ok((
            status,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        ))
    }

    async fn ok(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value> {
        let (status, v) = self.send(method, path, body).await?;
        if (200..300).contains(&status) {
            Ok(v)
        } else {
            Err(self.status_error(status, &v))
        }
    }

    /// Like [`Self::ok`] for requests that are safe to repeat (reads, upserts under a fixed id).
    async fn ok_idem(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let (status, v) = self.send_with(method, path, body, true).await?;
        if (200..300).contains(&status) {
            Ok(v)
        } else {
            Err(self.status_error(status, &v))
        }
    }

    /// Bulk-load points: `(id, text, payload)` in one request. For imports and benchmarks.
    pub async fn upsert_batch(
        &self,
        kind: Kind,
        items: Vec<(String, String, Value)>,
    ) -> Result<()> {
        let texts: Vec<String> = items.iter().map(|i| i.1.clone()).collect();
        let vectors: Vec<Value> = match &self.cfg.vectors {
            VectorSource::CloudInference { model, .. } => texts
                .iter()
                .map(|t| json!({"text": t, "model": model}))
                .collect(),
            VectorSource::Client(e) => e
                .embed(&texts)
                .await?
                .into_iter()
                .map(|v| json!(v))
                .collect(),
        };
        let points: Vec<Value> = items
            .into_iter()
            .zip(vectors)
            .map(|((id, _, payload), vector)| json!({"id": id, "vector": vector, "payload": payload}))
            .collect();
        self.ok_idem(
            reqwest::Method::PUT,
            &format!("/collections/{}/points?wait=true", self.coll(kind)),
            Some(json!({ "points": points })),
        )
        .await?;
        Ok(())
    }

    /// The vector (or inference document) for `text`, in the shape Qdrant expects.
    async fn vector_for(&self, text: &str) -> Result<Value> {
        match &self.cfg.vectors {
            VectorSource::CloudInference { model, .. } => {
                Ok(json!({ "text": text, "model": model }))
            }
            VectorSource::Client(e) => {
                // A hung or failing local embedder must not stall every field lookup: same breaker as Qdrant.
                // Fail fast if Qdrant is already known to be down; the request itself takes the trial permit.
                self.breaker.peek()?;
                match e.embed(&[text.to_string()]).await {
                    Ok(mut v) => match v.pop() {
                        Some(v) => Ok(json!(v)),
                        None => Err(MemoryError::Embed("embedder returned no vector".into())),
                    },
                    Err(err) => {
                        self.breaker.note_failure(&err.to_string(), self.cooldown);
                        Err(err)
                    }
                }
            }
        }
    }
}

impl QdrantStore {
    /// Learn whether the server can run a ranking formula. A server older than 1.14 rejects the request
    /// with an opaque format error, so ranked search falls back to plain search plus the same formula
    /// evaluated here. If the version cannot be read, assume it is supported (the common case).
    async fn probe_version(&self) {
        let known = match self.send_with(reqwest::Method::GET, "/", None, true).await {
            Ok((200, v)) => v["version"].as_str().and_then(parse_version),
            _ => None,
        };
        let state = match known {
            Some(v) if v < MIN_FORMULA_VERSION => FORMULA_NO,
            _ => FORMULA_YES,
        };
        self.formula.store(state, Ordering::Relaxed);
    }

    /// Force the pre-1.14 path (client-side scoring) regardless of the server version. For parity tests.
    #[doc(hidden)]
    pub fn force_client_side_ranking(&self) {
        self.formula.store(FORMULA_NO, Ordering::Relaxed);
    }

    /// Refuse to run against vectors built by a different embedding model. Two models can share a
    /// dimension, and then nothing errors: similarity scores are just quietly meaningless. The model name is
    /// stored on first use; collections created before it was recorded adopt the configured model. Problems
    /// reading or writing the marker are not fatal (memory still works; the check just cannot run).
    async fn check_model(&self) -> Result<()> {
        let model = self.cfg.vectors.model_id();
        let name = self.coll(Kind::Profile);
        let path = format!("/collections/{name}/points/{META_ID}");
        let Ok((status, v)) = self
            .send_with(reqwest::Method::GET, &path, None, true)
            .await
        else {
            return Ok(());
        };
        match status {
            200 => {
                if let Some(stored) = v["result"]["payload"]["model"]
                    .as_str()
                    .filter(|m| *m != model)
                {
                    return Err(MemoryError::Invalid(format!(
                        "collection {name} was built with embedding model {stored:?} but the configured model is {model:?}. \
                         Their vectors are not comparable, so lookups would silently return wrong matches. Switch back to {stored:?}, \
                         or use a different collection prefix to start a fresh memory (re-saving facts re-embeds them)"
                    )));
                }
                Ok(())
            }
            404 => {
                let _ = self
                    .upsert(
                        Kind::Profile,
                        META_ID,
                        &model,
                        json!({ "ostler_meta": "embedding_model", "model": model, "dim": self.cfg.vectors.dim() }),
                    )
                    .await;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Payload indexes. Creating an index that already exists is a no-op, so this runs on every start.
    async fn ensure_indexes(&self, name: &str) {
        // Tenant index first: with is_tenant Qdrant co-locates a tenant's points for fast filtered search.
        let mut specs: Vec<(&str, Value)> =
            vec![("tenant", json!({ "type": "keyword", "is_tenant": true }))];
        for field in [
            "key_norm",
            "fingerprint",
            "autocomplete",
            "id_hint",
            "host",
            "labels_norm",
        ] {
            specs.push((field, json!("keyword")));
        }
        // Integer indexes serve both range filters (pruning) and the score-boosting formula.
        for field in ["verified_ts", "wins", "losses"] {
            specs.push((field, json!("integer")));
        }
        for (field, schema) in specs {
            let _ = self
                .ok(
                    reqwest::Method::PUT,
                    &format!("/collections/{name}/index?wait=true"),
                    Some(json!({ "field_name": field, "field_schema": schema })),
                )
                .await;
        }
    }
}

#[async_trait]
impl Store for QdrantStore {
    fn degraded(&self) -> Option<String> {
        self.breaker.degraded()
    }

    async fn ensure(&self) -> Result<()> {
        self.probe_version().await;
        for kind in Kind::ALL {
            let name = &self.coll(kind);
            let (status, info) = self
                .send(reqwest::Method::GET, &format!("/collections/{name}"), None)
                .await?;
            if status == 200 {
                let size = info["result"]["config"]["params"]["vectors"]["size"].as_u64();
                if let Some(size) = size {
                    if size as usize != self.cfg.vectors.dim() {
                        return Err(MemoryError::Invalid(format!(
                            "collection {name} has {size}-dim vectors but the configured model produces {}; \
                             pick a matching model or use a different collection prefix",
                            self.cfg.vectors.dim()
                        )));
                    }
                }
                // Idempotent: collections made by older versions gain the newer indexes.
                self.ensure_indexes(name).await;
                continue;
            }
            if status != 404 {
                return Err(self.status_error(status, &info));
            }
            let (created, body) = self
                .send(
                    reqwest::Method::PUT,
                    &format!("/collections/{name}"),
                    Some(json!({
                        "vectors": { "size": self.cfg.vectors.dim(), "distance": "Cosine", "on_disk": self.cfg.on_disk },
                        "hnsw_config": { "on_disk": self.cfg.on_disk },
                        "on_disk_payload": true
                    })),
                )
                .await?;
            // 409: another process created it between our GET and PUT. Same end state; carry on.
            if !(200..300).contains(&created) && created != 409 {
                return Err(self.status_error(created, &body));
            }
            self.ensure_indexes(name).await;
        }
        self.check_model().await
    }

    async fn upsert(&self, kind: Kind, id: &str, text: &str, payload: Value) -> Result<()> {
        let vector = self.vector_for(text).await?;
        self.ok_idem(
            reqwest::Method::PUT,
            &format!("/collections/{}/points?wait=true", self.coll(kind)),
            Some(json!({ "points": [{ "id": id, "vector": vector, "payload": payload }] })),
        )
        .await?;
        Ok(())
    }

    async fn search(
        &self,
        kind: Kind,
        text: &str,
        filter: &Filter,
        limit: usize,
    ) -> Result<Vec<Hit>> {
        if limit == 0 {
            return Ok(vec![]);
        }
        let query = self.vector_for(text).await?;
        let v = self
            .ok_idem(
                reqwest::Method::POST,
                &format!("/collections/{}/points/query", self.coll(kind)),
                Some(json!({ "query": query, "filter": filter.to_qdrant(), "limit": limit, "with_payload": true })),
            )
            .await?;
        Ok(parse_hits(&v["result"]["points"]))
    }

    async fn search_ranked(
        &self,
        kind: Kind,
        text: &str,
        filter: &Filter,
        limit: usize,
        rank: &Rank,
    ) -> Result<Vec<Hit>> {
        if limit == 0 {
            return Ok(vec![]);
        }
        if self.formula.load(Ordering::Relaxed) == FORMULA_NO {
            // Same candidate pool as the server path (4x, at least 16), the similarity floor, then `Rank::score`.
            let pool = limit.saturating_mul(4).max(16);
            let mut hits: Vec<Hit> = self
                .search(kind, text, filter, pool)
                .await?
                .into_iter()
                .filter(|h| h.score >= rank.min_similarity)
                .map(|mut h| {
                    h.score = rank.score(h.score, &h.payload);
                    h
                })
                .collect();
            hits.sort_by(|a, b| b.score.total_cmp(&a.score));
            hits.truncate(limit);
            return Ok(hits);
        }
        let vector = self.vector_for(text).await?;
        let v = self
            .ok_idem(
                reqwest::Method::POST,
                &format!("/collections/{}/points/query", self.coll(kind)),
                Some(rank.to_qdrant_query(vector, filter, limit)),
            )
            .await?;
        Ok(parse_hits(&v["result"]["points"]))
    }

    async fn scroll(&self, kind: Kind, filter: &Filter, limit: usize) -> Result<Vec<Hit>> {
        if limit == 0 {
            return Ok(vec![]);
        }
        let v = self
            .ok_idem(
                reqwest::Method::POST,
                &format!("/collections/{}/points/scroll", self.coll(kind)),
                Some(json!({ "filter": filter.to_qdrant(), "limit": limit, "with_payload": true, "with_vector": false })),
            )
            .await?;
        Ok(parse_hits(&v["result"]["points"]))
    }

    async fn delete(&self, kind: Kind, id: &str) -> Result<()> {
        self.ok(
            reqwest::Method::POST,
            &format!("/collections/{}/points/delete?wait=true", self.coll(kind)),
            Some(json!({ "points": [id] })),
        )
        .await?;
        Ok(())
    }

    async fn delete_matching(&self, kind: Kind, filter: &Filter) -> Result<()> {
        if !filter.is_tenant_scoped() {
            return Err(MemoryError::Invalid(
                "refusing a bulk delete that is not scoped to a tenant".into(),
            ));
        }
        self.ok(
            reqwest::Method::POST,
            &format!("/collections/{}/points/delete?wait=true", self.coll(kind)),
            Some(json!({ "filter": filter.to_qdrant() })),
        )
        .await?;
        Ok(())
    }

    async fn count(&self, kind: Kind, filter: &Filter) -> Result<u64> {
        let v = self
            .ok_idem(
                reqwest::Method::POST,
                &format!("/collections/{}/points/count", self.coll(kind)),
                Some(json!({ "filter": filter.to_qdrant(), "exact": true })),
            )
            .await?;
        Ok(v["result"]["count"].as_u64().unwrap_or(0))
    }
}

fn parse_hits(points: &Value) -> Vec<Hit> {
    points
        .as_array()
        .map(|a| {
            a.iter()
                .map(|p| Hit {
                    id: p["id"]
                        .as_str()
                        .map(String::from)
                        .unwrap_or_else(|| p["id"].to_string()),
                    score: p["score"].as_f64().unwrap_or(1.0) as f32,
                    payload: p["payload"].clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::parse_version;

    #[test]
    fn parses_qdrant_versions() {
        assert_eq!(parse_version("1.12.6"), Some((1, 12)));
        assert_eq!(parse_version("v1.14.0-rc1"), Some((1, 14)));
        assert_eq!(parse_version("1.20.0"), Some((1, 20)));
        assert_eq!(parse_version("nonsense"), None);
        assert!((1, 12) < super::MIN_FORMULA_VERSION && (1, 14) >= super::MIN_FORMULA_VERSION);
    }

    use super::*;
    use wiremock::matchers::{body_partial_json, header, method, path, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn cloud(url: String) -> QdrantStore {
        QdrantStore::new(QdrantConfig {
            url,
            api_key: Some("k3y".into()),
            vectors: VectorSource::cloud_default(),
            timeout: Duration::from_secs(5),
            on_disk: false,
            allow_insecure: false,
            collection_prefix: String::new(),
        })
        .unwrap()
        // The tests below count requests exactly; retries are exercised by their own tests.
        .with_retry(0, Duration::from_millis(1))
    }

    fn retrying(url: String, max: u32) -> QdrantStore {
        cloud(url).with_retry(max, Duration::from_millis(1))
    }

    /// Cloud Inference must send text+model documents (no client-side vectors) with the api-key header.
    /// This pins the request shape from Qdrant's docs; it cannot prove a live cluster accepts it.
    #[tokio::test]
    async fn cloud_inference_upsert_sends_text_and_model() {
        let srv = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/collections/ostler_profile/points"))
            .and(header("api-key", "k3y"))
            .and(body_partial_json(json!({"points":[{"id":"abc","vector":{"text":"email","model":"sentence-transformers/all-minilm-l6-v2"}}]})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status":"ok"})))
            .expect(1)
            .mount(&srv)
            .await;
        cloud(srv.uri())
            .upsert(Kind::Profile, "abc", "email", json!({"tenant":"t"}))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn cloud_inference_query_sends_text_and_tenant_filter() {
        let srv = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/collections/ostler_profile/points/query"))
            .and(body_partial_json(json!({
                "query": {"text": "zip", "model": "sentence-transformers/all-minilm-l6-v2"},
                "filter": {"must": [{"key": "tenant", "match": {"value": "t1"}}]},
                "limit": 3
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"result":{"points":[{"id":"x","score":0.8,"payload":{"key":"zip"}}]}}),
            ))
            .expect(1)
            .mount(&srv)
            .await;
        let hits = cloud(srv.uri())
            .search(Kind::Profile, "zip", &Filter::tenant("t1"), 3)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert!((hits[0].score - 0.8).abs() < 1e-6);
    }

    #[tokio::test]
    async fn dimension_mismatch_is_a_clear_error() {
        let srv = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/collections/ostler_profile"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"result":{"config":{"params":{"vectors":{"size":1024,"distance":"Cosine"}}}}})))
            .mount(&srv)
            .await;
        let err = cloud(srv.uri()).ensure().await.unwrap_err().to_string();
        assert!(err.contains("1024") && err.contains("384"), "{err}");
    }

    #[tokio::test]
    async fn http_errors_surface_status() {
        let srv = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(403).set_body_string("forbidden"))
            .mount(&srv)
            .await;
        let err = cloud(srv.uri())
            .scroll(Kind::Profile, &Filter::tenant("t"), 1)
            .await
            .unwrap_err();
        assert!(
            matches!(err, MemoryError::Status { status: 403, .. }),
            "{err}"
        );
    }

    /// A failing cluster must cost two requests, not one timeout per field: after that lookups fail
    /// fast with `Unavailable` and the store reports itself degraded.
    #[tokio::test]
    async fn repeated_server_failures_open_the_breaker() {
        let srv = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .expect(2)
            .mount(&srv)
            .await;
        let store = cloud(srv.uri());
        assert!(store.degraded().is_none());
        let f = Filter::tenant("t");
        for _ in 0..2 {
            let e = store.scroll(Kind::Profile, &f, 1).await.unwrap_err();
            assert!(matches!(e, MemoryError::Status { status: 503, .. }), "{e}");
        }
        for _ in 0..3 {
            let e = store.scroll(Kind::Profile, &f, 1).await.unwrap_err();
            assert!(matches!(e, MemoryError::Unavailable(_)), "{e}");
        }
        assert!(store.degraded().unwrap().contains("503"));
        // `expect(2)` is verified when `srv` drops: the five calls reached the server twice.
    }

    /// After the cooldown exactly one request is the trial; the rest keep failing fast until it reports back.
    #[tokio::test]
    async fn after_the_cooldown_only_one_request_is_let_through_as_the_trial() {
        let srv = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(2)
            .with_priority(1)
            .mount(&srv)
            .await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(400))
                    .set_body_json(json!({"result":{"points":[]}})),
            )
            .with_priority(2)
            .mount(&srv)
            .await;
        let store = Arc::new(cloud(srv.uri()).with_cooldown(Duration::from_millis(150)));
        let f = Filter::tenant("t");
        for _ in 0..2 {
            assert!(store.scroll(Kind::Profile, &f, 1).await.is_err());
        }
        assert!(store.degraded().is_some());
        tokio::time::sleep(Duration::from_millis(250)).await;

        let mut tasks = vec![];
        for _ in 0..12 {
            let (s, f) = (store.clone(), f.clone());
            tasks.push(tokio::spawn(
                async move { s.scroll(Kind::Profile, &f, 1).await },
            ));
        }
        let (mut ok, mut fast_fail) = (0, 0);
        for t in tasks {
            match t.await.unwrap() {
                Ok(_) => ok += 1,
                Err(MemoryError::Unavailable(_)) => fast_fail += 1,
                Err(e) => panic!("{e}"),
            }
        }
        assert_eq!((ok, fast_fail), (1, 11), "more than one trial went through");
        assert_eq!(
            srv.received_requests().await.unwrap().len(),
            3,
            "2 failures + 1 trial"
        );
        // The trial succeeded: the breaker is closed and traffic flows again.
        assert!(store.degraded().is_none());
        assert!(store.scroll(Kind::Profile, &f, 1).await.is_ok());
    }

    /// A failed trial reopens the breaker for a full cooldown; the next trial happens only after it.
    #[tokio::test]
    async fn a_failed_trial_reopens_the_breaker() {
        let srv = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&srv)
            .await;
        let store = cloud(srv.uri()).with_cooldown(Duration::from_millis(150));
        let f = Filter::tenant("t");
        for _ in 0..2 {
            let _ = store.scroll(Kind::Profile, &f, 1).await;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        let e = store.scroll(Kind::Profile, &f, 1).await.unwrap_err();
        assert!(matches!(e, MemoryError::Status { status: 503, .. }), "{e}");
        // Trial failed: straight back to failing fast, without touching the server.
        for _ in 0..3 {
            let e = store.scroll(Kind::Profile, &f, 1).await.unwrap_err();
            assert!(matches!(e, MemoryError::Unavailable(_)), "{e}");
        }
        assert_eq!(srv.received_requests().await.unwrap().len(), 3);
    }

    /// A trial whose future is dropped (task cancelled, caller timed out) must not wedge the breaker.
    #[tokio::test]
    async fn an_abandoned_trial_does_not_wedge_the_breaker() {
        let srv = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(2)
            .with_priority(1)
            .mount(&srv)
            .await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(2000))
                    .set_body_json(json!({"result":{"points":[]}})),
            )
            .with_priority(2)
            .mount(&srv)
            .await;
        let store = cloud(srv.uri()).with_cooldown(Duration::from_millis(100));
        let f = Filter::tenant("t");
        for _ in 0..2 {
            let _ = store.scroll(Kind::Profile, &f, 1).await;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
        // The trial starts, then its caller gives up.
        let r = tokio::time::timeout(
            Duration::from_millis(100),
            store.scroll(Kind::Profile, &f, 1),
        )
        .await;
        assert!(r.is_err(), "expected the slow trial to be abandoned");
        // The next request must be allowed to be a new trial instead of failing fast forever.
        let e = tokio::time::timeout(
            Duration::from_millis(100),
            store.scroll(Kind::Profile, &f, 1),
        )
        .await;
        assert!(e.is_err(), "the next request should have reached the server (and timed out there), not been refused");
        assert_eq!(srv.received_requests().await.unwrap().len(), 4);
    }

    /// The reason that reaches the agent is short, single-line and never carries the API key or credentials in the URL.
    #[tokio::test]
    async fn error_messages_are_capped_stripped_and_free_of_secrets() {
        let srv = MockServer::start().await;
        let body = format!(
            "Wrong input: bad value for k3y\n\t{}{}",
            "A".repeat(5000),
            "\u{1b}[31mred\0"
        );
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_string(body))
            .mount(&srv)
            .await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(418).set_body_string(format!("{}\nk3y", "B".repeat(5000))),
            )
            .mount(&srv)
            .await;
        let store = cloud(srv.uri());
        let e = store
            .scroll(Kind::Profile, &Filter::tenant("t"), 1)
            .await
            .unwrap_err()
            .to_string();
        assert!(e.chars().count() < 300, "{} chars", e.chars().count());
        assert!(
            !e.contains('\n') && !e.contains('\t') && !e.contains('\u{1b}') && !e.contains('\0'),
            "{e:?}"
        );
        assert!(!e.contains("k3y"), "api key echoed: {e}");
        // ensure(): the non-404 branch used to echo the whole body.
        let e = store.ensure().await.unwrap_err().to_string();
        assert!(e.chars().count() < 300, "{} chars", e.chars().count());
        assert!(!e.contains('\n') && !e.contains("k3y"), "{e:?}");

        // Connection failures do not repeat credentials from the URL.
        let dead = QdrantStore::new(QdrantConfig {
            url: "http://user:hunter2@127.0.0.1:1".into(),
            api_key: Some("k3y".into()),
            vectors: VectorSource::cloud_default(),
            timeout: Duration::from_secs(2),
            on_disk: false,
            allow_insecure: false,
            collection_prefix: String::new(),
        })
        .unwrap();
        let e = dead
            .scroll(Kind::Profile, &Filter::tenant("t"), 1)
            .await
            .unwrap_err()
            .to_string();
        assert!(!e.contains("hunter2"), "{e}");
        assert!(dead.degraded().is_none());
        let _ = dead.scroll(Kind::Profile, &Filter::tenant("t"), 1).await;
        assert!(!dead.degraded().unwrap().contains("hunter2"));
    }

    /// Two processes starting at once both try to create the collections; the loser's 409 is not an error.
    #[tokio::test]
    async fn a_concurrent_creation_conflict_is_not_an_error() {
        let srv = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/collections/ostler_profile"))
            .respond_with(ResponseTemplate::new(404))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&srv)
            .await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(
                    json!({"result":{"config":{"params":{"vectors":{"size":384}}}}}),
                ),
            )
            .with_priority(2)
            .mount(&srv)
            .await;
        Mock::given(method("PUT"))
            .and(path("/collections/ostler_profile"))
            .respond_with(
                ResponseTemplate::new(409)
                    .set_body_json(json!({"status":{"error":"already exists"}})),
            )
            .with_priority(1)
            .mount(&srv)
            .await;
        Mock::given(method("PUT"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status":"ok"})))
            .with_priority(3)
            .mount(&srv)
            .await;
        cloud(srv.uri()).ensure().await.unwrap();
    }

    #[tokio::test]
    async fn limit_zero_asks_nothing_and_a_huge_limit_does_not_overflow() {
        let srv = MockServer::start().await;
        let store = cloud(srv.uri());
        assert!(store
            .scroll(Kind::Profile, &Filter::tenant("t"), 0)
            .await
            .unwrap()
            .is_empty());
        assert!(store
            .search(Kind::Profile, "x", &Filter::tenant("t"), 0)
            .await
            .unwrap()
            .is_empty());
        assert!(srv.received_requests().await.unwrap().is_empty());
        let r = Rank {
            now: 1,
            success_weight: 0.1,
            recency_weight: 0.1,
            half_life_secs: 1,
            min_similarity: 0.0,
        };
        let q = r.to_qdrant_query(json!([1.0]), &Filter::tenant("t"), usize::MAX);
        assert!(q["prefetch"]["limit"].is_number());
    }

    /// A 4xx means the server answered; it must not trip the breaker.
    #[tokio::test]
    async fn client_errors_do_not_open_the_breaker() {
        let srv = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&srv)
            .await;
        let store = cloud(srv.uri());
        for _ in 0..4 {
            let e = store
                .scroll(Kind::Profile, &Filter::tenant("t"), 1)
                .await
                .unwrap_err();
            assert!(matches!(e, MemoryError::Status { status: 403, .. }), "{e}");
        }
        assert!(store.degraded().is_none());
    }

    #[tokio::test]
    async fn idempotent_requests_retry_transient_failures_then_succeed() {
        let srv = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/collections/ostler_profile/points/scroll"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(2)
            .expect(2)
            .mount(&srv)
            .await;
        Mock::given(method("POST"))
            .and(path("/collections/ostler_profile/points/scroll"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"result":{"points":[]}})))
            .expect(1)
            .mount(&srv)
            .await;
        let store = retrying(srv.uri(), 2);
        store
            .scroll(Kind::Profile, &Filter::tenant("t"), 1)
            .await
            .unwrap();
        // A retried blip is one outcome for the breaker: it stays closed.
        assert!(store.degraded().is_none());
    }

    #[tokio::test]
    async fn upserts_retry_but_deletes_never_do() {
        let srv = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/collections/ostler_profile/points"))
            .respond_with(ResponseTemplate::new(502))
            .up_to_n_times(1)
            .expect(1)
            .mount(&srv)
            .await;
        Mock::given(method("PUT"))
            .and(path("/collections/ostler_profile/points"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"result":{}})))
            .expect(1)
            .mount(&srv)
            .await;
        Mock::given(method("POST"))
            .and(path("/collections/ostler_profile/points/delete"))
            .respond_with(ResponseTemplate::new(503))
            .expect(1)
            .mount(&srv)
            .await;
        let store = retrying(srv.uri(), 3);
        store
            .upsert(Kind::Profile, "id", "text", json!({"tenant": "t"}))
            .await
            .unwrap();
        let e = store.delete(Kind::Profile, "id").await.unwrap_err();
        assert!(matches!(e, MemoryError::Status { status: 503, .. }), "{e}");
    }

    #[tokio::test]
    async fn retries_are_bounded_and_skip_client_and_server_bugs() {
        let srv = MockServer::start().await;
        // 500 and 4xx are not transient: exactly one request each.
        Mock::given(method("POST"))
            .and(path("/collections/ostler_profile/points/count"))
            .respond_with(ResponseTemplate::new(400))
            .expect(1)
            .mount(&srv)
            .await;
        // A persistently unavailable server gets 1 + max attempts, then the error surfaces.
        Mock::given(method("POST"))
            .and(path("/collections/ostler_profile/points/scroll"))
            .respond_with(ResponseTemplate::new(503))
            .expect(3)
            .mount(&srv)
            .await;
        let store = retrying(srv.uri(), 2);
        let f = Filter::tenant("t");
        assert!(matches!(
            store.count(Kind::Profile, &f).await.unwrap_err(),
            MemoryError::Status { status: 400, .. }
        ));
        assert!(matches!(
            store.scroll(Kind::Profile, &f, 1).await.unwrap_err(),
            MemoryError::Status { status: 503, .. }
        ));
    }

    #[test]
    fn backoff_grows_is_capped_and_jittered_within_bounds() {
        let r = Retry {
            max: 5,
            base: Duration::from_millis(100),
            cap: Duration::from_millis(500),
        };
        for attempt in 0..40 {
            let ceil =
                Duration::from_millis(100u64.saturating_mul(1 << attempt.min(20))).min(r.cap);
            let d = r.delay(attempt);
            assert!(
                d <= ceil && d >= ceil / 2,
                "attempt {attempt}: {d:?} vs {ceil:?}"
            );
        }
    }

    fn collection_ok(size: u64) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(
            json!({"result":{"config":{"params":{"vectors":{"size":size,"distance":"Cosine"}}}}}),
        )
    }

    #[tokio::test]
    async fn a_different_embedding_model_with_the_same_dimension_is_refused_with_advice() {
        let srv = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path_regex("^/collections/ostler_[a-z]+$"))
            .respond_with(collection_ok(384))
            .mount(&srv)
            .await;
        Mock::given(method("PUT"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"result":{}})))
            .mount(&srv)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/collections/ostler_profile/points/{META_ID}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"result":{"id":META_ID,"payload":{"model":"some/other-model-384","dim":384}}}),
            ))
            .mount(&srv)
            .await;
        let err = cloud(srv.uri()).ensure().await.unwrap_err().to_string();
        assert!(
            err.contains("some/other-model-384")
                && err.contains("sentence-transformers/all-minilm-l6-v2")
                && err.contains("collection prefix"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn the_model_is_recorded_on_first_use_and_the_same_model_passes() {
        let srv = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path_regex("^/collections/ostler_[a-z]+$"))
            .respond_with(collection_ok(384))
            .mount(&srv)
            .await;
        Mock::given(method("PUT"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"result":{}})))
            .mount(&srv)
            .await;
        let marker = format!("/collections/ostler_profile/points/{META_ID}");
        Mock::given(method("GET"))
            .and(path(marker.clone()))
            .respond_with(ResponseTemplate::new(404))
            .up_to_n_times(1)
            .mount(&srv)
            .await;
        Mock::given(method("GET"))
            .and(path(marker))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"result":{"payload":{"model":"sentence-transformers/all-minilm-l6-v2"}}}),
            ))
            .mount(&srv)
            .await;
        let store = cloud(srv.uri());
        store.ensure().await.unwrap(); // first use: marker absent, gets written
        store.ensure().await.unwrap(); // second: marker matches
        let wrote = srv.received_requests().await.unwrap().iter().any(|r| {
            r.method.as_str() == "PUT"
                && r.url.path() == "/collections/ostler_profile/points"
                && String::from_utf8_lossy(&r.body).contains("embedding_model")
        });
        assert!(wrote, "the marker point was never written");
    }
}

#[cfg(test)]
mod redirect_tests {
    use super::*;
    use wiremock::matchers::any;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn a_redirect_never_carries_the_api_key_elsewhere() {
        let target = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200))
            .mount(&target)
            .await;
        let front = MockServer::start().await;
        Mock::given(any())
            .respond_with(
                ResponseTemplate::new(307)
                    .insert_header("location", format!("{}/steal", target.uri())),
            )
            .mount(&front)
            .await;
        let store = QdrantStore::new(QdrantConfig {
            url: front.uri(),
            api_key: Some("SECRETKEY".into()),
            vectors: VectorSource::cloud_default(),
            timeout: Duration::from_secs(5),
            on_disk: false,
            allow_insecure: false,
            collection_prefix: String::new(),
        })
        .unwrap();
        let _ = store.ensure().await;
        assert!(
            target.received_requests().await.unwrap().is_empty(),
            "the redirect was followed"
        );
    }
}

#[cfg(test)]
mod transport_tests {
    use super::check_transport as t;

    #[test]
    fn https_and_loopback_pass_remote_http_is_refused() {
        assert!(t("https://x.cloud.qdrant.io:6333", false).is_ok());
        assert!(t("http://localhost:6333", false).is_ok());
        assert!(t("http://127.0.0.1:6333", false).is_ok());
        assert!(t("http://[::1]:6333", false).is_ok());
        assert!(t("http://qdrant.internal:6333", false).is_err());
        assert!(t("http://203.0.113.5:6333", false).is_err());
        assert!(t("http://localhost.evil.com:6333", false).is_err());
        assert!(t("http://qdrant.internal:6333", true).is_ok());
        assert!(t("ftp://x", true).is_err());
    }
}
