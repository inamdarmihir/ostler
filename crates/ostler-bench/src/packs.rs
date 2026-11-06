//! Domain-pack runner: starts the fixture server, drives each pack task with the scripted reference
//! agent through the in-process MCP tool layer, judges it with `check_outcome` against the server's
//! records, and scores claims against outcomes (false success).
//!
//! The scripted agent is a deterministic baseline, NOT a model: a hand-written sequence per task that
//! knows the page layout. It exists to exercise the harness, the fixtures and the scoring end to end.

use std::path::{Path, PathBuf};
use std::time::Instant;

use ostler_fixtures::packs::{check_outcome, pack_tasks, FinalAnswer, PackTask, PACKS};
use ostler_fixtures::FixtureServer;
use serde_json::{json, Value};

use crate::falsesuccess::{Claim, FalseSuccess, Scored};
use crate::scripted::{self, Done};

/// What happened to one task.
pub enum Status {
    /// Run to completion.
    Ran {
        claim: Claim,
        passed: bool,
        failures: Vec<String>,
        answer: FinalAnswer,
        calls: usize,
        note: String,
    },
    /// Not run, with the reason (`needs-tool:<name>` or `no-script`).
    Skipped(String),
}

/// One task's result.
pub struct TaskResult {
    pub task: PackTask,
    pub ms: u128,
    pub status: Status,
}

/// Why a task is not run by the scripted agent, if it is not.
pub fn skip_reason(task: &PackTask) -> Option<String> {
    if let Some(t) = task.needs_tools().first() {
        return Some(format!("needs-tool:{t}"));
    }
    if !scripted::has_script(&task.id) {
        return Some("no-script".to_string());
    }
    None
}

/// Run the selected tasks.
pub async fn run_tasks(
    pack: Option<&str>,
    only: Option<&str>,
) -> anyhow::Result<(Vec<TaskResult>, u128)> {
    let dir = tempfile::tempdir()?;
    // Files that tasks tagged `needs-file:<name>` expect in the upload directory.
    for name in ["resume.pdf", "site-plan.pdf"] {
        std::fs::write(dir.path().join(name), b"%PDF-1.4 placeholder\n")?;
    }
    let srv = FixtureServer::start();
    let mut results = vec![];
    let t0 = Instant::now();
    for p in PACKS {
        if pack.is_some_and(|x| x != "all" && x != p.name) {
            continue;
        }
        for task in pack_tasks(p.name).map_err(anyhow::Error::msg)? {
            if only.is_some_and(|o| !task.id.contains(o)) {
                continue;
            }
            let t = Instant::now();
            let status = match skip_reason(&task) {
                Some(r) => Status::Skipped(r),
                None => run_one(&srv, &task, dir.path()).await,
            };
            results.push(TaskResult {
                task,
                ms: t.elapsed().as_millis(),
                status,
            });
        }
    }
    Ok((results, t0.elapsed().as_millis()))
}

async fn run_one(srv: &FixtureServer, task: &PackTask, upload_dir: &Path) -> Status {
    srv.reset();
    // A fresh browser per task: cart, storage and page state cannot leak between tasks.
    let app = crate::suite::new_app(upload_dir);
    let done = scripted::run(&app, srv, task, upload_dir).await;
    app.shutdown().await;
    let Done {
        claim,
        answer,
        calls,
        note,
    } = done;
    // Late fetches land after the last click returns.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let records = srv.records();
    let verdict = check_outcome(task, &records, &answer);
    Status::Ran {
        claim,
        passed: verdict.passed,
        failures: verdict.failures,
        answer,
        calls,
        note,
    }
}

/// Score the tasks that ran.
pub fn score(results: &[TaskResult]) -> FalseSuccess {
    let items: Vec<Scored> = results
        .iter()
        .filter_map(|r| match &r.status {
            Status::Ran { claim, passed, .. } => Some(Scored {
                id: r.task.id.clone(),
                claim: *claim,
                outcome_passed: *passed,
            }),
            Status::Skipped(_) => None,
        })
        .collect();
    FalseSuccess::score(&items)
}

fn task_json(r: &TaskResult) -> Value {
    let base = json!({"id": r.task.id, "pack": r.task.pack, "title": r.task.title,
        "difficulty": r.task.difficulty, "risk": r.task.risk.as_str()});
    let mut v = base;
    match &r.status {
        Status::Ran {
            claim,
            passed,
            failures,
            answer,
            calls,
            note,
        } => {
            v["status"] = json!("ran");
            v["claim"] = json!(claim.as_str());
            v["outcome_passed"] = json!(passed);
            v["verdict"] = json!(match (claim, passed) {
                (Claim::Success, true) => "true_success",
                (Claim::Success, false) => "FALSE_SUCCESS",
                (Claim::Failure, false) => "true_failure",
                (Claim::Failure, true) => "false_failure",
                (Claim::Unsure, true) => "unsure_passed",
                (Claim::Unsure, false) => "unsure_failed",
            });
            v["failures"] = json!(failures);
            v["answer"] = json!(answer.text);
            v["cited"] = json!(answer.cited);
            v["calls"] = json!(calls);
            v["ms"] = json!(r.ms);
            v["note"] = json!(note);
        }
        Status::Skipped(why) => {
            v["status"] = json!("skipped");
            v["skipped"] = json!(why);
        }
    }
    v
}

/// The JSON report. `agent` is always `scripted`.
pub fn report_json(results: &[TaskResult], total_ms: u128) -> Value {
    let ran = results
        .iter()
        .filter(|r| matches!(r.status, Status::Ran { .. }))
        .count();
    let mut per_pack = vec![];
    for p in PACKS {
        let rs: Vec<&TaskResult> = results.iter().filter(|r| r.task.pack == p.name).collect();
        if rs.is_empty() {
            continue;
        }
        let items: Vec<Scored> = rs
            .iter()
            .filter_map(|r| match &r.status {
                Status::Ran { claim, passed, .. } => Some(Scored {
                    id: r.task.id.clone(),
                    claim: *claim,
                    outcome_passed: *passed,
                }),
                Status::Skipped(_) => None,
            })
            .collect();
        per_pack
            .push(json!({"pack": p.name, "false_success": FalseSuccess::score(&items).to_json()}));
    }
    json!({
        "agent": "scripted",
        "agent_note": "Deterministic reference script, NOT a model. It knows the page layouts, trusts what the page says about its own success, and exists to exercise the fixtures, the outcome checks and the false-success scoring.",
        "generated_unix": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        "tasks_total": results.len(),
        "tasks_ran": ran,
        "tasks_skipped": results.len() - ran,
        "total_ms": total_ms,
        "false_success": score(results).to_json(),
        "per_pack": per_pack,
        "tasks": results.iter().map(task_json).collect::<Vec<_>>(),
    })
}

/// The markdown report.
pub fn report_markdown(results: &[TaskResult], total_ms: u128) -> String {
    let mut s = String::from("# Domain packs: scripted reference agent\n\n");
    s.push_str("**agent: scripted** (a deterministic baseline script, not a model; it knows the page layouts and trusts the page's own success messages). Outcomes are judged by what the fixture server received plus the final answer, never by the agent's claim.\n\n");
    s.push_str("| task | pack | diff | claim | outcome | verdict | calls | detail |\n|---|---|---:|---|---|---|---:|---|\n");
    for r in results {
        match &r.status {
            Status::Ran {
                claim,
                passed,
                failures,
                calls,
                ..
            } => {
                let v = match (claim, passed) {
                    (Claim::Success, true) => "true success",
                    (Claim::Success, false) => "**FALSE SUCCESS**",
                    (Claim::Failure, false) => "true failure",
                    (Claim::Failure, true) => "false failure",
                    (Claim::Unsure, _) => "unsure",
                };
                s.push_str(&format!(
                    "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
                    r.task.id,
                    r.task.pack,
                    r.task.difficulty,
                    claim.as_str(),
                    if *passed { "pass" } else { "fail" },
                    v,
                    calls,
                    failures.join("; ").replace('|', "/")
                ));
            }
            Status::Skipped(why) => s.push_str(&format!(
                "| {} | {} | {} | - | - | skipped: {} | - | |\n",
                r.task.id, r.task.pack, r.task.difficulty, why
            )),
        }
    }
    let ran = results
        .iter()
        .filter(|r| matches!(r.status, Status::Ran { .. }))
        .count();
    s.push_str(&format!(
        "\n{} tasks ({} ran, {} skipped) in {:.1}s.\n\n",
        results.len(),
        ran,
        results.len() - ran,
        total_ms as f64 / 1000.0
    ));
    s.push_str("## False success\n\n");
    s.push_str(&score(results).to_markdown());
    s.push_str("\nSkipped tasks need tools Ostler does not have yet (`needs-tool:<name>` tags) and are not counted.\n");
    s
}

/// `ostler-bench packs`.
pub async fn run(
    pack: Option<String>,
    only: Option<String>,
    scripted: bool,
    out_dir: PathBuf,
    write: bool,
) -> anyhow::Result<Value> {
    if !scripted {
        anyhow::bail!(
            "only the scripted reference agent is implemented here; pass --scripted. Model-driven runs belong to the Python harness (scripts/eval)."
        );
    }
    if let Some(p) = pack.as_deref() {
        if p != "all" && !PACKS.iter().any(|x| x.name == p) {
            anyhow::bail!(
                "unknown pack {p:?}; packs: {}",
                PACKS.iter().map(|x| x.name).collect::<Vec<_>>().join(", ")
            );
        }
    }
    let (results, total_ms) = run_tasks(pack.as_deref(), only.as_deref()).await?;
    let md = report_markdown(&results, total_ms);
    println!("{md}");
    let json = report_json(&results, total_ms);
    if write {
        std::fs::create_dir_all(&out_dir)?;
        let stem = format!("packs-{}-scripted", pack.as_deref().unwrap_or("all"));
        std::fs::write(out_dir.join(format!("{stem}.md")), &md)?;
        std::fs::write(
            out_dir.join(format!("{stem}.json")),
            serde_json::to_string_pretty(&json)?,
        )?;
        eprintln!("wrote {}/{stem}.{{md,json}}", out_dir.display());
    }
    Ok(json)
}
