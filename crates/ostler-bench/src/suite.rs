//! Realistic-forms suite: runs `form_inspect` / `form_fill` / `form_submit` through the MCP tool layer
//! against 30 local fixtures and judges each by what the *fixture server* received, never by what the
//! tool said. Reports the pass rate, wrong-fills (must be 0), honest refusals, skips and time.

use std::time::Instant;

use ostler_browser::{LaunchOptions, Policy};
use ostler_fixtures::suite::{tasks, Expect, Task, RESUME};
use ostler_fixtures::FixtureServer;
use ostler_mcp::{App, AppConfig, Profile};
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The server received exactly the ground-truth state.
    Pass,
    /// A task that should be refused was refused, and the tool said why.
    Refused,
    /// The tool stopped and handed off to a human (CAPTCHA, sensitive value), nothing wrong was sent.
    Skipped,
    /// Nothing wrong was sent, but the task did not complete (or the tool did not explain a refusal).
    Fail,
    /// The server received a wrong value, or the tool claimed success that the server does not show.
    Wrong,
}

pub struct Outcome {
    pub id: &'static str,
    pub pattern: &'static str,
    pub verdict: Verdict,
    pub ms: u128,
    pub calls: usize,
    pub detail: String,
    pub transcript: String,
}

fn fill_values(v: &Value, resume: &str) -> Value {
    match v {
        Value::String(s) if s == RESUME => json!(resume),
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, x)| (k.clone(), fill_values(x, resume)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Lines in a fill/submit result that mean "this did not go cleanly".
fn problem_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| {
            l.starts_with("FAILED")
                || l.contains(" FAILED ")
                || l.starts_with("UNMATCHED")
                || l.contains("PAGE SAYS")
                || l.starts_with("PAGE ERROR")
                || l.contains("WARNING:")
                || l.starts_with("NOT ACCEPTED")
                || l.starts_with("NOT SUBMITTED")
                || l.starts_with("SERVER ERROR")
                || l.starts_with("Nothing changed")
                || l.starts_with("Still empty and required")
                || l.starts_with("CHALLENGE")
                || l.contains("CHALLENGE:")
        })
        .map(|l| l.chars().take(200).collect())
        .collect()
}

fn handoff(text: &str) -> bool {
    let t = text.to_lowercase();
    t.contains("challenge:") || t.contains("ask the user") || t.contains("needs a human")
}

pub fn matches_expected(got: &Value, want: &[(&'static str, Value)]) -> Result<(), String> {
    for (k, v) in want {
        if &got[*k] != v {
            return Err(format!("{k}: server has {} but expected {v}", got[*k]));
        }
    }
    Ok(())
}

pub async fn run_task(app: &App, srv: &FixtureServer, task: &Task, resume: &str) -> Outcome {
    let t0 = Instant::now();
    srv.reset();
    let mut calls = 0usize;
    let mut transcript = String::new();
    let mut problems: Vec<String> = vec![];
    let mut last_submit_claimed_ok = false;
    let nav = app
        .call(
            "navigate",
            &json!({"url": srv.url(task.page), "observe": "none"}),
        )
        .await;
    calls += 1;
    if nav.is_error {
        return Outcome {
            id: task.id,
            pattern: task.pattern,
            verdict: Verdict::Fail,
            ms: t0.elapsed().as_millis(),
            calls,
            detail: format!("navigate failed: {}", nav.text_content()),
            transcript: String::new(),
        };
    }
    'steps: for step in &task.steps {
        let ins = app.call("form_inspect", &json!({})).await;
        calls += 1;
        transcript.push_str(&ins.text_content());
        let fill = app
            .call("form_fill", &json!({"values": fill_values(step, resume)}))
            .await;
        calls += 1;
        let ft = fill.text_content();
        transcript.push_str(&format!("\n{ft}\n"));
        let mut p = problem_lines(&ft);
        if fill.is_error {
            p.push(format!("error: {}", ft.lines().next().unwrap_or("")));
        }
        // A real agent stops on a fill problem rather than submitting a half-filled form.
        if !p.is_empty() {
            problems.extend(p);
            break 'steps;
        }
        let sub = app.call("form_submit", &json!({})).await;
        calls += 1;
        let st = sub.text_content();
        transcript.push_str(&format!("\n{st}\n"));
        last_submit_claimed_ok = st.starts_with("Submitted") || st.starts_with("Advanced");
        if sub.is_error || !last_submit_claimed_ok {
            problems.extend(problem_lines(&st));
            if problems.is_empty() {
                problems.push(st.lines().next().unwrap_or("").chars().take(200).collect());
            }
            break 'steps;
        }
    }
    // Late fetches land after the click returns; give them a moment before judging.
    srv.wait_for(1, std::time::Duration::from_millis(1500));
    let got = srv.last();
    let first = problems.first().cloned().unwrap_or_default();
    let (verdict, detail) = match (&task.expect, got) {
        (Expect::Submitted(want), Some(got)) => match matches_expected(&got, want) {
            Ok(()) => (Verdict::Pass, String::new()),
            Err(e) => (Verdict::Wrong, format!("server received wrong data: {e}")),
        },
        (Expect::Submitted(_), None) => {
            if problems.is_empty() && last_submit_claimed_ok {
                (
                    Verdict::Wrong,
                    "tool reported Submitted but the server received nothing".into(),
                )
            } else if handoff(&transcript) {
                (Verdict::Skipped, format!("handed off: {first}"))
            } else {
                (Verdict::Fail, first)
            }
        }
        (Expect::Refused { .. }, Some(got)) => (
            Verdict::Wrong,
            format!("server accepted a submission it should have refused: {got}"),
        ),
        (Expect::Refused { must_mention }, None) => {
            let said = transcript
                .to_lowercase()
                .contains(&must_mention.to_lowercase());
            if problems.is_empty() && last_submit_claimed_ok {
                (
                    Verdict::Wrong,
                    "tool claimed success on a refused form".into(),
                )
            } else if problems.is_empty() {
                (
                    Verdict::Fail,
                    "no submission, but the tool flagged nothing".into(),
                )
            } else if said {
                (Verdict::Refused, format!("said: {first}"))
            } else {
                (
                    Verdict::Fail,
                    format!("refused without saying why ({must_mention:?} missing): {first}"),
                )
            }
        }
    };
    Outcome {
        id: task.id,
        pattern: task.pattern,
        verdict,
        ms: t0.elapsed().as_millis(),
        calls,
        detail,
        transcript,
    }
}

pub fn new_app(resume_dir: &std::path::Path) -> App {
    App::new(AppConfig {
        launch: LaunchOptions::default(),
        policy: Policy {
            upload_dirs: vec![resume_dir.to_path_buf()],
            ..Default::default()
        },
        profile: Profile::All,
        memory: None,
        learn: false,
        require_confirm_submit: false,
        verbose: false,
    })
}

pub async fn run_all(only: Option<&str>) -> (Vec<Outcome>, u128) {
    let dir = tempfile::tempdir().expect("tempdir");
    let resume = dir.path().join("resume.pdf");
    std::fs::write(&resume, b"%PDF-1.4 fake resume\n").expect("resume");
    let resume_path = resume.to_string_lossy().to_string();
    let srv = FixtureServer::start();
    let mut out = vec![];
    let t0 = Instant::now();
    for task in tasks() {
        if only.is_some_and(|o| !task.id.contains(o)) {
            continue;
        }
        // A fresh browser per task: one task's page state or crash cannot leak into the next.
        let app = new_app(dir.path());
        out.push(run_task(&app, &srv, &task, &resume_path).await);
        app.shutdown().await;
    }
    (out, t0.elapsed().as_millis())
}

pub struct Summary {
    pub total: usize,
    pub pass: usize,
    pub refused: usize,
    pub skipped: usize,
    pub fail: usize,
    pub wrong: usize,
    pub should_succeed: usize,
}

pub fn summarize(o: &[Outcome]) -> Summary {
    let n = |v: Verdict| o.iter().filter(|x| x.verdict == v).count();
    let should_succeed = o.iter().filter(|x| !x.id.starts_with("fail_")).count();
    Summary {
        total: o.len(),
        pass: n(Verdict::Pass),
        refused: n(Verdict::Refused),
        skipped: n(Verdict::Skipped),
        fail: n(Verdict::Fail),
        wrong: n(Verdict::Wrong),
        should_succeed,
    }
}

pub async fn run(only: Option<String>) -> anyhow::Result<Value> {
    let (out, total_ms) = run_all(only.as_deref()).await;
    println!(
        "{:<28} {:<8} {:>6} {:>5}  detail",
        "task", "verdict", "ms", "calls"
    );
    for o in &out {
        if std::env::var("OSTLER_SUITE_TRANSCRIPT").is_ok() && o.verdict != Verdict::Pass {
            println!("---- {} ----\n{}", o.id, o.transcript);
        }
        println!(
            "{:<28} {:<8} {:>6} {:>5}  {}",
            o.id,
            format!("{:?}", o.verdict).to_uppercase(),
            o.ms,
            o.calls,
            o.detail
        );
    }
    let s = summarize(&out);
    let done = s.pass + s.refused;
    println!(
        "\n{} tasks in {:.1}s: {} pass, {} correctly refused, {} skipped (hand-off), {} failed honestly, {} WRONG-FILLS",
        s.total, total_ms as f64 / 1000.0, s.pass, s.refused, s.skipped, s.fail, s.wrong
    );
    println!(
        "pass rate {:.0}% ({}/{}); of the {} tasks that should succeed: {}/{} completed",
        100.0 * done as f64 / s.total.max(1) as f64,
        done,
        s.total,
        s.should_succeed,
        s.pass,
        s.should_succeed
    );
    let rows: Vec<Value> = out
        .iter()
        .map(|o| json!({"task": o.id, "pattern": o.pattern, "verdict": format!("{:?}", o.verdict), "ms": o.ms, "calls": o.calls, "detail": o.detail}))
        .collect();
    let result = json!({"total": s.total, "pass": s.pass, "refused": s.refused, "skipped": s.skipped,
        "failed": s.fail, "wrong_fills": s.wrong, "total_ms": total_ms, "rows": rows});
    if s.wrong > 0 {
        anyhow::bail!("{} wrong-fills", s.wrong);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expected_state_comparison_is_exact_and_names_the_key() {
        let got = json!({"a": "1", "b": "2"});
        assert!(matches_expected(&got, &[("a", json!("1"))]).is_ok());
        let e = matches_expected(&got, &[("b", json!("3"))]).unwrap_err();
        assert!(e.starts_with("b:"), "{e}");
        assert!(matches_expected(&got, &[("c", json!(""))]).is_err());
    }

    #[test]
    fn problem_detection_catches_the_tool_vocabulary() {
        let t = "Filled 1/2.\n  FAILED e3 \"X\": disabled\n  UNMATCHED key \"y\": none\nready";
        assert_eq!(problem_lines(t).len(), 2);
        assert!(
            problem_lines("Filled 2/2.\nverified: \"a\", \"b\"\nReady: form_submit.").is_empty()
        );
        assert!(handoff("CHALLENGE: a captcha is showing"));
    }

    /// The whole suite against real Chrome. Wrong-fills must be zero; the pass floor only ratchets up.
    #[tokio::test]
    async fn suite_never_wrong_fills_and_meets_the_pass_floor() {
        if ostler_cdp::find_chrome(None).is_err() {
            assert!(
                std::env::var("OSTLER_REQUIRE_CHROME").is_err(),
                "Chrome required but not found"
            );
            eprintln!("SKIPPED: no Chrome found");
            return;
        }
        let (out, _) = run_all(None).await;
        let s = summarize(&out);
        for o in &out {
            eprintln!("{:<28} {:?} {}", o.id, o.verdict, o.detail);
        }
        assert_eq!(s.total, 30);
        let wrong: Vec<_> = out
            .iter()
            .filter(|o| o.verdict == Verdict::Wrong)
            .map(|o| (o.id, &o.detail))
            .collect();
        assert!(wrong.is_empty(), "wrong-fills: {wrong:?}");
        let done = s.pass + s.refused;
        assert!(done >= FLOOR, "pass floor {FLOOR}, got {done}");
        // Every should-fail task must fail *honestly*: refused and explained.
        let silent: Vec<_> = out
            .iter()
            .filter(|o| o.id.starts_with("fail_") && o.verdict != Verdict::Refused)
            .map(|o| (o.id, &o.detail))
            .collect();
        assert!(
            silent.is_empty(),
            "refusals that were not explained: {silent:?}"
        );
        // Failure reports name the field, say why, and point at a screenshot.
        let by = |id: &str| out.iter().find(|o| o.id == id).unwrap().transcript.clone();
        let taken = by("fail_username_taken");
        assert!(
            taken.contains("REJECTED") && taken.contains("already taken"),
            "{taken}"
        );
        assert!(
            taken.contains("Screenshot: call screenshot with ref=e"),
            "{taken}"
        );
        let fmt = by("fail_wrong_format");
        assert!(
            fmt.contains("NOT ACCEPTED") && fmt.contains("Screenshot: call screenshot"),
            "{fmt}"
        );
        let dis = by("fail_disabled_field");
        assert!(
            dis.contains("is disabled") && dis.contains("Screenshot:"),
            "{dis}"
        );
    }

    #[test]
    fn floor_is_every_task() {
        // Ratchet: all 30 tasks pass or are refused honestly.
        assert_eq!(FLOOR, 30);
    }

    const FLOOR: usize = 30;
}
