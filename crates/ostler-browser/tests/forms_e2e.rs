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
    setup_with(Policy::default()).await
}

async fn setup_with(policy: Policy) -> Option<(FixtureServer, Browser)> {
    if ostler_cdp::find_chrome(None).is_err() {
        assert!(
            std::env::var("OSTLER_REQUIRE_CHROME").is_err(),
            "Chrome required but not found"
        );
        eprintln!("SKIPPED: no Chrome found");
        return None;
    }
    let server = FixtureServer::start();
    let browser = Browser::launch(LaunchOptions::default(), policy)
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
async fn datetime_local_is_verified_not_assumed() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page().navigate(&srv.url("/datetime.html")).await.unwrap();
    let rep = b
        .page()
        .fill_form(req(json!({"Start": "2001-03-05 3:30 PM"})), None)
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    // The page rewrites this one after we write it: that must be a failure, not "verified".
    let rep = b
        .page()
        .fill_form(req(json!({"Clamped slot": "2001-03-05T15:30"})), None)
        .await
        .unwrap();
    assert_eq!(rep.failed(), 1, "{rep:#?}");
    b.close().await;
}

#[tokio::test]
async fn combobox_that_diverges_from_the_request_fails() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page()
        .navigate(&srv.url("/combobox_diverge.html"))
        .await
        .unwrap();
    let rep = b
        .page()
        .fill_form(req(json!({"Country": "United States"})), None)
        .await
        .unwrap();
    assert_eq!(rep.failed(), 1, "{rep:#?}");
    // Unrelated exact options still verify.
    b.page()
        .navigate(&srv.url("/combobox_diverge.html"))
        .await
        .unwrap();
    let rep = b
        .page()
        .fill_form(req(json!({"Country": "Uruguay"})), None)
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    b.close().await;
}

#[tokio::test]
async fn an_action_that_navigates_is_not_replayed_on_the_next_page() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page().navigate(&srv.url("/replay_a.html")).await.unwrap();
    let snap = b.page().snapshot(SnapshotOpts::default()).await.unwrap();
    let r = snap
        .text
        .lines()
        .find(|l| l.contains("button \"Go\""))
        .and_then(|l| l.split('[').nth(1))
        .and_then(|l| l.split(']').next())
        .unwrap_or_else(|| panic!("Go button in snapshot: {}", snap.text))
        .to_string();
    // The button is under an overlay, so this takes the script-click path. The click posts once and
    // navigates; the page that replaces it has a button with the same look and must not be clicked.
    b.page()
        .click(&r)
        .await
        .expect("click that navigates succeeds");
    srv.wait_for(1, Duration::from_secs(3));
    tokio::time::sleep(Duration::from_millis(400)).await;
    let hits: Vec<_> = srv.submissions().iter().map(|s| s["hit"].clone()).collect();
    assert_eq!(
        hits,
        vec![json!("A")],
        "replayed on the next page: {hits:?}"
    );
    b.close().await;
}

#[tokio::test]
async fn eval_that_loses_its_page_is_replayed_but_eval_once_is_not() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    // Posts a hit, schedules a navigation, then keeps awaiting: the page is replaced mid-evaluation.
    let js = "(async()=>{fetch('/submit',{method:'POST',keepalive:true,headers:{'content-type':'application/json'},body:JSON.stringify({hit:'js'})});setTimeout(()=>{location.href='/replay_b.html'},20);await new Promise(r=>setTimeout(r,3000));return 1})()";
    b.page().navigate(&srv.url("/replay_a.html")).await.unwrap();
    let _ = b.page().eval_once(js).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        srv.submissions().len(),
        1,
        "eval_once must run the script at most once"
    );
    b.close().await;
}

#[tokio::test]
async fn verification_walls_are_detected_but_not_over_reported() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    // An embedded widget blocks the form.
    b.page()
        .navigate(&srv.url("/captcha_widget.html"))
        .await
        .unwrap();
    let c = b
        .page()
        .challenge()
        .await
        .unwrap()
        .expect("widget detected");
    assert_eq!((c.vendor.as_str(), c.interstitial), ("reCAPTCHA", false));
    // The person solves it; the wall is gone.
    let snap = b.page().snapshot(SnapshotOpts::default()).await.unwrap();
    let solve = snap
        .text
        .lines()
        .find(|l| l.contains("Simulate the person"))
        .and_then(|l| l.split('[').nth(1))
        .and_then(|l| l.split(']').next())
        .unwrap_or_else(|| panic!("solve button: {}", snap.text))
        .to_string();
    b.page().click(&solve).await.unwrap();
    assert!(b.page().challenge().await.unwrap().is_none());

    // A page that replaces the site with a check.
    b.page()
        .navigate(&srv.url("/captcha_interstitial.html"))
        .await
        .unwrap();
    let c = b
        .page()
        .challenge()
        .await
        .unwrap()
        .expect("interstitial detected");
    assert!(c.interstitial, "{c:?}");

    // A search engine's bot wall is a form with only a submit button; it is still a wall.
    b.page()
        .navigate(&srv.url("/captcha_search_wall.html"))
        .await
        .unwrap();
    let c = b
        .page()
        .challenge()
        .await
        .unwrap()
        .expect("search wall detected");
    assert_eq!((c.vendor.as_str(), c.interstitial), ("DuckDuckGo", true));

    // Not walls: an invisible-v3 badge, a plain form, a long page that merely mentions verification.
    for page in ["/captcha_badge_only.html", "/basic.html", "/longform.html"] {
        b.page().navigate(&srv.url(page)).await.unwrap();
        assert_eq!(b.page().challenge().await.unwrap(), None, "{page}");
    }
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
    let dir = tempfile::tempdir().unwrap();
    let cv_path = dir.path().join("cv.txt");
    std::fs::write(&cv_path, "cv").unwrap();
    let Some((srv, b)) = setup_with(Policy {
        upload_dirs: vec![dir.path().to_path_buf()],
        ..Default::default()
    })
    .await
    else {
        return;
    };
    b.page().navigate(&srv.url("/types.html")).await.unwrap();
    let rep = b
        .page()
        .fill_form(
            req(json!({
                "Date of birth": "March 5, 2001", "Preferred time": "3:30 PM", "Guests": 4, "Satisfaction": 7,
                "Plan": "Pro", "Interests": ["AI", "Data"], "Dietary needs": ["Vegan"],
                "CV": cv_path.to_str().unwrap(), "Notes": "Window seat please", "terms": "yes"
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
        .ends_with(cv_path.file_name().unwrap().to_str().unwrap()));
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
        ..Default::default()
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
            req(json!({"Name on card": "Ada Lovelace", "Card number": "4111111111111111", "Account PIN": "9876", "Passport no.": "X1234567"})),
            None,
        )
        .await
        .unwrap();
    // Report: masked, including any note text.
    let dump = format!("{rep:?}");
    assert!(
        !dump.contains("4111111111111111") && !dump.contains("9876") && !dump.contains("X1234567"),
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
        !snap.text.contains("4111111111111111")
            && !snap.text.contains("9876")
            && !snap.text.contains("X1234567"),
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
        ..Default::default()
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

#[tokio::test]
async fn visual_only_error_cues_are_reported() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page().navigate(&srv.url("/visual.html")).await.unwrap();
    let out = b.page().submit(None, None).await.unwrap();
    assert_eq!(out.status, SubmitStatus::ValidationFailed, "{out:#?}");
    let msgs = format!("{out:?}");
    // Red text with no ARIA/class, red border with no text, and a floating toast.
    assert!(msgs.contains("does not look like an email"), "{msgs}");
    assert!(msgs.contains("red border"), "{msgs}");
    assert!(msgs.contains("Could not send your message"), "{msgs}");
    assert!(srv.last().is_none());
    // Fixing the fields clears every cue and submits.
    let rep = b
        .page()
        .fill_form(
            req(json!({"email": "ada@example.com", "phone": "+1 415 555 0100"})),
            None,
        )
        .await
        .unwrap();
    // The page only re-validates on submit, so leftover cues are flagged as possibly stale.
    assert!(
        rep.field_errors
            .iter()
            .all(|e| e.message.contains("shown before this fill")),
        "{rep:#?}"
    );
    let out = b.page().submit(None, None).await.unwrap();
    assert_eq!(out.status, SubmitStatus::Submitted, "{out:#?}");
    assert_eq!(srv.last().unwrap()["phone"], "+1 415 555 0100");
    b.close().await;
}

#[tokio::test]
async fn look_lists_click_targets_and_cleans_up_its_overlay() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page().navigate(&srv.url("/visual.html")).await.unwrap();
    // Trigger the error state so the legend has to carry it.
    b.page().submit(None, None).await.unwrap();
    let (png, list) = b.page().look(40).await.unwrap();
    assert!(png.starts_with(b"\x89PNG"), "not a PNG");
    let items = list["items"].as_array().unwrap();
    let find = |name: &str| {
        items
            .iter()
            .find(|t| t["name"] == name)
            .unwrap_or_else(|| panic!("{name} missing: {list}"))
    };
    assert_eq!(find("Send")["purpose"], "submits the form");
    assert_eq!(find("Email")["purpose"], "type here");
    let email_state = find("Email")["state"].to_string();
    assert!(email_state.contains("error"), "{email_state}");
    assert_eq!(list["covered"], 0);
    // Boxes are removed again, so later snapshots and screenshots are clean.
    let left = b
        .page()
        .eval("document.getElementById('__wm_marks') === null")
        .await
        .unwrap();
    assert_eq!(left, json!(true));
    b.close().await;
}

#[tokio::test]
async fn look_skips_elements_hidden_under_an_overlay() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page().navigate(&srv.url("/overlay.html")).await.unwrap();
    let (_, list) = b.page().look(40).await.unwrap();
    let names: Vec<String> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap_or("").to_string())
        .collect();
    // Only the dialog's button is reachable; the form behind it is reported as covered.
    assert_eq!(names, vec!["Accept all"], "{list}");
    assert_eq!(list["covered"], 2, "{list}");
    b.close().await;
}

#[tokio::test]
async fn text_a_person_cannot_see_is_not_shown_to_the_agent() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page()
        .navigate(&srv.url("/concealed.html"))
        .await
        .unwrap();
    let snap = b.page().snapshot(SnapshotOpts::default()).await.unwrap();
    for hidden in [
        "OPACITY_INJECTION",
        "FONT_INJECTION",
        "OFFSCREEN_INJECTION",
        "WHITE_INJECTION",
        "NESTED_INJECTION",
        "MIXED_INJECTION",
        "LABEL_INJECTION",
    ] {
        assert!(
            !snap.text.contains(hidden),
            "{hidden} reached the agent:\n{}",
            snap.text
        );
    }
    // What a person sees, and what is meant for assistive tech, stays.
    for shown in [
        "VISIBLE_PARAGRAPH",
        "tail text stays",
        "SCREENREADER_ONLY",
        "Accept terms",
    ] {
        assert!(
            snap.text.contains(shown),
            "{shown} was dropped:\n{}",
            snap.text
        );
    }
    // The agent is told something was left out instead of silently losing it.
    assert!(
        snap.text.contains("invisible text run(s) omitted"),
        "{}",
        snap.text
    );
    // A control that is itself invisible (native checkbox under a custom one) must remain usable.
    let rep = b
        .page()
        .fill_form(req(json!({"Accept terms": true, "Name": "Ada"})), None)
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    b.close().await;
}

#[tokio::test]
async fn long_text_is_split_and_cuts_are_marked() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page().navigate(&srv.url("/article.html")).await.unwrap();
    let s = b.page().snapshot(SnapshotOpts::default()).await.unwrap();
    assert!(s.text.contains("FINALWORD."), "{}", s.text);
    assert!(s.text.lines().all(|l| l.len() < 700), "{}", s.text);
    assert!(s.text.contains("more chars cut]"), "{}", s.text);
    b.close().await;
}

#[tokio::test]
async fn footer_collapses_and_expands_by_ref() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page()
        .navigate(&srv.url("/bigfooter.html"))
        .await
        .unwrap();
    let full = b.page().snapshot(SnapshotOpts::default()).await.unwrap();
    assert!(full.text.contains("Link 30"), "{}", full.text);
    let i = b
        .page()
        .snapshot(SnapshotOpts {
            mode: SnapshotMode::Interactive,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        i.text.contains("30 links omitted; snapshot ref=e"),
        "{}",
        i.text
    );
    assert!(!i.text.contains("Link 30"), "{}", i.text);
    let r = i
        .text
        .split("snapshot ref=")
        .nth(1)
        .unwrap()
        .split(' ')
        .next()
        .unwrap()
        .to_string();
    let open = b
        .page()
        .snapshot(SnapshotOpts {
            scope: Some(r),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(open.text.contains("Link 30"), "{}", open.text);
    b.close().await;
}

#[tokio::test]
async fn full_page_screenshot_does_not_stall_when_a_popup_took_the_front() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page().navigate(&srv.url("/basic.html")).await.unwrap();
    b.page()
        .eval("document.body.style.minHeight = '150000px'; window.open('about:blank'); 0")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(
        b.tabs().await.len() >= 2,
        "the popup should have become a tab"
    );
    let started = std::time::Instant::now();
    let png = b.page().screenshot(None, true, false).await.unwrap();
    assert!(png.len() > 1000, "empty capture");
    // Before the fix Chrome produced no frame for the hidden tab and this waited out the 30 s timeout.
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "took {:?}",
        started.elapsed()
    );
    b.close().await;
}

#[tokio::test]
async fn refs_survive_truncation_and_unknown_ref_is_actionable() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page()
        .navigate(&srv.url("/bigfooter.html"))
        .await
        .unwrap();
    let s = b
        .page()
        .snapshot(SnapshotOpts {
            max_chars: Some(150),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(s.truncated);
    // Past the cut, but numbered in document order (main: heading, Home link = e1; footer links follow).
    b.page().click("e20").await.unwrap();
    let err = b.page().click("e9999").await.unwrap_err().to_string();
    assert!(
        err.contains("page changed") && err.contains("snapshot"),
        "{err}"
    );
    b.close().await;
}

#[tokio::test]
async fn delta_after_navigating_to_similar_page_is_small() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page().navigate(&srv.url("/results.html")).await.unwrap();
    let full = b.page().snapshot(SnapshotOpts::default()).await.unwrap();
    b.page()
        .click("e42")
        .await
        .unwrap_or_else(|e| panic!("{e}\n{}", full.text));
    let d = b
        .page()
        .snapshot(SnapshotOpts {
            mode: SnapshotMode::Delta,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(d.text.contains("Result 41"), "{}", d.text);
    assert!(d.text.len() * 3 < full.text.len(), "{}", d.text);
    b.close().await;
}

#[tokio::test]
async fn wrapper_with_click_handler_does_not_swallow_the_page() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page().navigate(&srv.url("/spa.html")).await.unwrap();
    let i = b
        .page()
        .snapshot(SnapshotOpts {
            mode: SnapshotMode::Interactive,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(i.text.contains("Best commute pick"), "{}", i.text);
    assert!(i.text.contains("Compare"), "{}", i.text);
    // A one-link card footer is not boilerplate: no "0/1 links omitted" noise.
    assert!(!i.text.contains("footer ["), "{}", i.text);
    b.close().await;
}

#[tokio::test]
async fn delta_right_after_navigate_is_small() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page().navigate(&srv.url("/results.html")).await.unwrap();
    // The navigate view is interactive; the next delta must still have a baseline.
    b.page()
        .snapshot(SnapshotOpts {
            mode: SnapshotMode::Interactive,
            ..Default::default()
        })
        .await
        .unwrap();
    let d = b
        .page()
        .snapshot(SnapshotOpts {
            mode: SnapshotMode::Delta,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(d.text.contains("no changes"), "{}", d.text);
    b.close().await;
}

#[tokio::test]
async fn back_with_no_history_says_so() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page().navigate(&srv.url("/basic.html")).await.unwrap();
    // The tab started on about:blank, which is not a page to return to.
    let o = b.page().back().await.unwrap();
    assert!(!o.changed, "{o:#?}");
    assert!(
        o.notes.iter().any(|n| n.contains("no earlier page")),
        "{o:#?}"
    );
    b.close().await;
}

#[tokio::test]
async fn hash_and_query_changes_keep_the_delta_baseline() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    let url = srv.url("/results.html");
    b.page().navigate(&url).await.unwrap();
    b.page()
        .snapshot(SnapshotOpts {
            mode: SnapshotMode::Interactive,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(b.page().baseline_matches(&format!("{url}#top")));
    assert!(b.page().baseline_matches(&format!("{url}?first=11")));
    assert!(!b.page().baseline_matches(&srv.url("/basic.html")));
    b.close().await;
}

#[tokio::test]
async fn a_value_the_page_clears_after_filling_is_reported_at_submit() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page().navigate(&srv.url("/drift.html")).await.unwrap();
    let rep = b
        .page()
        .fill_form(req(json!({"Day": "2026-12-25"})), None)
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    tokio::time::sleep(std::time::Duration::from_millis(2200)).await;
    let out = b.page().submit(None, None).await.unwrap();
    assert_eq!(out.drift.len(), 1, "{out:#?}");
    assert_eq!(out.drift[0].was, "2026-12-25");
    assert_eq!(out.drift[0].now, "");
    b.close().await;
}

#[tokio::test]
async fn a_widget_that_learns_values_from_keyup_keeps_what_was_filled() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page().navigate(&srv.url("/picker.html")).await.unwrap();
    let rep = b
        .page()
        .fill_form(req(json!({"Day": "12/25/2026"})), None)
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    // Pressing anywhere else makes the widget rewrite the box from what it learned.
    let snap = b.page().snapshot(SnapshotOpts::default()).await.unwrap();
    let other = snap
        .text
        .lines()
        .find(|l| l.contains("\"Other\""))
        .and_then(|l| l.split("[").nth(1))
        .and_then(|l| l.split("]").next())
        .unwrap()
        .to_string();
    b.page().click(&other).await.unwrap();
    let day = b.page().snapshot(SnapshotOpts::default()).await.unwrap();
    assert!(day.text.contains("12/25/2026"), "{}", day.text);
    b.close().await;
}

#[tokio::test]
async fn a_heading_ref_reads_its_section_only() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page().navigate(&srv.url("/sections.html")).await.unwrap();
    let full = b.page().snapshot(SnapshotOpts::default()).await.unwrap();
    let r = full
        .text
        .lines()
        .find(|l| l.contains("\"Technology\""))
        .and_then(|l| l.split("[").nth(1))
        .and_then(|l| l.split("]").next())
        .unwrap_or_else(|| panic!("no ref on heading: {}", full.text))
        .to_string();
    let s = b
        .page()
        .snapshot(SnapshotOpts {
            scope: Some(r),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(s.text.contains("Feedforward microphones"), "{}", s.text);
    assert!(s.text.contains("Detailed feedforward text"), "{}", s.text);
    assert!(!s.text.contains("History text"), "{}", s.text);
    assert!(!s.text.contains("Intro text"), "{}", s.text);
    b.close().await;
}

#[tokio::test]
async fn on_a_page_without_a_form_only_a_real_action_button_is_a_submit_candidate() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page().navigate(&srv.url("/formless.html")).await.unwrap();
    let ir = b.page().forms().await.unwrap();
    assert_eq!(ir.forms.len(), 1, "{ir:#?}");
    let labels: Vec<&str> = ir.forms[0]
        .submit
        .iter()
        .map(|s| s.label.as_str())
        .collect();
    // The header's "Search" button and the type-less "Show tips" button submit nothing.
    assert_eq!(labels, vec!["Save profile"], "{ir:#?}");
    b.close().await;
}

#[tokio::test]
async fn an_in_place_confirmation_after_an_xhr_post_counts_as_submitted() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page().navigate(&srv.url("/xhr_form.html")).await.unwrap();
    let rep = b
        .page()
        .fill_form(
            req(json!({"Your name": "Ada", "Your email": "ada@example.com"})),
            None,
        )
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    let out = b.page().submit(None, None).await.unwrap();
    // The URL did not change and the form is still showing, but the page said "has been sent".
    assert_eq!(out.status, SubmitStatus::Submitted, "{out:#?}");
    assert!(
        out.excerpt.iter().any(|l| l.contains("has been sent")),
        "{:?}",
        out.excerpt
    );
    assert_eq!(srv.last().unwrap()["email"], "ada@example.com");
    b.close().await;
}

#[tokio::test]
async fn an_xhr_form_that_says_nothing_stays_unchanged() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    // Same form, but the page never says anything after the post: we cannot know it worked.
    b.page()
        .navigate(&srv.url("/xhr_form.html?quiet=1"))
        .await
        .unwrap();
    let rep = b
        .page()
        .fill_form(
            req(json!({"Your name": "Ada", "Your email": "ada@example.com"})),
            None,
        )
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    let out = b.page().submit(None, None).await.unwrap();
    assert_eq!(out.status, SubmitStatus::Unchanged, "{out:#?}");
    b.close().await;
}

#[tokio::test]
async fn a_confirmation_page_with_an_optional_input_is_submitted_not_advanced() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page().navigate(&srv.url("/order.html")).await.unwrap();
    let rep = b
        .page()
        .fill_form(req(json!({"Item": "Horseshoe"})), None)
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    let out = b.page().submit(None, None).await.unwrap();
    assert_eq!(out.status, SubmitStatus::Submitted, "{out:#?}");
    assert!(out.next_form.is_none(), "{out:#?}");
    b.close().await;
}

#[tokio::test]
async fn a_field_error_is_attached_to_its_own_field_not_its_neighbours() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page()
        .navigate(&srv.url("/shared_errors.html"))
        .await
        .unwrap();
    let ir = b.page().forms().await.unwrap();
    let err = |label: &str| {
        ir.forms[0]
            .fields
            .iter()
            .find(|f| f.label == label)
            .unwrap_or_else(|| panic!("no field {label}: {ir:#?}"))
            .error
            .clone()
    };
    assert_eq!(err("First name"), None, "{ir:#?}");
    assert_eq!(err("Last name").as_deref(), Some("Last name is required"));
    // Help text is not an error.
    assert_eq!(err("Phone"), None, "{ir:#?}");
    b.close().await;
}

#[tokio::test]
async fn a_bare_submit_sends_the_form_that_was_filled_not_the_first_one() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page()
        .navigate(&srv.url("/two_forms.html"))
        .await
        .unwrap();
    let rep = b
        .page()
        .fill_form(req(json!({"Item": "Astrolabe"})), None)
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    let out = b.page().submit(None, None).await.unwrap();
    assert!(
        out.action.url.contains("item=Astrolabe") && !out.action.url.contains("nl="),
        "sent the wrong form: {}",
        out.action.url
    );
    b.close().await;
}

#[tokio::test]
async fn a_bare_submit_with_several_forms_and_nothing_filled_refuses_to_guess() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page()
        .navigate(&srv.url("/two_forms.html"))
        .await
        .unwrap();
    let err = b.page().submit(None, None).await.unwrap_err().to_string();
    assert!(
        err.contains("2 forms") && err.contains("pass `form`"),
        "{err}"
    );
    b.close().await;
}

#[tokio::test]
async fn an_http_500_after_submit_is_a_server_error_not_success() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page()
        .navigate(&srv.url("/server_error.html"))
        .await
        .unwrap();
    let rep = b
        .page()
        .fill_form(req(json!({"Message": "hello"})), None)
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    let out = b.page().submit(None, None).await.unwrap();
    assert_eq!(out.status, SubmitStatus::ServerError, "{out:#?}");
    b.close().await;
}

// ---------------------------------------------------------------- wave 2: correctness defects from the public-site trial

async fn checked_states(b: &Browser) -> Value {
    b.page()
        .eval("[...document.querySelectorAll('input[type=checkbox],input[type=radio]')].map(i=>i.checked)")
        .await
        .unwrap()
}

#[tokio::test]
async fn d2_checkbox_label_is_the_text_after_the_control() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page().navigate(&srv.url("/checks.html")).await.unwrap();
    let ir = b.page().forms().await.unwrap();
    let labels: Vec<String> = ir.forms[0].fields.iter().map(|f| f.label.clone()).collect();
    assert_eq!(labels, vec!["checkbox 1", "checkbox 2"], "{ir:#?}");
    // box 1 starts unchecked, box 2 checked: ask for the opposite of each.
    assert_eq!(checked_states(&b).await, json!([false, true]));
    let rep = b
        .page()
        .fill_form(req(json!({"checkbox 1": true, "checkbox 2": false})), None)
        .await
        .unwrap();
    assert_eq!(rep.fields.len(), 2, "{rep:#?}");
    assert!(rep.ok(), "{rep:#?}");
    assert_eq!(checked_states(&b).await, json!([true, false]), "{rep:#?}");
    b.close().await;
}

#[tokio::test]
async fn d5_options_of_a_group_are_addressable() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page().navigate(&srv.url("/choices.html")).await.unwrap();
    let ir = b.page().forms().await.unwrap();
    let f = &ir.forms[0].fields;
    assert_eq!(f[0].label, "Highest level of education", "{ir:#?}");
    let opts: Vec<&str> = f[0].options.iter().map(|o| o.label.as_str()).collect();
    assert_eq!(
        opts,
        vec!["High school", "College", "Grad school"],
        "{ir:#?}"
    );
    let opts: Vec<&str> = f[1].options.iter().map(|o| o.label.as_str()).collect();
    assert_eq!(opts, vec!["Email", "Text message", "Post"], "{ir:#?}");
    // group question + option label
    let rep = b
        .page()
        .fill_form(req(json!({"Highest level of education": "College"})), None)
        .await
        .unwrap();
    assert!(
        rep.ok() && rep.fields[0].status == FillStatus::Verified,
        "{rep:#?}"
    );
    assert_eq!(
        checked_states(&b).await,
        json!([false, true, false, false, false, false, false])
    );
    // the option label alone
    let rep = b
        .page()
        .fill_form(req(json!({"Grad school": true})), None)
        .await
        .unwrap();
    assert!(rep.ok() && rep.fields.len() == 1, "{rep:#?}");
    assert_eq!(
        checked_states(&b).await,
        json!([false, false, true, false, false, false, false])
    );
    // several checkbox options at once, each kept
    let rep = b
        .page()
        .fill_form(
            req(json!({"Email": true, "Post": true, "Default checkbox": true})),
            None,
        )
        .await
        .unwrap();
    assert!(rep.ok() && rep.fields.len() == 3, "{rep:#?}");
    assert_eq!(
        checked_states(&b).await,
        json!([false, false, true, true, false, true, true])
    );
    let rep = b
        .page()
        .fill_form(req(json!({"Email": false})), None)
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    assert_eq!(
        checked_states(&b).await,
        json!([false, false, true, false, false, true, true])
    );
    // a radio option cannot be unchecked: say so, do not pretend
    let rep = b
        .page()
        .fill_form(req(json!({"Grad school": false})), None)
        .await
        .unwrap();
    assert_eq!(rep.failed(), 1, "{rep:#?}");
    // the name attribute still addresses the whole group
    let rep = b
        .page()
        .fill_form(
            req(json!({"my-radio": "High school", "my-check": ["Text message", "Post"]})),
            None,
        )
        .await
        .unwrap();
    assert!(rep.ok() && rep.fields.len() == 2, "{rep:#?}");
    assert_eq!(
        checked_states(&b).await,
        json!([true, false, false, false, true, true, true])
    );
    b.close().await;
}

#[tokio::test]
async fn d4_combobox_widget_reads_back_the_displayed_value() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page()
        .navigate(&srv.url("/react_select.html"))
        .await
        .unwrap();
    let rep = b
        .page()
        .fill_form(req(json!({"Select State": "NCR"})), None)
        .await
        .unwrap();
    assert_eq!(rep.fields.len(), 1, "{rep:#?}");
    assert_eq!(rep.fields[0].status, FillStatus::Verified, "{rep:#?}");
    assert_eq!(rep.fields[0].observed, json!("NCR"), "{rep:#?}");
    let out = b.page().submit(None, None).await.unwrap();
    assert_eq!(out.status, SubmitStatus::Submitted, "{out:#?}");
    assert_eq!(srv.last().unwrap()["state"], "NCR");
    b.close().await;
}

#[tokio::test]
async fn d4_combobox_pick_that_did_not_take_still_fails() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page()
        .navigate(&srv.url("/react_select_broken.html"))
        .await
        .unwrap();
    let rep = b
        .page()
        .fill_form(req(json!({"Select State": "NCR"})), None)
        .await
        .unwrap();
    assert_eq!(rep.fields[0].status, FillStatus::Failed, "{rep:#?}");
    b.close().await;
}

#[tokio::test]
async fn d6_verified_value_that_violates_constraints_carries_a_warning() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page()
        .navigate(&srv.url("/constraints.html"))
        .await
        .unwrap();
    let rep = b
        .page()
        .fill_form(
            req(json!({"Pickup time": "23:00", "Contact email": "nope", "Code": "abc", "Quantity": 42, "Nickname": "x"})),
            None,
        )
        .await
        .unwrap();
    assert_eq!(rep.fields.len(), 5, "{rep:#?}");
    assert!(
        rep.fields.iter().all(|f| f.status == FillStatus::Verified),
        "{rep:#?}"
    );
    let w = |label: &str| {
        rep.fields
            .iter()
            .find(|f| f.label == label)
            .and_then(|f| f.warning.clone())
    };
    let t = w("Pickup time").expect("time warning");
    assert!(
        t.contains("23:00")
            && t.contains("outside the allowed range")
            && t.contains("11:00..21:00"),
        "{t}"
    );
    assert!(
        w("Contact email").expect("email").contains("email"),
        "{rep:#?}"
    );
    assert!(w("Code").expect("pattern").contains("pattern"), "{rep:#?}");
    assert!(w("Quantity").expect("number").contains("1..10"), "{rep:#?}");
    assert_eq!(w("Nickname"), None, "{rep:#?}");
    // A valid value carries none.
    let rep = b
        .page()
        .fill_form(req(json!({"Pickup time": "12:30", "Contact email": "a@b.co", "Code": "ABC-123", "Quantity": 3})), None)
        .await
        .unwrap();
    assert!(rep.fields.iter().all(|f| f.warning.is_none()), "{rep:#?}");
    b.close().await;
}

#[tokio::test]
async fn d1_text_date_field_gets_its_own_format() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page().navigate(&srv.url("/datefmt.html")).await.unwrap();
    let ir = b.page().forms().await.unwrap();
    assert_eq!(
        ir.forms[0].fields[0].format.as_deref(),
        Some("mm/dd/yyyy"),
        "{ir:#?}"
    );
    assert_eq!(
        ir.forms[0].fields[1].format.as_deref(),
        Some("dd-mm-yyyy"),
        "{ir:#?}"
    );
    let rep = b
        .page()
        .fill_form(
            req(json!({"Departure date": "2026-11-02", "Return date": "2026-11-09"})),
            None,
        )
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    assert_eq!(rep.fields[0].observed, json!("11/02/2026"), "{rep:#?}");
    assert_eq!(rep.fields[1].observed, json!("09-11-2026"), "{rep:#?}");
    assert!(rep.fields.iter().all(|f| f.warning.is_none()), "{rep:#?}");
    let out = b.page().submit(None, None).await.unwrap();
    assert_eq!(out.status, SubmitStatus::Submitted, "{out:#?}");
    let got = srv.last().unwrap();
    assert_eq!(got["d"], "11/02/2026");
    assert_eq!(got["r"], "09-11-2026");
    // Not a date in any known shape: typed as given, with a warning about the expected format.
    b.page().navigate(&srv.url("/datefmt.html")).await.unwrap();
    let rep = b
        .page()
        .fill_form(req(json!({"Departure date": "next tuesday"})), None)
        .await
        .unwrap();
    let w = rep.fields[0].warning.clone().expect("warning");
    assert!(w.contains("mm/dd/yyyy"), "{w}");
    b.close().await;
}

#[tokio::test]
async fn d3_bad_login_after_reload_is_always_not_accepted() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    for round in 0..3 {
        b.page()
            .navigate(&srv.url("/login_flash.html"))
            .await
            .unwrap();
        let rep = b
            .page()
            .fill_form(req(json!({"Username": "bad", "Password": "nope"})), None)
            .await
            .unwrap();
        assert!(rep.ok(), "{rep:#?}");
        let out = b.page().submit(None, None).await.unwrap();
        assert_eq!(
            out.status,
            SubmitStatus::ValidationFailed,
            "round {round}: {out:#?}"
        );
        assert!(
            out.page_errors
                .iter()
                .any(|e| e.contains("Your username is invalid")),
            "round {round}: {out:#?}"
        );
    }
    // The same round trip with a plain, unclassed message: still the server's answer, still not accepted.
    for round in 0..2 {
        b.page()
            .navigate(&srv.url("/login_flash.html"))
            .await
            .unwrap();
        b.page()
            .fill_form(req(json!({"Username": "plain", "Password": "nope"})), None)
            .await
            .unwrap();
        let out = b.page().submit(None, None).await.unwrap();
        assert_eq!(
            out.status,
            SubmitStatus::ValidationFailed,
            "round {round}: {out:#?}"
        );
        assert!(
            out.page_errors.iter().any(|e| e.contains("incorrect")),
            "round {round}: {out:#?}"
        );
    }
    b.close().await;
}

#[tokio::test]
async fn d8_counts_and_messages_for_unmatched_disabled_and_readonly() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page().navigate(&srv.url("/lockout.html")).await.unwrap();
    let rep = b
        .page()
        .fill_form(
            req(json!({"Nonexistent field": "x", "Customer name": "A"})),
            None,
        )
        .await
        .unwrap();
    assert_eq!(rep.fields.len(), 1, "{rep:#?}");
    assert_eq!(rep.unmatched.len(), 1, "{rep:#?}");
    assert_eq!(rep.unmatched[0].key, "Nonexistent field");
    // a disabled control that matches is named as such
    let rep = b
        .page()
        .fill_form(req(json!({"Account ID": "x"})), None)
        .await
        .unwrap();
    assert!(rep.fields.is_empty(), "{rep:#?}");
    let u = &rep.unmatched[0];
    assert!(u.reason.contains("disabled"), "{u:?}");
    assert!(u.candidates.is_empty(), "{u:?}");
    // a read-only plain text field: no talk of dates
    let rep = b
        .page()
        .fill_form(req(json!({"Reference": "zzz"})), None)
        .await
        .unwrap();
    assert_eq!(rep.failed(), 1, "{rep:#?}");
    let note = rep.fields[0].note.clone().unwrap();
    assert!(
        note.contains("read-only") && !note.contains("date"),
        "{note}"
    );
    b.close().await;
}

#[tokio::test]
async fn d8_unlabelled_controls_are_named_by_what_they_are() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page()
        .navigate(&srv.url("/unlabelled.html"))
        .await
        .unwrap();
    let ir = b.page().forms().await.unwrap();
    let shown: Vec<String> = ir.forms[0].fields.iter().map(|f| f.display()).collect();
    assert_eq!(
        shown,
        vec![
            "unlabelled checkbox",
            "unlabelled text",
            "nick",
            "Search here"
        ],
        "{ir:#?}"
    );
    b.close().await;
}

/// A page with constant unrelated background activity (polling fetch, DOM ticker, recursive timer, animation,
/// long poll) must not make every settle run to its ceiling.
#[tokio::test]
async fn a_busy_page_fills_fast_and_still_verifies() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page().navigate(&srv.url("/chatty.html")).await.unwrap();
    let calls0 = b.cdp_calls();
    let t0 = std::time::Instant::now();
    let rep = b
        .page()
        .fill_form(
            req(json!({"First name": "Ada", "Last name": "Lovelace", "Email": "ada@example.com", "Plan": "Pro"})),
            None,
        )
        .await
        .unwrap();
    let wall = t0.elapsed();
    eprintln!(
        "CHATTY fill: wall={wall:?} elapsed_ms={} unsettled={} cdp_calls={}",
        rep.elapsed_ms,
        rep.unsettled,
        b.cdp_calls() - calls0
    );
    assert_eq!(rep.fields.len(), 4, "{rep:#?}");
    assert!(
        rep.fields.iter().all(|f| f.status == FillStatus::Verified),
        "{rep:#?}"
    );
    assert_eq!(rep.unsettled, 0, "{rep:#?}");
    assert!(
        wall < Duration::from_millis(2500),
        "busy-page fill took {wall:?}"
    );
    b.close().await;
}

#[tokio::test]
async fn w4_email_key_prefers_the_real_field_over_a_group_option() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page()
        .navigate(&srv.url("/contactpref.html"))
        .await
        .unwrap();
    let rep = b
        .page()
        .fill_form(req(json!({"Email": "a@b.co"})), None)
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    assert!(rep.unmatched.is_empty(), "{rep:#?}");
    assert_eq!(rep.fields[0].observed, json!("a@b.co"), "{rep:#?}");
    // The option is still reachable through its group: key = option label alone is taken by the real field,
    // but the group itself and the qualified option label work.
    let rep = b
        .page()
        .fill_form(req(json!({"Contact preference": "Phone"})), None)
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    b.close().await;
}

#[tokio::test]
async fn w4_billing_and_shipping_address_keys_use_the_fieldset() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page()
        .navigate(&srv.url("/addresses.html"))
        .await
        .unwrap();
    let rep = b
        .page()
        .fill_form(
            req(json!({"Billing Address": "1 Main St", "shipping address": "9 Side Rd"})),
            None,
        )
        .await
        .unwrap();
    assert!(rep.ok() && rep.unmatched.is_empty(), "{rep:#?}");
    b.page().submit(None, None).await.ok();
    // A bare "Address" is still ambiguous.
    b.page()
        .navigate(&srv.url("/addresses.html"))
        .await
        .unwrap();
    let rep = b
        .page()
        .fill_form(req(json!({"Address": "x"})), None)
        .await
        .unwrap();
    assert_eq!(rep.unmatched.len(), 1, "{rep:#?}");
    assert!(rep.unmatched[0].reason.contains("ambiguous"), "{rep:#?}");
    b.close().await;
}

#[tokio::test]
async fn w4_readonly_dmy_datepicker_gets_the_iso_value() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page()
        .navigate(&srv.url("/datepicker_dmy.html"))
        .await
        .unwrap();
    let rep = b
        .page()
        .fill_form(req(json!({"Arrival": "2026-03-04"})), None)
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    assert_eq!(rep.fields[0].status, FillStatus::Verified, "{rep:#?}");
    b.close().await;
}

#[tokio::test]
async fn w4_assumed_date_format_is_disclosed() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page()
        .navigate(&srv.url("/datepicker_assumed.html"))
        .await
        .unwrap();
    let rep = b
        .page()
        .fill_form(req(json!({"Start date": "2026-03-04"})), None)
        .await
        .unwrap();
    let f = &rep.fields[0];
    assert_eq!(f.observed, json!("03/04/2026"), "{rep:#?}");
    let w = f.warning.clone().unwrap_or_default();
    assert!(w.contains("assumed mm/dd/yyyy"), "{rep:#?}");
    b.close().await;
}

#[tokio::test]
async fn w4_bad_years_never_panic_and_are_refused() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page().navigate(&srv.url("/datefmt.html")).await.unwrap();
    for v in ["March 5, -5", "March 5, 0024x", "March 5, 3024"] {
        let rep = b
            .page()
            .fill_form(req(json!({"Departure date": v})), None)
            .await
            .unwrap();
        assert!(rep.fields[0].warning.is_some(), "{v}: {rep:#?}");
    }
    let rep = b
        .page()
        .fill_form(req(json!({"Departure date": "March 5, 24"})), None)
        .await
        .unwrap();
    assert_eq!(rep.fields[0].observed, json!("03/05/2024"), "{rep:#?}");
    b.close().await;
}

#[tokio::test]
async fn w4_last_form_does_not_survive_navigation() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page()
        .navigate(&srv.url("/two_forms.html"))
        .await
        .unwrap();
    let rep = b
        .page()
        .fill_form(req(json!({"Item": "Astrolabe"})), None)
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    b.page()
        .navigate(&srv.url("/two_forms.html?again=1"))
        .await
        .unwrap();
    let err = b.page().submit(None, None).await.unwrap_err().to_string();
    assert!(err.contains("none was filled"), "{err}");
    b.close().await;
}

#[tokio::test]
async fn two_forms_that_match_equally_fill_the_first_and_name_the_other() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page()
        .navigate(&srv.url("/twin_forms.html"))
        .await
        .unwrap();
    let rep = b
        .page()
        .fill_form(req(json!({"Name": "Ada"})), None)
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    let got = b
        .page()
        .eval("[document.getElementById('n1').value, document.getElementById('n2').value]")
        .await
        .unwrap();
    assert_eq!(got, json!(["Ada", ""]), "a tie must go to the first form");
    assert_eq!(rep.rival_forms.len(), 1, "{rep:#?}");

    // Choosing the other form explicitly works and reports no rival.
    let second = rep.rival_forms[0].clone();
    let rep2 = b
        .page()
        .fill_form(
            FillRequest {
                values: vals(json!({"Name": "Grace"})),
                form: Some(second),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    assert!(rep2.ok() && rep2.rival_forms.is_empty(), "{rep2:#?}");
    let got = b
        .page()
        .eval("document.getElementById('n2').value")
        .await
        .unwrap();
    assert_eq!(got, json!("Grace"));
    b.close().await;
}

#[tokio::test]
async fn a_stalled_subresource_does_not_hold_navigation_for_the_whole_timeout() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    // The page's form is usable at once; its one image takes 25 s. `load` would wait for it.
    let t0 = std::time::Instant::now();
    let nav = b
        .page()
        .navigate(&srv.url("/slow_load.html"))
        .await
        .unwrap();
    let took = t0.elapsed();
    assert!(
        took < Duration::from_secs(15),
        "navigate waited {took:?} for a stalled image"
    );
    assert!(
        nav.warnings.iter().any(|w| w.contains("still loading")),
        "the agent must be told the page was cut short: {:?}",
        nav.warnings
    );
    let rep = b
        .page()
        .fill_form(req(json!({"Email": "ada@example.com"})), None)
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    b.close().await;
}

#[tokio::test]
async fn a_normal_page_navigates_without_a_cut_short_warning() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    let nav = b.page().navigate(&srv.url("/basic.html")).await.unwrap();
    assert!(nav.warnings.is_empty(), "{:?}", nav.warnings);
    b.close().await;
}
