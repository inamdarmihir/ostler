//! Regressions found by the realistic-forms suite (`ostler-bench suite`). Each test failed before its fix.
//! Set OSTLER_REQUIRE_CHROME=1 (CI does) to fail instead of skip when Chrome is absent.

use std::collections::BTreeMap;

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
    FillRequest {
        values: v
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<BTreeMap<_, _>>(),
        ..Default::default()
    }
}

#[tokio::test]
async fn a_bare_day_number_picks_the_padded_option_and_dependent_district_loads() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page()
        .navigate(&srv.url("/suite_tax_dob.html"))
        .await
        .unwrap();
    let rep = b
        .page()
        .fill_form(
            req(json!({"Name as per PAN":"Ada","PAN":"abcde1234f","Aadhaar number":"123456789012",
                "Day":"5","Month":"March","Year":"1990","State":"Karnataka","District":"Mysuru","Mobile number":"9876543210"})),
            None,
        )
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    let out = b.page().submit(None, None).await.unwrap();
    assert_eq!(out.status, SubmitStatus::Submitted, "{out:#?}");
    let got = srv.last().unwrap();
    assert_eq!(got["dob_day"], "05");
    assert_eq!(got["pan"], "ABCDE1234F");
    assert_eq!(got["district"], "Mysuru");
    b.close().await;
}

#[tokio::test]
async fn inputs_in_nested_shadow_roots_belong_to_one_form_with_its_submit_button() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page()
        .navigate(&srv.url("/suite_shadow_nested.html"))
        .await
        .unwrap();
    let ir = b.page().forms().await.unwrap();
    assert_eq!(ir.forms.len(), 1, "{ir:#?}");
    assert_eq!(ir.forms[0].fields.len(), 2, "{ir:#?}");
    assert_eq!(ir.forms[0].submit.len(), 1, "{ir:#?}");
    let rep = b
        .page()
        .fill_form(
            req(json!({"Company name":"Analytical Engines","VAT number":"GB123456789"})),
            None,
        )
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    let out = b.page().submit(None, None).await.unwrap();
    assert_eq!(out.status, SubmitStatus::Submitted, "{out:#?}");
    assert!(srv.wait_for(1, std::time::Duration::from_secs(3)));
    assert_eq!(srv.last().unwrap()["vat"], "GB123456789");
    b.close().await;
}

#[tokio::test]
async fn an_array_for_a_chip_combobox_picks_each_and_is_never_called_verified() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page()
        .navigate(&srv.url("/suite_chips.html"))
        .await
        .unwrap();
    let rep = b
        .page()
        .fill_form(req(json!({"Skills":["Rust","Go"]})), None)
        .await
        .unwrap();
    // Before the fix this typed the JSON text into the box and reported it Verified while nothing was chosen.
    assert_eq!(rep.fields.len(), 1, "{rep:#?}");
    assert_eq!(rep.fields[0].status, FillStatus::Unverified, "{rep:#?}");
    let out = b.page().submit(None, None).await.unwrap();
    assert_eq!(out.status, SubmitStatus::Submitted, "{out:#?}");
    assert!(srv.wait_for(1, std::time::Duration::from_secs(3)));
    assert_eq!(srv.last().unwrap()["skills"], json!(["Go", "Rust"]));
    b.close().await;
}

#[tokio::test]
async fn a_chip_value_that_is_not_offered_fails_instead_of_being_typed() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page()
        .navigate(&srv.url("/suite_chips.html"))
        .await
        .unwrap();
    let rep = b
        .page()
        .fill_form(req(json!({"Skills":["Rust","COBOL"]})), None)
        .await
        .unwrap();
    assert_eq!(rep.fields[0].status, FillStatus::Failed, "{rep:#?}");
    let note = rep.fields[0].note.clone().unwrap_or_default();
    assert!(note.contains("picked 1 of 2"), "{note}");
    b.close().await;
}

#[tokio::test]
async fn an_address_suggestion_is_picked_by_its_street_part() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page()
        .navigate(&srv.url("/suite_address_auto.html"))
        .await
        .unwrap();
    let rep = b
        .page()
        .fill_form(
            req(json!({"Street address":"1600 Amphitheatre Parkway","City":"Mountain View","State":"CA","ZIP code":"94043"})),
            None,
        )
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    b.page().submit(None, None).await.unwrap();
    assert!(srv.wait_for(1, std::time::Duration::from_secs(3)));
    assert_eq!(srv.last().unwrap()["zip"], "94043");
    // Two suggestions share the "1600 " start but only one has this street before the comma: the
    // pick must not guess when the street part is ambiguous or absent.
    b.page()
        .navigate(&srv.url("/suite_address_auto.html"))
        .await
        .unwrap();
    let rep = b
        .page()
        .fill_form(req(json!({"Street address":"1600"})), None)
        .await
        .unwrap();
    assert_ne!(rep.fields[0].status, FillStatus::Verified, "{rep:#?}");
    b.close().await;
}

#[tokio::test]
async fn a_key_naming_a_collapsed_field_says_it_is_hidden_not_missing() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page()
        .navigate(&srv.url("/suite_reveal.html"))
        .await
        .unwrap();
    let rep = b
        .page()
        .fill_form(req(json!({"Employer name":"Analytical Engines"})), None)
        .await
        .unwrap();
    assert_eq!(rep.unmatched.len(), 1, "{rep:#?}");
    let why = &rep.unmatched[0].reason;
    assert!(why.contains("hidden right now"), "{why}");
    assert!(why.contains("Employer name"), "{why}");
    // A key that names nothing at all is still plain "not found".
    let rep = b
        .page()
        .fill_form(req(json!({"Mother's maiden name":"Smith"})), None)
        .await
        .unwrap();
    assert!(
        rep.unmatched[0].reason.contains("no field on the page"),
        "{rep:#?}"
    );
    b.close().await;
}

/// `iframe_hosted_form` failed once on a slow Windows runner with "no form fields found": the fill read the
/// page before the same-origin iframe had loaded. The iframe here is served 1.5s late, so the first look
/// always finds nothing; the fill must wait for the frame and then find its fields.
#[tokio::test]
async fn a_form_in_a_slow_iframe_is_waited_for_not_reported_missing() {
    let Some((srv, b)) = setup().await else {
        return;
    };
    b.page().navigate(&srv.url("/basic.html")).await.unwrap();
    let inner = srv.url("/suite_iframe_inner.html?delay_ms=1500");
    b.page()
        .eval(&format!(
            "document.body.innerHTML = '<h1>Contact sales</h1><iframe id=\"fr\" src=\"{inner}\" \
             style=\"width:100%;height:340px\"></iframe>'; true"
        ))
        .await
        .unwrap();
    let rep = b
        .page()
        .fill_form(
            req(json!({"Company":"Analytical Engines","Team size":"11-50","Work email":"ada@analytical.example"})),
            None,
        )
        .await
        .unwrap();
    assert!(rep.ok(), "{rep:#?}");
    b.close().await;
}
