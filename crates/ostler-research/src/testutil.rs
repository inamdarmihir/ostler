//! Test-only mock HTTP server on `tiny_http`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// A canned response.
pub(crate) struct Resp {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub delay_ms: u64,
}

impl Resp {
    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into(),
            delay_ms: 0,
        }
    }
    pub fn json(body: &str) -> Self {
        Self::new(200, body).header("Content-Type", "application/json")
    }
    pub fn header(mut self, k: &str, v: &str) -> Self {
        self.headers.push((k.into(), v.into()));
        self
    }
    pub fn delay(mut self, ms: u64) -> Self {
        self.delay_ms = ms;
        self
    }
}

/// One request the mock saw.
#[derive(Debug, Clone)]
pub(crate) struct Seen {
    pub url: String,
    pub headers: Vec<(String, String)>,
}

impl Seen {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Running mock server; stops when dropped.
pub(crate) struct Mock {
    /// `http://127.0.0.1:port`
    pub base: String,
    pub seen: Arc<Mutex<Vec<Seen>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Mock {
    pub fn requests(&self) -> Vec<Seen> {
        self.seen.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Start a server answering every request with `handler(request path+query)`.
pub(crate) fn serve<F>(handler: F) -> Mock
where
    F: Fn(&str) -> Resp + Send + 'static,
{
    let server = tiny_http::Server::http("127.0.0.1:0").expect("bind mock server");
    let port = server.server_addr().to_ip().expect("ip addr").port();
    let stop = Arc::new(AtomicBool::new(false));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (stop2, seen2) = (stop.clone(), seen.clone());
    let thread = std::thread::spawn(move || {
        while !stop2.load(Ordering::SeqCst) {
            let Ok(Some(req)) = server.recv_timeout(Duration::from_millis(50)) else {
                continue;
            };
            let url = req.url().to_owned();
            let headers = req
                .headers()
                .iter()
                .map(|h| (h.field.to_string(), h.value.to_string()))
                .collect();
            if let Ok(mut g) = seen2.lock() {
                g.push(Seen {
                    url: url.clone(),
                    headers,
                });
            }
            let r = handler(&url);
            if r.delay_ms > 0 {
                std::thread::sleep(Duration::from_millis(r.delay_ms));
            }
            let mut resp = tiny_http::Response::from_data(r.body).with_status_code(r.status);
            for (k, v) in r.headers {
                if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
                    resp.add_header(h);
                }
            }
            let _ = req.respond(resp);
        }
    });
    Mock {
        base: format!("http://127.0.0.1:{port}"),
        seen,
        stop,
        thread: Some(thread),
    }
}
