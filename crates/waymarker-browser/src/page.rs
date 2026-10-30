use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::time::{sleep, Instant};
use waymarker_cdp::{CdpError, Session};
use waymarker_forms::{FormIr, SubmitControl};

use crate::policy::Policy;

pub const RUNTIME_JS: &str = include_str!("runtime.js");

#[derive(Debug, thiserror::Error)]
pub enum BrowserError {
    #[error(transparent)]
    Cdp(#[from] CdpError),
    #[error("page script error: {0}")]
    Js(String),
    #[error("blocked by policy: {0}")]
    Policy(String),
    #[error("navigation failed: {0}")]
    Navigation(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, BrowserError>;

#[derive(Default)]
struct Shared {
    main_frame: Mutex<Option<String>>,
    started: AtomicU64,
    committed: AtomicU64,
    same_doc: AtomicU64,
    loaded: AtomicU64,
    dialogs: Mutex<Vec<String>>,
}

#[derive(Default)]
struct PageState {
    prev_lines: Vec<String>,
    prev_url: String,
}

#[derive(Clone)]
pub struct Page {
    pub(crate) session: Session,
    shared: Arc<Shared>,
    state: Arc<Mutex<PageState>>,
    pub(crate) policy: Arc<Policy>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PageInfo {
    pub url: String,
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotMode {
    #[default]
    Full,
    /// Only what changed since the previous snapshot of the same URL.
    Delta,
    /// Interactive elements and headings only.
    Interactive,
}

#[derive(Debug, Clone, Default)]
pub struct SnapshotOpts {
    pub mode: SnapshotMode,
    pub scope: Option<String>,
    pub max_chars: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SnapshotOut {
    pub url: String,
    pub title: String,
    pub text: String,
    pub truncated: bool,
    pub lines: usize,
}

/// Result of an action that may have changed the page.
#[derive(Debug, Clone, Serialize, Default)]
pub struct ActionOutcome {
    pub navigated: bool,
    pub url: String,
    pub title: String,
    pub warnings: Vec<String>,
    pub dialogs: Vec<String>,
}

const CONTEXT_GONE: [&str; 4] = [
    "Execution context was destroyed",
    "Cannot find context with specified id",
    "Inspected target navigated or closed",
    "Target closed",
];

impl Page {
    pub async fn attach(session: Session, policy: Policy) -> Result<Self> {
        let page = Self {
            session,
            shared: Arc::default(),
            state: Arc::default(),
            policy: Arc::new(policy),
        };
        let s = &page.session;
        s.call("Page.enable", json!({})).await?;
        s.call("Runtime.enable", json!({})).await?;
        s.call("DOM.enable", json!({})).await?;
        // Headless Chrome otherwise reports the page as unfocused, which suppresses focus/blur/change events.
        s.call(
            "Emulation.setFocusEmulationEnabled",
            json!({ "enabled": true }),
        )
        .await?;
        s.call(
            "Page.addScriptToEvaluateOnNewDocument",
            json!({ "source": RUNTIME_JS, "runImmediately": true }),
        )
        .await?;
        let tree = s.call("Page.getFrameTree", json!({})).await?;
        *page.shared.main_frame.lock().unwrap() =
            tree["frameTree"]["frame"]["id"].as_str().map(String::from);

        let mut events = s.events();
        let shared = page.shared.clone();
        let sess = s.clone();
        tokio::spawn(async move {
            while let Some(ev) = events.next().await {
                let is_main = |frame_id: Option<&str>| {
                    frame_id.is_some() && frame_id == shared.main_frame.lock().unwrap().as_deref()
                };
                match ev.method.as_str() {
                    "Page.frameStartedLoading" if is_main(ev.params["frameId"].as_str()) => {
                        shared.started.fetch_add(1, Ordering::SeqCst);
                    }
                    "Page.frameNavigated" if ev.params["frame"].get("parentId").is_none() => {
                        shared.committed.fetch_add(1, Ordering::SeqCst);
                    }
                    "Page.navigatedWithinDocument" if is_main(ev.params["frameId"].as_str()) => {
                        shared.same_doc.fetch_add(1, Ordering::SeqCst);
                    }
                    "Page.loadEventFired" => {
                        shared.loaded.fetch_add(1, Ordering::SeqCst);
                    }
                    "Page.javascriptDialogOpening" => {
                        let kind = ev.params["type"].as_str().unwrap_or("dialog");
                        let msg = ev.params["message"].as_str().unwrap_or("");
                        shared
                            .dialogs
                            .lock()
                            .unwrap()
                            .push(format!("{kind}: {msg}"));
                        let default = ev.params["defaultPrompt"].as_str().unwrap_or("");
                        let _ = sess
                            .call(
                                "Page.handleJavaScriptDialog",
                                json!({ "accept": true, "promptText": default }),
                            )
                            .await;
                    }
                    _ => {}
                }
            }
        });
        Ok(page)
    }

    // ------------------------------------------------------------------ evaluation

    /// Evaluate an expression in the page, retrying across navigations.
    pub async fn eval(&self, expression: &str) -> Result<Value> {
        let mut last = String::new();
        for attempt in 0..8 {
            if attempt > 0 {
                sleep(Duration::from_millis(60)).await;
            }
            let res = self
                .session
                .call(
                    "Runtime.evaluate",
                    json!({
                        "expression": expression,
                        "returnByValue": true,
                        "awaitPromise": true,
                        "userGesture": true,
                    }),
                )
                .await;
            let res = match res {
                Ok(r) => r,
                Err(CdpError::Protocol { message, .. })
                    if CONTEXT_GONE.iter().any(|m| message.contains(m)) =>
                {
                    last = message;
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            if let Some(ex) = res.get("exceptionDetails") {
                let text = ex["exception"]["description"]
                    .as_str()
                    .or_else(|| ex["exception"]["value"].as_str())
                    .or_else(|| ex["text"].as_str())
                    .unwrap_or("script error")
                    .to_string();
                if CONTEXT_GONE.iter().any(|m| text.contains(m)) {
                    last = text;
                    continue;
                }
                let first = text
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim_start_matches("Error: ")
                    .to_string();
                return Err(BrowserError::Js(first));
            }
            return Ok(res["result"].get("value").cloned().unwrap_or(Value::Null));
        }
        Err(BrowserError::Js(format!("page kept navigating: {last}")))
    }

    /// Call a function on the injected runtime: `__wm.name(args...)`.
    pub async fn rt(&self, name: &str, args: &[Value]) -> Result<Value> {
        let argv = args
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let expr = format!(
            "(typeof __wm==='undefined'?(()=>{{throw new Error('page runtime unavailable on this page')}})():__wm.{name}({argv}))"
        );
        self.eval(&expr).await
    }

    pub async fn info(&self) -> Result<PageInfo> {
        let v = self
            .eval("({url: location.href, title: document.title})")
            .await?;
        Ok(PageInfo {
            url: v["url"].as_str().unwrap_or("").to_string(),
            title: v["title"].as_str().unwrap_or("").to_string(),
        })
    }

    pub fn take_dialogs(&self) -> Vec<String> {
        std::mem::take(&mut *self.shared.dialogs.lock().unwrap())
    }

    // ------------------------------------------------------------------ navigation

    pub async fn navigate(&self, url: &str) -> Result<ActionOutcome> {
        self.policy.check(url).map_err(BrowserError::Policy)?;
        let loaded_before = self.shared.loaded.load(Ordering::SeqCst);
        let res = self
            .session
            .call("Page.navigate", json!({ "url": url }))
            .await?;
        if let Some(err) = res["errorText"].as_str() {
            return Err(BrowserError::Navigation(format!("{err} ({url})")));
        }
        // Same-document navigations (hash changes) never fire a load event.
        if res.get("loaderId").is_some() {
            self.wait_loaded(loaded_before, Duration::from_secs(30))
                .await;
        }
        self.settle(3000, 150).await;
        self.finish_outcome(true, vec![]).await
    }

    async fn wait_loaded(&self, loaded_before: u64, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.shared.loaded.load(Ordering::SeqCst) > loaded_before {
                return true;
            }
            sleep(Duration::from_millis(15)).await;
        }
        false
    }

    pub async fn settle(&self, max_ms: u64, quiet_ms: u64) -> Value {
        self.rt("settle", &[json!(max_ms), json!(quiet_ms)])
            .await
            .unwrap_or(Value::Null)
    }

    /// Run `action`, then wait for whatever it caused (navigation or in-page updates) to finish.
    pub async fn after<F, Fut, T>(&self, action: F) -> Result<(T, ActionOutcome)>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let started = self.shared.started.load(Ordering::SeqCst);
        let committed = self.shared.committed.load(Ordering::SeqCst);
        let same_doc = self.shared.same_doc.load(Ordering::SeqCst);
        let loaded = self.shared.loaded.load(Ordering::SeqCst);
        let value = action().await?;
        // Give a navigation a moment to begin; most start within a frame or two.
        let probe = Instant::now() + Duration::from_millis(180);
        let mut nav = false;
        while Instant::now() < probe {
            if self.shared.started.load(Ordering::SeqCst) > started
                || self.shared.committed.load(Ordering::SeqCst) > committed
            {
                nav = true;
                break;
            }
            sleep(Duration::from_millis(15)).await;
        }
        let mut warnings = vec![];
        if nav && !self.wait_loaded(loaded, Duration::from_secs(20)).await {
            warnings.push("page did not finish loading within 20s; continuing".into());
        }
        self.settle(3000, 150).await;
        let spa = self.shared.same_doc.load(Ordering::SeqCst) > same_doc;
        let outcome = self.finish_outcome(nav || spa, warnings).await?;
        Ok((value, outcome))
    }

    async fn finish_outcome(
        &self,
        navigated: bool,
        mut warnings: Vec<String>,
    ) -> Result<ActionOutcome> {
        let info = self.info().await.unwrap_or(PageInfo {
            url: String::new(),
            title: String::new(),
        });
        if let Err(msg) = self.policy.check(&info.url) {
            // A redirect or link led somewhere forbidden: leave immediately.
            let _ = self
                .session
                .call("Page.navigate", json!({ "url": "about:blank" }))
                .await;
            return Err(BrowserError::Policy(format!(
                "page moved to {}: {msg}",
                info.url
            )));
        }
        let dialogs = self.take_dialogs();
        if !dialogs.is_empty() {
            warnings.push("JavaScript dialogs were auto-accepted".into());
        }
        Ok(ActionOutcome {
            navigated,
            url: info.url,
            title: info.title,
            warnings,
            dialogs,
        })
    }

    pub async fn back(&self) -> Result<ActionOutcome> {
        let (_, out) = self
            .after(|| async { self.eval("history.back()").await })
            .await?;
        Ok(out)
    }

    // ------------------------------------------------------------------ snapshots

    pub async fn snapshot(&self, opts: SnapshotOpts) -> Result<SnapshotOut> {
        let o = json!({
            "interactiveOnly": opts.mode == SnapshotMode::Interactive,
            "ref": opts.scope,
            "maxChars": opts.max_chars.unwrap_or(20000),
        });
        let v = self.rt("snapshot", &[o]).await?;
        let lines: Vec<String> = v["lines"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|l| l.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let url = v["url"].as_str().unwrap_or("").to_string();
        let title = v["title"].as_str().unwrap_or("").to_string();
        let truncated = v["truncated"].as_bool().unwrap_or(false);

        let mut st = self.state.lock().unwrap();
        let same_page = st.prev_url == url && opts.scope.is_none();
        let body = if opts.mode == SnapshotMode::Delta && same_page && !st.prev_lines.is_empty() {
            let (added, removed, unchanged) = diff_lines(&st.prev_lines, &lines);
            let mut s = format!("(delta: {unchanged} unchanged lines omitted)\n");
            if added.is_empty() && removed.is_empty() {
                s.push_str("no changes\n");
            }
            for l in &added {
                s.push_str("+ ");
                s.push_str(l.trim_start());
                s.push('\n');
            }
            for l in removed.iter().take(15) {
                s.push_str("- ");
                s.push_str(l.trim_start());
                s.push('\n');
            }
            if removed.len() > 15 {
                s.push_str(&format!("- … and {} more removed\n", removed.len() - 15));
            }
            s
        } else {
            let mut s = lines.join("\n");
            s.push('\n');
            s
        };
        if opts.scope.is_none() && opts.mode != SnapshotMode::Interactive {
            st.prev_lines = lines.clone();
            st.prev_url = url.clone();
        }
        Ok(SnapshotOut {
            url,
            title,
            text: body,
            truncated,
            lines: lines.len(),
        })
    }

    // ------------------------------------------------------------------ input

    pub async fn click(&self, r: &str) -> Result<ActionOutcome> {
        let (warn, mut out) = self
            .after(|| async {
                let info = self.rt("clickInfo", &[json!(r)]).await?;
                let mut warn = None;
                if info["disabled"].as_bool() == Some(true) {
                    return Err(BrowserError::Other(format!("{r} is disabled")));
                }
                if info["zero"].as_bool() == Some(true) || info["covered"].as_bool() == Some(true) {
                    if info["covered"].as_bool() == Some(true) {
                        warn = Some(format!(
                            "{r} was covered by {}; used a script click instead of a real pointer click",
                            info["covered_by"].as_str().unwrap_or("another element")
                        ));
                    }
                    self.rt("jsClick", &[json!(r)]).await?;
                } else {
                    self.mouse_click(info["x"].as_f64().unwrap_or(0.0), info["y"].as_f64().unwrap_or(0.0)).await?;
                }
                Ok(warn)
            })
            .await?;
        if let Some(w) = warn {
            out.warnings.push(w);
        }
        Ok(out)
    }

    async fn mouse_click(&self, x: f64, y: f64) -> Result<()> {
        let s = &self.session;
        s.call(
            "Input.dispatchMouseEvent",
            json!({ "type": "mouseMoved", "x": x, "y": y }),
        )
        .await?;
        s.call(
            "Input.dispatchMouseEvent",
            json!({ "type": "mousePressed", "x": x, "y": y, "button": "left", "buttons": 1, "clickCount": 1 }),
        )
        .await?;
        s.call(
            "Input.dispatchMouseEvent",
            json!({ "type": "mouseReleased", "x": x, "y": y, "button": "left", "buttons": 0, "clickCount": 1 }),
        )
        .await?;
        Ok(())
    }

    /// Focus an element and replace its contents with `text`, as a user typing would.
    pub async fn type_into(&self, r: &str, text: &str) -> Result<()> {
        let p = self.rt("prepare", &[json!(r)]).await?;
        if p["disabled"].as_bool() == Some(true) {
            return Err(BrowserError::Other(format!("{r} is disabled")));
        }
        if p["readonly"].as_bool() == Some(true) {
            return Err(BrowserError::Other(format!("{r} is read-only")));
        }
        if text.is_empty() {
            self.rt("setNative", &[json!(r), json!("")]).await?;
        } else {
            self.session
                .call("Input.insertText", json!({ "text": text }))
                .await?;
        }
        Ok(())
    }

    pub async fn type_text(&self, r: &str, text: &str, submit: bool) -> Result<ActionOutcome> {
        let (_, out) = self
            .after(|| async {
                self.type_into(r, text).await?;
                if submit {
                    self.press_raw("Enter").await?;
                } else {
                    self.rt("finish", &[json!(r)]).await?;
                }
                Ok(())
            })
            .await?;
        Ok(out)
    }

    pub async fn press(&self, key: &str) -> Result<ActionOutcome> {
        let (_, out) = self.after(|| async { self.press_raw(key).await }).await?;
        Ok(out)
    }

    async fn press_raw(&self, combo: &str) -> Result<()> {
        let parts: Vec<&str> = combo.split('+').collect();
        let (key, mods) = parts
            .split_last()
            .ok_or_else(|| BrowserError::Other("empty key".into()))?;
        let mut modifiers = 0;
        for m in mods {
            modifiers |= match m.to_lowercase().as_str() {
                "alt" => 1,
                "control" | "ctrl" => 2,
                "meta" | "cmd" | "command" => 4,
                "shift" => 8,
                other => return Err(BrowserError::Other(format!("unknown modifier {other}"))),
            };
        }
        let (name, code, vk, text): (String, String, u32, Option<String>) =
            match key.to_lowercase().as_str() {
                "enter" | "return" => ("Enter".into(), "Enter".into(), 13, Some("\r".into())),
                "tab" => ("Tab".into(), "Tab".into(), 9, None),
                "escape" | "esc" => ("Escape".into(), "Escape".into(), 27, None),
                "backspace" => ("Backspace".into(), "Backspace".into(), 8, None),
                "delete" => ("Delete".into(), "Delete".into(), 46, None),
                "arrowdown" | "down" => ("ArrowDown".into(), "ArrowDown".into(), 40, None),
                "arrowup" | "up" => ("ArrowUp".into(), "ArrowUp".into(), 38, None),
                "arrowleft" | "left" => ("ArrowLeft".into(), "ArrowLeft".into(), 37, None),
                "arrowright" | "right" => ("ArrowRight".into(), "ArrowRight".into(), 39, None),
                "home" => ("Home".into(), "Home".into(), 36, None),
                "end" => ("End".into(), "End".into(), 35, None),
                "pageup" => ("PageUp".into(), "PageUp".into(), 33, None),
                "pagedown" => ("PageDown".into(), "PageDown".into(), 34, None),
                "space" | " " => (" ".into(), "Space".into(), 32, Some(" ".into())),
                k if k.chars().count() == 1 => {
                    let c = key.chars().next().unwrap();
                    let up = c.to_ascii_uppercase();
                    (
                        c.to_string(),
                        format!("Key{up}"),
                        up as u32,
                        Some(c.to_string()),
                    )
                }
                other => return Err(BrowserError::Other(format!("unsupported key {other:?}"))),
            };
        let printable = modifiers & 0b0110 == 0;
        let mut down = json!({
            "type": if text.is_some() && printable { "keyDown" } else { "rawKeyDown" },
            "key": name, "code": code, "windowsVirtualKeyCode": vk, "modifiers": modifiers,
        });
        if let (Some(t), true) = (&text, printable) {
            down["text"] = json!(t);
        }
        self.session.call("Input.dispatchKeyEvent", down).await?;
        self.session
            .call(
                "Input.dispatchKeyEvent",
                json!({ "type": "keyUp", "key": name, "code": code, "windowsVirtualKeyCode": vk, "modifiers": modifiers }),
            )
            .await?;
        Ok(())
    }

    pub async fn upload(&self, r: &str, paths: &[String]) -> Result<()> {
        for p in paths {
            if !std::path::Path::new(p).is_file() {
                return Err(BrowserError::Other(format!("file not found: {p}")));
            }
        }
        let obj = self
            .session
            .call(
                "Runtime.evaluate",
                json!({ "expression": format!("__wm.el({})", json!(r)), "returnByValue": false }),
            )
            .await?;
        let object_id = obj["result"]["objectId"]
            .as_str()
            .ok_or_else(|| BrowserError::Other(format!("could not resolve {r}")))?;
        self.session
            .call(
                "DOM.setFileInputFiles",
                json!({ "files": paths, "objectId": object_id }),
            )
            .await?;
        Ok(())
    }

    pub async fn hover(&self, r: &str) -> Result<()> {
        let info = self.rt("clickInfo", &[json!(r)]).await?;
        self.session
            .call(
                "Input.dispatchMouseEvent",
                json!({ "type": "mouseMoved", "x": info["x"], "y": info["y"] }),
            )
            .await?;
        self.settle(800, 80).await;
        Ok(())
    }

    // ------------------------------------------------------------------ waiting / screenshots

    pub async fn wait_for(
        &self,
        text: Option<&str>,
        text_gone: Option<&str>,
        selector: Option<&str>,
        timeout: Duration,
    ) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        loop {
            let mut ok = true;
            if let Some(t) = text {
                ok &= self
                    .rt("textPresent", &[json!(t)])
                    .await?
                    .as_bool()
                    .unwrap_or(false);
            }
            if let Some(t) = text_gone {
                ok &= !self
                    .rt("textPresent", &[json!(t)])
                    .await?
                    .as_bool()
                    .unwrap_or(false);
            }
            if let Some(s) = selector {
                ok &= self
                    .rt("selectorPresent", &[json!(s)])
                    .await?
                    .as_bool()
                    .unwrap_or(false);
            }
            if ok {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            sleep(Duration::from_millis(100)).await;
        }
    }

    pub async fn screenshot(
        &self,
        scope: Option<&str>,
        full_page: bool,
        jpeg: bool,
    ) -> Result<Vec<u8>> {
        let mut params = json!({ "format": if jpeg { "jpeg" } else { "png" } });
        if jpeg {
            params["quality"] = json!(70);
        }
        if let Some(r) = scope {
            let rect = self.rt("rect", &[json!(r)]).await?;
            params["clip"] = json!({ "x": rect["x"], "y": rect["y"], "width": rect["width"], "height": rect["height"], "scale": 1 });
            params["captureBeyondViewport"] = json!(true);
        } else if full_page {
            let m = self
                .session
                .call("Page.getLayoutMetrics", json!({}))
                .await?;
            let sz = &m["cssContentSize"];
            params["clip"] =
                json!({ "x": 0, "y": 0, "width": sz["width"], "height": sz["height"], "scale": 1 });
            params["captureBeyondViewport"] = json!(true);
        }
        let res = self.session.call("Page.captureScreenshot", params).await?;
        let data = res["data"]
            .as_str()
            .ok_or_else(|| BrowserError::Other("no screenshot data".into()))?;
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| BrowserError::Other(e.to_string()))
    }

    // ------------------------------------------------------------------ forms

    pub async fn forms(&self) -> Result<FormIr> {
        let v = self.rt("forms", &[]).await?;
        serde_json::from_value(v)
            .map_err(|e| BrowserError::Other(format!("could not read form model: {e}")))
    }

    pub async fn field(&self, r: &str) -> Result<waymarker_forms::Field> {
        let v = self.rt("field", &[json!(r)]).await?;
        serde_json::from_value(v)
            .map_err(|e| BrowserError::Other(format!("could not read field: {e}")))
    }

    pub async fn click_submit(&self, c: &SubmitControl) -> Result<ActionOutcome> {
        self.click(&c.r#ref).await
    }
}

/// Multiset line diff: (added, removed, unchanged count).
fn diff_lines(prev: &[String], next: &[String]) -> (Vec<String>, Vec<String>, usize) {
    use std::collections::HashMap;
    let mut counts: HashMap<&str, i64> = HashMap::new();
    for l in prev {
        *counts.entry(l.as_str()).or_default() += 1;
    }
    let mut added = vec![];
    let mut unchanged = 0;
    for l in next {
        match counts.get_mut(l.as_str()) {
            Some(c) if *c > 0 => {
                *c -= 1;
                unchanged += 1;
            }
            _ => added.push(l.clone()),
        }
    }
    let mut removed = vec![];
    for l in prev {
        if let Some(c) = counts.get_mut(l.as_str()) {
            if *c > 0 {
                *c -= 1;
                removed.push(l.clone());
            }
        }
    }
    (added, removed, unchanged)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_detects_changes() {
        let a: Vec<String> = ["x", "y", "z"].iter().map(|s| s.to_string()).collect();
        let b: Vec<String> = ["x", "z", "w"].iter().map(|s| s.to_string()).collect();
        let (added, removed, same) = diff_lines(&a, &b);
        assert_eq!(added, vec!["w"]);
        assert_eq!(removed, vec!["y"]);
        assert_eq!(same, 2);
    }

    #[test]
    fn diff_handles_duplicates() {
        let a: Vec<String> = ["x", "x"].iter().map(|s| s.to_string()).collect();
        let b: Vec<String> = ["x"].iter().map(|s| s.to_string()).collect();
        let (added, removed, same) = diff_lines(&a, &b);
        assert!(added.is_empty());
        assert_eq!(removed, vec!["x"]);
        assert_eq!(same, 1);
    }
}
