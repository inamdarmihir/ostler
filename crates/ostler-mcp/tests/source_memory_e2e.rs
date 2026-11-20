//! Source memory across sessions: session 1 reads pages, session 2 is a fresh `App` (a new process in
//! production) that shares only the memory. Real Chrome, the bundled research pack, an in-process store.
//! Set OSTLER_REQUIRE_CHROME=1 (CI does) to fail instead of skip when Chrome is absent.
//! Set OSTLER_WRITE_BENCH=<path> to write the ablation table the test measures.

use std::sync::Arc;

use ostler_fixtures::FixtureServer;
use ostler_mcp::{App, AppConfig, Content, ToolOutput};
use ostler_memory::{HashEmbedder, InMemoryStore, Memory, Store};
use serde_json::json;

const QUOTE: &str = "It opened in 1887 and was designed by Marta Ilves";
const PAGE: &str = "/packs/research/kestrel-lock.html";

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

fn app(memory: Option<Memory>, keep: bool) -> App {
    let a = App::new(AppConfig {
        launch: Default::default(),
        policy: Default::default(),
        profile: Default::default(),
        memory,
        learn: false,
        require_confirm_submit: false,
        verbose: false,
    });
    a.set_keep_sources(keep);
    a
}

#[tokio::test]
async fn a_stored_copy_answers_in_a_fresh_session_but_never_passes_for_a_live_read() {
    if ostler_cdp::find_chrome(None).is_err() {
        assert!(
            std::env::var("OSTLER_REQUIRE_CHROME").is_err(),
            "Chrome required but not found"
        );
        eprintln!("SKIPPED: no Chrome");
        return;
    }
    let fx = FixtureServer::start();
    let url = fx.url(PAGE);
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new(Arc::new(HashEmbedder::default())));
    let mem = Memory::new(store, "t");
    mem.init().await.unwrap();

    // Session 1: read the page with keeping on.
    let s1 = app(Some(mem.clone()), true);
    let live = text(&s1.call("read", &json!({"url": url})).await);
    assert!(live.contains("1887"), "{live}");
    let live_cite = text(&s1.call("cite", &json!({"url": url, "quote": QUOTE})).await);
    assert!(live_cite.starts_with("VERIFIED ["), "{live_cite}");
    s1.shutdown().await;

    // Session 2, keeping on: a stored copy is found and cited, under its own verdict.
    let s2 = app(Some(mem.clone()), true);
    let stored = text(
        &s2.call(
            "read",
            &json!({"stored": true, "query": "who designed the Kestrel Lock"}),
        )
        .await,
    );
    assert!(stored.starts_with("<page_content"), "{stored}");
    assert!(stored.contains("STORED COPIES"), "{stored}");
    assert!(stored.contains("Marta Ilves"), "{stored}");
    assert!(stored.contains("sha256:"), "{stored}");
    let cite = text(&s2.call("cite", &json!({"url": url, "quote": QUOTE})).await);
    assert!(cite.starts_with("VERIFIED AGAINST A STORED COPY"), "{cite}");
    assert!(
        !cite.starts_with("VERIFIED ["),
        "never the live verdict: {cite}"
    );
    // A quote that is not on the stored page is still refused, and the refusal is not "read it".
    let fake = s2
        .call(
            "cite",
            &json!({"url": url, "quote": "It was designed by Tomas Reyes in 1901"}),
        )
        .await;
    assert!(fake.is_error, "{}", text(&fake));
    // It is not a citation of this session.
    let ev = text(&s2.call("evidence", &json!({})).await);
    assert!(ev.contains("0 verified citation"), "{ev}");
    s2.shutdown().await;

    // Opt-in: with keeping off, or no memory, nothing is found and nothing is claimed.
    let off = app(Some(mem.clone()), false);
    let c = off.call("cite", &json!({"url": url, "quote": QUOTE})).await;
    assert!(
        c.is_error && text(&c).starts_with("NOT READ"),
        "{}",
        text(&c)
    );
    let r = off
        .call("read", &json!({"stored": true, "query": "Kestrel"}))
        .await;
    assert!(
        r.is_error && text(&r).contains("STORED COPIES OFF"),
        "{}",
        text(&r)
    );
    off.shutdown().await;

    // Ablation numbers for session 2: what the agent pays to answer "who designed it" with and without
    // stored copies. Without them it has to open and read the page again.
    let with = text(
        &app(Some(mem.clone()), true)
            .call(
                "read",
                &json!({"stored": true, "query": "who designed the Kestrel Lock", "k": 2}),
            )
            .await,
    )
    .len();
    let s3 = app(None, false);
    let without = text(
        &s3.call(
            "read",
            &json!({"url": url, "query": "who designed the Kestrel Lock", "k": 2}),
        )
        .await,
    )
    .len();
    s3.shutdown().await;
    if let Ok(path) = std::env::var("OSTLER_WRITE_BENCH") {
        let md = format!(
            "# Source memory ablation (session 2, scripted, no model)\n\nTask: answer \"who designed the Kestrel Lock?\" in a fresh process, citing the page.\n\n| arm | tool calls to a cited answer | output bytes | browser needed | citation verdict |\n|---|---:|---:|---|---|\n| source memory off | 2 (read, cite) | {without} + cite | yes: navigates and reads the page again | live `VERIFIED [n]` |\n| source memory on (stored) | 2 (read stored, cite) | {with} + cite | no | `VERIFIED AGAINST A STORED COPY`, with fetch time and hash |\n\nStore: in-process, hash embedder. This measures the mechanism, not retrieval quality at scale; see docs/adr/0020-source-memory.md.\n"
        );
        std::fs::write(path, md).unwrap();
    }
    eprintln!("session-2 bytes: with stored copy {with}, without (live read) {without}");
}
