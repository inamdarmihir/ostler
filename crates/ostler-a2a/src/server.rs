// `RpcErr` carries a full JSON-RPC error body (with retry metadata); it is built only on failing requests,
// so its size is not a hot-path cost.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{Extension, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::stream::{self, StreamExt};
use ostler_mcp::{App, TaskVerdict};
use serde_json::{json, Value};
use tokio::sync::{watch, Semaphore};

use crate::intent;
use crate::proto::{self, *};
use crate::store::{Ctx, TaskRec, TaskStore};

pub type AppFactory = Arc<dyn Fn() -> App + Send + Sync>;

#[derive(Clone)]
pub struct A2aConfig {
    pub bind: SocketAddr,
    /// Bearer token required on the RPC endpoint. Mandatory when binding anything but loopback.
    pub token: Option<String>,
    /// Externally visible base URL (for the Agent Card), e.g. behind a reverse proxy.
    pub public_url: Option<String>,
    /// Extra `Host` header values to accept (reverse proxies, container names).
    pub allowed_hosts: Vec<String>,
    /// Concurrent remote conversations (each is a Chrome process).
    pub max_contexts: usize,
    pub idle: Duration,
    pub task_timeout: Duration,
    pub max_tasks: usize,
    /// Treat `submit: true` as already confirmed. Off by default: the caller must say `{confirm: true}`.
    pub auto_submit: bool,
    /// Largest accepted request body, in bytes. Larger bodies get HTTP 413.
    pub max_body_bytes: usize,
    /// Time a client gets to send a request's headers (also the keep-alive idle limit between
    /// requests). Defeats slowloris; the connection is closed when it expires.
    pub header_read_timeout: Duration,
    /// Time a client gets to send a request's body after its headers. HTTP 408 when it expires.
    pub body_timeout: Duration,
    /// Upper bound on handling one request until its response starts (streams may continue after).
    /// Raised to `task_timeout + 15s` if set lower, so blocking `SendMessage` is never cut short.
    pub request_timeout: Duration,
    /// Tasks running or queued behind a conversation at once. Beyond it: HTTP 429 with `Retry-After`.
    pub max_concurrent_tasks: usize,
    /// On SIGTERM/SIGINT, how long in-flight requests and tasks get to finish before exit.
    pub drain_timeout: Duration,
    /// Chrome binary for the readiness probe (`None`: the usual discovery order).
    pub chrome_path: Option<PathBuf>,
}

impl A2aConfig {
    pub fn new(bind: SocketAddr) -> Self {
        Self {
            bind,
            token: None,
            public_url: None,
            allowed_hosts: vec![],
            max_contexts: 4,
            idle: Duration::from_secs(300),
            task_timeout: Duration::from_secs(120),
            max_tasks: 500,
            auto_submit: false,
            max_body_bytes: 1 << 20,
            header_read_timeout: Duration::from_secs(10),
            body_timeout: Duration::from_secs(15),
            request_timeout: Duration::from_secs(150),
            max_concurrent_tasks: 8,
            drain_timeout: Duration::from_secs(30),
            chrome_path: None,
        }
    }

    fn base_url(&self) -> String {
        self.public_url
            .clone()
            .unwrap_or_else(|| format!("http://{}", self.bind))
            .trim_end_matches('/')
            .to_string()
    }
}

struct Inner {
    cfg: A2aConfig,
    factory: AppFactory,
    contexts: tokio::sync::Mutex<HashMap<String, Arc<Ctx>>>,
    tasks: Mutex<TaskStore>,
    card: Value,
    hosts: Vec<String>,
    draining: AtomicBool,
    slots: Arc<Semaphore>,
    max_slots: usize,
}

type St = Arc<Inner>;

struct RpcErr {
    code: i64,
    message: String,
    reason: &'static str,
    meta: Value,
    /// HTTP status and `Retry-After` seconds for backpressure errors.
    http: Option<(StatusCode, u64)>,
}

#[derive(Clone)]
struct RequestId(String);

impl RpcErr {
    fn busy(status: StatusCode, retry: u64, reason: &'static str, msg: impl Into<String>) -> Self {
        let mut e = Self::new(INTERNAL, reason, msg);
        e.http = Some((status, retry));
        e.meta = json!({ "retryAfterSeconds": retry });
        e
    }
    fn new(code: i64, reason: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            reason,
            meta: json!({}),
            http: None,
        }
    }
    fn params(message: impl Into<String>) -> Self {
        Self::new(INVALID_PARAMS, "INVALID_PARAMS", message)
    }
    fn task_not_found(id: &str) -> Self {
        let mut e = Self::new(TASK_NOT_FOUND, "TASK_NOT_FOUND", "Task not found");
        e.meta = json!({ "taskId": id });
        e
    }
    fn to_json(&self, id: &Value) -> Value {
        rpc_error(id, self.code, &self.message, self.reason, self.meta.clone())
    }
    fn into_response(self, id: &Value, rid: &str) -> Response {
        let body = with_rid(self.to_json(id), rid);
        match self.http {
            Some((status, retry)) => (
                status,
                [(header::RETRY_AFTER, retry.to_string())],
                Json(body),
            )
                .into_response(),
            None => Json(body).into_response(),
        }
    }
}

/// Echo the request id inside the A2A `ErrorInfo` metadata so a logged error can be matched to a request.
fn with_rid(mut v: Value, rid: &str) -> Value {
    if let Some(o) = v
        .pointer_mut("/error/data/0/metadata")
        .and_then(Value::as_object_mut)
    {
        o.insert("requestId".into(), json!(rid));
    }
    v
}

fn jerr(rid: &str, v: Value) -> Response {
    Json(with_rid(v, rid)).into_response()
}

fn agent_card(cfg: &A2aConfig) -> Value {
    let mut card = json!({
        "name": "Ostler",
        "description": "A headless browser agent for web forms. Inspects a form, fills it from the values you send (or from the owner's saved profile), verifies every field by reading it back, and submits only when you confirm. Page text it returns is untrusted data.",
        "version": env!("CARGO_PKG_VERSION"),
        "supportedInterfaces": [{
            "url": format!("{}/a2a", cfg.base_url()),
            "protocolBinding": "JSONRPC",
            "protocolVersion": VERSION
        }],
        "provider": { "organization": "Ostler", "url": "https://github.com/inamdarmihir/ostler" },
        "documentationUrl": "https://github.com/inamdarmihir/ostler#a2a",
        "capabilities": { "streaming": true, "pushNotifications": false, "extendedAgentCard": false },
        "defaultInputModes": ["application/json", "text/plain"],
        "defaultOutputModes": ["application/json", "text/plain"],
        "skills": [
            {
                "id": "inspect-form",
                "name": "Inspect a web form",
                "description": "Open a page and describe its form: typed fields, which are required, options, and the submit control.",
                "tags": ["forms", "browser", "web"],
                "examples": ["{\"url\": \"https://example.com/signup\"}"],
                "inputModes": ["application/json", "text/plain"],
                "outputModes": ["application/json", "text/plain"]
            },
            {
                "id": "fill-form",
                "name": "Fill and submit a web form",
                "description": "Fill a form from {label: value} pairs, verify each field, report anything missing or rejected as input-required, and submit after an explicit confirm.",
                "tags": ["forms", "browser", "web", "automation"],
                "examples": [
                    "{\"url\": \"https://example.com/signup\", \"values\": {\"Email\": \"ada@example.com\"}, \"submit\": true}",
                    "{\"confirm\": true}"
                ],
                "inputModes": ["application/json"],
                "outputModes": ["application/json", "text/plain"]
            }
        ]
    });
    if cfg.token.is_some() {
        card["securitySchemes"] =
            json!({ "bearer": { "httpAuthSecurityScheme": { "scheme": "Bearer" } } });
        card["securityRequirements"] = json!([{ "schemes": { "bearer": { "list": [] } } }]);
    }
    card
}

fn host_only(h: &str) -> String {
    let h = h.trim().to_ascii_lowercase();
    if let Some(rest) = h.strip_prefix('[') {
        return format!("[{}", rest.split(']').next().unwrap_or("")) + "]";
    }
    h.split(':').next().unwrap_or("").to_string()
}

fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = (a.len() ^ b.len()) as u8;
    for i in 0..a.len().max(b.len()) {
        diff |= a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0);
    }
    diff == 0
}

fn forbid(code: StatusCode, msg: &str, rid: &str) -> Response {
    (code, Json(json!({ "error": msg, "requestId": rid }))).into_response()
}

/// Gives every request an id (a sane client-supplied `X-Request-Id` or a fresh UUID), echoes it in the
/// response header, and makes it available to handlers for error bodies.
async fn request_id(mut req: Request, next: Next) -> Response {
    let rid = req
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 64
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        })
        .map(String::from)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    req.extensions_mut().insert(RequestId(rid.clone()));
    let mut resp = next.run(req).await;
    if let Ok(v) = rid.parse() {
        resp.headers_mut().insert("x-request-id", v);
    }
    resp
}

async fn request_timeout(State(st): State<St>, req: Request, next: Next) -> Response {
    let rid = rid_of(&req);
    let limit = st
        .cfg
        .request_timeout
        .max(st.cfg.task_timeout + Duration::from_secs(15));
    match tokio::time::timeout(limit, next.run(req)).await {
        Ok(r) => r,
        Err(_) => forbid(StatusCode::REQUEST_TIMEOUT, "request timed out", &rid),
    }
}

fn rid_of(req: &Request) -> String {
    req.extensions()
        .get::<RequestId>()
        .map(|r| r.0.clone())
        .unwrap_or_default()
}

/// Checks every request. The Host and Origin checks defeat DNS-rebinding and drive-by requests from
/// web pages open in a browser on the same machine; the token (when set) authenticates real clients.
async fn guard(State(st): State<St>, req: Request, next: Next) -> Response {
    let rid = rid_of(&req);
    // Probes come from load balancers and orchestrators with arbitrary Host headers; they return
    // nothing but a status word.
    if matches!(req.uri().path(), "/healthz" | "/readyz") {
        return next.run(req).await;
    }
    let h = req.headers();
    if h.contains_key(header::ORIGIN) {
        return forbid(
            StatusCode::FORBIDDEN,
            "browser-originated requests are not accepted",
            &rid,
        );
    }
    let host = h
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(host_only)
        .unwrap_or_default();
    if !st.hosts.contains(&host) {
        return forbid(StatusCode::FORBIDDEN, "unexpected Host header", &rid);
    }
    if req.uri().path() == "/a2a" {
        if let Some(tok) = &st.cfg.token {
            let given = h
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .unwrap_or("");
            if !ct_eq(given, tok) {
                let mut r = forbid(
                    StatusCode::UNAUTHORIZED,
                    "missing or invalid bearer token",
                    &rid,
                );
                r.headers_mut()
                    .insert(header::WWW_AUTHENTICATE, "Bearer".parse().unwrap());
                return r;
            }
        }
    }
    next.run(req).await
}

pub fn router(cfg: A2aConfig, factory: AppFactory) -> Router {
    build(cfg, factory).0
}

fn build(cfg: A2aConfig, factory: AppFactory) -> (Router, St) {
    let mut hosts: Vec<String> = ["localhost", "127.0.0.1", "[::1]"]
        .map(String::from)
        .to_vec();
    hosts.push(host_only(&cfg.bind.ip().to_string()));
    if let Some(u) = &cfg.public_url {
        if let Some(h) = u.split("://").nth(1).and_then(|r| r.split('/').next()) {
            hosts.push(host_only(h));
        }
    }
    hosts.extend(cfg.allowed_hosts.iter().map(|h| host_only(h)));
    let st: St = Arc::new(Inner {
        card: agent_card(&cfg),
        tasks: Mutex::new(TaskStore::new(cfg.max_tasks)),
        contexts: tokio::sync::Mutex::new(HashMap::new()),
        factory,
        draining: AtomicBool::new(false),
        slots: Arc::new(Semaphore::new(cfg.max_concurrent_tasks.max(1))),
        max_slots: cfg.max_concurrent_tasks.max(1),
        cfg,
        hosts,
    });
    spawn_reaper(st.clone());
    let app = Router::new()
        .route("/.well-known/agent-card.json", get(card))
        .route("/.well-known/agent.json", get(card))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/a2a", post(rpc))
        .layer(middleware::from_fn_with_state(st.clone(), guard))
        .layer(middleware::from_fn_with_state(st.clone(), request_timeout))
        .layer(middleware::from_fn(request_id))
        .with_state(st.clone());
    (app, st)
}

/// Liveness: the process is up and serving. No auth, no detail.
async fn healthz() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

/// Readiness: not shutting down and a Chrome binary can be found to launch. No auth, no detail.
async fn readyz(State(st): State<St>) -> Response {
    let ready = !st.draining.load(Ordering::Relaxed)
        && ostler_cdp::find_chrome(st.cfg.chrome_path.as_deref()).is_ok();
    let (code, status) = if ready {
        (StatusCode::OK, "ready")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "not_ready")
    };
    (code, Json(json!({ "status": status }))).into_response()
}

pub async fn serve(cfg: A2aConfig, factory: AppFactory) -> std::io::Result<()> {
    if !cfg.bind.ip().is_loopback() && cfg.token.is_none() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "refusing to listen on a non-loopback address without a bearer token (--a2a-token)",
        ));
    }
    let listener = tokio::net::TcpListener::bind(cfg.bind).await?;
    eprintln!(
        "ostler A2A listening on http://{} (card: /.well-known/agent-card.json)",
        listener.local_addr()?
    );
    serve_on(listener, cfg, factory, os_signal()).await
}

async fn os_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut term) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

/// Serve on an already-bound listener until `shutdown` completes, then stop accepting, let in-flight
/// requests and tasks finish for up to `cfg.drain_timeout`, abort whatever is left and close every
/// browser. `serve` wires this to SIGTERM/SIGINT.
pub async fn serve_on(
    listener: tokio::net::TcpListener,
    cfg: A2aConfig,
    factory: AppFactory,
    shutdown: impl Future<Output = ()>,
) -> std::io::Result<()> {
    use hyper_util::rt::{TokioIo, TokioTimer};
    use hyper_util::server::graceful::GracefulShutdown;
    use hyper_util::service::TowerToHyperService;

    let (header_timeout, drain) = (cfg.header_read_timeout, cfg.drain_timeout);
    let (app, st) = build(cfg, factory);
    let mut http = hyper::server::conn::http1::Builder::new();
    http.timer(TokioTimer::new())
        .header_read_timeout(header_timeout);
    let graceful = GracefulShutdown::new();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                };
                let conn = http.serve_connection(
                    TokioIo::new(stream),
                    TowerToHyperService::new(app.clone()),
                );
                let conn = graceful.watch(conn);
                tokio::spawn(async move {
                    let _ = conn.await;
                });
            }
        }
    }
    st.draining.store(true, Ordering::Relaxed);
    drop(listener);
    eprintln!(
        "ostler A2A shutting down: draining in-flight work for up to {}s",
        drain.as_secs()
    );
    let drained = tokio::time::timeout(drain, async {
        graceful.shutdown().await;
        while st.slots.available_permits() < st.max_slots {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    if drained.is_err() {
        eprintln!("ostler A2A: drain deadline reached, aborting remaining tasks");
    }
    for t in lock_tasks(&st).map.values_mut() {
        if let Some(h) = t.abort.take() {
            h.abort();
        }
    }
    let ctxs: Vec<Arc<Ctx>> = st.contexts.lock().await.drain().map(|(_, c)| c).collect();
    for c in ctxs {
        c.app.shutdown().await;
    }
    Ok(())
}

fn spawn_reaper(st: St) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        loop {
            tick.tick().await;
            reap(&st).await;
        }
    });
}

/// Close sessions nobody has used for a while; each holds a Chrome process.
async fn reap(st: &St) {
    let idle: Vec<(String, Arc<Ctx>)> = {
        let map = st.contexts.lock().await;
        map.iter()
            .filter(|(_, c)| c.idle_for() > st.cfg.idle && c.run_lock.try_lock().is_ok())
            .map(|(k, c)| (k.clone(), c.clone()))
            .collect()
    };
    for (id, ctx) in idle {
        st.contexts.lock().await.remove(&id);
        ctx.app.shutdown().await;
    }
}

async fn card(State(st): State<St>) -> impl IntoResponse {
    (
        [(header::CACHE_CONTROL, "max-age=60")],
        Json(st.card.clone()),
    )
}

async fn context(st: &St, id: &str) -> Result<Arc<Ctx>, RpcErr> {
    {
        let map = st.contexts.lock().await;
        if let Some(c) = map.get(id) {
            c.touch();
            return Ok(c.clone());
        }
    }
    reap(st).await;
    let mut map = st.contexts.lock().await;
    if let Some(c) = map.get(id) {
        return Ok(c.clone());
    }
    if map.len() >= st.cfg.max_contexts {
        return Err(RpcErr::busy(
            StatusCode::SERVICE_UNAVAILABLE,
            15,
            "TOO_MANY_CONTEXTS",
            format!(
                "server is at its limit of {} concurrent conversations; retry later",
                st.cfg.max_contexts
            ),
        ));
    }
    let ctx = Arc::new(Ctx::new((st.factory)()));
    map.insert(id.to_string(), ctx.clone());
    Ok(ctx)
}

/// Read the body within the size and time limits. The error is a ready-to-send response.
#[allow(clippy::result_large_err)]
async fn read_body(st: &St, headers: &HeaderMap, body: Body, rid: &str) -> Result<Bytes, Response> {
    let max = st.cfg.max_body_bytes;
    let too_big = || {
        let mut r = jerr(
            rid,
            rpc_error(
                &Value::Null,
                INVALID_REQUEST,
                &format!("request body exceeds the {max}-byte limit"),
                "PAYLOAD_TOO_LARGE",
                json!({ "maxBytes": max }),
            ),
        );
        *r.status_mut() = StatusCode::PAYLOAD_TOO_LARGE;
        r
    };
    if headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
        .is_some_and(|n| n > max)
    {
        return Err(too_big());
    }
    match tokio::time::timeout(st.cfg.body_timeout, axum::body::to_bytes(body, max)).await {
        Ok(Ok(b)) => Ok(b),
        Ok(Err(e)) => {
            let mut src: Option<&dyn std::error::Error> = Some(&e);
            while let Some(s) = src {
                if s.to_string().contains("length limit") {
                    return Err(too_big());
                }
                src = s.source();
            }
            let mut r = jerr(
                rid,
                rpc_error(
                    &Value::Null,
                    INVALID_REQUEST,
                    "could not read request body",
                    "BAD_BODY",
                    json!({}),
                ),
            );
            *r.status_mut() = StatusCode::BAD_REQUEST;
            Err(r)
        }
        Err(_) => Err(forbid(
            StatusCode::REQUEST_TIMEOUT,
            "timed out reading request body",
            rid,
        )),
    }
}

async fn rpc(
    State(st): State<St>,
    Extension(RequestId(rid)): Extension<RequestId>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let body = match read_body(&st, &headers, body, &rid).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let req: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => {
            return jerr(
                &rid,
                rpc_error(
                    &Value::Null,
                    PARSE_ERROR,
                    "Invalid JSON payload",
                    "PARSE_ERROR",
                    json!({}),
                ),
            )
        }
    };
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    if !req.is_object() || req.get("jsonrpc") != Some(&json!("2.0")) {
        return jerr(
            &rid,
            rpc_error(
                &id,
                INVALID_REQUEST,
                "Request payload validation error",
                "INVALID_REQUEST",
                json!({}),
            ),
        );
    }
    if let Some(v) = headers.get("a2a-version").and_then(|v| v.to_str().ok()) {
        if !v.trim().is_empty() && !v.trim().starts_with("1.") {
            let mut e = RpcErr::new(
                VERSION_NOT_SUPPORTED,
                "VERSION_NOT_SUPPORTED",
                "Unsupported A2A version",
            );
            e.meta = json!({ "requested": v, "supported": VERSION });
            return e.into_response(&id, &rid);
        }
    }
    let params = req.get("params").cloned().unwrap_or(Value::Null);
    let Some(method) = req.get("method").and_then(Value::as_str) else {
        return jerr(
            &rid,
            rpc_error(
                &id,
                INVALID_REQUEST,
                "method is required",
                "INVALID_REQUEST",
                json!({}),
            ),
        );
    };
    let result = match method {
        "SendMessage" => send(&st, &params, false).await.map(|o| match o {
            Out::Json(v) => v,
            Out::Stream(..) => json!({}),
        }),
        "SendStreamingMessage" => match send(&st, &params, true).await {
            Ok(Out::Stream(task_id, rx)) => return sse(&st, id, task_id, rx).into_response(),
            Ok(Out::Json(v)) => Ok(v),
            Err(e) => Err(e),
        },
        "SubscribeToTask" => match subscribe(&st, &params) {
            Ok((task_id, rx)) => return sse(&st, id, task_id, rx).into_response(),
            Err(e) => Err(e),
        },
        "GetTask" => get_task(&st, &params),
        "ListTasks" => list_tasks(&st, &params),
        "CancelTask" => cancel(&st, &params),
        "GetExtendedAgentCard" => Err(RpcErr::new(
            EXTENDED_CARD_NOT_CONFIGURED,
            "EXTENDED_AGENT_CARD_NOT_CONFIGURED",
            "No extended Agent Card is configured",
        )),
        m if m.contains("PushNotification") => Err(RpcErr::new(
            PUSH_NOT_SUPPORTED,
            "PUSH_NOTIFICATION_NOT_SUPPORTED",
            "Push notifications are not supported",
        )),
        _ => Err(RpcErr::new(
            METHOD_NOT_FOUND,
            "METHOD_NOT_FOUND",
            "Method not found",
        )),
    };
    match result {
        Ok(v) => Json(rpc_ok(&id, v)).into_response(),
        Err(e) => e.into_response(&id, &rid),
    }
}

enum Out {
    Json(Value),
    Stream(String, watch::Receiver<u64>),
}

fn lock_tasks(st: &St) -> std::sync::MutexGuard<'_, TaskStore> {
    st.tasks.lock().unwrap_or_else(|e| e.into_inner())
}

fn get_task(st: &St, p: &Value) -> Result<Value, RpcErr> {
    let id = p
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcErr::params("id is required"))?;
    let hl = p
        .get("historyLength")
        .and_then(Value::as_u64)
        .map(|n| n as usize);
    let tasks = lock_tasks(st);
    tasks
        .map
        .get(id)
        .map(|t| t.json(hl))
        .ok_or_else(|| RpcErr::task_not_found(id))
}

fn list_tasks(st: &St, p: &Value) -> Result<Value, RpcErr> {
    let ctx = p.get("contextId").and_then(Value::as_str);
    let state = p.get("status").and_then(Value::as_str);
    let size = p
        .get("pageSize")
        .and_then(Value::as_u64)
        .unwrap_or(50)
        .clamp(1, 100) as usize;
    let tasks = lock_tasks(st);
    let mut all: Vec<&TaskRec> = tasks
        .map
        .values()
        .filter(|t| ctx.is_none_or(|c| t.context_id == c) && state.is_none_or(|s| t.state == s))
        .collect();
    all.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    let total = all.len();
    Ok(json!({
        "tasks": all.into_iter().take(size).map(|t| t.json(None)).collect::<Vec<_>>(),
        "nextPageToken": "",
        "totalSize": total,
    }))
}

fn cancel(st: &St, p: &Value) -> Result<Value, RpcErr> {
    let id = p
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcErr::params("id is required"))?;
    let mut tasks = lock_tasks(st);
    let t = tasks
        .map
        .get_mut(id)
        .ok_or_else(|| RpcErr::task_not_found(id))?;
    if is_terminal(t.state) {
        let mut e = RpcErr::new(
            TASK_NOT_CANCELABLE,
            "TASK_NOT_CANCELABLE",
            "Task is already finished",
        );
        e.meta = json!({ "taskId": id, "state": t.state });
        return Err(e);
    }
    if let Some(h) = t.abort.take() {
        h.abort();
    }
    t.set_state(CANCELED, "Canceled by the caller.");
    Ok(t.json(None))
}

fn subscribe(st: &St, p: &Value) -> Result<(String, watch::Receiver<u64>), RpcErr> {
    let id = p
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcErr::params("id is required"))?;
    let tasks = lock_tasks(st);
    let t = tasks
        .map
        .get(id)
        .ok_or_else(|| RpcErr::task_not_found(id))?;
    if is_terminal(t.state) {
        let mut e = RpcErr::new(
            UNSUPPORTED_OPERATION,
            "UNSUPPORTED_OPERATION",
            "Task is already finished",
        );
        e.meta = json!({ "taskId": id, "state": t.state });
        return Err(e);
    }
    Ok((id.to_string(), t.tx.subscribe()))
}

async fn send(st: &St, p: &Value, streaming: bool) -> Result<Out, RpcErr> {
    let msg = p
        .get("message")
        .ok_or_else(|| RpcErr::params("message is required"))?;
    if msg
        .get("parts")
        .and_then(Value::as_array)
        .is_none_or(|a| a.is_empty())
    {
        return Err(RpcErr::params(
            "message.parts must contain at least one part",
        ));
    }
    let return_now = p
        .pointer("/configuration/returnImmediately")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let given_task = msg.get("taskId").and_then(Value::as_str);
    let mut context_id = msg
        .get("contextId")
        .and_then(Value::as_str)
        .map(String::from);

    // Continuing a task: it must exist and still be open.
    if let Some(tid) = given_task {
        let tasks = lock_tasks(st);
        let t = tasks
            .map
            .get(tid)
            .ok_or_else(|| RpcErr::task_not_found(tid))?;
        if is_terminal(t.state) {
            let mut e = RpcErr::new(
                UNSUPPORTED_OPERATION,
                "UNSUPPORTED_OPERATION",
                "Task is finished and cannot accept more messages",
            );
            e.meta = json!({ "taskId": tid, "state": t.state });
            return Err(e);
        }
        context_id = Some(t.context_id.clone());
    }
    let context_id = context_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let parsed = intent::parse(msg);
    if !parsed.understood {
        return Ok(Out::Json(
            json!({ "message": proto::agent_message(intent::HELP, &context_id, given_task) }),
        ));
    }
    let mut task = parsed.task;
    if st.cfg.auto_submit && task.submit {
        task.confirm = true;
    }

    if st.draining.load(Ordering::Relaxed) {
        return Err(RpcErr::busy(
            StatusCode::SERVICE_UNAVAILABLE,
            5,
            "SHUTTING_DOWN",
            "server is shutting down and not accepting new work",
        ));
    }
    // Bounded concurrency: refuse rather than queue without limit. The permit is held by the
    // spawned task (waiting on its conversation's turn counts) and freed when it ends or is aborted.
    let permit = st.slots.clone().try_acquire_owned().map_err(|_| {
        RpcErr::busy(
            StatusCode::TOO_MANY_REQUESTS,
            5,
            "TOO_MANY_TASKS_IN_FLIGHT",
            format!(
                "server is already running its limit of {} concurrent tasks; retry later",
                st.max_slots
            ),
        )
    })?;
    let ctx = context(st, &context_id).await?;
    let (task_id, rx) = {
        let mut tasks = lock_tasks(st);
        let tid = match given_task {
            Some(t) => t.to_string(),
            None => {
                let rec = TaskRec::new(&context_id);
                let id = rec.id.clone();
                tasks
                    .insert(rec)
                    .map_err(|m| RpcErr::new(INTERNAL, "TOO_MANY_TASKS", m))?;
                id
            }
        };
        let rec = tasks.map.get_mut(&tid).expect("task just ensured");
        rec.history.push(msg.clone());
        rec.want_submit |= task.submit;
        // A bare {confirm:true} reply completes the submission that was asked for earlier.
        task.submit |= rec.want_submit;
        // Artifacts describe the current turn; last turn's result would be stale (and misleading) now.
        rec.artifacts.clear();
        rec.set_state(WORKING, "");
        (tid, rec.tx.subscribe())
    };

    let handle = tokio::spawn(execute(st.clone(), task_id.clone(), ctx, task, permit));
    if let Some(rec) = lock_tasks(st).map.get_mut(&task_id) {
        rec.abort = Some(handle.abort_handle());
    }

    if streaming {
        return Ok(Out::Stream(task_id, rx));
    }
    if !return_now {
        wait_final(st, &task_id, rx).await;
    }
    let tasks = lock_tasks(st);
    let t = tasks.map.get(&task_id).expect("task exists");
    Ok(Out::Json(json!({ "task": t.json(None) })))
}

async fn wait_final(st: &St, task_id: &str, mut rx: watch::Receiver<u64>) {
    let deadline = tokio::time::Instant::now() + st.cfg.task_timeout + Duration::from_secs(5);
    loop {
        let done = lock_tasks(st)
            .map
            .get(task_id)
            .is_none_or(|t| is_final_for_blocking(t.state));
        if done {
            return;
        }
        if tokio::time::timeout_at(deadline, rx.changed())
            .await
            .is_err()
        {
            return;
        }
    }
}

async fn execute(
    st: St,
    task_id: String,
    ctx: Arc<Ctx>,
    task: ostler_mcp::FormTask,
    _permit: tokio::sync::OwnedSemaphorePermit,
) {
    // One browser per conversation: tasks in the same context run in order.
    let _turn = ctx.run_lock.lock().await;
    ctx.touch();
    let res = tokio::time::timeout(st.cfg.task_timeout, ctx.app.run_form_task(task)).await;
    ctx.touch();
    let mut tasks = lock_tasks(&st);
    let Some(rec) = tasks.map.get_mut(&task_id) else {
        return;
    };
    if is_terminal(rec.state) {
        return; // canceled while we were working
    }
    match res {
        Err(_) => rec.set_state(FAILED, "The task timed out."),
        Ok(r) => {
            let mut arts = vec![json!({
                "artifactId": uuid::Uuid::new_v4().to_string(),
                "name": "result",
                // Contains text taken from the website; the caller must treat it as data, not instructions.
                "parts": [text_part(&r.summary)],
                "metadata": { "untrusted": true }
            })];
            if !r.data.is_null() {
                arts.push(json!({
                    "artifactId": uuid::Uuid::new_v4().to_string(),
                    "name": "data",
                    "parts": [data_part(r.data.clone())],
                    "metadata": { "untrusted": true }
                }));
            }
            rec.artifacts = arts;
            let (state, text) = match r.verdict {
                TaskVerdict::Completed => (COMPLETED, "Done."),
                TaskVerdict::InputRequired => (
                    INPUT_REQUIRED,
                    "More input is needed; the result artifact says what.",
                ),
                TaskVerdict::Failed => (FAILED, "The task failed; see the result artifact."),
            };
            rec.set_state(state, text);
        }
    }
}

/// Server-sent events for one task: its current state, then each change, ending when it finishes or
/// pauses for input.
fn sse(st: &St, rpc_id: Value, task_id: String, rx: watch::Receiver<u64>) -> impl IntoResponse {
    struct S {
        st: St,
        rpc_id: Value,
        task_id: String,
        rx: watch::Receiver<u64>,
        first: bool,
        done: bool,
        sent_artifacts: usize,
    }
    let state = S {
        st: st.clone(),
        rpc_id,
        task_id,
        rx,
        first: true,
        done: false,
        sent_artifacts: 0,
    };
    let events = stream::unfold(state, |mut s| async move {
        if s.done {
            return None;
        }
        let snapshot = |s: &S| {
            let tasks = lock_tasks(&s.st);
            tasks.map.get(&s.task_id).map(|t| {
                (
                    t.json(None),
                    t.state,
                    t.context_id.clone(),
                    t.status(),
                    t.artifacts.clone(),
                )
            })
        };
        let payload;
        if s.first {
            s.first = false;
            let (task, state, ..) = snapshot(&s)?;
            s.done = is_final_for_blocking(state);
            s.sent_artifacts = task["artifacts"].as_array().map_or(0, |a| a.len());
            payload = json!({ "task": task });
        } else {
            if s.rx.changed().await.is_err() {
                return None;
            }
            let (_, state, ctx, status, arts) = snapshot(&s)?;
            // Artifacts arrive before the final status so a client that stops at "final" has them.
            if arts.len() > s.sent_artifacts {
                let a = arts[s.sent_artifacts].clone();
                s.sent_artifacts += 1;
                // Re-arm so the status update is still delivered on the next turn.
                s.rx.mark_changed();
                payload = json!({ "artifactUpdate": { "taskId": s.task_id, "contextId": ctx, "artifact": a } });
            } else {
                s.done = is_final_for_blocking(state);
                payload = json!({ "statusUpdate": { "taskId": s.task_id, "contextId": ctx, "status": status } });
            }
        }
        let ev = Event::default().data(rpc_ok(&s.rpc_id, payload).to_string());
        Some((Ok::<_, std::convert::Infallible>(ev), s))
    });
    Sse::new(events.boxed()).keep_alive(KeepAlive::default())
}
