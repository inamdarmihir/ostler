mod config;

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
    /// Check that Chrome launches and (if configured) Qdrant is reachable.
    Doctor(MemoryArgs),
    /// Save your settings (Qdrant URL and API key, tenant, defaults) to the config file and verify them.
    Init(InitArgs),
    /// Show or edit the config file.
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },
    /// Print (or with --apply, run) the exact setup for an MCP client, using this binary's absolute path.
    Setup {
        #[arg(value_enum)]
        client: Client,
        /// Run the client's own CLI to register Waymarker (only supported for `claude`).
        #[arg(long)]
        apply: bool,
        /// Extra `waymarker serve` flags, after `--` (e.g. `-- --lean --tools forms`).
        #[arg(last = true)]
        serve_args: Vec<String>,
    },
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

#[derive(Args)]
struct InitArgs {
    /// Qdrant URL, e.g. https://<cluster>.cloud.qdrant.io:6333 (the :6333 is added for Qdrant Cloud if missing).
    #[arg(long)]
    qdrant_url: Option<String>,
    /// Qdrant API key. Prefer the interactive prompt or the QDRANT_API_KEY variable: flags show up in `ps`.
    #[arg(long, env = "QDRANT_API_KEY", hide_env_values = true)]
    qdrant_api_key: Option<String>,
    #[arg(long)]
    tenant: Option<String>,
    /// Save `learn = true` (opt in to remembering values typed after accepted forms).
    #[arg(long)]
    learn: bool,
    /// Save `lean = true` (less RAM; no images in screenshots).
    #[arg(long)]
    lean: bool,
    /// Embeddings: `cloud` (Qdrant Cloud Inference) or `dev-hash` (non-semantic, for open-source Qdrant).
    #[arg(long, value_enum)]
    embeddings: Option<Embeddings>,
    /// Never prompt; only use the flags given.
    #[arg(long)]
    yes: bool,
    /// Save without testing the connection.
    #[arg(long)]
    no_check: bool,
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Print the effective file contents (API key masked).
    Show,
    /// Print the config file path.
    Path,
    /// Set one value, e.g. `waymarker config set tenant alice`.
    Set { key: String, value: String },
    /// Remove one value.
    Unset { key: String },
}

fn prompt(question: &str, default: &str) -> Result<String> {
    use std::io::{BufRead, Write};
    if default.is_empty() {
        eprint!("{question}: ");
    } else {
        eprint!("{question} [{default}]: ");
    }
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    let line = line.trim();
    Ok(if line.is_empty() {
        default.to_string()
    } else {
        line.to_string()
    })
}

fn yes_no(question: &str, default: bool) -> Result<bool> {
    let a = prompt(
        &format!("{question} (y/n)"),
        if default { "y" } else { "n" },
    )?;
    Ok(matches!(a.to_lowercase().as_str(), "y" | "yes" | "true"))
}

async fn init(a: InitArgs) -> Result<()> {
    use std::io::IsTerminal;
    let mut cfg = config::read()?;
    let any_flag =
        a.qdrant_url.is_some() || a.tenant.is_some() || a.learn || a.lean || a.embeddings.is_some();
    let interactive = !a.yes && !any_flag && std::io::stdin().is_terminal();

    let mut url = a.qdrant_url.clone();
    let mut key = a.qdrant_api_key.clone();
    if interactive {
        eprintln!("Waymarker setup. Press Enter to keep the value shown in [brackets].\n");
        eprintln!("Memory is optional. Without a Qdrant URL everything except memory still works.");
        let cur = cfg.get("qdrant_url").cloned().unwrap_or_default();
        let u = prompt("Qdrant URL (blank = no memory)", &cur)?;
        if !u.is_empty() {
            url = Some(u);
            eprintln!("API key: input is visible; leave blank for a local Qdrant without auth.");
            let has = cfg.contains_key("qdrant_api_key");
            let k = prompt(
                if has {
                    "Qdrant API key (blank = keep saved)"
                } else {
                    "Qdrant API key"
                },
                "",
            )?;
            if !k.is_empty() {
                key = Some(k);
            }
            let t = prompt(
                "Tenant (separates users on a shared cluster)",
                cfg.get("tenant").map(String::as_str).unwrap_or("default"),
            )?;
            cfg.insert("tenant".into(), t);
            let learn = yes_no(
                "Remember non-sensitive values the agent types after a form is accepted (--learn)?",
                cfg.get("learn").map(|v| v == "true").unwrap_or(false),
            )?;
            cfg.insert("learn".into(), learn.to_string());
        }
        let lean = yes_no(
            "Lean mode (about 37% less RAM; screenshots omit images)?",
            cfg.get("lean").map(|v| v == "true").unwrap_or(false),
        )?;
        cfg.insert("lean".into(), lean.to_string());
    }

    if let Some(u) = url {
        let u = config::normalize_qdrant_url(&u);
        cfg.insert("qdrant_url".into(), u);
    }
    if let Some(k) = key {
        cfg.insert("qdrant_api_key".into(), k);
    }
    if let Some(t) = a.tenant {
        cfg.insert("tenant".into(), t);
    }
    if a.learn {
        cfg.insert("learn".into(), "true".into());
    }
    if a.lean {
        cfg.insert("lean".into(), "true".into());
    }
    if let Some(e) = a.embeddings {
        let v = match e {
            Embeddings::Cloud => "cloud",
            Embeddings::DevHash => "dev-hash",
        };
        cfg.insert("embeddings".into(), v.into());
    }
    if cfg.is_empty() {
        eprintln!("Nothing to save. Run `waymarker init` in a terminal, or pass flags such as --qdrant-url.");
        return Ok(());
    }

    let path = config::write(&cfg)?;
    println!("Saved {} (owner-only permissions).", path.display());

    if cfg.contains_key("qdrant_url") && !a.no_check {
        // Re-read through the same path `serve` uses so the check tests exactly what will run.
        for (k, env) in config::KEYS {
            if let Some(v) = cfg.get(*k) {
                std::env::set_var(env, v);
            }
        }
        if let Cmd::Doctor(m) = Cli::parse_from(["waymarker", "doctor"]).cmd {
            let mem = build_memory(&m)?;
            match mem {
                Some(mem) => match mem.init().await {
                    Ok(()) => println!(
                        "Qdrant: reachable, collections ready (tenant {:?}). OK",
                        mem.tenant
                    ),
                    Err(e) => {
                        println!("Qdrant: FAILED: {e}\nThe settings were saved anyway. Fix the URL/key with `waymarker config set`, then run `waymarker doctor`.");
                        std::process::exit(1);
                    }
                },
                None => println!("Memory is off."),
            }
        }
    }
    println!("\nNext: `waymarker setup claude --apply` (or codex / cursor / vscode).");
    Ok(())
}

fn config_cmd(cmd: ConfigCmd) -> Result<()> {
    match cmd {
        ConfigCmd::Path => println!("{}", config::path().display()),
        ConfigCmd::Show => {
            let cfg = config::read()?;
            println!("# {}", config::path().display());
            if cfg.is_empty() {
                println!("(empty; run `waymarker init`)");
            }
            for (k, v) in &cfg {
                println!("{k} = {}", config::mask(k, v));
            }
            let from_env: Vec<_> = config::KEYS
                .iter()
                .filter(|(k, e)| std::env::var_os(e).is_some() && !cfg.contains_key(*k))
                .map(|(_, e)| *e)
                .collect();
            if !from_env.is_empty() {
                println!("# also set in your environment: {}", from_env.join(", "));
            }
        }
        ConfigCmd::Set { key, value } => {
            anyhow::ensure!(
                config::env_for(&key).is_some(),
                "unknown key {key:?}; valid keys: {}",
                config::KEYS
                    .iter()
                    .map(|(k, _)| *k)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            let mut cfg = config::read()?;
            let value = if key == "qdrant_url" {
                config::normalize_qdrant_url(&value)
            } else {
                value
            };
            cfg.insert(key.clone(), value.clone());
            config::write(&cfg)?;
            println!("{key} = {}", config::mask(&key, &value));
        }
        ConfigCmd::Unset { key } => {
            let mut cfg = config::read()?;
            anyhow::ensure!(
                cfg.remove(&key).is_some(),
                "{key:?} is not set in the config file"
            );
            config::write(&cfg)?;
            println!("removed {key}");
        }
    }
    Ok(())
}

#[derive(Clone, Copy, ValueEnum)]
enum Client {
    Claude,
    Codex,
    Cursor,
    Vscode,
    /// Generic `mcpServers` JSON.
    Json,
}

fn setup(client: Client, apply: bool, serve_args: &[String]) -> Result<()> {
    // The absolute path avoids "command not found" when the client's PATH differs from your shell's.
    let exe = std::env::current_exe()?.display().to_string();
    let mut args = vec!["serve".to_string()];
    args.extend(serve_args.iter().cloned());
    let json_args = serde_json::to_string(&args)?;
    match client {
        Client::Claude => {
            let cmd = format!("claude mcp add waymarker -- {exe} {}", args.join(" "));
            if apply {
                let st = std::process::Command::new("claude")
                    .args(["mcp", "add", "waymarker", "--", &exe])
                    .args(&args)
                    .status()
                    .map_err(|e| anyhow::anyhow!("could not run `claude` ({e}); run this yourself:\n  {cmd}"))?;
                if !st.success() {
                    anyhow::bail!("`claude mcp add` failed; run it yourself to see why:\n  {cmd}");
                }
                println!("Registered. Restart Claude Code, then ask it to use waymarker.");
            } else {
                println!("{cmd}\n\n(add --apply to run it for you)");
            }
        }
        Client::Codex => println!(
            "Add to ~/.codex/config.toml:\n\n[mcp_servers.waymarker]\ncommand = {exe:?}\nargs = {json_args}"
        ),
        Client::Cursor => println!(
            "Add to ~/.cursor/mcp.json (or .cursor/mcp.json in a project):\n\n{}",
            mcp_json(&exe, &args)
        ),
        Client::Vscode => println!(
            "Add to .vscode/mcp.json:\n\n{}",
            serde_json::to_string_pretty(&serde_json::json!({"servers": {"waymarker": {"type": "stdio", "command": exe, "args": args}}}))?
        ),
        Client::Json => println!("{}", mcp_json(&exe, &args)),
    }
    if apply && !matches!(client, Client::Claude) {
        eprintln!("note: --apply only supports `claude`; other clients' config files are left untouched. Paste the snippet above.");
    }
    Ok(())
}

fn mcp_json(exe: &str, args: &[String]) -> String {
    serde_json::to_string_pretty(
        &serde_json::json!({"mcpServers": {"waymarker": {"command": exe, "args": args}}}),
    )
    .unwrap()
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
    #[arg(long, value_enum, default_value = "auto", env = "WAYMARKER_MEMORY")]
    memory: MemoryMode,
    #[arg(long, env = "QDRANT_URL")]
    qdrant_url: Option<String>,
    #[arg(long, env = "QDRANT_API_KEY", hide_env_values = true)]
    qdrant_api_key: Option<String>,
    #[arg(
        long,
        value_enum,
        default_value = "cloud",
        env = "WAYMARKER_EMBEDDINGS"
    )]
    embeddings: Embeddings,
    /// Cloud Inference model (its dimension must match --embedding-dim).
    #[arg(
        long,
        default_value = "sentence-transformers/all-minilm-l6-v2",
        env = "WAYMARKER_EMBEDDING_MODEL"
    )]
    embedding_model: String,
    #[arg(long, default_value_t = 384)]
    embedding_dim: usize,
    /// Keep Qdrant vectors on disk (memory-mapped): much less RAM per user, slightly slower search.
    #[arg(long, env = "WAYMARKER_QDRANT_ON_DISK")]
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
    #[arg(long, default_value = "all", env = "WAYMARKER_TOOLS")]
    tools: String,
    #[arg(long)]
    headed: bool,
    #[arg(long, env = "WAYMARKER_CHROME")]
    chrome_path: Option<PathBuf>,
    /// Smaller footprint: lean Chrome flags and no image/font/media downloads (screenshots will lack images).
    #[arg(long, env = "WAYMARKER_LEAN")]
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
    /// Include timings, fingerprints and ids in tool output. Off by default: they cost the agent context for no benefit.
    #[arg(long, env = "WAYMARKER_VERBOSE")]
    verbose: bool,
    /// After an accepted submit, save the non-sensitive values the agent typed as profile facts.
    #[arg(long, env = "WAYMARKER_LEARN")]
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
    config::load_into_env();
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
                verbose: a.verbose,
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
        Cmd::Init(a) => init(a).await?,
        Cmd::Config { cmd } => config_cmd(cmd)?,
        Cmd::Setup {
            client,
            apply,
            serve_args,
        } => setup(client, apply, &serve_args)?,
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
    println!("\nNext: `waymarker setup claude|codex|cursor|vscode|json` prints the exact config for your client.");
    if !ok {
        std::process::exit(1);
    }
    Ok(())
}
