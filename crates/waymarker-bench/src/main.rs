//! Benchmarks for the questions an engineer or investor asks first:
//! how many tool calls / tokens / dollars per form, how fast, how much memory per session,
//! what does memory cost at scale, and how many sessions fit on a machine.
//!
//! Everything here is measured on the machine it runs on and printed with its assumptions.

mod cost;
mod forms;
mod memory;
mod scale;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "waymarker-bench")]
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
    /// Browser footprint: default vs lean Chrome (processes, memory, startup, task time).
    Footprint {
        #[arg(long, default_value_t = 3)]
        reps: usize,
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
