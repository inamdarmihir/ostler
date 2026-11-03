//! Forms benchmark.
//!
//! Two interaction patterns are run through the *same* Ostler engine so only the pattern differs:
//!
//! * `engine`: what Ostler is designed for: navigate, one `form_fill` per step, one `form_submit` per step.
//! * `per-field`: how primitive-tool agents drive a form: one call per field, each returning the full page
//!   snapshot, then a click on the submit button. This models the *pattern* of snapshot-per-action tools;
//!   it is NOT a measurement of any other product.

use std::time::Instant;

use ostler_browser::{LaunchOptions, Policy};
use ostler_fixtures::FixtureServer;
use ostler_mcp::{App, AppConfig, Profile};
use serde_json::{json, Value};

use crate::cost::{self, Call, CostArgs};

pub(crate) struct Scenario {
    pub(crate) name: &'static str,
    pub(crate) page: &'static str,
    pub(crate) steps: Vec<Vec<(&'static str, Value)>>,
    expect: Vec<(&'static str, Value)>,
    /// Supported by the per-field baseline driver (needs one control per label).
    primitive: bool,
}

pub(crate) fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "basic contact form",
            page: "/basic.html",
            primitive: true,
            steps: vec![vec![
                ("Full name", json!("Ada Lovelace")),
                ("Email", json!("ada@example.com")),
                ("Message", json!("Hello")),
                ("Subscribe to newsletter", json!(true)),
            ]],
            expect: vec![
                ("email", json!("ada@example.com")),
                ("newsletter", json!("yes")),
            ],
        },
        Scenario {
            name: "validation (3 rules)",
            page: "/validation.html",
            primitive: true,
            steps: vec![vec![
                ("Email", json!("ada@example.com")),
                ("Age", json!(36)),
                ("ZIP code", json!("94107")),
            ]],
            expect: vec![("zip", json!("94107"))],
        },
        Scenario {
            name: "dependent dropdowns",
            page: "/dependent.html",
            primitive: true,
            steps: vec![vec![
                ("Recipient", json!("Ada")),
                ("Country", json!("Germany")),
                ("State / region", json!("Bavaria")),
                ("City", json!("Munich")),
            ]],
            expect: vec![("country", json!("de")), ("state", json!("by"))],
        },
        Scenario {
            name: "3-step wizard",
            page: "/wizard.html",
            primitive: true,
            steps: vec![
                vec![
                    ("First name", json!("Ada")),
                    ("Last name", json!("Lovelace")),
                ],
                vec![
                    ("Email", json!("ada@example.com")),
                    ("Phone", json!("5551234567")),
                ],
                vec![("I confirm the details are correct", json!(true))],
            ],
            expect: vec![
                ("first_name", json!("Ada")),
                ("email", json!("ada@example.com")),
            ],
        },
        Scenario {
            name: "controlled inputs",
            page: "/controlled.html",
            primitive: true,
            steps: vec![vec![
                ("First name", json!("Ada")),
                ("Last name", json!("Lovelace")),
                ("Company", json!("Analytical Engines")),
            ]],
            expect: vec![
                ("first", json!("Ada")),
                ("company", json!("Analytical Engines")),
            ],
        },
        Scenario {
            name: "custom combobox",
            page: "/combobox.html",
            primitive: true,
            steps: vec![vec![("Country", json!("Germany"))]],
            expect: vec![("country", json!("Germany"))],
        },
        Scenario {
            name: "shadow DOM",
            page: "/shadow.html",
            primitive: true,
            steps: vec![vec![
                ("Your name", json!("Ada")),
                ("Email address", json!("ada@example.com")),
            ]],
            expect: vec![("email", json!("ada@example.com"))],
        },
        Scenario {
            name: "same-origin iframe",
            page: "/iframe.html",
            primitive: true,
            steps: vec![vec![
                ("Subject", json!("Broken login")),
                ("Details", json!("Cannot sign in")),
            ]],
            expect: vec![("subject", json!("Broken login"))],
        },
        Scenario {
            name: "long form (20 fields)",
            page: "/longform.html",
            primitive: true,
            steps: vec![vec![
                ("First name", json!("Ada")),
                ("Last name", json!("Lovelace")),
                ("Email", json!("ada@example.com")),
                ("Phone", json!("5551234567")),
                ("Address line 1", json!("12 Analytical St")),
                ("Address line 2", json!("Suite 4")),
                ("City", json!("San Francisco")),
                ("State", json!("California")),
                ("ZIP code", json!("94107")),
                ("Country", json!("United States")),
                ("Current company", json!("Analytical Engines")),
                ("Job title", json!("Engineer")),
                ("LinkedIn URL", json!("https://linkedin.com/in/ada")),
                ("Website", json!("https://ada.example.com")),
                ("Earliest start date", json!("2026-12-01")),
                ("Expected salary (USD)", json!(150000)),
                ("How did you hear about us?", json!("Web search")),
                ("Cover letter", json!("I would love to build the Engine.")),
                ("Willing to relocate", json!(true)),
                ("I agree to the privacy policy", json!(true)),
            ]],
            expect: vec![
                ("email", json!("ada@example.com")),
                ("state", json!("ca")),
                ("salary", json!("150000")),
            ],
        },
        Scenario {
            name: "rich inputs (9 fields)",
            page: "/types.html",
            primitive: false,
            steps: vec![vec![
                ("Date of birth", json!("March 5, 2001")),
                ("Preferred time", json!("3:30 PM")),
                ("Guests", json!(4)),
                ("Plan", json!("Pro")),
                ("Interests", json!(["AI", "Data"])),
                ("Dietary needs", json!(["Vegan"])),
                ("Notes", json!("Window seat")),
                ("I accept the terms", json!(true)),
            ]],
            expect: vec![("dob", json!("2001-03-05")), ("plan", json!("pro"))],
        },
    ]
}

#[derive(Default, Clone)]
pub(crate) struct Run {
    calls: Vec<Call>,
    tool_ms: u128,
    cdp: u64,
    pub(crate) ok: bool,
    pub(crate) note: String,
}

fn tokens(bytes: usize) -> u64 {
    (bytes as u64).div_ceil(4)
}

pub(crate) fn new_app() -> App {
    new_app_with(false)
}

pub(crate) fn new_app_with(lean: bool) -> App {
    App::new(AppConfig {
        launch: LaunchOptions {
            lean,
            ..Default::default()
        },
        policy: Policy::default(),
        profile: Profile::All,
        memory: None,
        learn: false,
        require_confirm_submit: false,
        verbose: false,
    })
}

async fn tool(app: &App, run: &mut Run, name: &str, args: Value) -> (bool, String) {
    let t = Instant::now();
    let out = app.call(name, &args).await;
    run.tool_ms += t.elapsed().as_millis();
    run.calls.push(Call {
        args_tokens: tokens(args.to_string().len()),
        out_tokens: tokens(out.bytes()),
    });
    (out.is_error, out.text_content())
}

/// Find `[eN]` for the line mentioning `"label"` in a snapshot.
fn find_ref(snapshot: &str, label: &str) -> Option<String> {
    let needle = format!("\"{}\"", label.to_lowercase());
    for line in snapshot.lines() {
        let l = line.to_lowercase();
        if l.contains(&needle) {
            if let Some(i) = line.find("[e") {
                let rest = &line[i + 1..];
                if let Some(j) = rest.find(']') {
                    return Some(rest[..j].to_string());
                }
            }
        }
    }
    None
}

fn find_submit_ref(snapshot: &str) -> Option<String> {
    for line in snapshot.lines() {
        let t = line.trim_start();
        if !t.starts_with("button ") {
            continue;
        }
        let l = t.to_lowercase();
        if l.contains("back") || l.contains("cancel") {
            continue;
        }
        if [
            "continue",
            "submit",
            "send",
            "ship",
            "create",
            "save",
            "join",
            "open ticket",
            "verify",
            "register",
        ]
        .iter()
        .any(|w| l.contains(w))
        {
            if let Some(i) = t.find("[e") {
                let rest = &t[i + 1..];
                return rest.find(']').map(|j| rest[..j].to_string());
            }
        }
    }
    None
}

pub(crate) async fn run_engine(app: &App, srv: &FixtureServer, s: &Scenario) -> Run {
    let mut r = Run::default();
    srv.reset();
    tool(
        app,
        &mut r,
        "navigate",
        json!({"url": srv.url(s.page), "observe": "interactive"}),
    )
    .await;
    for step in &s.steps {
        let values: serde_json::Map<String, Value> = step
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        let (err, txt) = tool(app, &mut r, "form_fill", json!({"values": values})).await;
        if err || txt.contains("FAILED") || txt.contains("UNMATCHED") {
            r.note = format!(
                "fill problem: {}",
                txt.lines()
                    .find(|l| l.contains("FAILED") || l.contains("UNMATCHED"))
                    .unwrap_or(&txt)
            );
            return r;
        }
        let (err, txt) = tool(app, &mut r, "form_submit", json!({})).await;
        if err
            || txt.starts_with("NOT SUBMITTED")
            || txt.starts_with("NOT ACCEPTED")
            || txt.starts_with("Nothing changed")
        {
            r.note = format!("submit problem: {}", txt.lines().next().unwrap_or(""));
            return r;
        }
    }
    finish(app, srv, s, &mut r).await;
    r
}

async fn run_per_field(app: &App, srv: &FixtureServer, s: &Scenario) -> Run {
    let mut r = Run::default();
    srv.reset();
    let (_, mut snap) = tool(
        app,
        &mut r,
        "navigate",
        json!({"url": srv.url(s.page), "observe": "full"}),
    )
    .await;
    for step in &s.steps {
        for (label, value) in step {
            let Some(rf) = find_ref(&snap, label) else {
                r.note = format!("no ref for {label:?} in snapshot");
                return r;
            };
            // One agent call = fill + the full snapshot the tool would return with it.
            let t = Instant::now();
            let fill = app
                .call("fill_field", &json!({"ref": rf, "value": value}))
                .await;
            let shot = app.call("snapshot", &json!({"mode": "full"})).await;
            r.tool_ms += t.elapsed().as_millis();
            r.calls.push(Call {
                args_tokens: tokens(json!({"ref": rf, "value": value}).to_string().len()),
                out_tokens: tokens(fill.bytes() + shot.bytes()),
            });
            if fill.is_error || fill.text_content().contains("FAILED") {
                r.note = format!("fill problem on {label:?}");
                return r;
            }
            snap = shot.text_content();
        }
        let Some(sub) = find_submit_ref(&snap) else {
            r.note = "no submit button in snapshot".into();
            return r;
        };
        let (_, next) = tool(app, &mut r, "click", json!({"ref": sub, "observe": "full"})).await;
        snap = next;
    }
    finish(app, srv, s, &mut r).await;
    r
}

/// Like `run_engine`, but safe when several sessions share one fixture server: it never resets
/// the server and judges success by whether *any* recorded submission matches the expectation.
pub(crate) async fn run_engine_isolated(app: &App, srv: &FixtureServer, s: &Scenario) -> Run {
    let mut r = Run::default();
    let before = srv.submissions().len();
    tool(
        app,
        &mut r,
        "navigate",
        json!({"url": srv.url(s.page), "observe": "interactive"}),
    )
    .await;
    for step in &s.steps {
        let values: serde_json::Map<String, Value> = step
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        let (err, txt) = tool(app, &mut r, "form_fill", json!({"values": values})).await;
        if err || txt.contains("FAILED") || txt.contains("UNMATCHED") {
            r.note = format!("fill problem: {txt}");
            return r;
        }
        let (err, txt) = tool(app, &mut r, "form_submit", json!({})).await;
        if err || txt.starts_with("NOT SUBMITTED") || txt.starts_with("NOT ACCEPTED") {
            r.note = format!("submit problem: {txt}");
            return r;
        }
    }
    let deadline = Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let subs = srv.submissions();
        if subs
            .iter()
            .skip(before.saturating_sub(64))
            .any(|got| s.expect.iter().all(|(k, v)| &got[*k] == v))
        {
            r.ok = true;
            return r;
        }
        if Instant::now() > deadline {
            r.note = "no matching submission recorded".into();
            return r;
        }
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }
}

async fn finish(app: &App, srv: &FixtureServer, s: &Scenario, r: &mut Run) {
    // Submissions are recorded server-side; give async fetches a moment to land.
    srv.wait_for(1, std::time::Duration::from_secs(3));
    let got = srv.last().unwrap_or(Value::Null);
    r.ok = s.expect.iter().all(|(k, v)| &got[*k] == v);
    if !r.ok {
        r.note = format!("server received {got}");
    }
    r.cdp = app.browser_metrics().await.map(|m| m.0).unwrap_or(0);
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    if v.is_empty() {
        0.0
    } else {
        v[v.len() / 2]
    }
}

pub async fn run(reps: usize, cost_args: CostArgs) -> anyhow::Result<Value> {
    let srv = FixtureServer::start();
    let app = new_app();

    // Cold start: first call launches Chrome.
    let t = Instant::now();
    app.call(
        "navigate",
        &json!({"url": "about:blank", "observe": "none"}),
    )
    .await;
    let cold_ms = t.elapsed().as_millis();
    let rss_mib = app.browser_metrics().await.and_then(|m| m.1).unwrap_or(0) / 1024;
    let schema_tokens = tokens(app.schema_bytes());
    println!("Cold start (launch Chrome + first navigate): {cold_ms} ms; browser memory idle: {rss_mib} MiB [{}]; tool schemas: ~{schema_tokens} tokens\n", ostler_cdp::memory_method());
    println!("Assumptions: price in ${}/Mtok, out ${}/Mtok, LLM turn {}s, base context {} tokens. Tokens = bytes/4 (estimate).", cost_args.price_in, cost_args.price_out, cost_args.llm_turn_secs, cost_args.base_context);
    println!("Baseline = per-field interaction pattern run through the SAME engine (not another product).\n");
    println!(
        "{:<24} {:<10} {:>5} {:>8} {:>9} {:>8} {:>9} {:>8} {:>7}  result",
        "scenario", "mode", "calls", "out tok", "in tok", "tool ms", "e2e s", "$ cached", "CDP"
    );

    let mut rows = vec![];
    let (mut tot_e, mut tot_p) = (
        (0u64, 0u64, 0f64, 0f64, 0f64),
        (0u64, 0u64, 0f64, 0f64, 0f64),
    );
    let mut failures = 0;
    for s in scenarios() {
        for (mode, enabled) in [("engine", true), ("per-field", s.primitive)] {
            if !enabled {
                continue;
            }
            let mut runs = vec![];
            for _ in 0..reps {
                let before = app.browser_metrics().await.map(|m| m.0).unwrap_or(0);
                let mut r = if mode == "engine" {
                    run_engine(&app, &srv, &s).await
                } else {
                    run_per_field(&app, &srv, &s).await
                };
                r.cdp = r.cdp.saturating_sub(before);
                runs.push(r);
            }
            let ok = runs.iter().all(|r| r.ok);
            if !ok {
                failures += 1;
            }
            let mut ms: Vec<f64> = runs.iter().map(|r| r.tool_ms as f64).collect();
            let tool_ms = median(&mut ms);
            let r0 = &runs[0];
            let est = cost::estimate(&r0.calls, schema_tokens, &cost_args);
            let out_tok: u64 = r0.calls.iter().map(|c| c.out_tokens).sum();
            let e2e = tool_ms / 1000.0 + est.turns as f64 * cost_args.llm_turn_secs;
            println!(
                "{:<24} {:<10} {:>5} {:>8} {:>9} {:>8.0} {:>9.1} {:>8.4} {:>7}  {}",
                s.name,
                mode,
                r0.calls.len(),
                out_tok,
                est.input_tokens,
                tool_ms,
                e2e,
                est.usd_cached,
                r0.cdp,
                if ok {
                    "ok".to_string()
                } else {
                    format!("FAILED {}", r0.note)
                }
            );
            if s.primitive {
                let t = if mode == "engine" {
                    &mut tot_e
                } else {
                    &mut tot_p
                };
                t.0 += r0.calls.len() as u64;
                t.1 += est.input_tokens + est.output_tokens;
                t.2 += tool_ms;
                t.3 += e2e;
                t.4 += est.usd_cached;
            }
            rows.push(json!({
                "scenario": s.name, "mode": mode, "calls": r0.calls.len(), "out_tokens": out_tok,
                "input_tokens": est.input_tokens, "output_tokens": est.output_tokens, "tool_ms": tool_ms, "e2e_s": e2e,
                "usd_uncached": est.usd_uncached, "usd_cached": est.usd_cached, "cdp_calls": r0.cdp, "ok": ok, "note": r0.note,
            }));
        }
    }
    println!(
        "\nTotals over the {} scenarios both modes support:",
        scenarios().iter().filter(|s| s.primitive).count()
    );
    for (name, t) in [("engine", tot_e), ("per-field", tot_p)] {
        println!("  {name:<10} calls {:>3}  tokens {:>7}  tool {:>6.0} ms  e2e {:>6.1} s  ${:.4} (cached)", t.0, t.1, t.2, t.3, t.4);
    }
    if tot_e.0 > 0 && tot_p.0 > 0 {
        println!(
            "  ratio per-field/engine: calls {:.1}x, tokens {:.1}x, e2e latency {:.1}x, cost {:.1}x",
            tot_p.0 as f64 / tot_e.0 as f64, tot_p.1 as f64 / tot_e.1 as f64, tot_p.3 / tot_e.3, tot_p.4 / tot_e.4
        );
    }
    app.shutdown().await;
    if failures > 0 {
        anyhow::bail!("{failures} scenario/mode combinations failed");
    }
    Ok(
        json!({"cold_start_ms": cold_ms, "idle_rss_mib": rss_mib, "schema_tokens": schema_tokens, "rows": rows,
              "totals": {"engine": {"calls": tot_e.0, "tokens": tot_e.1, "tool_ms": tot_e.2, "e2e_s": tot_e.3, "usd_cached": tot_e.4},
                         "per_field": {"calls": tot_p.0, "tokens": tot_p.1, "tool_ms": tot_p.2, "e2e_s": tot_p.3, "usd_cached": tot_p.4}}}),
    )
}
