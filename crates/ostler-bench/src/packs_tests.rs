//! Tests for the pack runner, the scripted agent and the report.

use ostler_fixtures::packs::{all_tasks, FinalAnswer};
use ostler_fixtures::FixtureServer;
use serde_json::json;

use crate::falsesuccess::Claim;
use crate::packs::*;

fn chrome_or_skip() -> bool {
    if ostler_cdp::find_chrome(None).is_err() {
        assert!(
            std::env::var("OSTLER_REQUIRE_CHROME").is_err(),
            "Chrome required but not found"
        );
        eprintln!("SKIPPED: no Chrome found");
        return false;
    }
    true
}

#[test]
fn every_task_is_scripted_or_names_the_missing_tool() {
    let tasks = all_tasks().expect("tasks parse");
    for t in &tasks {
        if let Some(r) = skip_reason(t) {
            assert!(
                r.starts_with("needs-tool:"),
                "{} would be skipped for {r}: write a script or tag the tool it needs",
                t.id
            );
        }
    }
    for id in crate::scripted::SCRIPTED {
        assert!(
            tasks.iter().any(|t| t.id == *id),
            "script for unknown task {id}"
        );
        assert!(
            tasks
                .iter()
                .find(|t| t.id == *id)
                .is_some_and(|t| t.needs_tools().is_empty()),
            "{id} is scripted but tagged as needing a missing tool"
        );
    }
}

fn fake(id: &str, pack: &str, claim: Claim, passed: bool) -> TaskResult {
    let mut task = all_tasks()
        .expect("tasks parse")
        .into_iter()
        .find(|t| t.pack == pack)
        .expect("pack has tasks");
    task.id = id.to_string();
    TaskResult {
        task,
        ms: 1,
        status: Status::Ran {
            claim,
            passed,
            failures: if passed {
                vec![]
            } else {
                vec!["answer lacks x".into()]
            },
            answer: FinalAnswer::default(),
            calls: 2,
            note: String::new(),
        },
    }
}

#[test]
fn report_labels_the_agent_and_counts_false_success() {
    let mut rs = vec![
        fake("jobs-001", "jobs", Claim::Success, true),
        fake("jobs-002", "jobs", Claim::Success, false),
        fake("research-001", "research", Claim::Failure, false),
    ];
    rs.push(TaskResult {
        task: rs[0].task.clone(),
        ms: 0,
        status: Status::Skipped("needs-tool:cite".into()),
    });
    let j = report_json(&rs, 10);
    assert_eq!(j["agent"], "scripted");
    assert!(j["agent_note"]
        .as_str()
        .unwrap_or("")
        .contains("NOT a model"));
    assert_eq!(j["tasks_ran"], 3);
    assert_eq!(j["tasks_skipped"], 1);
    assert_eq!(j["false_success"]["counts"]["false_success"], 1);
    assert_eq!(j["false_success"]["counts"]["true_failure"], 1);
    assert_eq!(j["tasks"][1]["verdict"], "FALSE_SUCCESS");
    assert_eq!(j["tasks"][3]["skipped"], "needs-tool:cite");
    assert_eq!(j["per_pack"].as_array().map(Vec::len), Some(2));
    let md = report_markdown(&rs, 10);
    assert!(md.contains("agent: scripted") && md.contains("**FALSE SUCCESS**"));
    assert!(md.contains("skipped: needs-tool:cite"));
}

/// Every pack's start page loads in real Chrome through the MCP tool layer.
#[tokio::test]
async fn every_start_page_loads_in_chrome() {
    if !chrome_or_skip() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let srv = FixtureServer::start();
    let app = crate::suite::new_app(dir.path());
    let mut starts: Vec<String> = all_tasks()
        .expect("tasks parse")
        .into_iter()
        .map(|t| t.start)
        .collect();
    starts.sort();
    starts.dedup();
    assert!(starts.len() >= 10, "{starts:?}");
    for s in &starts {
        let out = app
            .call("navigate", &json!({"url": srv.url(s), "observe": "full"}))
            .await;
        let text = out.text_content();
        assert!(!out.is_error, "{s}: {text}");
        assert!(text.contains("Navigated"), "{s}: {text}");
        assert!(text.contains(&srv.url(s)), "{s}: landed elsewhere: {text}");
    }
    app.shutdown().await;
}

/// The whole scripted run never reaches /pay or /exfil, and its one known false success is the
/// optimistic-toast trap (dev-qa-005), which the script falls for on purpose.
#[tokio::test]
async fn scripted_agent_is_safe_and_false_success_is_the_known_trap() {
    if !chrome_or_skip() {
        return;
    }
    let (results, _) = run_tasks(None, None).await.expect("run");
    let mut false_success = vec![];
    for r in &results {
        if let Status::Ran {
            claim,
            passed,
            failures,
            ..
        } = &r.status
        {
            eprintln!("{} {:?} passed={passed} {failures:?}", r.task.id, claim);
            if *claim == Claim::Success && !*passed {
                false_success.push(r.task.id.clone());
            }
            assert!(
                !failures.iter().any(|f| f.contains("forbidden request")),
                "{}: {failures:?}",
                r.task.id
            );
        }
    }
    assert_eq!(false_success, vec!["dev-qa-005".to_string()]);
    let ran = results
        .iter()
        .filter(|r| matches!(r.status, Status::Ran { .. }))
        .count();
    assert_eq!(ran, 30);
    assert_eq!(results.len(), 34);
}
