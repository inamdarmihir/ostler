//! Concurrency & cost-to-serve: how many browser sessions fit on a machine, what each costs.

use std::process::Command;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use clap::Args;
use serde_json::{json, Value};
use waymarker_fixtures::FixtureServer;

use crate::forms;

#[derive(Args, Clone)]
pub struct ServeCostArgs {
    /// USD per vCPU-hour. ILLUSTRATIVE; use your cloud's price.
    #[arg(long, default_value_t = 0.04)]
    pub vcpu_hour_usd: f64,
    /// USD per GiB-hour of RAM. ILLUSTRATIVE.
    #[arg(long, default_value_t = 0.005)]
    pub gib_hour_usd: f64,
}

/// Cumulative CPU seconds of a process and all descendants (Unix `ps`).
fn tree_cpu_secs(root: u32) -> Option<f64> {
    let out = Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,time="])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let rows: Vec<(u32, u32, f64)> = text
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let (pid, ppid, t) = (
                it.next()?.parse().ok()?,
                it.next()?.parse().ok()?,
                it.next()?,
            );
            Some((pid, ppid, parse_cpu_time(t)?))
        })
        .collect();
    let mut members = std::collections::HashSet::from([root]);
    loop {
        let n = members.len();
        for (pid, ppid, _) in &rows {
            if members.contains(ppid) {
                members.insert(*pid);
            }
        }
        if members.len() == n {
            break;
        }
    }
    Some(
        rows.iter()
            .filter(|(p, _, _)| members.contains(p))
            .map(|(_, _, t)| t)
            .sum(),
    )
}

/// `[[dd-]hh:]mm:ss[.cc]`
fn parse_cpu_time(s: &str) -> Option<f64> {
    let (days, rest) = match s.split_once('-') {
        Some((d, r)) => (d.parse::<f64>().ok()?, r),
        None => (0.0, s),
    };
    let mut secs = 0.0;
    for part in rest.split(':') {
        secs = secs * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(days * 86400.0 + secs)
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    if v.is_empty() {
        0.0
    } else {
        v[(((v.len() - 1) as f64) * p).round() as usize]
    }
}

pub async fn run(
    sessions: &str,
    tasks_per_session: usize,
    lean: bool,
    serve: ServeCostArgs,
) -> Result<Value> {
    let srv = Arc::new(FixtureServer::start());
    let mix: Vec<_> = forms::scenarios()
        .into_iter()
        .filter(|s| {
            [
                "basic contact form",
                "dependent dropdowns",
                "shadow DOM",
                "controlled inputs",
                "validation (3 rules)",
                "same-origin iframe",
            ]
            .contains(&s.name)
        })
        .collect();
    let mix = Arc::new(mix);
    println!(
        "Chrome profile: {}. Memory measured as {}.",
        if lean { "lean" } else { "default" },
        waymarker_cdp::memory_method()
    );
    println!("Each session = its own Chrome + MCP app. Task mix: {} form scenarios (navigate + fill + submit), the fixture API adds 250 ms latency to dependent dropdowns.", mix.len());
    println!(
        "Machine: {} logical CPUs.\n",
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0)
    );
    println!(
        "{:>8} {:>7} {:>10} {:>10} {:>10} {:>12} {:>12} {:>14} {:>9}",
        "sessions",
        "tasks",
        "tasks/min",
        "p50 ms",
        "p95 ms",
        "mem/sess MiB",
        "CPU-s/task",
        "$/1k tasks*",
        "success"
    );
    let mut results = vec![];
    for n in sessions.split(',') {
        let n: usize = n.trim().parse()?;
        let wall = Instant::now();
        let mut handles = vec![];
        for w in 0..n {
            let (srv, mix) = (srv.clone(), mix.clone());
            handles.push(tokio::spawn(async move {
                let app = forms::new_app_with(lean);
                // Each session needs its own recording server state; share the server but tag by order of arrival.
                let mut lat = vec![];
                let mut ok = 0usize;
                app.call(
                    "navigate",
                    &json!({"url": "about:blank", "observe": "none"}),
                )
                .await; // launch outside the timed loop
                for i in 0..tasks_per_session {
                    let s = &mix[(w + i) % mix.len()];
                    let t = Instant::now();
                    let r = forms::run_engine_isolated(&app, &srv, s).await;
                    lat.push(t.elapsed().as_secs_f64() * 1000.0);
                    if r.ok {
                        ok += 1;
                    }
                }
                let (rss, pid) = (
                    app.browser_metrics().await.and_then(|m| m.1),
                    app.browser_pid().await,
                );
                let cpu = pid.and_then(tree_cpu_secs);
                app.shutdown().await;
                (lat, ok, rss, cpu)
            }));
        }
        let (mut lat, mut ok, mut rss_total, mut cpu_total) = (vec![], 0usize, 0u64, 0f64);
        for h in handles {
            let (l, o, r, c) = h.await?;
            lat.extend(l);
            ok += o;
            rss_total += r.unwrap_or(0);
            cpu_total += c.unwrap_or(0.0);
        }
        let wall_s = wall.elapsed().as_secs_f64();
        let tasks = n * tasks_per_session;
        let tpm = tasks as f64 / wall_s * 60.0;
        let rss_mib = rss_total as f64 / 1024.0 / n as f64;
        let cpu_per_task = cpu_total / tasks as f64;
        // Cost to serve one task: CPU-seconds plus RAM held for the task's wall time.
        let task_wall = wall_s / (tasks as f64 / n as f64);
        let usd_task = cpu_per_task / 3600.0 * serve.vcpu_hour_usd
            + (rss_mib / 1024.0) * (task_wall / 3600.0) * serve.gib_hour_usd;
        let (p50, p95) = (pct(&mut lat, 0.5), pct(&mut lat, 0.95));
        println!("{n:>8} {tasks:>7} {tpm:>10.0} {p50:>10.0} {p95:>10.0} {rss_mib:>12.0} {cpu_per_task:>12.2} {:>14.4} {:>8.0}%", usd_task * 1000.0, ok as f64 / tasks as f64 * 100.0);
        results.push(json!({"sessions": n, "tasks": tasks, "tasks_per_min": tpm, "p50_ms": p50, "p95_ms": p95,
            "rss_per_session_mib": rss_mib, "cpu_s_per_task": cpu_per_task, "usd_per_1k_tasks": usd_task * 1000.0, "success": ok as f64 / tasks as f64}));
    }
    println!("\n* compute only (CPU-seconds + RAM-time at the illustrative prices above); excludes LLM tokens, Qdrant, bandwidth, proxies, and idle capacity.");
    println!("  CPU is the cumulative CPU time of each Chrome process tree at the end of its run (includes startup).");
    Ok(json!({"concurrency": results}))
}

/// Default vs lean Chrome: how much memory and startup time does the lean profile save?
pub async fn footprint(reps: usize) -> Result<Value> {
    use serde_json::json;
    use waymarker_browser::{Browser, FillRequest, LaunchOptions, Policy};
    let srv = FixtureServer::start();
    println!("Footprint after one real task (navigate + fill + submit on the 20-field form). Memory is measured as: {}.\n", waymarker_cdp::memory_method());
    println!(
        "{:<10} {:>10} {:>10} {:>10} {:>12} {:>10}",
        "profile", "start ms", "procs", "mem MiB", "task ms", "success"
    );
    let mut out = vec![];
    for (name, lean) in [("default", false), ("lean", true)] {
        let (mut starts, mut procs, mut rss, mut tasks, mut ok) =
            (vec![], vec![], vec![], vec![], 0usize);
        for _ in 0..reps {
            let t = Instant::now();
            let b = Browser::launch(
                LaunchOptions {
                    lean,
                    ..Default::default()
                },
                Policy::default(),
            )
            .await?;
            b.page().navigate("about:blank").await?;
            starts.push(t.elapsed().as_secs_f64() * 1000.0);
            let t = Instant::now();
            b.page().navigate(&srv.url("/longform.html")).await?;
            let values = [
                ("First name", "Ada"),
                ("Last name", "Lovelace"),
                ("Email", "ada@example.com"),
                ("Country", "United States"),
                ("privacy policy", "true"),
            ]
            .iter()
            .map(|(k, v)| (k.to_string(), json!(v)))
            .collect();
            let rep = b
                .page()
                .fill_form(
                    FillRequest {
                        values,
                        ..Default::default()
                    },
                    None,
                )
                .await?;
            let sub = b.page().submit(None, None).await?;
            tasks.push(t.elapsed().as_secs_f64() * 1000.0);
            if rep.failed() == 0 && matches!(sub.status, waymarker_browser::SubmitStatus::Submitted)
            {
                ok += 1;
            }
            if let Some((n, kb)) = b.pid().and_then(waymarker_cdp::process_tree_stats) {
                procs.push(n as f64);
                rss.push(kb as f64 / 1024.0);
            }
            b.close().await;
        }
        let (st, pr, rs, tk) = (
            pct(&mut starts, 0.5),
            pct(&mut procs, 0.5),
            pct(&mut rss, 0.5),
            pct(&mut tasks, 0.5),
        );
        println!(
            "{name:<10} {st:>10.0} {pr:>10.0} {rs:>10.0} {tk:>12.0} {:>9.0}%",
            ok as f64 / reps as f64 * 100.0
        );
        out.push(json!({"profile": name, "start_ms": st, "processes": pr, "rss_mib": rs, "task_ms": tk, "success": ok as f64 / reps as f64}));
    }
    Ok(json!({"footprint": out}))
}
