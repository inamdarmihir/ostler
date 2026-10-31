//! Full stack: the real `ostler serve` binary, spoken to over MCP stdio like an agent client would.

use std::process::Stdio;
use std::time::{Duration, Instant};

use ostler_fixtures::FixtureServer;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

struct Client {
    child: Child,
    stdin: ChildStdin,
    out: tokio::io::Lines<BufReader<ChildStdout>>,
    next: u64,
}

impl Client {
    async fn start(args: &[&str]) -> Option<Self> {
        if ostler_cdp::find_chrome(None).is_err() {
            assert!(
                std::env::var("OSTLER_REQUIRE_CHROME").is_err(),
                "Chrome required but not found"
            );
            eprintln!("SKIPPED: no Chrome");
            return None;
        }
        let mut child = Command::new(env!("CARGO_BIN_EXE_ostler"))
            .arg("serve")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let out = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut c = Self {
            child,
            stdin,
            out,
            next: 1,
        };
        let init = c.rpc("initialize", json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}})).await;
        assert_eq!(init["result"]["serverInfo"]["name"], "ostler");
        c.notify("notifications/initialized").await;
        Some(c)
    }

    async fn send(&mut self, v: Value) {
        self.stdin
            .write_all(format!("{v}\n").as_bytes())
            .await
            .unwrap();
        self.stdin.flush().await.unwrap();
    }

    async fn notify(&mut self, method: &str) {
        self.send(json!({"jsonrpc":"2.0","method":method})).await;
    }

    async fn rpc(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await;
        let line = tokio::time::timeout(Duration::from_secs(60), self.out.next_line())
            .await
            .expect("timeout")
            .unwrap()
            .expect("eof");
        let v: Value = serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("non-JSON on stdout: {line:?} ({e})"));
        assert_eq!(v["id"], id, "{v}");
        v
    }

    /// Call a tool and return (is_error, text).
    async fn tool(&mut self, name: &str, args: Value) -> (bool, String) {
        let r = self
            .rpc("tools/call", json!({"name": name, "arguments": args}))
            .await;
        assert!(r.get("error").is_none(), "{r}");
        let text = r["result"]["content"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|c| c["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        (r["result"]["isError"].as_bool().unwrap_or(false), text)
    }
}

#[tokio::test]
async fn agent_session_with_memory() {
    let srv = FixtureServer::start();
    let Some(mut c) = Client::start(&["--memory", "ephemeral"]).await else {
        return;
    };

    let tools = c.rpc("tools/list", json!({})).await;
    assert!(tools["result"]["tools"].as_array().unwrap().len() >= 12);

    // Teach it who the user is. Secrets are refused.
    assert!(
        !c.tool(
            "memory_remember",
            json!({"key":"full_name","value":"Ada Lovelace"})
        )
        .await
        .0
    );
    assert!(
        !c.tool(
            "memory_remember",
            json!({"key":"email","value":"ada@example.com"})
        )
        .await
        .0
    );
    let (err, msg) = c
        .tool(
            "memory_remember",
            json!({"key":"password","value":"hunter2"}),
        )
        .await;
    assert!(err && msg.contains("never stores"), "{msg}");

    // navigate returns an observation, so no separate snapshot call is needed.
    let (err, nav) = c
        .tool("navigate", json!({"url": srv.url("/basic.html")}))
        .await;
    assert!(!err, "{nav}");
    assert!(
        nav.contains("<page_content untrusted=\"true\">") && nav.contains("textbox \"Full name\""),
        "{nav}"
    );

    let (_, insp) = c.tool("form_inspect", json!({})).await;
    assert!(
        insp.contains("Full name") && insp.contains("required") && !insp.contains("fingerprint="),
        "{insp}"
    );
    assert!(
        !insp.contains("filled successfully"),
        "first visit has no playbook yet: {insp}"
    );

    // The agent supplies nothing: memory fills the required fields.
    let (err, fill) = c.tool("form_fill", json!({"auto": "required"})).await;
    assert!(!err, "{fill}");
    assert!(
        fill.contains("Filled 2/2") && fill.contains("memory:"),
        "{fill}"
    );

    let (_, sub) = c.tool("form_submit", json!({})).await;
    assert!(sub.starts_with("Submitted"), "{sub}");
    let got = srv.last().unwrap();
    assert_eq!(
        (got["name"].as_str(), got["email"].as_str()),
        (Some("Ada Lovelace"), Some("ada@example.com"))
    );

    // Second visit: memory now recognises the form (write happens in the background).
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        c.tool(
            "navigate",
            json!({"url": srv.url("/basic.html"), "observe": "none"}),
        )
        .await;
        let (_, insp2) = c.tool("form_inspect", json!({})).await;
        if insp2.contains("filled successfully 1x before") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "playbook never appeared: {insp2}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let (_, stats) = c.tool("stats", json!({})).await;
    assert!(
        stats.contains("tool calls")
            && stats.contains("Browser memory")
            && stats.contains("form_fill"),
        "{stats}"
    );
    drop(c.stdin);
    let _ = tokio::time::timeout(Duration::from_secs(10), c.child.wait()).await;
}

#[tokio::test]
async fn validation_failure_is_learned_as_an_episode() {
    let srv = FixtureServer::start();
    let Some(mut c) = Client::start(&["--memory", "ephemeral"]).await else {
        return;
    };
    c.tool(
        "navigate",
        json!({"url": srv.url("/validation.html"), "observe": "none"}),
    )
    .await;
    let (_, fill) = c
        .tool(
            "form_fill",
            json!({"values": {"email":"nope","age":12,"zip":"123"}}),
        )
        .await;
    assert!(fill.contains("PAGE SAYS"), "{fill}");
    let (_, sub) = c.tool("form_submit", json!({})).await;
    assert!(sub.starts_with("NOT SUBMITTED"), "{sub}");
    assert!(srv.submissions().is_empty());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (_, insp) = c.tool("form_inspect", json!({})).await;
        if insp.contains("known issue") && insp.contains("ZIP must be 5 digits") {
            break;
        }
        assert!(Instant::now() < deadline, "episode never appeared: {insp}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn confirm_gate_and_domain_policy_over_the_wire() {
    let srv = FixtureServer::start();
    let Some(mut c) =
        Client::start(&["--require-confirm-submit", "--allow-domain", "127.0.0.1"]).await
    else {
        return;
    };
    let (err, _) = c
        .tool("navigate", json!({"url": "https://example.com"}))
        .await;
    assert!(err, "domain policy must block example.com");
    c.tool(
        "navigate",
        json!({"url": srv.url("/basic.html"), "observe": "none"}),
    )
    .await;
    c.tool(
        "form_fill",
        json!({"values": {"name":"A","email":"a@b.co"}}),
    )
    .await;
    let (err, msg) = c.tool("form_submit", json!({})).await;
    assert!(err && msg.contains("confirm"), "{msg}");
    assert!(srv.submissions().is_empty());
    let (err, msg) = c.tool("form_submit", json!({"confirm": true})).await;
    assert!(!err, "{msg}");
    assert!(srv.wait_for(1, Duration::from_secs(3)));
}

#[tokio::test]
async fn core_profile_exposes_only_core_tools() {
    let Some(mut c) = Client::start(&["--tools", "core"]).await else {
        return;
    };
    let tools = c.rpc("tools/list", json!({})).await;
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"click")
            && !names.contains(&"form_fill")
            && !names.contains(&"memory_list"),
        "{names:?}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn server_recovers_when_chrome_is_killed() {
    let srv = FixtureServer::start();
    let Some(mut c) = Client::start(&["--memory", "off"]).await else {
        return;
    };
    let (err, _) = c
        .tool(
            "navigate",
            json!({"url": srv.url("/basic.html"), "observe": "none"}),
        )
        .await;
    assert!(!err);
    // Kill Chrome (the server's only child) the way an OOM killer would.
    let pid = c.child.id().expect("server pid").to_string();
    let kids = std::process::Command::new("pgrep")
        .args(["-P", &pid])
        .output()
        .unwrap();
    let kids = String::from_utf8_lossy(&kids.stdout).to_string();
    assert!(!kids.trim().is_empty(), "expected a Chrome child process");
    for k in kids.split_whitespace() {
        std::process::Command::new("kill")
            .args(["-9", k])
            .status()
            .unwrap();
    }
    tokio::time::sleep(Duration::from_millis(500)).await;

    let t = Instant::now();
    let (err, text) = c
        .tool(
            "navigate",
            json!({"url": srv.url("/basic.html"), "observe": "none"}),
        )
        .await;
    assert!(!err, "{text}");
    assert!(
        text.contains("restarted"),
        "agent must be told state was lost: {text}"
    );
    assert!(
        t.elapsed() < Duration::from_secs(20),
        "recovery must not wait out a timeout"
    );
    let (_, insp) = c.tool("form_inspect", json!({})).await;
    assert!(insp.contains("Full name"), "{insp}");
    drop(c.stdin);
    let _ = tokio::time::timeout(Duration::from_secs(10), c.child.wait()).await;
}

#[tokio::test]
async fn tabs_share_cookies_and_context_and_popups_become_tabs() {
    let srv = FixtureServer::start();
    let Some(mut c) = Client::start(&["--memory", "off"]).await else {
        return;
    };
    // Tab 1 sets a cookie and a shared note.
    let (err, nav) = c
        .tool("navigate", json!({"url": srv.url("/popup.html")}))
        .await;
    assert!(!err, "{nav}");
    c.tool(
        "context",
        json!({"action": "set", "key": "applicant", "value": "Ada Lovelace"}),
    )
    .await;

    // A page-opened popup arrives as a tab, and the agent is told.
    let link = nav
        .lines()
        .find(|l| l.contains("Open the form in a new tab"))
        .and_then(|l| l.split('[').nth(1))
        .and_then(|s| s.split(']').next())
        .expect("link ref")
        .to_string();
    let (_, clicked) = c
        .tool("click", json!({"ref": link, "observe": "none"}))
        .await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    let (_, list) = c.tool("tabs", json!({"action": "list"})).await;
    assert!(
        list.contains("t2"),
        "popup should be a tab: {clicked}\n{list}"
    );
    assert!(list.contains("shared context: applicant"), "{list}");

    // A new tab shares cookies with the first (same browser context) and sees the same context.
    let (err, opened) = c
        .tool(
            "tabs",
            json!({"action": "new", "url": srv.url("/basic.html")}),
        )
        .await;
    assert!(!err, "{opened}");
    let (_, note) = c
        .tool("context", json!({"action": "get", "key": "applicant"}))
        .await;
    assert_eq!(note, "Ada Lovelace");
    // The cookie tab 1 set is visible from a different tab: they share one browser context.
    let (_, seen) = c
        .tool(
            "tabs",
            json!({"action": "new", "url": srv.url("/cookie.html")}),
        )
        .await;
    assert!(
        seen.contains("shared=from-first-tab"),
        "cookies were not shared: {seen}"
    );
    c.tool("tabs", json!({"action": "close", "tab": "t4"}))
        .await;

    // Switching changes which page the other tools act on.
    let (_, sw) = c
        .tool("tabs", json!({"action": "switch", "tab": "t1"}))
        .await;
    assert!(
        sw.contains("Active tab t1") && sw.contains("Open the form"),
        "{sw}"
    );
    let (_, sw) = c
        .tool("tabs", json!({"action": "switch", "tab": "t3"}))
        .await;
    assert!(sw.contains("Full name"), "{sw}");
    let (err, bad) = c
        .tool("tabs", json!({"action": "switch", "tab": "t99"}))
        .await;
    assert!(err && bad.contains("no tab"), "{bad}");

    // Closing the active tab falls back to another; closing everything leaves a blank one.
    let (_, closed) = c
        .tool("tabs", json!({"action": "close", "tab": "t3"}))
        .await;
    assert!(closed.contains("Closed t3"), "{closed}");
    for t in ["t1", "t2"] {
        c.tool("tabs", json!({"action": "close", "tab": t})).await;
    }
    let (err, list) = c.tool("tabs", json!({"action": "list"})).await;
    assert!(!err && list.contains("about:blank"), "{list}");
    drop(c.stdin);
    let _ = tokio::time::timeout(Duration::from_secs(10), c.child.wait()).await;
}
