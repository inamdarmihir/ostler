//! Regression tests: late dependent work during a fill, checkbox label sides, and search-result false positives.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use ostler_browser::{Browser, FillRequest, FillStatus, LaunchOptions, Policy, SubmitStatus};
use ostler_fixtures::FixtureServer;
use serde_json::{json, Value};

async fn setup() -> Option<(FixtureServer, Browser)> {
    if ostler_cdp::find_chrome(None).is_err() {
        assert!(
            std::env::var("OSTLER_REQUIRE_CHROME").is_err(),
            "Chrome required but not found"
        );
        eprintln!("SKIPPED: no Chrome found");
        return None;
    }
    let server = FixtureServer::start();
    let browser = Browser::launch(LaunchOptions::default(), Policy::default())
        .await
        .expect("launch");
    Some((server, browser))
}

fn req(v: Value) -> FillRequest {
    let values: BTreeMap<String, Value> = v
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    FillRequest {
        values,
        ..Default::default()
    }
}

/// Clean means the fill claims nothing is wrong: no unsettled work and no page error on the bad field.
fn says_clean(rep: &ostler_browser::FillReport) -> bool {
    rep.unsettled == 0 && rep.field_errors.is_empty() && rep.page_errors.is_empty()
}

#[tokio::test]
async fn w1_same_get_from_five_fields_is_not_ignored() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page().navigate(&srv.url("/recheck.html")).await.unwrap();
    let rep = b
        .page()
        .fill_form(
            req(json!({"Code one": "a", "Code two": "b", "Code three": "c", "Code four": "d", "Code five": "bad"})),
            None,
        )
        .await
        .unwrap();
    let late = rep
        .field_errors
        .iter()
        .any(|e| e.message.contains("not allowed"));
    assert!(
        late || !says_clean(&rep),
        "claimed clean too early: {rep:#?}"
    );
    assert!(late, "late error missed: {rep:#?}");
    b.close().await;
}

#[tokio::test]
async fn w1_debounce_then_slow_check_is_waited_for() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page()
        .navigate(&srv.url("/username_check.html"))
        .await
        .unwrap();
    let rep = b
        .page()
        .fill_form(req(json!({"Username": "admin"})), None)
        .await
        .unwrap();
    let late = rep
        .field_errors
        .iter()
        .any(|e| e.message.contains("already taken"));
    assert!(
        late || !says_clean(&rep),
        "claimed clean too early: {rep:#?}"
    );
    assert!(late, "late error missed: {rep:#?}");
    // A free name stays clean and is not slowed beyond the check itself.
    b.page()
        .navigate(&srv.url("/username_check.html"))
        .await
        .unwrap();
    let t0 = Instant::now();
    let rep = b
        .page()
        .fill_form(req(json!({"Username": "zed"})), None)
        .await
        .unwrap();
    assert!(
        rep.fields.iter().all(|f| f.status == FillStatus::Verified) && says_clean(&rep),
        "{rep:#?}"
    );
    assert!(
        t0.elapsed() < Duration::from_millis(4500),
        "{:?}",
        t0.elapsed()
    );
    b.close().await;
}

#[tokio::test]
async fn w4_checkbox_with_text_before_each_control() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page()
        .navigate(&srv.url("/checks_text.html"))
        .await
        .unwrap();
    let ir = b.page().forms().await.unwrap();
    let labels: Vec<String> = ir.forms[0].fields.iter().map(|f| f.label.clone()).collect();
    assert_eq!(labels, vec!["Email", "Phone"], "{ir:#?}");
    let rep = b
        .page()
        .fill_form(req(json!({"Phone": true})), None)
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    let st = b
        .page()
        .eval("[...document.querySelectorAll('input[type=checkbox]')].map(i=>i.checked)")
        .await
        .unwrap();
    assert_eq!(st, json!([false, true]), "{rep:#?}");
    b.close().await;
}

#[tokio::test]
async fn w3_successful_search_is_not_a_validation_failure() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page()
        .navigate(&srv.url("/search_header.html"))
        .await
        .unwrap();
    b.page()
        .fill_form(req(json!({"Search": "chrome"})), None)
        .await
        .unwrap();
    let out = b.page().submit(None, None).await.unwrap();
    assert_ne!(out.status, SubmitStatus::ValidationFailed, "{out:#?}");
    assert!(out.page_errors.is_empty(), "{out:#?}");
    b.close().await;
}
