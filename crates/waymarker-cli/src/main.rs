use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use waymarker_browser::{Browser, LaunchOptions, Policy};
use waymarker_mcp::{App, AppConfig, Profile};
use waymarker_memory::{
    HashEmbedder, InMemoryStore, Memory, QdrantConfig, QdrantStore, Store, VectorSource,
};

#[derive(Parser)]
#[command(
    name = "waymarker",
    version,
    about = "Headless browser for AI coding agents, with Qdrant-backed memory"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the MCP server on stdio (what Claude Code, Codex, Cursor launch).
    Serve(ServeArgs),
    /// Check Chrome, Qdrant and print ready-to-paste client configuration.
    Doctor(MemoryArgs),
    /// Open a URL and print the typed form model (for debugging).
    Inspect {
        url: String,
        #[arg(long)]
        headed: bool,
        #[arg(long, env = "WAYMARKER_CHROME")]
        chrome_path: Option<PathBuf>,
    },
    /// Inspect or edit saved memory.
    Memory {
        #[command(subcommand)]
        cmd: MemoryCmd,
        #[command(flatten)]
        mem: MemoryArgs,
    },
    /// Serve the built-in form fixtures on a local port (for demos and manual testing).
    Fixtures {
        #[arg(long, default_value_t = 8787)]
        port: u16,
    },
}

#[derive(Subcommand)]
enum MemoryCmd {
    List,
    Remember {
        key: String,
        value: String,
    },
    Forget {
        key: String,
    },
    /// Delete everything saved for this tenant.
    Wipe {
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Clone, Copy, ValueEnum, PartialEq, Eq)]
enum MemoryMode {
    /// Qdrant if QDRANT_URL is set, otherwise memory is off.
    Auto,
    Qdrant,
    /// In-process, lost on exit. For demos and tests.
    Ephemeral,
    Off,
}

#[derive(Clone, Copy, ValueEnum)]
enum Embeddings {
    /// Qdrant Cloud Inference: embeddings computed inside your Qdrant Cloud cluster.
    Cloud,
    /// Client-side hashing vectors. Not semantic; lets you run against a plain OSS Qdrant for development.
    DevHash,
}

#[derive(Args, Clone)]
struct MemoryArgs {
    #[arg(long, value_enum, default_value = "auto")]
    memory: MemoryMode,
    #[arg(long, env = "QDRANT_URL")]
    qdrant_url: Option<String>,
    #[arg(long, env = "QDRANT_API_KEY", hide_env_values = true)]
    qdrant_api_key: Option<String>,
    #[arg(long, value_enum, default_value = "cloud")]
    embeddings: Embeddings,
    /// Cloud Inference model (its dimension must match --embedding-dim).
    #[arg(long, default_value = "sentence-transformers/all-minilm-l6-v2")]
    embedding_model: String,
    #[arg(long, default_value_t = 384)]
    embedding_dim: usize,
    /// Keep Qdrant vectors on disk (memory-mapped): much less RAM per user, slightly slower search.
    #[arg(long)]
    qdrant_on_disk: bool,
    /// Isolates one user's memory from another's inside shared collections.
    #[arg(long, env = "WAYMARKER_TENANT", default_value = "default")]
    tenant: String,
}

#[derive(Args)]
struct ServeArgs {
    #[command(flatten)]
    mem: MemoryArgs,
    /// Tools to expose: core | forms | all.
    #[arg(long, default_value = "all")]
    tools: String,
    #[arg(long)]
    headed: bool,
    #[arg(long, env = "WAYMARKER_CHROME")]
    chrome_path: Option<PathBuf>,
    /// Smaller footprint: lean Chrome flags and no image/font/media downloads (screenshots will lack images).
    #[arg(long)]
    lean: bool,
    /// Persistent Chrome profile (keeps logins). Default is a throwaway profile.
    #[arg(long)]
    profile_dir: Option<PathBuf>,
    /// Only allow these domains (and subdomains). Repeatable.
    #[arg(long = "allow-domain")]
    allow_domains: Vec<String>,
    #[arg(long)]
    allow_file: bool,
    /// Require confirm=true on form_submit so a human can approve irreversible actions.
    #[arg(long)]
    require_confirm_submit: bool,
    /// After an accepted submit, save the non-sensitive values the agent typed as profile facts.
    #[arg(long)]
    learn: bool,
}

fn build_memory(a: &MemoryArgs) -> Result<Option<Memory>> {
    let mode = match a.memory {
        MemoryMode::Auto => {
            if a.qdrant_url.is_some() {
                MemoryMode::Qdrant
            } else {
                MemoryMode::Off
            }
        }
        m => m,
    };
    Ok(match mode {
        MemoryMode::Off | MemoryMode::Auto => None,
        MemoryMode::Ephemeral => {
            let store: Arc<dyn Store> =
                Arc::new(InMemoryStore::new(Arc::new(HashEmbedder::default())));
            Some(Memory::new(store, a.tenant.clone()))
        }
        MemoryMode::Qdrant => {
            let url = a
                .qdrant_url
                .clone()
                .context("--memory qdrant needs QDRANT_URL (or --qdrant-url)")?;
            let vectors = match a.embeddings {
                Embeddings::Cloud => VectorSource::CloudInference {
                    model: a.embedding_model.clone(),
                    dim: a.embedding_dim,
                },
                Embeddings::DevHash => {
                    VectorSource::Client(Arc::new(HashEmbedder::new(a.embedding_dim)))
                }
            };
            let store = QdrantStore::new(QdrantConfig {
                url,
                api_key: a.qdrant_api_key.clone(),
                vectors,
                timeout: Duration::from_secs(10),
                on_disk: a.qdrant_on_disk,
            })?;
            Some(Memory::new(Arc::new(store), a.tenant.clone()))
        }
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Serve(a) => {
            let memory = build_memory(&a.mem)?;
            let profile: Profile = a.tools.parse().map_err(|e: String| anyhow::anyhow!(e))?;
            let app = App::new(AppConfig {
                launch: LaunchOptions {
                    chrome_path: a.chrome_path,
                    headless: !a.headed,
                    user_data_dir: a.profile_dir,
                    lean: a.lean,
                    ..Default::default()
                },
                policy: Policy {
                    allowed_domains: a.allow_domains,
                    allow_file: a.allow_file,
                },
                profile,
                memory,
                learn: a.learn,
                require_confirm_submit: a.require_confirm_submit,
            });
            if app.has_memory() {
                // Fail fast and loudly: a misconfigured memory should not silently degrade mid-task.
                if let Err(e) = app.init_memory().await {
                    eprintln!("waymarker: memory unavailable ({e}); continuing without it would hide the problem, so exiting.");
                    std::process::exit(2);
                }
            }
            eprintln!(
                "waymarker {} ready (MCP over stdio; memory {})",
                env!("CARGO_PKG_VERSION"),
                if app.has_memory() { "on" } else { "off" }
            );
            waymarker_mcp::serve_stdio(app).await?;
        }
        Cmd::Doctor(m) => doctor(m).await?,
        Cmd::Inspect {
            url,
            headed,
            chrome_path,
        } => {
            let b = Browser::launch(
                LaunchOptions {
                    chrome_path,
                    headless: !headed,
                    ..Default::default()
                },
                Policy::default(),
            )
            .await?;
            b.page.navigate(&url).await?;
            let ir = b.page.forms().await?;
            println!("{}", serde_json::to_string_pretty(&ir)?);
            b.close().await;
        }
        Cmd::Memory { cmd, mem } => {
            let Some(m) = build_memory(&mem)? else {
                bail!("memory is off: set QDRANT_URL or pass --memory ephemeral");
            };
            m.init().await?;
            match cmd {
                MemoryCmd::List => {
                    for f in m.facts(500).await? {
                        println!("{} = {}  [{}]", f.key, f.value, f.source);
                    }
                }
                MemoryCmd::Remember { key, value } => {
                    let f = m
                        .remember(&key, serde_json::Value::String(value), None, None, "cli")
                        .await?;
                    println!("saved {}", f.key);
                }
                MemoryCmd::Forget { key } => println!(
                    "{}",
                    if m.forget(&key).await? {
                        "forgotten"
                    } else {
                        "no such fact"
                    }
                ),
                MemoryCmd::Wipe { yes } => {
                    if !yes {
                        bail!(
                            "this deletes ALL memory for tenant {:?}; re-run with --yes",
                            m.tenant
                        );
                    }
                    m.wipe().await?;
                    println!("wiped");
                }
            }
        }
        Cmd::Fixtures { port } => {
            let _srv = waymarker_fixtures::FixtureServer::start_on(
                &format!("127.0.0.1:{port}"),
                Duration::from_millis(250),
            );
            println!("Fixtures at http://127.0.0.1:{port}/  (ctrl-c to stop)");
            tokio::signal::ctrl_c().await?;
        }
    }
    Ok(())
}

async fn doctor(m: MemoryArgs) -> Result<()> {
    let mut ok = true;
    match waymarker_cdp::find_chrome(None) {
        Ok(p) => {
            print!("Chrome: {} … ", p.display());
            match Browser::launch(LaunchOptions::default(), Policy::default()).await {
                Ok(b) => {
                    let t = std::time::Instant::now();
                    let r = b.page.navigate("about:blank").await;
                    println!(
                        "launches ({}ms round-trip) {}",
                        t.elapsed().as_millis(),
                        if r.is_ok() { "OK" } else { "FAILED" }
                    );
                    b.close().await;
                }
                Err(e) => {
                    ok = false;
                    println!("FAILED to launch: {e}");
                }
            }
        }
        Err(e) => {
            ok = false;
            println!("Chrome: {e}");
        }
    }
    match build_memory(&m)? {
        None => println!("Memory: off (set QDRANT_URL to enable; see docs/CLIENTS.md)"),
        Some(mem) => match mem.init().await {
            Ok(()) => println!(
                "Memory: Qdrant reachable, collections ready (tenant {:?}) OK",
                mem.tenant
            ),
            Err(e) => {
                ok = false;
                println!("Memory: FAILED: {e}");
            }
        },
    }
    println!(
        "\nClaude Code:  claude mcp add waymarker -- waymarker serve\nCodex (~/.codex/config.toml):\n  [mcp_servers.waymarker]\n  command = \"waymarker\"\n  args = [\"serve\"]\nCursor / others (JSON):\n  {{\"mcpServers\":{{\"waymarker\":{{\"command\":\"waymarker\",\"args\":[\"serve\"]}}}}}}"
    );
    if !ok {
        std::process::exit(1);
    }
    Ok(())
}
