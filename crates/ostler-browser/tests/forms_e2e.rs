//! End-to-end form tests against a real Chrome and local fixtures.
//! Set OSTLER_REQUIRE_CHROME=1 (CI does) to fail instead of skip when Chrome is absent.

use std::collections::BTreeMap;
use std::time::Duration;

use ostler_browser::{
    AutoMode, Browser, FillRequest, FillStatus, LaunchOptions, Policy, SnapshotMode, SnapshotOpts,
    SubmitStatus,
};
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
    b.page().navigate(&srv.url("/basic.html")).await.unwrap();
    let rep = b
        .page()
        .fill_form(req(json!({"Full name": "Ada Lovelace", "email": "ada@example.com", "message": "Hello there", "newsletter": true})), None)
        .await
        .unwrap();
    assert_eq!(rep.fields.len(), 4, "{rep:#?}");
    assert!(
        rep.fields.iter().all(|f| f.status == FillStatus::Verified),
        "{rep:#?}"
    );
    assert!(rep.ok(), "{rep:#?}");
    let out = b.page().submit(None, None).await.unwrap();
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
    b.page()
        .navigate(&srv.url("/validation.html"))
        .await
        .unwrap();
    // Submitting empty must surface errors, not claim success.
    let out = b.page().submit(None, None).await.unwrap();
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
        .page()
        .fill_form(req(json!({"email": "nope", "age": 12, "zip": "123"})), None)
        .await
        .unwrap();
    assert_eq!(rep.field_errors.len(), 3, "{rep:#?}");
    // Correct them and succeed.
    let rep = b
        .page()
        .fill_form(
            req(json!({"email": "ada@example.com", "age": 36, "zip": "94107"})),
            None,
        )
        .await
        .unwrap();
    assert!(rep.field_errors.is_empty(), "{rep:#?}");
    let out = b.page().submit(None, None).await.unwrap();
    assert_eq!(out.status, SubmitStatus::Submitted, "{out:#?}");
    assert_eq!(srv.last().unwrap()["zip"], "94107");
    b.close().await;
}

#[tokio::test]
async fn dependent_dropdowns_in_one_call() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page()
        .navigate(&srv.url("/dependent.html"))
        .await
        .unwrap();
    // Keys deliberately given "out of order": state before country.
    let rep = b
        .page()
        .fill_form(req(json!({"state": "Bavaria", "Recipient": "Ada", "country": "Germany", "city": "Munich"})), None)
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    assert_eq!(rep.fields.len(), 4);
    let out = b.page().submit(None, None).await.unwrap();
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
    b.page().navigate(&srv.url("/wizard.html")).await.unwrap();
    // Premature submit on step 1 must fail validation, not advance.
    let out = b.page().submit(None, None).await.unwrap();
    assert_eq!(out.status, SubmitStatus::ValidationFailed, "{out:#?}");

    let rep = b
        .page()
        .fill_form(
            req(json!({"first name": "Ada", "last name": "Lovelace"})),
            None,
        )
        .await
        .unwrap();
    assert_eq!(rep.step.as_ref().map(|s| (s.index, s.total)), Some((1, 3)));
    assert_eq!(
        b.page().submit(None, None).await.unwrap().status,
        SubmitStatus::Advanced
    );

    let rep = b
        .page()
        .fill_form(
            req(json!({"email": "ada@example.com", "phone": "5551234567"})),
            None,
        )
        .await
        .unwrap();
    assert_eq!(rep.step.as_ref().map(|s| s.index), Some(2), "{rep:#?}");
    assert_eq!(
        b.page().submit(None, None).await.unwrap().status,
        SubmitStatus::Advanced
    );

    let rep = b
        .page()
        .fill_form(req(json!({"confirm the details": true})), None)
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    let out = b.page().submit(None, None).await.unwrap();
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
    b.page()
        .navigate(&srv.url("/controlled.html"))
        .await
        .unwrap();
    let rep = b.page().fill_form(req(json!({"First name": "Ada", "Last name": "Lovelace", "Company": "Analytical Engines"})), None).await.unwrap();
    assert!(rep.ok(), "{rep:#?}");
    b.page().submit(None, None).await.unwrap();
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
    b.page().navigate(&srv.url("/combobox.html")).await.unwrap();
    let rep = b
        .page()
        .fill_form(req(json!({"Country": "Germany"})), None)
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    b.page().submit(None, None).await.unwrap();
    assert_eq!(srv.last().unwrap()["country"], "Germany");
    // Ambiguous text must not guess.
    srv.reset();
    b.page().navigate(&srv.url("/combobox.html")).await.unwrap();
    let rep = b
        .page()
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
    b.page().navigate(&srv.url("/shadow.html")).await.unwrap();
    let snap = b.page().snapshot(SnapshotOpts::default()).await.unwrap();
    assert!(snap.text.contains("textbox \"Your name\""), "{}", snap.text);
    let rep = b
        .page()
        .fill_form(
            req(json!({"Your name": "Ada", "Email address": "ada@example.com"})),
            None,
        )
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    assert_eq!(
        b.page().submit(None, None).await.unwrap().status,
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
    b.page().navigate(&srv.url("/iframe.html")).await.unwrap();
    let rep = b
        .page()
        .fill_form(
            req(json!({"Subject": "Broken login", "Details": "Cannot sign in"})),
            None,
        )
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    let out = b.page().submit(None, None).await.unwrap();
    assert!(srv.wait_for(1, Duration::from_secs(3)), "{out:#?}");
    assert_eq!(srv.last().unwrap()["subject"], "Broken login");
    b.close().await;
}

#[tokio::test]
async fn rich_input_types() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page().navigate(&srv.url("/types.html")).await.unwrap();
    let cv = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(cv.path(), "cv").unwrap();
    let rep = b
        .page()
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
        b.page().submit(None, None).await.unwrap().status,
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
    b.page().navigate(&srv.url("/masked.html")).await.unwrap();
    let rep = b
        .page()
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
    b.page().navigate(&srv.url("/basic.html")).await.unwrap();
    let rep = b
        .page()
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
    b.page().navigate(&srv.url("/types.html")).await.unwrap();
    let full = b.page().snapshot(SnapshotOpts::default()).await.unwrap();
    assert!(full.text.contains("textbox \"Notes\""), "{}", full.text);
    b.page()
        .fill_form(req(json!({"Notes": "Window seat please"})), None)
        .await
        .unwrap();
    let delta = b
        .page()
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
    if ostler_cdp::find_chrome(None).is_err() {
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
    let err = b
        .page()
        .navigate(&srv.url("/basic.html"))
        .await
        .unwrap_err();
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
    b.page().navigate(&srv.url("/basic.html")).await.unwrap();
    let rep = b
        .page()
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
    b.page().navigate(&srv.url("/basic.html")).await.unwrap();
    let snap = b.page().snapshot(SnapshotOpts::default()).await.unwrap();
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
    b.page().navigate(&srv.url("/longform.html")).await.unwrap();
    let rep = b
        .page()
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
        b.page().submit(None, None).await.unwrap().status,
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
    // The settle bug made *every* action wait ~3 s, so the best of three still fails with it present,
    // while one slow run on a loaded CI machine does not.
    let mut nav = Duration::MAX;
    for _ in 0..3 {
        let t = std::time::Instant::now();
        b.page().navigate(&srv.url("/basic.html")).await.unwrap();
        nav = nav.min(t.elapsed());
    }
    let t = std::time::Instant::now();
    b.page()
        .fill_form(req(json!({"name": "A", "email": "a@b.co"})), None)
        .await
        .unwrap();
    let fill = t.elapsed();
    let t = std::time::Instant::now();
    b.page().submit(None, None).await.unwrap();
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

#[tokio::test]
async fn custom_aria_widgets_and_contenteditable() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page().navigate(&srv.url("/aria.html")).await.unwrap();
    let rep = b
        .page()
        .fill_form(
            req(json!({"Contact method": "Phone", "Weekly digest": true, "I agree to the terms": true, "Short bio": "Rust and browsers"})),
            None,
        )
        .await
        .unwrap();
    assert!(rep.unmatched.is_empty(), "{rep:#?}");
    assert_eq!(rep.failed(), 0, "{rep:#?}");
    b.page().submit(None, None).await.unwrap();
    let got = srv.last().unwrap();
    assert_eq!(got["contact"], "phone", "{got}");
    assert_eq!(got["digest"], true, "{got}");
    assert_eq!(got["tos"], true, "{got}");
    assert_eq!(got["bio"], "Rust and browsers", "{got}");
    b.close().await;
}

#[tokio::test]
async fn conditional_fields_appear_and_are_filled_in_order() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page()
        .navigate(&srv.url("/conditional.html"))
        .await
        .unwrap();
    // Fields for "Bug report" do not exist until the type is chosen; the caller passes them anyway.
    let rep = b
        .page()
        .fill_form(
            req(json!({"Request type": "Bug report", "Steps to reproduce": "Click save", "Severity": "High",
                       "I want to add contact details": true, "Email": "a@b.co"})),
            None,
        )
        .await
        .unwrap();
    assert!(rep.unmatched.is_empty(), "{rep:#?}");
    assert_eq!(rep.failed(), 0, "{rep:#?}");
    assert_eq!(
        b.page().submit(None, None).await.unwrap().status,
        SubmitStatus::Submitted
    );
    let got = srv.last().unwrap();
    assert_eq!(got["kind"], "bug");
    assert_eq!(got["steps"], "Click save");
    assert_eq!(got["severity"], "high");
    assert_eq!(got["email"], "a@b.co");
    b.close().await;
}

#[tokio::test]
async fn hidden_branch_is_not_filled_and_validation_reports_page_message() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page()
        .navigate(&srv.url("/conditional.html"))
        .await
        .unwrap();
    b.page()
        .fill_form(
            req(json!({"Request type": "Billing", "Invoice number": "12"})),
            None,
        )
        .await
        .unwrap();
    let out = b.page().submit(None, None).await.unwrap();
    assert_eq!(out.status, SubmitStatus::ValidationFailed, "{out:#?}");
    assert!(
        format!("{out:?}").contains("INV-1234"),
        "page message should be surfaced: {out:#?}"
    );
    assert!(srv.last().is_none());
    b.close().await;
}

#[tokio::test]
async fn calendar_date_picker_readonly_input() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page()
        .navigate(&srv.url("/datepicker.html"))
        .await
        .unwrap();
    let rep = b
        .page()
        .fill_form(
            req(json!({"Check-in date": "2026-12-24", "Guests": 2})),
            None,
        )
        .await
        .unwrap();
    assert_eq!(rep.failed(), 0, "{rep:#?}");
    b.page().submit(None, None).await.unwrap();
    let got = srv.last().unwrap();
    assert_eq!(got["checkin"], "2026-12-24", "{got}");
    assert_eq!(got["guests"], "2");
    b.close().await;
}

#[tokio::test]
async fn ambiguous_key_stays_ambiguous_after_its_rival_is_filled() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page()
        .navigate(&srv.url("/ambiguous.html"))
        .await
        .unwrap();
    // "phone" fits both fields equally. It must not win Shipping just because Billing got filled first.
    let rep = b
        .page()
        .fill_form(req(json!({"Billing phone": "111", "phone": "222"})), None)
        .await
        .unwrap();
    assert_eq!(rep.fields.len(), 1, "{rep:#?}");
    assert_eq!(rep.unmatched.len(), 1, "{rep:#?}");
    assert_eq!(rep.unmatched[0].key, "phone");
    assert!(!rep.ok(), "an unmatched key must make the report not ok");
    b.page().submit(None, None).await.unwrap();
    let got = srv.last().unwrap();
    assert_eq!(got["billing"], "111");
    assert_eq!(
        got["shipping"], "",
        "the ambiguous value must not be typed anywhere: {got}"
    );
    b.close().await;
}

#[tokio::test]
async fn form_remounted_mid_fill_is_followed_not_abandoned() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page().navigate(&srv.url("/remount.html")).await.unwrap();
    let rep = b
        .page()
        .fill_form(
            req(json!({"Name": "Ada", "Plan": "Team", "Seats": 5})),
            None,
        )
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    assert_eq!(rep.fields.len(), 3, "{rep:#?}");
    b.page().submit(None, None).await.unwrap();
    let got = srv.last().unwrap();
    assert_eq!(got["name"], "Ada", "{got}");
    assert_eq!(got["plan"], "team");
    assert_eq!(got["seats"], "5");
    b.close().await;
}

#[tokio::test]
async fn keys_are_reported_when_the_form_disappears() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page().navigate(&srv.url("/remount.html")).await.unwrap();
    // Remove the form as soon as the plan is chosen; "Seats" can never be filled.
    b.page()
        .eval(
            "document.getElementById('p').addEventListener('change',()=>setTimeout(()=>{document.getElementById('root').innerHTML='<p>Gone</p>'},0))",
        )
        .await
        .unwrap();
    let rep = b
        .page()
        .fill_form(req(json!({"Plan": "Team", "Seats": 5})), None)
        .await
        .unwrap();
    assert!(!rep.ok(), "{rep:#?}");
    assert!(
        rep.unmatched.iter().any(|u| u.key == "Seats"),
        "a dropped key must be reported: {rep:#?}"
    );
    b.close().await;
}

#[tokio::test]
async fn secrets_do_not_come_back_through_reports_or_snapshots() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page().navigate(&srv.url("/payment.html")).await.unwrap();
    let rep = b
        .page()
        .fill_form(
            req(json!({"Name on card": "Ada Lovelace", "Card number": "4111111111111111", "Account PIN": "9876"})),
            None,
        )
        .await
        .unwrap();
    // Report: masked, including any note text.
    let dump = format!("{rep:?}");
    assert!(
        !dump.contains("4111111111111111") && !dump.contains("9876"),
        "{dump}"
    );
    // Snapshot: the same page, read back later, must not echo them either.
    let snap = b
        .page()
        .snapshot(SnapshotOpts {
            mode: SnapshotMode::Full,
            scope: None,
            max_chars: None,
        })
        .await
        .unwrap();
    assert!(
        !snap.text.contains("4111111111111111") && !snap.text.contains("9876"),
        "{}",
        snap.text
    );
    assert!(
        snap.text.contains("Ada Lovelace"),
        "ordinary values stay visible: {}",
        snap.text
    );
    b.close().await;
}

/// A page on an allowed host fires a request at a forbidden host. It must never arrive.
async fn leak_attempt(policy: Policy) -> bool {
    let srv = FixtureServer::start();
    let b = Browser::launch(LaunchOptions::default(), policy)
        .await
        .unwrap();
    // 127.0.0.1 is the allowed host; "localhost" is a different host that reaches the same server.
    b.page().navigate(&srv.url("/basic.html")).await.unwrap();
    let evil = srv.url("/submit").replace("127.0.0.1", "localhost");
    b.page()
        .eval(&format!(
            "fetch({evil:?},{{method:'POST',mode:'no-cors',headers:{{'content-type':'text/plain'}},body:JSON.stringify({{leak:'secret'}})}}).catch(()=>{{}})"
        ))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(800)).await;
    let arrived = srv.last().is_some();
    b.close().await;
    arrived
}

#[tokio::test]
async fn allowlist_blocks_requests_to_other_hosts_at_the_network() {
    if ostler_cdp::find_chrome(None).is_err() {
        return;
    }
    // Control: with no allowlist the request goes through, so the test can actually detect a leak.
    assert!(
        leak_attempt(Policy::default()).await,
        "control failed: the request never arrived"
    );
    let restricted = leak_attempt(Policy {
        allowed_domains: vec!["127.0.0.1".into()],
        allow_file: false,
    })
    .await;
    assert!(!restricted, "request to a forbidden host got through");
}

#[tokio::test]
async fn submit_gate_blocks_every_path_except_a_confirmed_submit() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.set_submit_gate(true).await.unwrap();
    b.page().navigate(&srv.url("/basic.html")).await.unwrap();
    b.page()
        .fill_form(
            req(json!({"Full name": "Ada Lovelace", "email": "ada@example.com", "message": "Hello there"})),
            None,
        )
        .await
        .unwrap();
    let ir = b.page().forms().await.unwrap();
    let submit_ref = ir.forms[0].submit[0].r#ref.clone();

    // Clicking the submit button directly, and pressing Enter in a field, both try to POST.
    let out = b.page().click(&submit_ref).await.unwrap();
    assert!(srv.last().is_none(), "click bypassed the gate");
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("require-confirm-submit")),
        "the agent must be told why nothing happened: {out:?}"
    );
    let name_ref = ir.forms[0].fields[0].r#ref.clone();
    b.page().click(&name_ref).await.unwrap();
    let _ = b.page().press("Enter").await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(srv.last().is_none(), "Enter bypassed the gate");

    // The confirmed path still works, on the same page the blocked attempts left intact.
    let res = b.page().submit(None, None).await.unwrap();
    assert_eq!(res.status, SubmitStatus::Submitted, "{res:#?}");
    assert_eq!(srv.last().unwrap()["name"], "Ada Lovelace");
    b.close().await;
}
