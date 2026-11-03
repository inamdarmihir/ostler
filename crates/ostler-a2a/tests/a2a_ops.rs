//! Operational hardening of the HTTP front door: limits, timeouts, backpressure, probes, request ids,
//! graceful shutdown. None of these need Chrome except the backpressure test.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ostler_a2a::{router, serve_on, A2aConfig, AppFactory};
use ostler_browser::{LaunchOptions, Policy};
use ostler_mcp::{App, AppConfig, Profile};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn factory() -> AppFactory {
    Arc::new(|| {
        App::new(AppConfig {
            launch: LaunchOptions::default(),
            policy: Policy::default(),
            profile: Profile::All,
            memory: None,
            learn: false,
            require_confirm_submit: false,
            verbose: false,
        })
    })
}

/// Serve with the real connection loop (header timeouts live there), stopping on `stop`.
async fn start(
    cfg: impl FnOnce(&mut A2aConfig),
) -> (
    String,
    SocketAddr,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<std::io::Result<()>>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mut c = A2aConfig::new(addr);
    cfg(&mut c);
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let h = tokio::spawn(serve_on(listener, c, factory(), async {
        let _ = rx.await;
    }));
    (format!("http://{addr}"), addr, tx, h)
}

fn rpc_body(method: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0","id":1,"method":method,"params":params})
}

#[tokio::test]
async fn probes_need_no_auth_and_leak_nothing() {
    let (base, _, _tx, _h) = start(|c| c.token = Some("secret".into())).await;
    let http = reqwest::Client::new();
    let r = http.get(format!("{base}/healthz")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.json::<Value>().await.unwrap(), json!({"status":"ok"}));
    let r = http.get(format!("{base}/readyz")).send().await.unwrap();
    let code = r.status().as_u16();
    let body: Value = r.json().await.unwrap();
    // Ready iff Chrome is installed here; either way the body is a single status word.
    assert_eq!(
        (code, body["status"].as_str().unwrap()),
        if ostler_cdp::find_chrome(None).is_ok() {
            (200, "ready")
        } else {
            (503, "not_ready")
        }
    );
    assert_eq!(body.as_object().unwrap().len(), 1);
    // The RPC endpoint still needs the token.
    let r = http
        .post(format!("{base}/a2a"))
        .json(&rpc_body("GetTask", json!({"id":"x"})))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
}

#[tokio::test]
async fn readyz_is_unavailable_without_a_launchable_browser() {
    let (base, _, _tx, _h) = start(|c| c.chrome_path = Some("/nonexistent/chrome".into())).await;
    let r = reqwest::get(format!("{base}/readyz")).await.unwrap();
    assert_eq!(r.status(), 503);
}

#[tokio::test]
async fn every_response_carries_a_request_id_and_errors_repeat_it() {
    let (base, _, _tx, _h) = start(|c| c.token = Some("secret".into())).await;
    let http = reqwest::Client::new();
    let r = http.get(format!("{base}/healthz")).send().await.unwrap();
    let generated = r.headers()["x-request-id"].to_str().unwrap().to_string();
    assert_eq!(generated.len(), 36);
    // A sane client id is echoed; an unsafe one is replaced.
    let r = http
        .get(format!("{base}/healthz"))
        .header("x-request-id", "client-123")
        .send()
        .await
        .unwrap();
    assert_eq!(r.headers()["x-request-id"], "client-123");
    let r = http
        .get(format!("{base}/healthz"))
        .header("x-request-id", "bad id with spaces")
        .send()
        .await
        .unwrap();
    assert_ne!(r.headers()["x-request-id"], "bad id with spaces");
    // Auth failure body.
    let r = http
        .post(format!("{base}/a2a"))
        .header("x-request-id", "rid-auth")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    assert_eq!(r.headers()["x-request-id"], "rid-auth");
    assert_eq!(r.json::<Value>().await.unwrap()["requestId"], "rid-auth");
    // JSON-RPC error body.
    let r = http
        .post(format!("{base}/a2a"))
        .bearer_auth("secret")
        .header("x-request-id", "rid-rpc")
        .json(&rpc_body("GetTask", json!({"id":"nope"})))
        .send()
        .await
        .unwrap();
    let id = r.headers()["x-request-id"].to_str().unwrap().to_string();
    let v: Value = r.json().await.unwrap();
    assert_eq!(id, "rid-rpc");
    assert_eq!(v["error"]["data"][0]["metadata"]["requestId"], "rid-rpc");
}

#[tokio::test]
async fn oversized_bodies_get_413_with_declared_and_chunked_lengths() {
    let (base, addr, _tx, _h) = start(|c| c.max_body_bytes = 1024).await;
    let http = reqwest::Client::new();
    let r = http
        .post(format!("{base}/a2a"))
        .body(vec![b'x'; 4096])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 413);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["error"]["data"][0]["reason"], "PAYLOAD_TOO_LARGE");
    // Chunked: no Content-Length to check up front.
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(
        b"POST /a2a HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
    )
    .await
    .unwrap();
    let chunk = "x".repeat(2048);
    s.write_all(format!("{:x}\r\n{chunk}\r\n0\r\n\r\n", chunk.len()).as_bytes())
        .await
        .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    assert!(out.starts_with("HTTP/1.1 413"), "{out}");
    // Under the limit still works.
    let r = http
        .post(format!("{base}/a2a"))
        .json(&rpc_body("GetTask", json!({"id":"x"})))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn slow_headers_are_cut_off() {
    let (_, addr, _tx, _h) = start(|c| c.header_read_timeout = Duration::from_millis(300)).await;
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(b"GET /healthz HTTP/1.1\r\nHost: local")
        .await
        .unwrap();
    let t = Instant::now();
    let mut buf = Vec::new();
    let res = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut buf)).await;
    assert!(res.is_ok(), "server kept a slowloris connection open");
    assert!(t.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn a_stalled_body_gets_408() {
    let (_, addr, _tx, _h) = start(|c| c.body_timeout = Duration::from_millis(300)).await;
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(b"POST /a2a HTTP/1.1\r\nHost: localhost\r\nContent-Length: 100\r\n\r\n{")
        .await
        .unwrap();
    let mut out = String::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut buf = [0u8; 512];
        let n = s.read(&mut buf).await.unwrap();
        out = String::from_utf8_lossy(&buf[..n]).into();
    })
    .await
    .expect("no response to a stalled body");
    assert!(out.starts_with("HTTP/1.1 408"), "{out}");
}

#[tokio::test]
async fn shutdown_stops_accepting_and_enforces_the_drain_deadline() {
    let (_, addr, tx, h) = start(|c| {
        c.drain_timeout = Duration::from_millis(400);
        c.body_timeout = Duration::from_secs(60);
    })
    .await;
    // An in-flight request that never finishes.
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(b"POST /a2a HTTP/1.1\r\nHost: localhost\r\nContent-Length: 100\r\n\r\n{")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let t = Instant::now();
    tx.send(()).unwrap();
    let res = tokio::time::timeout(Duration::from_secs(5), h)
        .await
        .expect("serve_on did not return after the drain deadline");
    res.unwrap().unwrap();
    assert!(
        t.elapsed() >= Duration::from_millis(300),
        "{:?}",
        t.elapsed()
    );
    // The listener is gone.
    assert!(TcpStream::connect(addr).await.is_err());
}

#[tokio::test]
async fn an_idle_server_shuts_down_immediately() {
    let (base, _, tx, h) = start(|c| c.drain_timeout = Duration::from_secs(30)).await;
    // Leave a keep-alive connection idle in the pool.
    let http = reqwest::Client::new();
    http.get(format!("{base}/healthz")).send().await.unwrap();
    let t = Instant::now();
    tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), h)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(t.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn a_full_server_answers_429_with_retry_after() {
    if ostler_cdp::find_chrome(None).is_err() {
        assert!(
            std::env::var("OSTLER_REQUIRE_CHROME").is_err(),
            "Chrome required"
        );
        eprintln!("SKIPPED: no Chrome found");
        return;
    }
    // A page that never answers keeps the first task busy.
    let hang = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let hang_addr = hang.local_addr().unwrap();
    tokio::spawn(async move {
        let mut held = vec![];
        while let Ok((s, _)) = hang.accept().await {
            held.push(s);
        }
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mut c = A2aConfig::new(addr);
    c.max_concurrent_tasks = 1;
    let app = router(c, factory());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let http = reqwest::Client::new();
    let msg = |ctx: &str| {
        rpc_body(
            "SendMessage",
            json!({
                "configuration": {"returnImmediately": true},
                "message": {
                    "messageId": format!("m-{ctx}"), "role": "ROLE_USER", "contextId": ctx,
                    "parts": [{"data": {"url": format!("http://{hang_addr}/")}, "mediaType": "application/json"}]
                }
            }),
        )
    };
    let r = http
        .post(format!("http://{addr}/a2a"))
        .json(&msg("a"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let r = http
        .post(format!("http://{addr}/a2a"))
        .json(&msg("b"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 429);
    assert!(r.headers().contains_key("retry-after"));
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["error"]["data"][0]["reason"], "TOO_MANY_TASKS_IN_FLIGHT");
}
