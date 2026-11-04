//! The double-submit guard against a real Chrome and the local fixtures.

use ostler_browser::{
    AutoMode, Browser, BrowserError, FillRequest, LaunchOptions, Policy, SubmitStatus,
};
use ostler_fixtures::FixtureServer;
use serde_json::json;

#[tokio::test]
async fn an_identical_resubmit_is_refused_unless_forced() {
    if ostler_cdp::find_chrome(None).is_err() {
        assert!(std::env::var("OSTLER_REQUIRE_CHROME").is_err());
        eprintln!("SKIPPED: no Chrome found");
        return;
    }
    let srv = FixtureServer::start();
    let b = Browser::launch(LaunchOptions::default(), Policy::default())
        .await
        .unwrap();
    let page = b.page();
    page.navigate(&srv.url("/basic.html")).await.unwrap();
    let values =
        json!({"Full name": "Ada Lovelace", "email": "ada@example.com", "message": "Hello there"});
    let fill = || FillRequest {
        values: values
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        auto: AutoMode::Off,
        ..Default::default()
    };
    page.fill_form(fill(), None).await.unwrap();
    let first = page.submit_with(None, None, false).await.unwrap();
    assert!(matches!(
        first.status,
        SubmitStatus::Submitted | SubmitStatus::Advanced
    ));
    // Same form again right away: refused with a message that names the override, and no data is sent.
    page.navigate(&srv.url("/basic.html")).await.unwrap();
    page.fill_form(fill(), None).await.unwrap();
    let err = page.submit_with(None, None, false).await.unwrap_err();
    assert!(matches!(err, BrowserError::NotActionable(_)), "{err:?}");
    assert!(err.to_string().contains("again=true"), "{err}");
    // The explicit override goes through.
    assert!(page.submit_with(None, None, true).await.is_ok());
    b.close().await;
}
