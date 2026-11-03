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
    ("basic", include_str!("../fixtures/basic.html")),
    ("validation", include_str!("../fixtures/validation.html")),
    ("dependent", include_str!("../fixtures/dependent.html")),
    ("wizard", include_str!("../fixtures/wizard.html")),
    ("controlled", include_str!("../fixtures/controlled.html")),
    ("combobox", include_str!("../fixtures/combobox.html")),
    (
        "combobox_diverge",
        include_str!("../fixtures/combobox_diverge.html"),
    ),
    ("datetime", include_str!("../fixtures/datetime.html")),
    ("replay_a", include_str!("../fixtures/replay_a.html")),
    ("replay_b", include_str!("../fixtures/replay_b.html")),
    ("redesign_v1", include_str!("../fixtures/redesign_v1.html")),
    ("redesign_v2", include_str!("../fixtures/redesign_v2.html")),
    (
        "captcha_widget",
        include_str!("../fixtures/captcha_widget.html"),
    ),
    (
        "captcha_interstitial",
        include_str!("../fixtures/captcha_interstitial.html"),
    ),
    ("article", include_str!("../fixtures/article.html")),
    ("bigfooter", include_str!("../fixtures/bigfooter.html")),
    ("results", include_str!("../fixtures/results.html")),
    (
        "captcha_search_wall",
        include_str!("../fixtures/captcha_search_wall.html"),
    ),
    ("noop", include_str!("../fixtures/noop.html")),
    ("spa", include_str!("../fixtures/spa.html")),
    ("drift", include_str!("../fixtures/drift.html")),
    ("sections", include_str!("../fixtures/sections.html")),
    ("picker", include_str!("../fixtures/picker.html")),
    ("login", include_str!("../fixtures/login.html")),
    (
        "captcha_badge_only",
        include_str!("../fixtures/captcha_badge_only.html"),
    ),
    ("shadow", include_str!("../fixtures/shadow.html")),
    ("iframe", include_str!("../fixtures/iframe.html")),
    ("types", include_str!("../fixtures/types.html")),
    ("masked", include_str!("../fixtures/masked.html")),
    ("longform", include_str!("../fixtures/longform.html")),
    ("ambiguous", include_str!("../fixtures/ambiguous.html")),
    ("remount", include_str!("../fixtures/remount.html")),
    ("payment", include_str!("../fixtures/payment.html")),
    ("popup", include_str!("../fixtures/popup.html")),
    ("cookie", include_str!("../fixtures/cookie.html")),
    ("visual", include_str!("../fixtures/visual.html")),
    ("concealed", include_str!("../fixtures/concealed.html")),
    ("injection", include_str!("../fixtures/injection.html")),
    ("overlay", include_str!("../fixtures/overlay.html")),
    ("aria", include_str!("../fixtures/aria.html")),
    ("conditional", include_str!("../fixtures/conditional.html")),
    ("datepicker", include_str!("../fixtures/datepicker.html")),
    ("formless", include_str!("../fixtures/formless.html")),
    ("xhr_form", include_str!("../fixtures/xhr_form.html")),
    ("order", include_str!("../fixtures/order.html")),
    ("order_done", include_str!("../fixtures/order_done.html")),
    ("two_forms", include_str!("../fixtures/two_forms.html")),
    (
        "server_error",
        include_str!("../fixtures/server_error.html"),
    ),
    (
        "shared_errors",
        include_str!("../fixtures/shared_errors.html"),
    ),
    ("checks", include_str!("../fixtures/checks.html")),
    ("choices", include_str!("../fixtures/choices.html")),
    (
        "react_select",
        include_str!("../fixtures/react_select.html"),
    ),
    (
        "react_select_broken",
        include_str!("../fixtures/react_select_broken.html"),
    ),
    ("constraints", include_str!("../fixtures/constraints.html")),
    ("datefmt", include_str!("../fixtures/datefmt.html")),
    ("login_flash", include_str!("../fixtures/login_flash.html")),
    ("lockout", include_str!("../fixtures/lockout.html")),
    ("unlabelled", include_str!("../fixtures/unlabelled.html")),
    ("chatty", include_str!("../fixtures/chatty.html")),
    ("recheck", include_str!("../fixtures/recheck.html")),
    (
        "username_check",
        include_str!("../fixtures/username_check.html"),
    ),
    (
        "search_header",
        include_str!("../fixtures/search_header.html"),
    ),
    (
        "search_results",
        include_str!("../fixtures/search_results.html"),
    ),
    ("checks_text", include_str!("../fixtures/checks_text.html")),
    ("contactpref", include_str!("../fixtures/contactpref.html")),
    ("addresses", include_str!("../fixtures/addresses.html")),
    (
        "datepicker_dmy",
        include_str!("../fixtures/datepicker_dmy.html"),
    ),
    (
        "datepicker_assumed",
        include_str!("../fixtures/datepicker_assumed.html"),
    ),
    ("reorder", include_str!("../fixtures/reorder.html")),
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
                        // Background chatter for the "busy page" fixture.
                        (Method::Get, "/ping") => {
                            let _ = req.respond(Response::from_string("ok"));
                        }
                        // Slow validation endpoints for the "late error" fixtures.
                        (Method::Get, "/api/check") => {
                            thread::sleep(Duration::from_millis(400));
                            let _ = req.respond(Response::from_string("ok"));
                        }
                        (Method::Get, "/api/user") => {
                            thread::sleep(Duration::from_millis(1200));
                            let name = url
                                .query_pairs()
                                .find(|(k, _)| k == "name")
                                .map(|(_, v)| v.to_string())
                                .unwrap_or_default();
                            let _ = req.respond(Response::from_string(if name == "admin" {
                                "taken"
                            } else {
                                "free"
                            }));
                        }
                        (Method::Get, "/poll") => {
                            thread::sleep(Duration::from_secs(4));
                            let _ = req.respond(Response::from_string("{}"));
                        }
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
                        // Server-rendered login: a bad login redirects back to the page, which shows a flash.
                        (Method::Post, "/login_flash") => {
                            let mut body = String::new();
                            let _ = req.as_reader().read_to_string(&mut body);
                            // "plain" users get the unstyled message, everyone else the flash banner.
                            let which = if body.contains("plain") { 2 } else { 1 };
                            let loc = Header::from_bytes(
                                "Location",
                                format!("/login_flash.html?e={which}"),
                            )
                            .unwrap();
                            let _ = req.respond(
                                Response::from_string("")
                                    .with_status_code(302)
                                    .with_header(loc),
                            );
                        }
                        (Method::Get, "/login_flash.html") => {
                            let flash = match url.query_pairs().find(|(k, _)| k == "e") {
                                Some((_, v)) if v == "1" => "<div id=\"flash\" class=\"flash error\">Your username is invalid!<a href=\"#\" class=\"close\">x</a></div>",
                                _ => "",
                            };
                            let plain = match url.query_pairs().find(|(k, _)| k == "e") {
                                Some((_, v)) if v == "2" => {
                                    "<p>The password you entered is incorrect.</p>"
                                }
                                _ => "",
                            };
                            let body = include_str!("../fixtures/login_flash.html")
                                .replace("<!--FLASH-->", flash)
                                .replace("<!--MSG-->", plain);
                            let _ = req.respond(Response::from_string(body).with_header(html));
                        }
                        // A form post the server fails: a 500 with an ordinary-looking error page.
                        (Method::Post, "/fail") => {
                            let page = "<!doctype html><title>Error</title><h1>Internal Server Error</h1><p>Something went wrong.</p>";
                            let _ = req.respond(
                                Response::from_string(page)
                                    .with_status_code(500)
                                    .with_header(html),
                            );
                        }
                        // A normal-looking page served with a 404 status (GitHub style).
                        (Method::Get, "/missing") => {
                            let body = include_str!("../fixtures/notfound.html");
                            let _ = req.respond(
                                Response::from_string(body)
                                    .with_status_code(404)
                                    .with_header(html),
                            );
                        }
                        // An empty 403: Chrome reports ERR_HTTP_RESPONSE_CODE_FAILURE.
                        (Method::Get, "/forbidden") => {
                            let _ = req.respond(Response::from_string("").with_status_code(403));
                        }
                        (Method::Get, "/doc.pdf") => {
                            let pdf =
                                Header::from_bytes("Content-Type", "application/pdf").unwrap();
                            let _ = req.respond(
                                Response::from_data(
                                    b"%PDF-1.4\n1 0 obj<<>>endobj\ntrailer<<>>\n%%EOF\n".to_vec(),
                                )
                                .with_header(pdf),
                            );
                        }
                        (Method::Get, "/members") => {
                            let loc = Header::from_bytes("Location", "/login.html").unwrap();
                            let _ = req.respond(
                                Response::from_string("")
                                    .with_status_code(302)
                                    .with_header(loc),
                            );
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
