//! A2A over real HTTP against a real browser and the local form fixtures.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use ostler_a2a::{router, A2aConfig, AppFactory};
use ostler_browser::{LaunchOptions, Policy};
use ostler_fixtures::FixtureServer;
use ostler_mcp::{App, AppConfig, Profile};
use serde_json::{json, Value};

fn factory() -> AppFactory {
    Arc::new(|| {
        App::new(AppConfig {
            launch: LaunchOptions::default(),
            policy: Policy::default(),
            profile: Profile::All,
            memory: None,
            learn: false,
            require_confirm_submit: false,
            verbose: false,
        })
    })
}

struct Srv {
    base: String,
    http: reqwest::Client,
}

async fn start(cfg: impl FnOnce(&mut A2aConfig)) -> Option<(Srv, FixtureServer)> {
    if ostler_cdp::find_chrome(None).is_err() {
        assert!(
            std::env::var("OSTLER_REQUIRE_CHROME").is_err(),
            "Chrome required"
        );
        eprintln!("SKIPPED: no Chrome found");
        return None;
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let mut c = A2aConfig::new(addr);
    cfg(&mut c);
    let app = router(c, factory());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .unwrap();
    Some((
        Srv {
            base: format!("http://{addr}"),
            http,
        },
        FixtureServer::start(),
    ))
}

impl Srv {
    async fn rpc(&self, method: &str, params: Value) -> Value {
        self.http
            .post(format!("{}/a2a", self.base))
            .header("A2A-Version", "1.0")
            .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }
    async fn send(&self, parts: Value, extra: Value) -> Value {
        let mut message = json!({"messageId": uuid(), "role": "ROLE_USER", "parts": parts});
        if let Value::Object(o) = extra {
            for (k, v) in o {
                message[k] = v;
            }
        }
        self.rpc("SendMessage", json!({ "message": message })).await
    }
}

fn uuid() -> String {
    format!(
        "m-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn data(v: Value) -> Value {
    json!([{ "data": v, "mediaType": "application/json" }])
}

#[tokio::test]
async fn agent_card_is_served_and_describes_the_skills() {
    let Some((s, _)) = start(|_| {}).await else {
        return;
    };
    let card: Value = s
        .http
        .get(format!("{}/.well-known/agent-card.json", s.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(card["name"], "Ostler");
    assert_eq!(card["supportedInterfaces"][0]["protocolBinding"], "JSONRPC");
    assert_eq!(card["supportedInterfaces"][0]["protocolVersion"], "1.0");
    assert!(card["supportedInterfaces"][0]["url"]
        .as_str()
        .unwrap()
        .ends_with("/a2a"));
    let ids: Vec<&str> = card["skills"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["inspect-form", "fill-form"]);
    assert_eq!(card["capabilities"]["streaming"], true);
    assert!(
        card.get("securitySchemes").is_none(),
        "no token configured, so no auth advertised"
    );
}

#[tokio::test]
async fn requests_from_web_pages_and_unknown_hosts_are_refused() {
    let Some((s, _)) = start(|c| c.token = Some("s3cret-token".into())).await else {
        return;
    };
    let body = json!({"jsonrpc":"2.0","id":1,"method":"GetTask","params":{"id":"x"}});
    let url = format!("{}/a2a", s.base);
    // No token, wrong token.
    let r = s.http.post(&url).json(&body).send().await.unwrap();
    assert_eq!(r.status(), 401);
    let r = s
        .http
        .post(&url)
        .bearer_auth("nope")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    // A web page in the user's browser (has Origin) must not be able to drive this.
    let r = s
        .http
        .post(&url)
        .bearer_auth("s3cret-token")
        .header("Origin", "https://evil.test")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    // DNS-rebinding style: right IP, attacker's Host header.
    let r = s
        .http
        .post(&url)
        .bearer_auth("s3cret-token")
        .header("Host", "evil.test")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    // The right token works, and the card advertises the scheme without exposing the token.
    let r: Value = s
        .http
        .post(&url)
        .bearer_auth("s3cret-token")
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["error"]["code"], -32001, "{r}");
    let card = s
        .http
        .get(format!("{}/.well-known/agent-card.json", s.base))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(card.contains("httpAuthSecurityScheme") && !card.contains("s3cret-token"));
}

#[tokio::test]
async fn protocol_errors_use_the_a2a_codes() {
    let Some((s, _)) = start(|_| {}).await else {
        return;
    };
    let r = s.rpc("GetTask", json!({"id": "missing"})).await;
    assert_eq!(r["error"]["code"], -32001);
    assert_eq!(r["error"]["data"][0]["reason"], "TASK_NOT_FOUND");
    assert_eq!(
        s.rpc("CancelTask", json!({"id": "missing"})).await["error"]["code"],
        -32001
    );
    assert_eq!(s.rpc("Nope", json!({})).await["error"]["code"], -32601);
    assert_eq!(
        s.rpc("CreateTaskPushNotificationConfig", json!({})).await["error"]["code"],
        -32003
    );
    assert_eq!(
        s.rpc("GetExtendedAgentCard", json!({})).await["error"]["code"],
        -32007
    );
    assert_eq!(
        s.rpc("SendMessage", json!({})).await["error"]["code"],
        -32602
    );
    // Wrong protocol version.
    let r: Value = s
        .http
        .post(format!("{}/a2a", s.base))
        .header("A2A-Version", "9.9")
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"GetTask","params":{"id":"x"}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["error"]["code"], -32009);
    // Not JSON-RPC at all.
    let r: Value = s
        .http
        .post(format!("{}/a2a", s.base))
        .body("{nope")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["error"]["code"], -32700);
    // Chatter gets a helpful message, not a task.
    let r = s
        .send(json!([{"text": "hi, book me a flight"}]), json!({}))
        .await;
    assert!(
        r["result"]["message"]["parts"][0]["text"]
            .as_str()
            .unwrap()
            .contains("data part"),
        "{r}"
    );
    assert!(r["result"].get("task").is_none());
}

#[tokio::test]
async fn inspect_then_fill_and_submit_with_input_required_and_confirmation() {
    let Some((s, fx)) = start(|_| {}).await else {
        return;
    };
    let url = fx.url("/basic.html");

    // 1. A bare inspect.
    let r = s.send(data(json!({ "url": url })), json!({})).await;
    let task = &r["result"]["task"];
    assert_eq!(task["status"]["state"], "TASK_STATE_COMPLETED", "{r}");
    let ctx = task["contextId"].as_str().unwrap().to_string();
    let dump = task["artifacts"].to_string();
    assert!(
        dump.contains("Full name") && dump.contains("untrusted"),
        "{dump}"
    );

    // 2. Fill only one required field and ask to submit: the server needs the rest.
    let r = s
        .send(
            data(json!({ "url": url, "values": {"Full name": "Ada Lovelace"}, "submit": true })),
            json!({}),
        )
        .await;
    let task = &r["result"]["task"];
    assert_eq!(task["status"]["state"], "TASK_STATE_INPUT_REQUIRED", "{r}");
    let (tid, ctx2) = (
        task["id"].as_str().unwrap().to_string(),
        task["contextId"].as_str().unwrap().to_string(),
    );
    assert_ne!(ctx, ctx2, "separate conversations get separate browsers");
    assert!(
        task["artifacts"].to_string().contains("Email"),
        "must say what is missing: {task}"
    );

    // 3. Supply it on the same task. Everything is verified, so now it waits for a confirm.
    let r = s
        .send(
            data(json!({ "values": {"email": "ada@example.com", "message": "Hello there"} })),
            json!({"taskId": tid}),
        )
        .await;
    let task = &r["result"]["task"];
    assert_eq!(task["status"]["state"], "TASK_STATE_INPUT_REQUIRED", "{r}");
    assert!(task["artifacts"].to_string().contains("confirm"), "{task}");
    assert!(
        fx.last().is_none(),
        "nothing may be submitted before the caller confirms"
    );

    // 4. Confirm: the form is actually submitted and the task completes.
    let r = s
        .send(data(json!({ "confirm": true })), json!({"taskId": tid}))
        .await;
    assert_eq!(
        r["result"]["task"]["status"]["state"], "TASK_STATE_COMPLETED",
        "{r}"
    );
    // Artifacts describe the latest turn: the stale "email is missing" result must be gone.
    let arts = r["result"]["task"]["artifacts"].to_string();
    assert!(
        arts.contains("Submitted") && !arts.contains("Still empty"),
        "stale artifacts: {arts}"
    );
    let got = fx.last().expect("form was submitted");
    assert_eq!(got["name"], "Ada Lovelace");
    assert_eq!(got["email"], "ada@example.com");

    // 5. A finished task accepts no more messages; GetTask still returns it with history.
    let r = s
        .send(data(json!({ "confirm": true })), json!({"taskId": tid}))
        .await;
    assert_eq!(r["error"]["code"], -32004, "{r}");
    let r = s
        .rpc("GetTask", json!({"id": tid, "historyLength": 10}))
        .await;
    assert_eq!(r["result"]["status"]["state"], "TASK_STATE_COMPLETED");
    assert_eq!(r["result"]["history"].as_array().unwrap().len(), 3);
    let r = s.rpc("CancelTask", json!({"id": tid})).await;
    assert_eq!(r["error"]["code"], -32002);
    let r = s.rpc("ListTasks", json!({"contextId": ctx2})).await;
    assert_eq!(r["result"]["tasks"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn auto_submit_skips_the_confirmation_round() {
    let Some((s, fx)) = start(|c| c.auto_submit = true).await else {
        return;
    };
    let r = s.send(data(json!({
        "url": fx.url("/basic.html"),
        "values": {"Full name": "Grace Hopper", "email": "grace@example.com", "message": "Hi"},
        "submit": true
    })), json!({})).await;
    assert_eq!(
        r["result"]["task"]["status"]["state"], "TASK_STATE_COMPLETED",
        "{r}"
    );
    assert_eq!(fx.last().unwrap()["name"], "Grace Hopper");
}

#[tokio::test]
async fn streaming_delivers_the_task_then_updates_to_a_final_status() {
    let Some((s, fx)) = start(|_| {}).await else {
        return;
    };
    let resp = s.http.post(format!("{}/a2a", s.base))
        .json(&json!({"jsonrpc":"2.0","id":7,"method":"SendStreamingMessage","params":{"message":{
            "messageId":"m1","role":"ROLE_USER","parts":data(json!({"url": fx.url("/basic.html")}))}}}))
        .send().await.unwrap();
    assert!(resp.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/event-stream"));
    let body = resp.text().await.unwrap();
    let events: Vec<Value> = body
        .lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .map(|l| serde_json::from_str(l.trim()).unwrap())
        .collect();
    assert!(events.len() >= 2, "{body}");
    assert!(
        events[0]["result"]["task"]["id"].is_string(),
        "first event is the task: {body}"
    );
    assert_eq!(events[0]["id"], 7);
    let last = events.last().unwrap();
    assert_eq!(
        last["result"]["statusUpdate"]["status"]["state"], "TASK_STATE_COMPLETED",
        "{body}"
    );
    assert!(
        events
            .iter()
            .any(|e| e["result"].get("artifactUpdate").is_some()),
        "{body}"
    );
}

#[tokio::test]
async fn conversation_limit_protects_the_machine() {
    let Some((s, fx)) = start(|c| c.max_contexts = 1).await else {
        return;
    };
    let ok = s
        .send(data(json!({"url": fx.url("/basic.html")})), json!({}))
        .await;
    assert!(ok["result"]["task"].is_object(), "{ok}");
    let r = s
        .send(data(json!({"url": fx.url("/basic.html")})), json!({}))
        .await;
    assert_eq!(r["error"]["data"][0]["reason"], "TOO_MANY_CONTEXTS", "{r}");
    // The same conversation can continue.
    let ctx = ok["result"]["task"]["contextId"].as_str().unwrap();
    let again = s
        .send(
            data(json!({"url": fx.url("/basic.html")})),
            json!({"contextId": ctx}),
        )
        .await;
    assert!(again["result"]["task"].is_object(), "{again}");
}

#[tokio::test]
async fn a_url_outside_the_allowlist_fails_the_task_not_the_server() {
    // Policy is applied by the browser, whichever protocol asked.
    if ostler_cdp::find_chrome(None).is_err() {
        return;
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let f: AppFactory = Arc::new(|| {
        App::new(AppConfig {
            launch: LaunchOptions::default(),
            policy: Policy {
                allowed_domains: vec!["example.org".into()],
                allow_file: false,
            },
            profile: Profile::All,
            memory: None,
            learn: false,
            require_confirm_submit: false,
            verbose: false,
        })
    });
    let app = router(A2aConfig::new(addr), f);
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let s = Srv {
        base: format!("http://{addr}"),
        http: reqwest::Client::new(),
    };
    let fx = FixtureServer::start();
    let r = s
        .send(data(json!({"url": fx.url("/basic.html")})), json!({}))
        .await;
    assert_eq!(
        r["result"]["task"]["status"]["state"], "TASK_STATE_FAILED",
        "{r}"
    );
    assert!(
        r["result"]["task"]["artifacts"]
            .to_string()
            .contains("not in the allowed domains"),
        "{r}"
    );
}

#[tokio::test]
async fn a_later_turn_streams_its_own_artifacts() {
    let Some((s, fx)) = start(|_| {}).await else {
        return;
    };
    let r = s
        .send(
            data(json!({"url": fx.url("/basic.html"), "values": {"Full name": "Ada"}, "submit": true})),
            json!({}),
        )
        .await;
    let tid = r["result"]["task"]["id"].as_str().unwrap().to_string();
    let resp = s
        .http
        .post(format!("{}/a2a", s.base))
        .json(
            &json!({"jsonrpc":"2.0","id":2,"method":"SendStreamingMessage","params":{"message":{
            "messageId":"m2","role":"ROLE_USER","taskId":tid,
            "parts":data(json!({"values":{"email":"a@b.co","message":"hi"}}))}}}),
        )
        .send()
        .await
        .unwrap();
    let body = resp.text().await.unwrap();
    let events: Vec<Value> = body
        .lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .map(|l| serde_json::from_str(l.trim()).unwrap())
        .collect();
    assert!(
        events[0]["result"]["task"].get("artifacts").is_none(),
        "the opening snapshot must not carry the previous turn's artifacts: {body}"
    );
    assert!(
        events
            .iter()
            .any(|e| e["result"].get("artifactUpdate").is_some()),
        "this turn's artifacts must be streamed: {body}"
    );
}
