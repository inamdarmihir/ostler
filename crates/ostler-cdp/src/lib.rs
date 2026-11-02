//! Minimal async Chrome DevTools Protocol client.
//!
//! One connection to the browser (a private pipe by default, a WebSocket as fallback),
//! many flattened target sessions.
//! Requests are matched to responses by id; events are fanned out on a
//! broadcast channel so any number of waiters can observe them.

mod launch;

pub use launch::{
    find_chrome, lean_args, memory_method, process_tree_rss_kb, process_tree_stats, Chrome,
    LaunchOptions,
};

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

#[derive(Debug, thiserror::Error)]
pub enum CdpError {
    #[error(
        "could not find Chrome or Chromium. Install one \
         (macOS: `brew install --cask google-chrome`; Debian/Ubuntu: `sudo apt install chromium`; \
         Windows: https://www.google.com/chrome) or point Ostler at it with OSTLER_CHROME=/path/to/chrome"
    )]
    ChromeNotFound,
    #[error("the Chrome path {0:?} (from --chrome-path or OSTLER_CHROME) does not exist; fix it or unset it to auto-detect")]
    ChromePathMissing(String),
    #[error("failed to launch browser: {0}")]
    Launch(String),
    #[error("websocket error: {0}")]
    Ws(String),
    #[error("CDP error {code}: {message}")]
    Protocol { code: i64, message: String },
    #[error("timed out after {0:?} waiting for {1}")]
    Timeout(Duration, String),
    #[error("connection closed")]
    Closed,
    #[error("unexpected response: {0}")]
    Unexpected(String),
}

pub type Result<T> = std::result::Result<T, CdpError>;

/// A protocol event, optionally scoped to a target session.
#[derive(Debug, Clone)]
pub struct Event {
    pub method: String,
    pub params: Value,
    pub session_id: Option<String>,
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>;

/// Messages larger than this are not kept as spare buffer capacity between pipe reads.
#[cfg(unix)]
const PIPE_BUF_KEEP: usize = 16 << 20;

pub struct Connection {
    /// Serialized CDP messages to send; a transport task frames and writes them.
    out: mpsc::UnboundedSender<String>,
    pending: Pending,
    next_id: AtomicU64,
    calls: AtomicU64,
    events: broadcast::Sender<Event>,
    /// Set as soon as the transport is gone, so callers fail at once instead of waiting out the timeout.
    closed: Arc<AtomicBool>,
}

/// Route one decoded message from the browser to its waiting request or to event subscribers.
fn dispatch(v: Value, pending: &Pending, events: &broadcast::Sender<Event>) {
    if let Some(id) = v.get("id").and_then(Value::as_u64) {
        let tx = pending.lock().unwrap().remove(&id);
        if let Some(tx) = tx {
            let res = match v.get("error") {
                Some(e) => Err(CdpError::Protocol {
                    code: e.get("code").and_then(Value::as_i64).unwrap_or(-1),
                    message: e
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_string(),
                }),
                None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
            };
            let _ = tx.send(res);
        }
    } else if let Some(method) = v.get("method").and_then(Value::as_str) {
        let _ = events.send(Event {
            method: method.to_string(),
            params: v.get("params").cloned().unwrap_or(Value::Null),
            session_id: v.get("sessionId").and_then(Value::as_str).map(String::from),
        });
    }
}

/// Transport gone: refuse new requests first, then fail every in-flight one.
fn fail_all(closed: &AtomicBool, pending: &Pending) {
    closed.store(true, Ordering::SeqCst);
    let drained: Vec<_> = pending.lock().unwrap().drain().collect();
    for (_, tx) in drained {
        let _ = tx.send(Err(CdpError::Closed));
    }
}

impl Connection {
    fn assemble(
        out: mpsc::UnboundedSender<String>,
        pending: Pending,
        events: broadcast::Sender<Event>,
        closed: Arc<AtomicBool>,
    ) -> Arc<Self> {
        Arc::new(Self {
            out,
            pending,
            next_id: AtomicU64::new(1),
            calls: AtomicU64::new(0),
            events,
            closed,
        })
    }

    /// Connect over a WebSocket (`--remote-debugging-port`). Fallback transport; the default is a pipe.
    pub async fn connect(ws_url: &str) -> Result<Arc<Self>> {
        // Chrome sends big messages (a full-page PNG is one base64 string); the default 16/64 MiB
        // limits would kill the whole connection on a tall page.
        let cfg = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
            .max_message_size(Some(512 << 20))
            .max_frame_size(Some(512 << 20));
        let (ws, _) = tokio_tungstenite::connect_async_with_config(ws_url, Some(cfg), false)
            .await
            .map_err(|e| CdpError::Ws(e.to_string()))?;
        let (mut sink, mut stream) = ws.split();
        let (out, mut out_rx) = mpsc::unbounded_channel::<String>();
        let pending: Pending = Arc::default();
        let (events, _) = broadcast::channel(1024);

        tokio::spawn(async move {
            while let Some(msg) = out_rx.recv().await {
                if sink.send(Message::Text(msg.into())).await.is_err() {
                    break;
                }
            }
        });

        let closed = Arc::new(AtomicBool::new(false));
        let reader_closed = closed.clone();
        let reader_pending = pending.clone();
        let reader_events = events.clone();
        tokio::spawn(async move {
            while let Some(Ok(msg)) = stream.next().await {
                let Message::Text(text) = msg else { continue };
                let Ok(v) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                dispatch(v, &reader_pending, &reader_events);
            }
            fail_all(&reader_closed, &reader_pending);
        });

        Ok(Self::assemble(out, pending, events, closed))
    }

    /// Speak CDP over Chrome's `--remote-debugging-pipe`: Chrome reads commands from `writer`'s
    /// peer and writes responses and events to `reader`'s peer. Every message is UTF-8 JSON
    /// terminated by a single NUL byte.
    #[cfg(unix)]
    pub(crate) fn from_pipe(
        reader: tokio::net::unix::pipe::Receiver,
        mut writer: tokio::net::unix::pipe::Sender,
    ) -> Arc<Self> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let (out, mut out_rx) = mpsc::unbounded_channel::<String>();
        let pending: Pending = Arc::default();
        let (events, _) = broadcast::channel(1024);

        // Ends when the connection is dropped or Chrome stops reading. Dropping `writer` closes
        // Chrome's stdin-equivalent, which makes Chrome shut itself down.
        tokio::spawn(async move {
            while let Some(msg) = out_rx.recv().await {
                let mut frame = msg.into_bytes();
                frame.push(0);
                if writer.write_all(&frame).await.is_err() {
                    break;
                }
            }
        });

        let closed = Arc::new(AtomicBool::new(false));
        let reader_closed = closed.clone();
        let reader_pending = pending.clone();
        let reader_events = events.clone();
        tokio::spawn(async move {
            // A full-page PNG is one multi-MB frame, so the buffer grows to fit the frame.
            let mut reader = BufReader::new(reader);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match reader.read_until(0, &mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                // EOF mid-frame (no trailing NUL) means Chrome died while writing: drop the fragment.
                if buf.pop() != Some(0) {
                    break;
                }
                if let Ok(v) = serde_json::from_slice::<Value>(&buf) {
                    dispatch(v, &reader_pending, &reader_events);
                }
                if buf.capacity() > PIPE_BUF_KEEP {
                    buf = Vec::new();
                }
            }
            fail_all(&reader_closed, &reader_pending);
        });

        Self::assemble(out, pending, events, closed)
    }

    /// True once the browser connection has dropped (Chrome crashed or was killed).
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Total protocol round-trips issued on this connection (a latency/cost proxy).
    pub fn call_count(&self) -> u64 {
        self.calls.load(Ordering::Relaxed)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    /// Send a command and wait for its result.
    pub async fn call(
        &self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
        timeout: Duration,
    ) -> Result<Value> {
        if self.is_closed() {
            return Err(CdpError::Closed);
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.calls.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        // The reader may have drained `pending` between the check above and the insert.
        if self.is_closed() {
            self.pending.lock().unwrap().remove(&id);
            return Err(CdpError::Closed);
        }
        let mut msg = json!({ "id": id, "method": method, "params": params });
        if let Some(s) = session_id {
            msg["sessionId"] = json!(s);
        }
        self.out
            .send(msg.to_string())
            .map_err(|_| CdpError::Closed)?;
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(res)) => res,
            Ok(Err(_)) => Err(CdpError::Closed),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(CdpError::Timeout(timeout, method.to_string()))
            }
        }
    }
}

/// A handle bound to one attached target (page).
#[derive(Clone)]
pub struct Session {
    conn: Arc<Connection>,
    session_id: String,
    pub target_id: String,
}

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

impl Session {
    /// Create a blank page target and attach to it with a flattened session.
    pub async fn new_page(conn: Arc<Connection>) -> Result<Self> {
        let created = conn
            .call(
                "Target.createTarget",
                json!({ "url": "about:blank" }),
                None,
                DEFAULT_TIMEOUT,
            )
            .await?;
        let target_id = created["targetId"]
            .as_str()
            .ok_or_else(|| CdpError::Unexpected("no targetId".into()))?
            .to_string();
        Self::attach(conn, target_id).await
    }

    /// Attach to an existing page target (e.g. a popup the page opened).
    pub async fn attach(conn: Arc<Connection>, target_id: String) -> Result<Self> {
        let attached = conn
            .call(
                "Target.attachToTarget",
                json!({ "targetId": target_id, "flatten": true }),
                None,
                DEFAULT_TIMEOUT,
            )
            .await?;
        let session_id = attached["sessionId"]
            .as_str()
            .ok_or_else(|| CdpError::Unexpected("no sessionId".into()))?
            .to_string();
        Ok(Self {
            conn,
            session_id,
            target_id,
        })
    }

    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.call_timeout(method, params, DEFAULT_TIMEOUT).await
    }

    pub async fn call_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value> {
        self.conn
            .call(method, params, Some(&self.session_id), timeout)
            .await
    }

    /// Subscribe to events for this session only.
    pub fn events(&self) -> SessionEvents {
        SessionEvents {
            rx: self.conn.subscribe(),
            session_id: self.session_id.clone(),
        }
    }

    pub fn connection(&self) -> &Arc<Connection> {
        &self.conn
    }
}

pub struct SessionEvents {
    rx: broadcast::Receiver<Event>,
    session_id: String,
}

impl SessionEvents {
    /// Wait for the next event whose method is in `methods`.
    pub async fn wait_for(&mut self, methods: &[&str], timeout: Duration) -> Result<Event> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout(remaining, self.rx.recv()).await {
                Ok(Ok(ev)) => {
                    if ev.session_id.as_deref() == Some(&self.session_id)
                        && methods.contains(&ev.method.as_str())
                    {
                        return Ok(ev);
                    }
                }
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
                Ok(Err(broadcast::error::RecvError::Closed)) => return Err(CdpError::Closed),
                Err(_) => return Err(CdpError::Timeout(timeout, methods.join("|"))),
            }
        }
    }

    /// Next event for this session, or `None` once the connection is gone.
    pub async fn next(&mut self) -> Option<Event> {
        loop {
            match self.rx.recv().await {
                Ok(ev) if ev.session_id.as_deref() == Some(&self.session_id) => return Some(ev),
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }

    /// Non-blocking drain of everything currently buffered for this session.
    pub fn drain(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(ev) if ev.session_id.as_deref() == Some(&self.session_id) => out.push(ev),
                Ok(_) => {}
                Err(broadcast::error::TryRecvError::Lagged(_)) => {}
                Err(_) => break,
            }
        }
        out
    }
}
