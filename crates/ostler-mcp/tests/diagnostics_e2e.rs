//! `diagnostics` against the bundled QA app, which plants four problems on load. Real Chrome.
//! Set OSTLER_REQUIRE_CHROME=1 (CI does) to fail instead of skip when Chrome is absent.

use ostler_fixtures::FixtureServer;
use ostler_mcp::{App, AppConfig, Content, ToolOutput};
use serde_json::json;

fn text(o: &ToolOutput) -> String {
    o.content
        .iter()
        .filter_map(|c| match c {
            Content::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn diagnostics_reports_what_the_page_broke_once_and_then_only_what_is_new() {
    if ostler_cdp::find_chrome(None).is_err() {
        assert!(
            std::env::var("OSTLER_REQUIRE_CHROME").is_err(),
            "Chrome required but not found"
        );
        eprintln!("SKIPPED: no Chrome");
        return;
    }
    let fx = FixtureServer::start();
    let app = App::new(AppConfig {
        launch: Default::default(),
        policy: Default::default(),
        profile: Default::default(),
        memory: None,
        learn: false,
        require_confirm_submit: false,
        verbose: false,
    });
    app.call(
        "navigate",
        &json!({"url": fx.url("/packs/dev-qa/app.html")}),
    )
    .await;
    app.call("wait_for", &json!({"ms": 400})).await;

    let first = text(&app.call("diagnostics", &json!({})).await);
    assert!(first.contains("problem(s)"), "{first}");
    assert!(
        first.contains("<page_content untrusted"),
        "page text is fenced: {first}"
    );
    assert!(first.contains("cart-badge"), "console error: {first}");
    assert!(first.contains("TypeError"), "uncaught exception: {first}");
    assert!(
        first.contains("/packs/dev-qa/api/orders"),
        "failed request: {first}"
    );
    assert!(first.contains("500"), "status: {first}");
    assert!(
        first.contains("analytics.js") && first.contains("404"),
        "missing script: {first}"
    );

    // The mark moved: nothing new until the page does something else.
    let second = text(&app.call("diagnostics", &json!({})).await);
    assert!(second.starts_with("No new"), "{second}");
    // `from_start` ignores the mark.
    let again = text(&app.call("diagnostics", &json!({"from_start": true})).await);
    assert!(again.contains("cart-badge"), "{again}");
    app.shutdown().await;
}
