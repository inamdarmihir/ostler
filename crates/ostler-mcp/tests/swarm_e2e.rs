//! Subagents and agent swarms against one Ostler, and against many. Real Chrome, bundled pages.
//! Set OSTLER_REQUIRE_CHROME=1 (CI does) to fail instead of skip when Chrome is absent.
//! Set OSTLER_WRITE_BENCH=<path> to write the numbers the footprint test measures.

use std::sync::Arc;

use ostler_fixtures::FixtureServer;
use ostler_mcp::{App, AppConfig, Content, ToolOutput};
use ostler_memory::{HashEmbedder, InMemoryStore, Memory, Store};
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

fn app(memory: Option<Memory>, keep: bool, swarm: bool) -> App {
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
    a.set_swarm(swarm);
    a
}

fn have_chrome() -> bool {
    if ostler_cdp::find_chrome(None).is_err() {
        assert!(
            std::env::var("OSTLER_REQUIRE_CHROME").is_err(),
            "Chrome required but not found"
        );
        eprintln!("SKIPPED: no Chrome");
        return false;
    }
    true
}

const KESTREL: &str = "/packs/research/kestrel-lock.html";
const OLDFELD: &str = "/packs/research/oldfeld.html";

/// Agent A and agent B share one server. B opens its own page; A then reads without naming its tab.
#[tokio::test]
async fn a_shared_server_needs_explicit_tabs_and_swarm_mode_enforces_it() {
    if !have_chrome() {
        return;
    }
    let fx = FixtureServer::start();
    let (ka, ol) = (fx.url(KESTREL), fx.url(OLDFELD));

    // Default mode: the hazard is real. A's bare `read` returns B's page.
    let s = app(None, false, false);
    let a_nav = text(&s.call("navigate", &json!({"url": ka})).await);
    assert!(a_nav.contains("Kestrel"), "{a_nav}");
    s.call("tabs", &json!({"action": "new", "url": ol})).await; // agent B
    let a_read = text(&s.call("read", &json!({})).await);
    assert!(
        a_read.contains("Oldfeld") && !a_read.contains("Marta Ilves designed"),
        "A silently read B's page: {a_read}"
    );
    // Explicit tabs are safe in either mode.
    let tabs = text(&s.call("tabs", &json!({"action": "list"})).await);
    assert!(tabs.contains("t1") && tabs.contains("t2"), "{tabs}");
    let a_t1 = text(&s.call("read", &json!({"tab": "t1"})).await);
    assert!(a_t1.contains("1887"), "{a_t1}");
    s.shutdown().await;

    // Swarm mode: the same bare call is refused instead of answered from the wrong page.
    let w = app(None, false, true);
    w.call("navigate", &json!({"url": ka})).await;
    w.call("tabs", &json!({"action": "new", "url": ol})).await;
    let bare = w.call("read", &json!({})).await;
    assert!(
        bare.is_error && text(&bare).starts_with("AMBIGUOUS_TAB"),
        "{}",
        text(&bare)
    );
    let named = text(&w.call("read", &json!({"tab": "t1"})).await);
    assert!(named.contains("1887"), "{named}");
    // Notes, tab management and memory are not page calls and stay available.
    assert!(!w.call("tabs", &json!({"action": "list"})).await.is_error);
    assert!(!w.call("context", &json!({"action": "list"})).await.is_error);
    w.shutdown().await;
}

/// One agent reads and keeps the page; the rest answer from the shared memory and never start Chrome.
#[tokio::test]
async fn agents_served_from_shared_memory_never_launch_a_browser() {
    if !have_chrome() {
        return;
    }
    let fx = FixtureServer::start();
    let url = fx.url(KESTREL);
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new(Arc::new(HashEmbedder::default())));
    let mem = Memory::new(store, "swarm");
    mem.init().await.unwrap();

    let lead = app(Some(mem.clone()), true, false);
    lead.call("read", &json!({"url": url})).await;
    assert!(
        lead.browser_pid().await.is_some(),
        "the lead used a browser"
    );
    lead.shutdown().await;

    let mut launched = 0;
    for _ in 0..4 {
        let sub = app(Some(mem.clone()), true, false);
        let hit = text(
            &sub.call(
                "read",
                &json!({"stored": true, "query": "who designed the Kestrel Lock"}),
            )
            .await,
        );
        assert!(hit.contains("Marta Ilves"), "{hit}");
        let cite = text(
            &sub.call(
                "cite",
                &json!({"url": url, "quote": "It opened in 1887 and was designed by Marta Ilves"}),
            )
            .await,
        );
        assert!(cite.starts_with("VERIFIED AGAINST A STORED COPY"), "{cite}");
        if sub.browser_pid().await.is_some() {
            launched += 1;
        }
        sub.shutdown().await;
    }
    assert_eq!(launched, 0, "memory-served subagents must not start Chrome");
}

/// Browser memory for four agents: four Ostler processes (four Chromes) against one shared Ostler with
/// four tabs. The numbers are measured here; the assertion is only that sharing is not larger.
#[tokio::test]
async fn four_agents_cost_less_memory_in_one_shared_browser() {
    if !have_chrome() {
        return;
    }
    let fx = FixtureServer::start();
    let pages = [
        KESTREL,
        OLDFELD,
        "/packs/research/status.html",
        "/packs/research/review.html",
    ];

    let mut separate_kb = 0u64;
    let mut apps = vec![];
    for p in pages {
        let a = app(None, false, false);
        a.call("navigate", &json!({"url": fx.url(p)})).await;
        apps.push(a);
    }
    for a in &apps {
        separate_kb += a.browser_metrics().await.and_then(|m| m.1).unwrap_or(0);
    }
    for a in apps {
        a.shutdown().await;
    }

    let shared = app(None, false, true);
    shared
        .call("navigate", &json!({"url": fx.url(pages[0])}))
        .await;
    for p in &pages[1..] {
        shared
            .call("tabs", &json!({"action": "new", "url": fx.url(p)}))
            .await;
    }
    for t in ["t1", "t2", "t3", "t4"] {
        let r = shared.call("read", &json!({"tab": t})).await;
        assert!(!r.is_error, "{}", text(&r));
    }
    let shared_kb = shared
        .browser_metrics()
        .await
        .and_then(|m| m.1)
        .unwrap_or(0);
    shared.shutdown().await;

    let (sep, sh) = (separate_kb / 1024, shared_kb / 1024);
    eprintln!("4 agents: separate processes {sep} MiB, one shared browser {sh} MiB");
    assert!(
        sep > 0 && sh > 0,
        "memory was not measured (sep {sep}, shared {sh})"
    );
    assert!(sh < sep, "sharing should not cost more: {sh} vs {sep}");
    if let Ok(path) = std::env::var("OSTLER_WRITE_BENCH") {
        let md = format!(
            "# Four agents: separate Ostler processes vs one shared Ostler (measured)\n\nEach agent reads one fixture page. Browser process-tree memory after the reads.\n\n| setup | browsers | memory (MiB) |\n|---|---:|---:|\n| 4 Ostler processes, one Chrome each | 4 | {sep} |\n| 1 Ostler, 4 tabs (`--swarm`, explicit `tab`) | 1 | {sh} |\n\nOne machine (Linux container), headless Chrome, small static pages. Real pages are heavier and share less. The cost of sharing is that agents must pass `tab` on every page call, and that calls on one tab take turns.\n"
        );
        std::fs::write(path, md).unwrap();
    }
}
