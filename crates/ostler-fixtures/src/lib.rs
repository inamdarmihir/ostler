//! Local HTML form fixtures plus a tiny server that records what gets submitted.
//!
//! Tests assert on what the *server received*, not on what the page claims.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tiny_http::{Header, Method, Response, Server};

pub mod packs;
pub mod suite;

#[cfg(test)]
mod packs_tests;

/// Kill a Chrome process with SIGKILL, to simulate a browser crash mid-task.
///
/// A pid that has already exited is ignored. Pid 0 (and anything out of range) is refused, because
/// `kill(0, ..)` would signal the caller's whole process group.
#[cfg(unix)]
pub fn kill_chrome(pid: u32) {
    if let Ok(pid) = libc::pid_t::try_from(pid) {
        if pid > 0 {
            // SAFETY: kill(2) takes a pid and a signal number and touches no memory of ours; an error
            // (ESRCH for a dead pid) is ignored on purpose.
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
    }
}

/// No-op on platforms other than unix. The plan calls for `taskkill` on Windows; that is not wired up
/// here yet, so crash tests that need it are skipped there.
#[cfg(not(unix))]
pub fn kill_chrome(_pid: u32) {}

pub const PAGES: &[(&str, &str)] = &[
    ("basic", include_str!("../fixtures/basic.html")),
    ("scrollhover", include_str!("../fixtures/scrollhover.html")),
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
    ("twin_forms", include_str!("../fixtures/twin_forms.html")),
    ("slow_load", include_str!("../fixtures/slow_load.html")),
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
    (
        "suite_address_auto",
        include_str!("../fixtures/suite/address_auto.html"),
    ),
    (
        "suite_custom_select",
        include_str!("../fixtures/suite/custom_select.html"),
    ),
    (
        "suite_radio_cards",
        include_str!("../fixtures/suite/radio_cards.html"),
    ),
    (
        "suite_terms_scroll",
        include_str!("../fixtures/suite/terms_scroll.html"),
    ),
    (
        "suite_async_user",
        include_str!("../fixtures/suite/async_user.html"),
    ),
    (
        "suite_captcha_gate",
        include_str!("../fixtures/suite/captcha_gate.html"),
    ),
    (
        "suite_checkout",
        include_str!("../fixtures/suite/checkout.html"),
    ),
    ("suite_chips", include_str!("../fixtures/suite/chips.html")),
    (
        "suite_consent",
        include_str!("../fixtures/suite/consent.html"),
    ),
    ("suite_csrf", include_str!("../fixtures/suite/csrf.html")),
    (
        "suite_datalist",
        include_str!("../fixtures/suite/datalist.html"),
    ),
    (
        "suite_disabled_field",
        include_str!("../fixtures/suite/disabled_field.html"),
    ),
    (
        "suite_flight_dates",
        include_str!("../fixtures/suite/flight_dates.html"),
    ),
    (
        "suite_iframe_host",
        include_str!("../fixtures/suite/iframe_host.html"),
    ),
    (
        "suite_iframe_inner",
        include_str!("../fixtures/suite/iframe_inner.html"),
    ),
    ("suite_job", include_str!("../fixtures/suite/job.html")),
    (
        "suite_license_dob",
        include_str!("../fixtures/suite/license_dob.html"),
    ),
    (
        "suite_missing_field",
        include_str!("../fixtures/suite/missing_field.html"),
    ),
    (
        "suite_phone_cc",
        include_str!("../fixtures/suite/phone_cc.html"),
    ),
    (
        "suite_radio_other",
        include_str!("../fixtures/suite/radio_other.html"),
    ),
    (
        "suite_reveal",
        include_str!("../fixtures/suite/reveal.html"),
    ),
    (
        "suite_richtext",
        include_str!("../fixtures/suite/richtext.html"),
    ),
    (
        "suite_server_reject",
        include_str!("../fixtures/suite/server_reject.html"),
    ),
    (
        "suite_shadow_nested",
        include_str!("../fixtures/suite/shadow_nested.html"),
    ),
    (
        "suite_signup_pw",
        include_str!("../fixtures/suite/signup_pw.html"),
    ),
    (
        "suite_switches",
        include_str!("../fixtures/suite/switches.html"),
    ),
    (
        "suite_tax_dob",
        include_str!("../fixtures/suite/tax_dob.html"),
    ),
    (
        "suite_wrong_format",
        include_str!("../fixtures/suite/wrong_format.html"),
    ),
];

pub struct FixtureServer {
    /// The main listener's port.
    pub port: u16,
    /// The second listener's port: a different origin, used by the commerce pack's payment iframe.
    pub port2: u16,
    submissions: Arc<Mutex<Vec<Value>>>,
    records: Arc<Mutex<Vec<packs::Record>>>,
    pack_state: Arc<packs::PackState>,
    stop: Arc<AtomicBool>,
}

/// State shared by both listeners.
#[derive(Clone)]
struct Shared {
    subs: Arc<Mutex<Vec<Value>>>,
    records: Arc<Mutex<Vec<packs::Record>>>,
    pack_state: Arc<packs::PackState>,
    api_delay: Duration,
    pay_port: u16,
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
        let server2 = Server::http("127.0.0.1:0").expect("bind second fixture listener");
        let port2 = server2.server_addr().to_ip().expect("ip addr").port();
        let submissions: Arc<Mutex<Vec<Value>>> = Arc::default();
        let records: Arc<Mutex<Vec<packs::Record>>> = Arc::default();
        let pack_state: Arc<packs::PackState> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let shared = Shared {
            subs: submissions.clone(),
            records: records.clone(),
            pack_state: pack_state.clone(),
            api_delay,
            pay_port: port2,
        };
        for srv in [server, server2] {
            let (shared, stop2) = (shared.clone(), stop.clone());
            thread::spawn(move || serve(srv, shared, stop2));
        }
        Self {
            port,
            port2,
            submissions,
            records,
            pack_state,
            stop,
        }
    }
}

fn serve(server: Server, shared: Shared, stop2: Arc<AtomicBool>) {
    {
        {
            while !stop2.load(Ordering::Relaxed) {
                let Ok(Some(mut req)) = server.recv_timeout(Duration::from_millis(100)) else {
                    continue;
                };
                let shared = shared.clone();
                // One thread per request so slow endpoints don't serialise parallel sessions.
                thread::spawn(move || {
                    let full = format!("http://localhost{}", req.url());
                    let url = url::Url::parse(&full).unwrap();
                    let path = url.path().to_string();
                    let html =
                        Header::from_bytes("Content-Type", "text/html; charset=utf-8").unwrap();
                    let jsonh = Header::from_bytes("Content-Type", "application/json").unwrap();
                    let hdr = |name: &'static str| -> String {
                        req.headers()
                            .iter()
                            .find(|h| h.field.equiv(name))
                            .map(|h| h.value.to_string())
                            .unwrap_or_default()
                    };
                    let (ctype, referer, host) = (hdr("Content-Type"), hdr("Referer"), hdr("Host"));
                    // Read the body once, up front, so every request can be recorded.
                    let mut raw: Vec<u8> = vec![];
                    if !matches!(req.method(), Method::Get | Method::Head) {
                        let _ = req.as_reader().read_to_end(&mut raw);
                    }
                    let body_text = String::from_utf8_lossy(&raw).to_string();
                    let method = req.method().as_str().to_string();
                    let is_internal = path.starts_with("/__");
                    if !is_internal {
                        let rec = packs::make_record(
                            &method,
                            &path,
                            url.query().unwrap_or(""),
                            &ctype,
                            &raw,
                            &referer,
                        );
                        shared.records.lock().unwrap().push(rec);
                    }
                    let subs = shared.subs.clone();
                    let api_delay = shared.api_delay;
                    // Ground truth for the domain packs.
                    match (req.method().clone(), path.as_str()) {
                        (Method::Get, "/__records") => {
                            let all: Vec<Value> = shared
                                .records
                                .lock()
                                .unwrap()
                                .iter()
                                .map(packs::Record::to_json)
                                .collect();
                            let _ = req.respond(
                                Response::from_string(Value::Array(all).to_string())
                                    .with_header(jsonh),
                            );
                            return;
                        }
                        (Method::Post, "/__reset") => {
                            subs.lock().unwrap().clear();
                            shared.records.lock().unwrap().clear();
                            shared.pack_state.reset();
                            let _ = req.respond(Response::from_string("{}").with_header(jsonh));
                            return;
                        }
                        _ => {}
                    }
                    let body_json = packs::parse_body(&ctype, &raw);
                    let preq = packs::PackReq {
                        method: &method,
                        path: &path,
                        body: &body_json,
                        host: &host,
                        pay_port: shared.pay_port,
                    };
                    if let Some(r) = packs::route(&shared.pack_state, &preq) {
                        let ct = Header::from_bytes("Content-Type", r.content_type).unwrap();
                        let _ = req.respond(
                            Response::from_string(r.body)
                                .with_status_code(r.status)
                                .with_header(ct),
                        );
                        return;
                    }
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
                        // `/slow/<n>` answers after n seconds (capped at 60), for timeout and concurrency tests.
                        (Method::Get, p) if p.starts_with("/slow/") => {
                            match p["/slow/".len()..].parse::<u64>() {
                                Ok(n) => {
                                    let n = n.min(60);
                                    thread::sleep(Duration::from_secs(n));
                                    let _ = req.respond(Response::from_string(format!("slow {n}")));
                                }
                                Err(_) => {
                                    let _ = req.respond(
                                        Response::from_string("not found").with_status_code(404),
                                    );
                                }
                            }
                        }
                        // The "slow submit" trap: the form answers only after 15 seconds.
                        (Method::Post, "/trap/slow-submit") => {
                            thread::sleep(Duration::from_secs(15));
                            let page = "<!doctype html><meta charset=utf-8><title>Saved</title><h1>Saved</h1>";
                            let _ = req.respond(Response::from_string(page).with_header(html));
                        }
                        // A subresource that never arrives in useful time (an ad, a tracker): holds the
                        // `load` event open while the document itself is long since usable.
                        (Method::Get, "/hang.png") => {
                            thread::sleep(Duration::from_secs(25));
                            let _ = req.respond(Response::from_string("").with_status_code(404));
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
                        // Ground truth for external drivers (benchmarks): what the server actually received.
                        (Method::Get, "/__submissions") => {
                            let body = Value::Array(subs.lock().unwrap().clone()).to_string();
                            let _ = req.respond(Response::from_string(body).with_header(jsonh));
                        }
                        // A server that refuses a well-formed request with a message (expired voucher).
                        (Method::Post, "/api/reject") => {
                            let _ = req.respond(
                                Response::from_string(
                                    "{\"error\":\"This voucher code has expired.\"}",
                                )
                                .with_status_code(422)
                                .with_header(jsonh),
                            );
                        }
                        (Method::Post, "/submit") => {
                            let body = body_text.clone();
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
                            let body = body_text.clone();
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
                                    // `?delay_ms=<n>` (capped at 10s) serves any page late, e.g. an iframe
                                    // that is still loading when the agent first looks.
                                    if let Some(ms) = url
                                        .query_pairs()
                                        .find(|(k, _)| k == "delay_ms")
                                        .and_then(|(_, v)| v.parse::<u64>().ok())
                                    {
                                        thread::sleep(Duration::from_millis(ms.min(10_000)));
                                    }
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
        }
    }
}

impl FixtureServer {
    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }

    pub fn submissions(&self) -> Vec<Value> {
        self.submissions.lock().unwrap().clone()
    }

    pub fn last(&self) -> Option<Value> {
        self.submissions.lock().unwrap().last().cloned()
    }

    /// Forget submissions, recorded requests and per-page state (what `POST /__reset` does).
    pub fn reset(&self) {
        self.submissions.lock().unwrap().clear();
        self.records.lock().unwrap().clear();
        self.pack_state.reset();
    }

    /// Every request received since the last reset, in arrival order (what `GET /__records` serves).
    pub fn records(&self) -> Vec<packs::Record> {
        self.records.lock().unwrap().clone()
    }

    /// URL on the second origin (a different port, so a different origin than [`Self::url`]).
    pub fn url2(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port2, path)
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
