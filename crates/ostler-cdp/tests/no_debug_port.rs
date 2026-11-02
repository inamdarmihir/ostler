//! Regression guard: the default transport must not open any TCP debugging port.
//! Set OSTLER_REQUIRE_CHROME=1 (CI does) to fail instead of skip when Chrome is absent.
//! Needs `lsof` and `ps` (macOS and Linux); skipped on other platforms.

#![cfg(unix)]

use std::collections::HashSet;
use std::process::Command;
use std::time::Duration;

use ostler_cdp::{Chrome, LaunchOptions};
use serde_json::json;

/// PIDs of `root` and all of its descendants.
fn process_tree(root: u32) -> HashSet<u32> {
    let out = Command::new("ps")
        .args(["-A", "-o", "pid=,ppid="])
        .output()
        .expect("ps");
    let rows: Vec<(u32, u32)> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
        })
        .collect();
    let mut members = HashSet::from([root]);
    loop {
        let before = members.len();
        for (pid, ppid) in &rows {
            if members.contains(ppid) {
                members.insert(*pid);
            }
        }
        if members.len() == before {
            return members;
        }
    }
}

/// TCP LISTEN sockets (`pid`, `name`) held by any process in the tree.
fn listeners(tree: &HashSet<u32>) -> Vec<(u32, String)> {
    let out = Command::new("lsof")
        .args(["-nP", "-iTCP", "-sTCP:LISTEN", "-F", "pn"])
        .output()
        .expect("lsof is required for this test");
    let mut found = vec![];
    let mut pid = 0u32;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(p) = line.strip_prefix('p') {
            pid = p.parse().unwrap_or(0);
        } else if let Some(name) = line.strip_prefix('n') {
            if tree.contains(&pid) {
                found.push((pid, name.to_string()));
            }
        }
    }
    found
}

/// Launch, prove the browser answers, and return the TCP listeners in its process tree.
async fn launch_and_list_listeners() -> Vec<(u32, String)> {
    let chrome = Chrome::launch(LaunchOptions::default())
        .await
        .expect("launch");
    let conn = chrome.connect().await.expect("connect");
    conn.call(
        "Browser.getVersion",
        json!({}),
        None,
        Duration::from_secs(20),
    )
    .await
    .expect("browser answers over the transport");
    // Chrome spawns its network service / GPU helpers lazily; give them a moment to bind anything.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let tree = process_tree(chrome.pid().expect("pid"));
    let found = listeners(&tree);
    chrome.close().await;
    found
}

#[tokio::test]
async fn default_transport_opens_no_tcp_debugging_port() {
    if ostler_cdp::find_chrome(None).is_err() {
        assert!(
            std::env::var("OSTLER_REQUIRE_CHROME").is_err(),
            "Chrome required but not found"
        );
        eprintln!("SKIPPED: no Chrome found");
        return;
    }

    // Control first: the WebSocket fallback must be visible to this check, or the check is blind.
    std::env::set_var("OSTLER_CDP_TRANSPORT", "ws");
    let ws = launch_and_list_listeners().await;
    assert!(
        !ws.is_empty(),
        "control failed: ws transport should expose a LISTEN socket but none was found"
    );

    std::env::remove_var("OSTLER_CDP_TRANSPORT");
    let pipe = launch_and_list_listeners().await;
    assert!(
        pipe.is_empty(),
        "default transport must not listen on TCP, found: {pipe:?}"
    );
}
