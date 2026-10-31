//! Local HTML form fixtures plus a tiny server that records what gets submitted.
//!
//! Tests assert on what the *server received*, not on what the page claims.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tiny_http::{Header, Method, Response, Server};

pub const PAGES: &[(&str, &str)] = &[
    ("basic", include_str!("../../../fixtures/basic.html")),
    (
        "validation",
        include_str!("../../../fixtures/validation.html"),
    ),
    (
        "dependent",
        include_str!("../../../fixtures/dependent.html"),
    ),
    ("wizard", include_str!("../../../fixtures/wizard.html")),
    (
        "controlled",
        include_str!("../../../fixtures/controlled.html"),
    ),
    ("combobox", include_str!("../../../fixtures/combobox.html")),
    ("shadow", include_str!("../../../fixtures/shadow.html")),
    ("iframe", include_str!("../../../fixtures/iframe.html")),
    ("types", include_str!("../../../fixtures/types.html")),
    ("masked", include_str!("../../../fixtures/masked.html")),
    ("longform", include_str!("../../../fixtures/longform.html")),
    (
        "ambiguous",
        include_str!("../../../fixtures/ambiguous.html"),
    ),
    ("remount", include_str!("../../../fixtures/remount.html")),
    ("payment", include_str!("../../../fixtures/payment.html")),
    ("aria", include_str!("../../../fixtures/aria.html")),
    (
        "conditional",
        include_str!("../../../fixtures/conditional.html"),
    ),
    (
        "datepicker",
        include_str!("../../../fixtures/datepicker.html"),
    ),
];

pub struct FixtureServer {
    pub port: u16,
    submissions: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
}

fn states(country: &str) -> Value {
    match country {
        "us" => {
            json!([{"code":"ca","name":"California"},{"code":"ny","name":"New York"},{"code":"tx","name":"Texas"}])
        }
        "de" => json!([{"code":"by","name":"Bavaria"},{"code":"be","name":"Berlin"}]),
        "in" => json!([{"code":"ka","name":"Karnataka"},{"code":"mh","name":"Maharashtra"}]),
        _ => json!([]),
    }
}

impl FixtureServer {
    pub fn start() -> Self {
        Self::start_on("127.0.0.1:0", Duration::from_millis(250))
    }

    /// `api_delay` simulates network latency on the dependent-dropdown endpoint.
    pub fn start_on(addr: &str, api_delay: Duration) -> Self {
        let server = Server::http(addr).expect("bind fixture server");
        let port = server.server_addr().to_ip().expect("ip addr").port();
        let submissions: Arc<Mutex<Vec<Value>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (subs, stop2) = (submissions.clone(), stop.clone());
        thread::spawn(move || {
            while !stop2.load(Ordering::Relaxed) {
                let Ok(Some(mut req)) = server.recv_timeout(Duration::from_millis(100)) else {
                    continue;
                };
                let subs = subs.clone();
                // One thread per request so slow endpoints don't serialise parallel sessions.
                thread::spawn(move || {
                    let full = format!("http://localhost{}", req.url());
                    let url = url::Url::parse(&full).unwrap();
                    let path = url.path().to_string();
                    let html =
                        Header::from_bytes("Content-Type", "text/html; charset=utf-8").unwrap();
                    let jsonh = Header::from_bytes("Content-Type", "application/json").unwrap();
                    match (req.method().clone(), path.as_str()) {
                        (Method::Get, "/api/states") => {
                            thread::sleep(api_delay);
                            let c = url
                                .query_pairs()
                                .find(|(k, _)| k == "country")
                                .map(|(_, v)| v.to_string())
                                .unwrap_or_default();
                            let _ = req.respond(
                                Response::from_string(states(&c).to_string()).with_header(jsonh),
                            );
                        }
                        (Method::Post, "/submit") => {
                            let ctype = req
                                .headers()
                                .iter()
                                .find(|h| h.field.equiv("Content-Type"))
                                .map(|h| h.value.to_string())
                                .unwrap_or_default();
                            let mut body = String::new();
                            let _ = req.as_reader().read_to_string(&mut body);
                            let is_json = ctype.contains("json");
                            let parsed: Value = if is_json {
                                serde_json::from_str(&body).unwrap_or(Value::Null)
                            } else {
                                let mut m = serde_json::Map::new();
                                for (k, v) in url::form_urlencoded::parse(body.as_bytes()) {
                                    m.insert(k.to_string(), json!(v.to_string()));
                                }
                                Value::Object(m)
                            };
                            subs.lock().unwrap().push(parsed);
                            if is_json {
                                let _ = req.respond(
                                    Response::from_string("{\"ok\":true}").with_header(jsonh),
                                );
                            } else {
                                let page = "<!doctype html><title>Thanks</title><h1>Thank you</h1><p>Your message was received.</p>";
                                let _ = req.respond(Response::from_string(page).with_header(html));
                            }
                        }
                        (Method::Get, "/") => {
                            let list: String = PAGES
                                .iter()
                                .map(|(n, _)| format!("<li><a href=\"/{n}.html\">{n}</a></li>"))
                                .collect();
                            let page = format!("<!doctype html><title>Fixtures</title><h1>Fixtures</h1><ul>{list}</ul>");
                            let _ = req.respond(Response::from_string(page).with_header(html));
                        }
                        (Method::Get, p) if p.ends_with(".html") => {
                            let name = p.trim_start_matches('/').trim_end_matches(".html");
                            match PAGES.iter().find(|(n, _)| *n == name) {
                                Some((_, body)) => {
                                    let _ =
                                        req.respond(Response::from_string(*body).with_header(html));
                                }
                                None => {
                                    let _ = req.respond(
                                        Response::from_string("not found").with_status_code(404),
                                    );
                                }
                            }
                        }
                        _ => {
                            let _ = req
                                .respond(Response::from_string("not found").with_status_code(404));
                        }
                    }
                });
            }
        });
        Self {
            port,
            submissions,
            stop,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }

    pub fn submissions(&self) -> Vec<Value> {
        self.submissions.lock().unwrap().clone()
    }

    pub fn last(&self) -> Option<Value> {
        self.submissions.lock().unwrap().last().cloned()
    }

    pub fn reset(&self) {
        self.submissions.lock().unwrap().clear();
    }

    /// Block until at least `n` submissions arrived (or time out).
    pub fn wait_for(&self, n: usize, timeout: Duration) -> bool {
        let end = Instant::now() + timeout;
        while Instant::now() < end {
            if self.submissions.lock().unwrap().len() >= n {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        false
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
