//! End-to-end form tests against a real Chrome and local fixtures.
//! Set WAYMARKER_REQUIRE_CHROME=1 (CI does) to fail instead of skip when Chrome is absent.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};
use waymarker_browser::{
    AutoMode, Browser, FillRequest, FillStatus, LaunchOptions, Policy, SnapshotMode, SnapshotOpts,
    SubmitStatus,
};
use waymarker_fixtures::FixtureServer;

async fn setup() -> Option<(FixtureServer, Browser)> {
    if waymarker_cdp::find_chrome(None).is_err() {
        assert!(
            std::env::var("WAYMARKER_REQUIRE_CHROME").is_err(),
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

fn vals(v: Value) -> BTreeMap<String, Value> {
    v.as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

fn req(v: Value) -> FillRequest {
    FillRequest {
        values: vals(v),
        ..Default::default()
    }
}

#[tokio::test]
async fn basic_fill_and_native_post() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page.navigate(&srv.url("/basic.html")).await.unwrap();
    let rep = b
        .page
        .fill_form(req(json!({"Full name": "Ada Lovelace", "email": "ada@example.com", "message": "Hello there", "newsletter": true})), None)
        .await
        .unwrap();
    assert_eq!(rep.fields.len(), 4, "{rep:#?}");
    assert!(
        rep.fields.iter().all(|f| f.status == FillStatus::Verified),
        "{rep:#?}"
    );
    assert!(rep.ok(), "{rep:#?}");
    let out = b.page.submit(None, None).await.unwrap();
    assert_eq!(out.status, SubmitStatus::Submitted, "{out:#?}");
    assert!(out.action.navigated);
    assert!(
        out.excerpt.iter().any(|l| l.contains("Thank you")),
        "{:?}",
        out.excerpt
    );
    let got = srv.last().unwrap();
    assert_eq!(got["name"], "Ada Lovelace");
    assert_eq!(got["email"], "ada@example.com");
    assert_eq!(got["message"], "Hello there");
    assert_eq!(got["newsletter"], "yes");
    b.close().await;
}

#[tokio::test]
async fn validation_errors_are_reported_and_recoverable() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page.navigate(&srv.url("/validation.html")).await.unwrap();
    // Submitting empty must surface errors, not claim success.
    let out = b.page.submit(None, None).await.unwrap();
    assert_eq!(out.status, SubmitStatus::ValidationFailed, "{out:#?}");
    assert!(
        out.field_errors
            .iter()
            .any(|e| e.message.contains("valid email")),
        "{:?}",
        out.field_errors
    );
    assert!(srv.submissions().is_empty());
    // Bad values: fill succeeds but the page's complaints show up in the report.
    let rep = b
        .page
        .fill_form(req(json!({"email": "nope", "age": 12, "zip": "123"})), None)
        .await
        .unwrap();
    assert_eq!(rep.field_errors.len(), 3, "{rep:#?}");
    // Correct them and succeed.
    let rep = b
        .page
        .fill_form(
            req(json!({"email": "ada@example.com", "age": 36, "zip": "94107"})),
            None,
        )
        .await
        .unwrap();
    assert!(rep.field_errors.is_empty(), "{rep:#?}");
    let out = b.page.submit(None, None).await.unwrap();
    assert_eq!(out.status, SubmitStatus::Submitted, "{out:#?}");
    assert_eq!(srv.last().unwrap()["zip"], "94107");
    b.close().await;
}

#[tokio::test]
async fn dependent_dropdowns_in_one_call() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page.navigate(&srv.url("/dependent.html")).await.unwrap();
    // Keys deliberately given "out of order": state before country.
    let rep = b
        .page
        .fill_form(req(json!({"state": "Bavaria", "Recipient": "Ada", "country": "Germany", "city": "Munich"})), None)
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    assert_eq!(rep.fields.len(), 4);
    let out = b.page.submit(None, None).await.unwrap();
    assert_eq!(out.status, SubmitStatus::Submitted, "{out:#?}");
    let got = srv.last().unwrap();
    assert_eq!(
        (got["country"].as_str(), got["state"].as_str()),
        (Some("de"), Some("by"))
    );
    b.close().await;
}

#[tokio::test]
async fn multi_step_wizard() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page.navigate(&srv.url("/wizard.html")).await.unwrap();
    // Premature submit on step 1 must fail validation, not advance.
    let out = b.page.submit(None, None).await.unwrap();
    assert_eq!(out.status, SubmitStatus::ValidationFailed, "{out:#?}");

    let rep = b
        .page
        .fill_form(
            req(json!({"first name": "Ada", "last name": "Lovelace"})),
            None,
        )
        .await
        .unwrap();
    assert_eq!(rep.step.as_ref().map(|s| (s.index, s.total)), Some((1, 3)));
    assert_eq!(
        b.page.submit(None, None).await.unwrap().status,
        SubmitStatus::Advanced
    );

    let rep = b
        .page
        .fill_form(
            req(json!({"email": "ada@example.com", "phone": "5551234567"})),
            None,
        )
        .await
        .unwrap();
    assert_eq!(rep.step.as_ref().map(|s| s.index), Some(2), "{rep:#?}");
    assert_eq!(
        b.page.submit(None, None).await.unwrap().status,
        SubmitStatus::Advanced
    );

    let rep = b
        .page
        .fill_form(req(json!({"confirm the details": true})), None)
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    let out = b.page.submit(None, None).await.unwrap();
    assert_eq!(out.status, SubmitStatus::Submitted, "{out:#?}");
    let got = srv.last().unwrap();
    assert_eq!(got["first_name"], "Ada");
    assert_eq!(got["email"], "ada@example.com");
    b.close().await;
}

#[tokio::test]
async fn controlled_inputs_are_not_silently_dropped() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page.navigate(&srv.url("/controlled.html")).await.unwrap();
    let rep = b.page.fill_form(req(json!({"First name": "Ada", "Last name": "Lovelace", "Company": "Analytical Engines"})), None).await.unwrap();
    assert!(rep.ok(), "{rep:#?}");
    b.page.submit(None, None).await.unwrap();
    let got = srv.last().unwrap();
    assert_eq!(got["first"], "Ada");
    assert_eq!(got["company"], "Analytical Engines");
    b.close().await;
}

#[tokio::test]
async fn custom_combobox_picks_the_option() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page.navigate(&srv.url("/combobox.html")).await.unwrap();
    let rep = b
        .page
        .fill_form(req(json!({"Country": "Germany"})), None)
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    b.page.submit(None, None).await.unwrap();
    assert_eq!(srv.last().unwrap()["country"], "Germany");
    // Ambiguous text must not guess.
    srv.reset();
    b.page.navigate(&srv.url("/combobox.html")).await.unwrap();
    let rep = b
        .page
        .fill_form(req(json!({"Country": "United"})), None)
        .await
        .unwrap();
    assert_eq!(rep.failed(), 1, "{rep:#?}");
    b.close().await;
}

#[tokio::test]
async fn shadow_dom_form() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page.navigate(&srv.url("/shadow.html")).await.unwrap();
    let snap = b.page.snapshot(SnapshotOpts::default()).await.unwrap();
    assert!(snap.text.contains("textbox \"Your name\""), "{}", snap.text);
    let rep = b
        .page
        .fill_form(
            req(json!({"Your name": "Ada", "Email address": "ada@example.com"})),
            None,
        )
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    assert_eq!(
        b.page.submit(None, None).await.unwrap().status,
        SubmitStatus::Submitted
    );
    assert_eq!(srv.last().unwrap()["email"], "ada@example.com");
    b.close().await;
}

#[tokio::test]
async fn same_origin_iframe_form() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page.navigate(&srv.url("/iframe.html")).await.unwrap();
    let rep = b
        .page
        .fill_form(
            req(json!({"Subject": "Broken login", "Details": "Cannot sign in"})),
            None,
        )
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    let out = b.page.submit(None, None).await.unwrap();
    assert!(srv.wait_for(1, Duration::from_secs(3)), "{out:#?}");
    assert_eq!(srv.last().unwrap()["subject"], "Broken login");
    b.close().await;
}

#[tokio::test]
async fn rich_input_types() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page.navigate(&srv.url("/types.html")).await.unwrap();
    let cv = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(cv.path(), "cv").unwrap();
    let rep = b
        .page
        .fill_form(
            req(json!({
                "Date of birth": "March 5, 2001", "Preferred time": "3:30 PM", "Guests": 4, "Satisfaction": 7,
                "Plan": "Pro", "Interests": ["AI", "Data"], "Dietary needs": ["Vegan"],
                "CV": cv.path().to_str().unwrap(), "Notes": "Window seat please", "terms": "yes"
            })),
            None,
        )
        .await
        .unwrap();
    assert!(rep.unmatched.is_empty(), "{rep:#?}");
    assert_eq!(rep.failed(), 0, "{rep:#?}");
    assert_eq!(
        b.page.submit(None, None).await.unwrap().status,
        SubmitStatus::Submitted
    );
    let got = srv.last().unwrap();
    assert_eq!(got["dob"], "2001-03-05");
    assert_eq!(got["time"], "15:30");
    assert_eq!(got["guests"], "4");
    assert_eq!(got["plan"], "pro");
    assert_eq!(got["interests"], json!(["ai", "data"]));
    assert_eq!(got["diet"], json!(["vegan"]));
    assert_eq!(got["tos"], "1");
    assert!(got["cv"]
        .as_str()
        .unwrap()
        .ends_with(cv.path().file_name().unwrap().to_str().unwrap()));
    b.close().await;
}

#[tokio::test]
async fn masks_and_length_limits_are_handled_honestly() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page.navigate(&srv.url("/masked.html")).await.unwrap();
    let rep = b
        .page
        .fill_form(
            req(json!({"Mobile number": "5551234567", "Short code": "12345"})),
            None,
        )
        .await
        .unwrap();
    let phone = rep
        .fields
        .iter()
        .find(|f| f.label.contains("Mobile"))
        .unwrap();
    assert_eq!(phone.status, FillStatus::Reformatted, "{phone:#?}");
    assert_eq!(phone.observed, "(555) 123-4567");
    let code = rep
        .fields
        .iter()
        .find(|f| f.label.contains("code"))
        .unwrap();
    assert_eq!(code.status, FillStatus::Failed, "{code:#?}");
    assert!(
        code.note.as_deref().unwrap().contains("allows 4"),
        "{code:#?}"
    );
    b.close().await;
}

#[tokio::test]
async fn unmatched_and_ambiguous_keys_are_reported_not_guessed() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page.navigate(&srv.url("/basic.html")).await.unwrap();
    let rep = b
        .page
        .fill_form(
            req(json!({"favourite colour": "blue", "email": "a@b.co"})),
            None,
        )
        .await
        .unwrap();
    assert_eq!(rep.unmatched.len(), 1);
    assert_eq!(rep.unmatched[0].key, "favourite colour");
    assert_eq!(
        rep.missing_required.len(),
        1,
        "name is still required: {rep:#?}"
    );
    b.close().await;
}

#[tokio::test]
async fn delta_snapshots_shrink_output() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page.navigate(&srv.url("/types.html")).await.unwrap();
    let full = b.page.snapshot(SnapshotOpts::default()).await.unwrap();
    assert!(full.text.contains("textbox \"Notes\""), "{}", full.text);
    b.page
        .fill_form(req(json!({"Notes": "Window seat please"})), None)
        .await
        .unwrap();
    let delta = b
        .page
        .snapshot(SnapshotOpts {
            mode: SnapshotMode::Delta,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(delta.text.contains("Window seat"), "{}", delta.text);
    assert!(
        delta.text.len() * 3 < full.text.len(),
        "delta {} vs full {}",
        delta.text.len(),
        full.text.len()
    );
    b.close().await;
}

#[tokio::test]
async fn policy_blocks_disallowed_domains() {
    if waymarker_cdp::find_chrome(None).is_err() {
        return;
    }
    let srv = FixtureServer::start();
    let policy = Policy {
        allowed_domains: vec!["example.org".into()],
        allow_file: false,
    };
    let b = Browser::launch(LaunchOptions::default(), policy)
        .await
        .unwrap();
    let err = b.page.navigate(&srv.url("/basic.html")).await.unwrap_err();
    assert!(
        err.to_string().contains("not in the allowed domains"),
        "{err}"
    );
    b.close().await;
}

#[tokio::test]
async fn auto_mode_without_suggester_is_a_noop() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page.navigate(&srv.url("/basic.html")).await.unwrap();
    let rep = b
        .page
        .fill_form(
            FillRequest {
                auto: AutoMode::All,
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    assert!(rep.fields.is_empty());
    assert_eq!(rep.missing_required.len(), 2);
    b.close().await;
}

#[tokio::test]
async fn snapshot_keeps_controls_wrapped_in_labels() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page.navigate(&srv.url("/basic.html")).await.unwrap();
    let snap = b.page.snapshot(SnapshotOpts::default()).await.unwrap();
    assert!(
        snap.text.contains("checkbox \"Subscribe to newsletter\""),
        "{}",
        snap.text
    );
    // The label text is the control's name; it must not also appear as a duplicate text line.
    assert!(
        !snap.text.contains("text \"Subscribe to newsletter\""),
        "{}",
        snap.text
    );
    b.close().await;
}

#[tokio::test]
async fn long_form_fills_in_one_call() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page.navigate(&srv.url("/longform.html")).await.unwrap();
    let rep = b
        .page
        .fill_form(
            req(json!({
                "First name": "Ada", "Last name": "Lovelace", "Email": "ada@example.com", "Phone": "5551234567",
                "Address line 1": "12 Analytical St", "City": "San Francisco", "State": "California", "ZIP code": "94107",
                "Country": "United States", "Current company": "Analytical Engines", "Job title": "Engineer",
                "Earliest start date": "2026-12-01", "Expected salary": 150000, "How did you hear": "Web search",
                "Cover letter": "Hello", "Willing to relocate": true, "privacy policy": true
            })),
            None,
        )
        .await
        .unwrap();
    assert!(rep.unmatched.is_empty(), "{rep:#?}");
    assert_eq!(rep.failed(), 0, "{rep:#?}");
    assert_eq!(rep.fields.len(), 17);
    assert_eq!(
        b.page.submit(None, None).await.unwrap().status,
        SubmitStatus::Submitted
    );
    let got = srv.last().unwrap();
    assert_eq!(
        (
            got["state"].as_str(),
            got["country"].as_str(),
            got["salary"].as_str()
        ),
        (Some("ca"), Some("us"), Some("150000"))
    );
    b.close().await;
}

/// Regression guard: settle used to count its own polling as page activity and burned its full
/// 3 s cap on every action. A whole fill+submit on a simple page must stay far below that.
#[tokio::test]
async fn actions_settle_quickly() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    let t = std::time::Instant::now();
    b.page.navigate(&srv.url("/basic.html")).await.unwrap();
    let nav = t.elapsed();
    let t = std::time::Instant::now();
    b.page
        .fill_form(req(json!({"name": "A", "email": "a@b.co"})), None)
        .await
        .unwrap();
    let fill = t.elapsed();
    let t = std::time::Instant::now();
    b.page.submit(None, None).await.unwrap();
    let submit = t.elapsed();
    assert!(nav < Duration::from_millis(1500), "navigate took {nav:?}");
    assert!(
        fill < Duration::from_millis(1500),
        "2-field fill took {fill:?}"
    );
    assert!(
        submit < Duration::from_millis(2000),
        "submit took {submit:?}"
    );
    b.close().await;
}
