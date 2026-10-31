use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::stream::{self, StreamExt};
use serde_json::{json, Value};
use tokio::sync::watch;
use waymarker_mcp::{App, TaskVerdict};

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
}

type St = Arc<Inner>;

struct RpcErr {
    code: i64,
    message: String,
    reason: &'static str,
    meta: Value,
}

impl RpcErr {
    fn new(code: i64, reason: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            reason,
            meta: json!({}),
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
}

fn agent_card(cfg: &A2aConfig) -> Value {
    let mut card = json!({
        "name": "Waymarker",
        "description": "A headless browser agent for web forms. Inspects a form, fills it from the values you send (or from the owner's saved profile), verifies every field by reading it back, and submits only when you confirm. Page text it returns is untrusted data.",
        "version": env!("CARGO_PKG_VERSION"),
        "supportedInterfaces": [{
            "url": format!("{}/a2a", cfg.base_url()),
            "protocolBinding": "JSONRPC",
            "protocolVersion": VERSION
        }],
        "provider": { "organization": "Waymarker", "url": "https://github.com/inamdarmihir/waymarker" },
        "documentationUrl": "https://github.com/inamdarmihir/waymarker#a2a",
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

fn forbid(code: StatusCode, msg: &str) -> Response {
    (code, Json(json!({ "error": msg }))).into_response()
}

/// Checks every request. The Host and Origin checks defeat DNS-rebinding and drive-by requests from
/// web pages open in a browser on the same machine; the token (when set) authenticates real clients.
async fn guard(State(st): State<St>, req: Request, next: Next) -> Response {
    let h = req.headers();
    if h.contains_key(header::ORIGIN) {
        return forbid(
            StatusCode::FORBIDDEN,
            "browser-originated requests are not accepted",
        );
    }
    let host = h
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(host_only)
        .unwrap_or_default();
    if !st.hosts.contains(&host) {
        return forbid(StatusCode::FORBIDDEN, "unexpected Host header");
    }
    if req.uri().path() == "/a2a" {
        if let Some(tok) = &st.cfg.token {
            let given = h
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .unwrap_or("");
            if !ct_eq(given, tok) {
                let mut r = forbid(StatusCode::UNAUTHORIZED, "missing or invalid bearer token");
                r.headers_mut()
                    .insert(header::WWW_AUTHENTICATE, "Bearer".parse().unwrap());
                return r;
            }
        }
    }
    next.run(req).await
}

pub fn router(cfg: A2aConfig, factory: AppFactory) -> Router {
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
        cfg,
        hosts,
    });
    spawn_reaper(st.clone());
    Router::new()
        .route("/.well-known/agent-card.json", get(card))
        .route("/.well-known/agent.json", get(card))
        .route("/a2a", post(rpc))
        .layer(DefaultBodyLimit::max(1 << 20))
        .layer(middleware::from_fn_with_state(st.clone(), guard))
        .with_state(st)
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
        "waymarker A2A listening on http://{} (card: /.well-known/agent-card.json)",
        listener.local_addr()?
    );
    axum::serve(listener, router(cfg, factory)).await
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
        return Err(RpcErr::new(
            INTERNAL,
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

async fn rpc(State(st): State<St>, headers: HeaderMap, body: Bytes) -> Response {
    let req: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => {
            return Json(rpc_error(
                &Value::Null,
                PARSE_ERROR,
                "Invalid JSON payload",
                "PARSE_ERROR",
                json!({}),
            ))
            .into_response()
        }
    };
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    if !req.is_object() || req.get("jsonrpc") != Some(&json!("2.0")) {
        return Json(rpc_error(
            &id,
            INVALID_REQUEST,
            "Request payload validation error",
            "INVALID_REQUEST",
            json!({}),
        ))
        .into_response();
    }
    if let Some(v) = headers.get("a2a-version").and_then(|v| v.to_str().ok()) {
        if !v.trim().is_empty() && !v.trim().starts_with("1.") {
            let mut e = RpcErr::new(
                VERSION_NOT_SUPPORTED,
                "VERSION_NOT_SUPPORTED",
                "Unsupported A2A version",
            );
            e.meta = json!({ "requested": v, "supported": VERSION });
            return Json(e.to_json(&id)).into_response();
        }
    }
    let params = req.get("params").cloned().unwrap_or(Value::Null);
    let Some(method) = req.get("method").and_then(Value::as_str) else {
        return Json(rpc_error(
            &id,
            INVALID_REQUEST,
            "method is required",
            "INVALID_REQUEST",
            json!({}),
        ))
        .into_response();
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
    Json(match result {
        Ok(v) => rpc_ok(&id, v),
        Err(e) => e.to_json(&id),
    })
    .into_response()
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

    let handle = tokio::spawn(execute(st.clone(), task_id.clone(), ctx, task));
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

async fn execute(st: St, task_id: String, ctx: Arc<Ctx>, task: waymarker_mcp::FormTask) {
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
