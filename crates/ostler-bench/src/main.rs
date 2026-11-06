//! Benchmarks for the questions an engineer or investor asks first:
//! how many tool calls / tokens / dollars per form, how fast, how much memory per session,
//! what does memory cost at scale, and how many sessions fit on a machine.
//!
//! Everything here is measured on the machine it runs on and printed with its assumptions.

mod cost;
mod falsesuccess;
mod forms;
mod memory;
mod packs;
#[cfg(test)]
mod packs_tests;
mod real;
mod scale;
mod scripted;
mod suite;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "ostler-bench")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
    /// Also write machine-readable results here.
    #[arg(long, global = true)]
    json: Option<std::path::PathBuf>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Form tasks: engine vs per-field baseline; tool calls, tokens, latency, LLM cost model.
    Forms {
        #[arg(long, default_value_t = 3)]
        reps: usize,
        #[command(flatten)]
        cost: cost::CostArgs,
    },
    /// Qdrant memory at scale: filtered search latency, footprint.
    Memory {
        /// Comma list of TENANTSxFACTS, e.g. 1000x20,10000x20
        #[arg(long, default_value = "1000x20,10000x20")]
        sizes: String,
        #[arg(long, default_value_t = 300)]
        queries: usize,
        /// Create collections with on-disk vectors + HNSW.
        #[arg(long)]
        on_disk: bool,
    },
    /// Realistic-forms suite: 30 fixtures, judged by what the server received. Exits non-zero on any wrong-fill.
    Suite {
        /// Run only tasks whose id contains this text.
        #[arg(long)]
        only: Option<String>,
    },
    /// Real-site evaluation: fills live public practice forms (nothing is submitted) and judges each field by
    /// reading the page back with our own JavaScript. Exits non-zero on any false claim.
    Real {
        /// Run only sites whose id contains this text.
        #[arg(long)]
        only: Option<String>,
    },
    /// Browser footprint: default vs lean Chrome (processes, memory, startup, task time).
    Footprint {
        #[arg(long, default_value_t = 3)]
        reps: usize,
    },
    /// Serve the fixture pages on a fixed port (for the competitor comparison driver).
    /// Submissions are readable at GET /__submissions and cleared with POST /__reset.
    ServeFixtures {
        #[arg(long, default_value_t = 8765)]
        port: u16,
    },
    /// Domain packs (research, gov-forms, jobs, dev-qa, commerce): tasks judged by what the fixture server
    /// received, scored for false success. The only agent here is a deterministic script, NOT a model.
    Packs {
        /// Pack name, or `all`.
        #[arg(long, default_value = "all")]
        pack: String,
        /// Run only tasks whose id contains this text.
        #[arg(long)]
        only: Option<String>,
        /// Use the scripted reference agent (required; model-driven runs live in the Python harness).
        #[arg(long)]
        scripted: bool,
        /// Where to write packs-<pack>-scripted.{json,md}.
        #[arg(long, default_value = "bench-results")]
        out_dir: std::path::PathBuf,
        /// Print the tables only; write no files.
        #[arg(long)]
        no_write: bool,
    },
    /// Parallel browser sessions: throughput, memory per session, CPU per task, cost to serve.
    Concurrency {
        #[arg(long, default_value = "1,2,4,8")]
        sessions: String,
        #[arg(long, default_value_t = 6)]
        tasks_per_session: usize,
        /// Use the lean Chrome profile.
        #[arg(long)]
        lean: bool,
        #[command(flatten)]
        serve: scale::ServeCostArgs,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let result = match cli.cmd {
        Cmd::Forms { reps, cost } => forms::run(reps, cost).await?,
        Cmd::Memory {
            sizes,
            queries,
            on_disk,
        } => memory::run(&sizes, queries, on_disk).await?,
        Cmd::Suite { only } => suite::run(only).await?,
        Cmd::Real { only } => real::run(only).await?,
        Cmd::ServeFixtures { port } => {
            let srv = ostler_fixtures::FixtureServer::start_on(
                &format!("127.0.0.1:{port}"),
                std::time::Duration::from_millis(250),
            );
            println!("fixtures listening on http://127.0.0.1:{}", srv.port);
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            }
        }
        Cmd::Packs {
            pack,
            only,
            scripted,
            out_dir,
            no_write,
        } => packs::run(Some(pack), only, scripted, out_dir, !no_write).await?,
        Cmd::Footprint { reps } => scale::footprint(reps).await?,
        Cmd::Concurrency {
            sessions,
            tasks_per_session,
            lean,
            serve,
        } => scale::run(&sessions, tasks_per_session, lean, serve).await?,
    };
    if let Some(p) = cli.json {
        std::fs::write(&p, serde_json::to_string_pretty(&result)?)?;
        eprintln!("wrote {}", p.display());
    }
    Ok(())
}
