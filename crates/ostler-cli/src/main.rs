mod config;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use ostler_browser::{Browser, LaunchOptions, Policy};
use ostler_mcp::{App, AppConfig, Profile};
use ostler_memory::{
    tenant_prefix, HashEmbedder, InMemoryStore, Memory, OllamaEmbedder, QdrantConfig, QdrantStore,
    SealedStore, Store, VectorSource,
};

#[derive(Parser)]
#[command(
    name = "ostler",
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
    /// Check that Chrome launches, and report memory: Qdrant reachable, sealing, key, embeddings, isolation, transport.
    Doctor(MemoryArgs),
    /// Serve the Agent2Agent (A2A) protocol so other agents can hand this one form tasks over HTTP.
    A2a(A2aArgs),
    /// Guided setup: save Qdrant URL/key and the privacy choices (sealing, local embeddings, retention), then verify.
    Init(InitArgs),
    /// Show or edit the config file.
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },
    /// Print (or with --apply, run) the exact setup for an MCP client, using this binary's absolute path.
    ///
    /// Example: `ostler setup claude --apply -- --lean --tools forms` (flags after `--` go to `ostler serve`).
    /// Your saved settings (`ostler init`) are read at start-up, so they need not be repeated here.
    Setup {
        #[arg(value_enum)]
        client: Client,
        /// Run the client's own CLI to register Ostler (only supported for `claude`).
        #[arg(long)]
        apply: bool,
        /// Extra `ostler serve` flags, after `--` (e.g. `-- --lean --tools forms`).
        #[arg(last = true)]
        serve_args: Vec<String>,
    },
    /// Open a URL and print the typed form model (for debugging).
    Inspect {
        url: String,
        #[arg(long)]
        headed: bool,
        #[arg(long, env = "OSTLER_CHROME")]
        chrome_path: Option<PathBuf>,
    },
    /// Inspect or edit saved memory (profile facts, learned skills). Needs Qdrant: run `ostler init` first.
    #[command(
        after_help = "Typical use:\n  ostler memory remember email you@example.com   save a fact the agent can fill into forms\n  ostler memory list                              show saved facts\n  ostler memory skills                            show what Ostler learned per site\n  ostler memory --memory ephemeral list           try it without Qdrant (lost on exit)\n\nThe shared options work before or after the subcommand (`ostler memory --tenant alice list` and\n`ostler memory list --tenant alice`). They also work as environment variables and as keys in `ostler config`."
    )]
    Memory {
        #[command(subcommand)]
        cmd: MemoryCmd,
        #[command(flatten)]
        mem: MemoryArgs,
    },
    /// Serve the built-in form fixtures on a local port: the safe first demo (nothing real is submitted).
    Fixtures {
        /// Port to listen on (127.0.0.1 only).
        #[arg(long, default_value_t = 8787)]
        port: u16,
    },
}

#[derive(Args)]
struct A2aArgs {
    #[command(flatten)]
    serve: ServeArgs,
    /// Address to listen on. Anything other than loopback requires --a2a-token.
    #[arg(long, env = "OSTLER_A2A_BIND", default_value = "127.0.0.1:8765")]
    bind: std::net::SocketAddr,
    /// Bearer token clients must send. Prefer the environment variable or `ostler config set a2a_token`.
    #[arg(long, env = "OSTLER_A2A_TOKEN", hide_env_values = true)]
    a2a_token: Option<String>,
    /// Externally visible base URL for the Agent Card (reverse proxy, container).
    #[arg(long)]
    public_url: Option<String>,
    /// Additional accepted Host header values. Repeatable.
    #[arg(long = "allowed-host")]
    allowed_hosts: Vec<String>,
    /// Concurrent remote conversations; each one is a Chrome process.
    #[arg(long, default_value_t = 4)]
    max_contexts: usize,
    /// Close a conversation's browser after this many idle seconds.
    #[arg(long, default_value_t = 300)]
    idle_secs: u64,
    #[arg(long, default_value_t = 120)]
    task_timeout_secs: u64,
    /// Treat `submit: true` as already confirmed (skips the input-required confirmation step).
    #[arg(long)]
    a2a_auto_submit: bool,
    /// Remote agents may reach loopback, private and cloud-metadata addresses when no --allow-domain is set.
    /// Off by default in A2A mode: a remote agent should not be able to probe the machine's own network.
    #[arg(long)]
    allow_private_networks: bool,
}

async fn run_a2a(a: A2aArgs) -> Result<()> {
    let s = a.serve;
    let memory = build_memory(&s.mem)?;
    key_preflight(&s.mem, Preflight::Strict).await?;
    let profile: Profile = s.tools.parse().map_err(|e: String| anyhow::anyhow!(e))?;
    if s.profile_dir.is_some() {
        eprintln!("ostler: --profile-dir is ignored in A2A mode (each conversation needs its own browser profile).");
    }
    let block_private = !a.allow_private_networks;
    if s.allow_domains.is_empty() {
        if block_private {
            eprintln!("ostler: warning: no --allow-domain set, so remote agents can send this browser to any public website (private and internal addresses are blocked). Restrict it.");
        } else {
            eprintln!("ostler: warning: no --allow-domain set and --allow-private-networks given, so remote agents can reach any website and this machine's own network. Restrict it.");
        }
    }
    if let Some(m) = &memory {
        if let Err(e) = m.init().await {
            eprintln!("ostler: memory unavailable ({}); exiting rather than hiding the problem. Diagnose: `ostler doctor`.", explain(&e.to_string()));
            std::process::exit(2);
        }
    }
    let (chrome_path, headless, lean) = (s.chrome_path.clone(), !s.headed, s.lean);
    let (policy, learn, confirm, verbose) = (
        Policy {
            allowed_domains: s.allow_domains.clone(),
            allow_file: s.allow_file,
            upload_dirs: s.upload_dirs.clone(),
            block_private_networks: block_private,
            accept_dialogs: s.accept_dialogs,
        },
        s.learn,
        s.require_confirm_submit,
        s.verbose,
    );
    let factory: ostler_a2a::AppFactory = Arc::new(move || {
        App::new(AppConfig {
            launch: LaunchOptions {
                chrome_path: chrome_path.clone(),
                headless,
                lean,
                ..Default::default()
            },
            policy: policy.clone(),
            profile,
            memory: memory.clone(),
            learn,
            require_confirm_submit: confirm,
            verbose,
        })
    });
    let mut cfg = ostler_a2a::A2aConfig::new(a.bind);
    cfg.token = a.a2a_token;
    cfg.public_url = a.public_url;
    cfg.allowed_hosts = a.allowed_hosts;
    cfg.max_contexts = a.max_contexts.max(1);
    cfg.idle = Duration::from_secs(a.idle_secs);
    cfg.task_timeout = Duration::from_secs(a.task_timeout_secs);
    cfg.auto_submit = a.a2a_auto_submit;
    ostler_a2a::serve(cfg, factory).await?;
    Ok(())
}

#[derive(Args)]
struct InitArgs {
    /// Qdrant URL, e.g. https://<cluster>.cloud.qdrant.io:6333 (the :6333 is added for Qdrant Cloud if missing).
    #[arg(long)]
    qdrant_url: Option<String>,
    /// Qdrant API key. Prefer the interactive prompt or the QDRANT_API_KEY variable: flags show up in `ps`.
    #[arg(long, env = "QDRANT_API_KEY", hide_env_values = true)]
    qdrant_api_key: Option<String>,
    /// Name that separates your memory from others' on a shared cluster (default: "default").
    #[arg(long)]
    tenant: Option<String>,
    /// Save `learn = true` (opt in to remembering values typed after accepted forms).
    #[arg(long)]
    learn: bool,
    /// Save `lean = true` (less RAM; no images in screenshots).
    #[arg(long)]
    lean: bool,
    /// Embeddings: `cloud` (Qdrant Cloud Inference), `ollama` (a local Ollama: text stays on your machine) or `dev-hash` (non-semantic, for open-source Qdrant while developing).
    #[arg(long, value_enum)]
    embeddings: Option<Embeddings>,
    /// Encrypt memory on this machine before it reaches Qdrant: `auto` (default) or `on` seal, `off` stores it readable.
    #[arg(long, value_enum)]
    seal: Option<SealMode>,
    /// Forget learned skills and error episodes not verified for this many days (0 = keep forever). Default 180.
    #[arg(long)]
    retain_days: Option<u64>,
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
    /// Set one value, e.g. `ostler config set tenant alice`.
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
    let any_flag = a.qdrant_url.is_some()
        || a.tenant.is_some()
        || a.learn
        || a.lean
        || a.embeddings.is_some()
        || a.seal.is_some()
        || a.retain_days.is_some();
    let interactive = !a.yes && !any_flag && std::io::stdin().is_terminal();

    let mut url = a.qdrant_url.clone();
    let mut key = a.qdrant_api_key.clone();
    if interactive {
        eprintln!("Ostler setup. Press Enter to keep the value shown in [brackets].\n");
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
            let is_cloud = cfg
                .get("qdrant_url")
                .or(url.as_ref())
                .is_some_and(|u| u.contains("cloud.qdrant.io"));
            init_privacy_prompts(&mut cfg, is_cloud)?;
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
            Embeddings::Ollama => "ollama",
        };
        cfg.insert("embeddings".into(), v.into());
    }
    if cfg.get("embeddings").map(String::as_str) == Some("ollama")
        && !cfg.contains_key("embedding_model")
    {
        // The default model name is a Qdrant Cloud one; Ollama would answer "model not found".
        cfg.insert("embedding_model".into(), "all-minilm".into());
        cfg.insert("embedding_dim".into(), "384".into());
    }
    if let Some(s) = a.seal {
        cfg.insert(
            "seal".into(),
            match s {
                SealMode::Auto => "auto",
                SealMode::On => "on",
                SealMode::Off => "off",
            }
            .into(),
        );
    }
    if let Some(d) = a.retain_days {
        cfg.insert("retain_days".into(), d.to_string());
    }
    if cfg.is_empty() {
        eprintln!(
            "Nothing to save. Run `ostler init` in a terminal, or pass flags such as --qdrant-url."
        );
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
        if let Cmd::Doctor(m) = Cli::parse_from(["ostler", "doctor"]).cmd {
            println!();
            memory_report(&m);
            let checked = match build_memory(&m) {
                Ok(Some(mem)) => mem
                    .init()
                    .await
                    .map(|()| mem.tenant.clone())
                    .map_err(Into::into),
                Ok(None) => Ok(String::new()),
                Err(e) => Err(e),
            };
            match checked {
                Ok(t) if !t.is_empty() => {
                    println!("Qdrant: reachable, collections ready (tenant {t:?}). OK")
                }
                Ok(_) => println!("Memory is off."),
                Err(e) => {
                    println!(
                        "Qdrant: FAILED: {}\nThe settings were saved anyway. Fix them (`ostler config set <key> <value>`, or re-run `ostler init`), then run `ostler doctor`.",
                        explain(&e.to_string())
                    );
                    std::process::exit(1);
                }
            }
        }
    }
    println!("\nNext: connect your agent with `ostler setup claude --apply` (or codex / cursor / vscode / json),");
    println!("then try the safe demo: `ostler fixtures`.");
    Ok(())
}

/// The privacy questions of `ostler init`. Defaults are the safe choices; each answer is explained first.
fn init_privacy_prompts(
    cfg: &mut std::collections::BTreeMap<String, String>,
    is_cloud: bool,
) -> Result<()> {
    eprintln!("\nPrivacy choices for saved memory (Enter keeps the safe default):");
    eprintln!("  Sealing encrypts facts, playbooks and episodes on this machine before they reach Qdrant,");
    eprintln!("  and hides site names and field labels behind keyed hashes. Anyone who can read your Qdrant");
    eprintln!("  (a cloud operator, a leaked key) then sees only ciphertext.");
    let sealed = cfg.get("seal").map(String::as_str) != Some("off");
    let seal = yes_no("Seal memory (recommended)?", sealed)?;
    cfg.insert("seal".into(), if seal { "on" } else { "off" }.into());
    if seal {
        let path = config::memory_key_path();
        eprintln!("  The sealing key lives in {}", path.display());
        eprintln!("  (it is created for you, owner-only). Back it up: if the key is lost, sealed memory can NEVER be");
        eprintln!(
            "  read again; the only recovery is `ostler memory wipe --yes` and starting over."
        );
    } else {
        eprintln!("  Sealing is off: saved facts and the sites you use will be readable by whoever can read your Qdrant.");
    }
    eprintln!("\n  Embeddings turn text into vectors for search. `cloud` sends field labels and fact text to");
    eprintln!("  Qdrant Cloud Inference (works only on Qdrant Cloud). `ollama` keeps text on this machine and");
    eprintln!(
        "  only vectors leave (needs Ollama: https://ollama.com, then `ollama pull all-minilm`)."
    );
    eprintln!("  `dev-hash` is local but not semantic: for open-source Qdrant while developing.");
    let cur = cfg
        .get("embeddings")
        .cloned()
        .unwrap_or_else(|| if is_cloud { "cloud" } else { "ollama" }.into());
    let e = loop {
        let e = prompt("Embeddings (cloud / ollama / dev-hash)", &cur)?;
        if matches!(e.as_str(), "cloud" | "ollama" | "dev-hash") {
            break e;
        }
        eprintln!("  please type one of: cloud, ollama, dev-hash");
    };
    if e == "ollama" {
        eprintln!("  Remember to run `ollama pull all-minilm` (and keep Ollama running).");
    }
    cfg.insert("embeddings".into(), e);
    let cur = cfg
        .get("retain_days")
        .cloned()
        .unwrap_or_else(|| "180".into());
    eprintln!("\n  Retention: learned per-site skills and error episodes not verified for this many days are");
    eprintln!("  forgotten at start-up (0 = keep forever). Facts you saved yourself never expire.");
    let d = loop {
        let d = prompt("Retention in days", &cur)?;
        if d.parse::<u64>().is_ok() {
            break d;
        }
        eprintln!("  please type a whole number (0 = keep forever)");
    };
    cfg.insert("retain_days".into(), d);
    eprintln!();
    Ok(())
}

/// Plain-language memory status: what is stored where, who can read it, and what leaves this machine.
/// Never creates the sealing key.
fn memory_report(m: &MemoryArgs) {
    let mode = resolved_mode(m);
    let Some(url) = m
        .qdrant_url
        .as_deref()
        .filter(|_| mode == MemoryMode::Qdrant)
    else {
        match mode {
            MemoryMode::Ephemeral => println!("Memory: in-process only (--memory ephemeral); lost when the server exits."),
            _ => println!("Memory: off. Optional: `ostler init` saves a Qdrant URL so the agent can reuse your details and past fills.\n  Everything else works without it."),
        }
        return;
    };
    println!("Memory: on, Qdrant at {url} (tenant {:?})", m.tenant);
    let sealed = m.seal != SealMode::Off;
    if sealed {
        println!("  sealing:    ON. Saved facts, playbooks and episodes are encrypted here before they reach Qdrant; site names and field labels are hashed.");
        match config::memory_key_status() {
            config::KeyStatus::Env => println!("  key:        from the OSTLER_MEMORY_KEY environment variable. Keep a backup; if it is lost, sealed memory is unrecoverable."),
            config::KeyStatus::File { path, private } => {
                println!("  key:        {} ({})", path.display(), if private { "owner-only, good" } else { "NOT owner-only" });
                if !private {
                    println!("  warning:    other users on this machine can read the key. Run: chmod 600 {}", path.display());
                }
                println!("              Back it up. If it is lost, sealed memory is unrecoverable (`ostler memory wipe --yes` starts over).");
            }
            config::KeyStatus::Keychain { present } => println!("  key:        in {} ({}). Back it up: `ostler memory key export --yes-print`. If it is lost, sealed memory is unrecoverable.", config::key_location(), match present { Some(true) => "found", Some(false) => "not created yet; it will be on first use", None => "could not be read right now" }),
            config::KeyStatus::Missing { path } => println!("  key:        not created yet; it will be created at {} on first use. Back it up then: if it is lost, sealed memory is unrecoverable.", path.display()),
        }
    } else {
        println!("  sealing:    OFF. warning: anyone who can read your Qdrant can read your saved facts and the sites you use.\n              Turn on with `ostler config set seal on`, then `ostler memory seal` to protect what is already saved.");
    }
    match m.embeddings {
        Embeddings::Cloud => {
            println!("  embeddings: cloud. Field labels and fact text are sent to Qdrant Cloud Inference to be embedded (even when sealed).");
            println!("              To keep text on this machine: `ostler init --embeddings ollama`.");
            if !url.contains("cloud.qdrant.io") {
                println!("  warning:    Cloud Inference only exists on Qdrant Cloud, and this URL is not one. Use `--embeddings ollama` (semantic, local) or `dev-hash` (local, not semantic).");
            }
        }
        Embeddings::Ollama => {
            println!("  embeddings: local (ollama at {}). Only vectors leave this machine.", m.embedding_url);
            if m.embedding_model.contains('/') {
                println!("  warning:    model {:?} looks like a Qdrant Cloud model name, not an Ollama one. Run: ostler config set embedding_model all-minilm  (and `ollama pull all-minilm`)", m.embedding_model);
            }
        }
        Embeddings::DevHash => println!("  embeddings: local dev-hash. Nothing leaves this machine for embedding, but matching is not semantic (development only)."),
    }
    match m.isolation {
        Isolation::Shared => println!("  isolation:  shared collections; tenants are separated by a filter. Anyone holding the Qdrant key can read every tenant.\n              For per-tenant keys: `--isolation collection` plus `ostler memory token`."),
        Isolation::Collection => println!("  isolation:  one set of collections per tenant. Give clients a scoped key from `ostler memory token` instead of the admin key."),
    }
    if url.trim().starts_with("https://") {
        println!("  transport:  https (encrypted)");
    } else if config::is_remote_http(url) {
        if m.allow_insecure_qdrant {
            println!("  transport:  http to a remote host, ALLOWED by --allow-insecure-qdrant. warning: the API key and any unsealed data cross the network in clear text. Use https:// or a VPN/SSH tunnel.");
        } else {
            println!("  transport:  http to a remote host: REFUSED. Use an https:// URL (`ostler config set qdrant_url https://...`), or if a VPN/SSH tunnel protects the link, `ostler config set allow_insecure_qdrant true`.");
        }
    } else {
        println!("  transport:  http to this machine (fine: it never leaves it)");
    }
}

/// Append the next step to errors that have a known fix. Unknown errors pass through unchanged.
fn explain(msg: &str) -> String {
    let fix = if msg.contains("refusing plain http") {
        "use an https:// URL (`ostler config set qdrant_url https://...`), or, if a VPN or SSH tunnel protects the link, pass --allow-insecure-qdrant (`ostler config set allow_insecure_qdrant true`)"
    } else if msg.contains("memory.key") || msg.contains("OSTLER_MEMORY_KEY") {
        "the sealing key is unreadable or malformed. Restore your backup to the path named above (or fix OSTLER_MEMORY_KEY). With no backup, sealed memory cannot be recovered: delete the key file, then run `ostler memory wipe --yes` to start over"
    } else if msg.contains("cannot reach the local embedder") {
        "start Ollama (`ollama serve`), then `ollama pull all-minilm`; or switch with `ostler config set embeddings cloud` (Qdrant Cloud) / `dev-hash`"
    } else if msg.contains("embedder returned") && msg.contains("not found") {
        "the embedding model is not installed: `ollama pull <model>` (default for ollama is all-minilm), or set it with `ostler config set embedding_model <name>`"
    } else if msg.contains("error sending request")
        || msg.contains("connect")
        || msg.contains("timed out")
        || msg.contains("unavailable")
    {
        "Qdrant is not reachable. Check the URL with `ostler config show`, or start a local one: docker run -p 6333:6333 qdrant/qdrant"
    } else if msg.contains("401") || msg.contains("403") {
        "Qdrant rejected the API key. Re-run `ostler init`, or `ostler config set qdrant_api_key <key>`. With --isolation collection, use the scoped token from `ostler memory token` for the same --tenant"
    } else if msg.contains("-dim vectors") {
        "this collection was created with a different embedding model; use the same `--embedding-model`, or a different tenant/cluster"
    } else {
        return msg.to_string();
    };
    format!("{msg}\n  fix: {fix}")
}

fn config_cmd(cmd: ConfigCmd) -> Result<()> {
    match cmd {
        ConfigCmd::Path => println!("{}", config::path().display()),
        ConfigCmd::Show => {
            let cfg = config::read()?;
            println!("# {}", config::path().display());
            if cfg.is_empty() {
                println!("(empty; run `ostler init`)");
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
            let value = config::validate(&key, &value)?;
            let value = if key == "qdrant_url" {
                config::normalize_qdrant_url(&value)
            } else {
                value
            };
            if key == "key_store" {
                let new = if value == "keychain" {
                    config::KeyStore::Keychain
                } else {
                    config::KeyStore::File
                };
                for note in config::switch_key_store(new)? {
                    eprintln!("ostler: {note}");
                }
            }
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
            let cmd = format!("claude mcp add ostler -- {exe} {}", args.join(" "));
            if apply {
                let st = std::process::Command::new("claude")
                    .args(["mcp", "add", "ostler", "--", &exe])
                    .args(&args)
                    .status()
                    .map_err(|e| anyhow::anyhow!("could not run `claude` ({e}); run this yourself:\n  {cmd}"))?;
                if !st.success() {
                    anyhow::bail!("`claude mcp add` failed. If it says ostler already exists, run `claude mcp remove ostler` and retry; otherwise run it yourself to see why:\n  {cmd}");
                }
                println!("Registered. Restart Claude Code (or run /mcp), then ask it to use ostler.");
            } else {
                println!("{cmd}\n\n(add --apply to run it for you)");
            }
        }
        Client::Codex => println!(
            "Add to ~/.codex/config.toml:\n\n[mcp_servers.ostler]\ncommand = {exe:?}\nargs = {json_args}"
        ),
        Client::Cursor => println!(
            "Add to ~/.cursor/mcp.json (or .cursor/mcp.json in a project):\n\n{}",
            mcp_json(&exe, &args)
        ),
        Client::Vscode => println!(
            "Add to .vscode/mcp.json:\n\n{}",
            serde_json::to_string_pretty(&serde_json::json!({"servers": {"ostler": {"type": "stdio", "command": exe, "args": args}}}))?
        ),
        Client::Json => println!("{}", mcp_json(&exe, &args)),
    }
    if !(apply && matches!(client, Client::Claude)) {
        println!("\nThen restart the client so it starts the server.");
    }
    println!("First demo: run `ostler fixtures` in another terminal, then ask your agent to fill the form at http://127.0.0.1:8787/dependent.html (nothing real is submitted).");
    if apply && !matches!(client, Client::Claude) {
        eprintln!("note: --apply only supports `claude`; other clients' config files are left untouched. Paste the snippet above.");
    }
    Ok(())
}

fn mcp_json(exe: &str, args: &[String]) -> String {
    serde_json::to_string_pretty(
        &serde_json::json!({"mcpServers": {"ostler": {"command": exe, "args": args}}}),
    )
    .unwrap()
}

#[derive(Subcommand)]
enum MemoryCmd {
    /// Show saved profile facts (what the agent can fill into forms).
    List,
    /// Save a fact, e.g. `ostler memory remember email you@example.com`.
    Remember {
        /// Short name such as `email`, `full_name`, `phone`.
        key: String,
        value: String,
    },
    /// Delete one saved fact by key (see `memory list`).
    Forget { key: String },
    /// What Ostler has learned: per-form skills, with wins, losses and trusted field mappings.
    Skills {
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Forget stale, unproven skills and old episodes (not verified for this many days).
    Prune {
        #[arg(long, default_value_t = 90)]
        days: u64,
    },
    /// Print a Qdrant access token limited to one tenant's collections, to give a client instead of the admin key.
    ///
    /// Needs the admin key in QDRANT_API_KEY, `--isolation collection`, and Qdrant running with
    /// `service.jwt_rbac: true`. The token is printed on stdout; the explanation goes to stderr.
    Token {
        /// Tenant the token is for. Defaults to --tenant.
        #[arg(long)]
        for_tenant: Option<String>,
        /// Days until the token expires.
        #[arg(long, default_value_t = 30)]
        days: u64,
        /// Search and scroll only; no writes.
        #[arg(long)]
        read_only: bool,
    },
    /// Encrypt facts saved before sealing was on (one-time migration; run it with the same --tenant).
    ///
    /// Saved facts are kept and re-saved sealed. Learned skills and episodes from before are dropped
    /// (they are relearned). Back up memory.key afterwards: without it sealed memory cannot be read.
    Seal,
    /// Back up or move the sealing key (export / import).
    Key {
        #[command(subcommand)]
        cmd: KeyCmd,
    },
    /// Delete everything saved for this tenant (facts, skills, episodes). Irreversible.
    Wipe {
        /// Required: confirms the deletion.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum KeyCmd {
    /// Print the sealing key (base64) on stdout so you can back it up. Anyone with it can read your memory.
    ///
    /// Example: `ostler memory key export --yes-print > memory.key.backup && chmod 600 memory.key.backup`
    Export {
        /// Required: confirms you want the secret printed (it may land in terminal scrollback or a log).
        #[arg(long)]
        yes_print: bool,
    },
    /// Read a key (base64) from stdin and save it where the sealing key lives (file or keychain).
    ///
    /// Example: `ostler memory key import < memory.key.backup`
    Import {
        /// Replace a different key that is already there (the old one is saved next to the config first).
        #[arg(long)]
        force: bool,
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
    /// A local Ollama server: text is embedded on your machine and only vectors reach Qdrant.
    Ollama,
}

#[derive(Clone, Copy, ValueEnum, PartialEq, Eq)]
enum Isolation {
    /// All tenants share three collections, separated by a filter in every query (default).
    Shared,
    /// Each tenant gets its own collections, so a scoped token (`ostler memory token`) is enforced by Qdrant.
    Collection,
}

#[derive(Clone, Copy, ValueEnum, PartialEq, Eq)]
enum SealMode {
    /// Seal when memory is stored in Qdrant (default).
    Auto,
    /// Always seal.
    On,
    /// Store payloads readable by whoever runs the Qdrant server.
    Off,
}

#[derive(Args, Clone)]
struct MemoryArgs {
    /// Where memory lives. `auto` uses Qdrant when a URL is saved (`ostler init`), otherwise memory is off.
    #[arg(
        long,
        global = true,
        value_enum,
        default_value = "auto",
        env = "OSTLER_MEMORY"
    )]
    memory: MemoryMode,
    /// Qdrant REST URL, e.g. https://<cluster>.cloud.qdrant.io:6333 (or http://localhost:6333).
    #[arg(long, global = true, env = "QDRANT_URL")]
    qdrant_url: Option<String>,
    /// Qdrant API key. Prefer `ostler init` or the environment variable: flags show up in `ps`.
    #[arg(long, global = true, env = "QDRANT_API_KEY", hide_env_values = true)]
    qdrant_api_key: Option<String>,
    /// Who turns text into vectors. `cloud` sends field text to Qdrant Cloud Inference; `ollama` keeps it local.
    #[arg(
        long,
        global = true,
        value_enum,
        default_value = "cloud",
        env = "OSTLER_EMBEDDINGS"
    )]
    embeddings: Embeddings,
    /// Embedding model name (Cloud Inference name, or the Ollama name such as `all-minilm`); its dimension must match --embedding-dim.
    #[arg(
        long,
        global = true,
        default_value = "sentence-transformers/all-minilm-l6-v2",
        env = "OSTLER_EMBEDDING_MODEL"
    )]
    embedding_model: String,
    /// Vector size of the embedding model (384 for all-minilm).
    #[arg(
        long,
        global = true,
        default_value_t = 384,
        env = "OSTLER_EMBEDDING_DIM"
    )]
    embedding_dim: usize,
    /// Keep Qdrant vectors on disk (memory-mapped): much less RAM per user, slightly slower search.
    #[arg(long, global = true, env = "OSTLER_QDRANT_ON_DISK")]
    qdrant_on_disk: bool,
    /// Where the embedding server lives (--embeddings ollama).
    #[arg(
        long,
        global = true,
        default_value = "http://127.0.0.1:11434",
        env = "OSTLER_EMBEDDING_URL"
    )]
    embedding_url: String,
    /// Encrypt saved facts, playbooks and episodes on this machine before they reach Qdrant, and hide
    /// site names and field labels behind keyed hashes. The key stays here (see `memory.key`).
    #[arg(
        long,
        global = true,
        value_enum,
        default_value = "auto",
        env = "OSTLER_SEAL"
    )]
    seal: SealMode,
    /// How tenants are kept apart in Qdrant. `collection` pairs with scoped tokens: see `ostler memory token`.
    #[arg(
        long,
        global = true,
        value_enum,
        default_value = "shared",
        env = "OSTLER_ISOLATION"
    )]
    isolation: Isolation,
    /// Allow plain http:// to a non-loopback Qdrant. Refused by default: the API key and facts would be sent unencrypted.
    #[arg(long, global = true, env = "OSTLER_ALLOW_INSECURE_QDRANT")]
    allow_insecure_qdrant: bool,
    /// Isolates one user's memory from another's inside shared collections.
    #[arg(long, global = true, env = "OSTLER_TENANT", default_value = "default")]
    tenant: String,
}

#[derive(Args)]
struct ServeArgs {
    #[command(flatten)]
    mem: MemoryArgs,
    /// Tools to expose: core | forms | all.
    #[arg(long, default_value = "all", env = "OSTLER_TOOLS")]
    tools: String,
    #[arg(long)]
    headed: bool,
    #[arg(long, env = "OSTLER_CHROME")]
    chrome_path: Option<PathBuf>,
    /// Smaller footprint: lean Chrome flags and no image/font/media downloads (screenshots will lack images).
    #[arg(long, env = "OSTLER_LEAN")]
    lean: bool,
    /// Persistent Chrome profile (keeps logins). Default is a throwaway profile.
    #[arg(long)]
    profile_dir: Option<PathBuf>,
    /// Only allow these domains (and subdomains), optionally pinned to a port (example.com:8443). Repeatable.
    #[arg(long = "allow-domain")]
    allow_domains: Vec<String>,
    #[arg(long)]
    allow_file: bool,
    /// Directory file fields may be filled from. Repeatable. Without one, uploads are refused.
    #[arg(long = "upload-dir")]
    upload_dirs: Vec<PathBuf>,
    /// With no --allow-domain, refuse loopback, private, link-local (cloud metadata) and internal hosts.
    #[arg(long)]
    block_private_networks: bool,
    /// Answer the page's confirm() and prompt() dialogs with OK. By default they are dismissed.
    #[arg(long)]
    accept_dialogs: bool,
    /// Require confirm=true on form_submit so a human can approve irreversible actions.
    #[arg(long)]
    require_confirm_submit: bool,
    /// Include timings, fingerprints and ids in tool output. Off by default: they cost the agent context for no benefit.
    #[arg(long, env = "OSTLER_VERBOSE")]
    verbose: bool,
    /// After an accepted submit, save the non-sensitive values the agent typed as profile facts.
    #[arg(long, env = "OSTLER_LEARN")]
    learn: bool,
    /// On start, forget learned skills and error episodes not verified for this many days (0 = keep forever).
    /// Saved facts are never expired.
    #[arg(long, default_value_t = 180, env = "OSTLER_RETAIN_DAYS")]
    retain_days: u64,
}

fn resolved_mode(a: &MemoryArgs) -> MemoryMode {
    match a.memory {
        MemoryMode::Auto if a.qdrant_url.is_some() => MemoryMode::Qdrant,
        MemoryMode::Auto => MemoryMode::Off,
        m => m,
    }
}

/// `build_memory_inner`, with the next step appended to errors that have a known fix.
fn build_memory(a: &MemoryArgs) -> Result<Option<Memory>> {
    build_memory_inner(a).map_err(|e| anyhow::anyhow!(explain(&format!("{e:#}"))))
}

/// The Qdrant store exactly as configured, before any sealing wrapper.
fn qdrant_store(a: &MemoryArgs) -> Result<QdrantStore> {
    let url = a
        .qdrant_url
        .clone()
        .context("--memory qdrant needs a Qdrant URL: run `ostler init`, or set QDRANT_URL / pass --qdrant-url")?;
    let vectors = match a.embeddings {
        Embeddings::Cloud => VectorSource::CloudInference {
            model: a.embedding_model.clone(),
            dim: a.embedding_dim,
        },
        Embeddings::DevHash => VectorSource::Client(Arc::new(HashEmbedder::new(a.embedding_dim))),
        Embeddings::Ollama => VectorSource::Client(Arc::new(OllamaEmbedder::new(
            &a.embedding_url,
            &a.embedding_model,
            a.embedding_dim,
            a.allow_insecure_qdrant,
        )?)),
    };
    let store = QdrantStore::new(QdrantConfig {
        url,
        api_key: a.qdrant_api_key.clone(),
        vectors,
        timeout: Duration::from_secs(4),
        on_disk: a.qdrant_on_disk,
        allow_insecure: a.allow_insecure_qdrant,
        collection_prefix: if a.isolation == Isolation::Collection {
            tenant_prefix(&a.tenant)
        } else {
            String::new()
        },
    })?;
    Ok(store)
}

fn build_memory_inner(a: &MemoryArgs) -> Result<Option<Memory>> {
    let mode = resolved_mode(a);
    Ok(match mode {
        MemoryMode::Off | MemoryMode::Auto => None,
        MemoryMode::Ephemeral => {
            let store: Arc<dyn Store> =
                Arc::new(InMemoryStore::new(Arc::new(HashEmbedder::default())));
            Some(Memory::new(store, a.tenant.clone()))
        }
        MemoryMode::Qdrant => {
            let store = qdrant_store(a)?;
            let store: Arc<dyn Store> = if a.seal == SealMode::Off {
                eprintln!("ostler: warning: --seal off. Anyone who can read your Qdrant can read your saved facts and the sites you use.");
                Arc::new(store)
            } else {
                Arc::new(SealedStore::new(Arc::new(store), config::memory_key()?))
            };
            if a.seal != SealMode::Off && matches!(a.embeddings, Embeddings::Cloud) {
                eprintln!("ostler: note: payloads are sealed, but --embeddings cloud still sends field text to Qdrant Cloud Inference to embed. Use --embeddings ollama to keep it local.");
            }
            Some(Memory::new(store, a.tenant.clone()))
        }
    })
}

/// What `key_preflight` is for: `Strict` also warns about legacy plaintext; `Quiet` only guards the key.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Preflight {
    Strict,
    Quiet,
}

fn key_mismatch_message(unopenable: usize, tenant: &str) -> String {
    let created = if config::key_just_created() {
        format!(
            "\n  No key was found, so a NEW one was just created in {}. That usually means the key was lost or moved,\n  or this is a different machine/config than the one that saved the memory.",
            config::key_location()
        )
    } else {
        String::new()
    };
    let env = std::env::var_os("OSTLER_MEMORY_KEY").is_some();
    let restore = if env {
        "put the original key in OSTLER_MEMORY_KEY (it overrides stored keys)".to_string()
    } else {
        "ostler memory key import --force < your-backup.key   (--force keeps the current key as memory.key.old-<time>)"
            .to_string()
    };
    format!(
        "refusing to start: Qdrant already holds {unopenable} sealed point(s) for tenant {tenant:?} that the current memory key cannot open.{created}\n  Key in use: {}\n  Continuing would hide that memory and later overwrite it.\n  To recover (you have the original key):  {restore}\n  To start over (the key is gone for good): ostler memory wipe --yes --tenant {tenant}   (deletes this tenant's memory in Qdrant; irreversible)",
        config::key_location()
    )
}

/// Before memory is used: refuse when the key cannot open sealed data that is already there (silently
/// shadowing it would let the next write or wipe destroy it), and, in `Strict`, warn about legacy plaintext.
/// An unreachable Qdrant is not this function's concern: the normal start-up path reports it.
async fn key_preflight(a: &MemoryArgs, mode: Preflight) -> Result<()> {
    if resolved_mode(a) != MemoryMode::Qdrant || a.seal == SealMode::Off {
        return Ok(());
    }
    let raw: Arc<dyn Store> = Arc::new(qdrant_store(a)?);
    if raw.ensure().await.is_err() {
        return Ok(());
    }
    let key = config::memory_key()?;
    if let Ok(ostler_memory::KeyProbe::Mismatch { unopenable }) =
        ostler_memory::probe_key(raw.clone(), key, &a.tenant).await
    {
        bail!("{}", key_mismatch_message(unopenable, &a.tenant));
    }
    if mode == Preflight::Strict {
        let sealed = SealedStore::new(raw, key);
        let mut n = 0;
        for kind in ostler_memory::Kind::ALL {
            n += sealed.count_unsealed(kind, &a.tenant).await.unwrap_or(0);
        }
        if n > 0 {
            eprintln!("ostler: warning: {n} saved memory point(s) for tenant {:?} are still unencrypted (readable by whoever runs your Qdrant, hidden from the agent). Fix: ostler memory seal", a.tenant);
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    config::load_into_env();
    match Cli::parse().cmd {
        Cmd::Serve(a) => {
            let memory = build_memory(&a.mem)?;
            key_preflight(&a.mem, Preflight::Strict).await?;
            if let (Some(m), true) = (memory.clone(), a.retain_days > 0) {
                let secs = a.retain_days.saturating_mul(86_400);
                tokio::spawn(async move {
                    if m.init().await.is_ok() {
                        let _ = m.prune(secs).await;
                    }
                });
            }
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
                    upload_dirs: a.upload_dirs,
                    block_private_networks: a.block_private_networks,
                    accept_dialogs: a.accept_dialogs,
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
                    eprintln!("ostler: memory unavailable ({}); continuing without it would hide the problem, so exiting.\n  To run without memory: `ostler config unset qdrant_url` (or pass --memory off). Diagnose: `ostler doctor`.", explain(&e.to_string()));
                    std::process::exit(2);
                }
            }
            eprintln!(
                "ostler {} ready (MCP over stdio; memory {})",
                env!("CARGO_PKG_VERSION"),
                if app.has_memory() { "on" } else { "off" }
            );
            ostler_mcp::serve_stdio(app).await?;
        }
        Cmd::A2a(a) => run_a2a(a).await?,
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
            b.page().navigate(&url).await?;
            let ir = b.page().forms().await?;
            println!("{}", serde_json::to_string_pretty(&ir)?);
            b.close().await;
        }
        Cmd::Memory {
            cmd:
                MemoryCmd::Token {
                    for_tenant,
                    days,
                    read_only,
                },
            mem,
        } => {
            let admin = mem
                .qdrant_api_key
                .clone()
                .context("minting a token needs the Qdrant admin (full-access) key: set QDRANT_API_KEY=<admin key>, or save it with `ostler init`")?;
            anyhow::ensure!(
                days > 0,
                "--days must be at least 1 (a token that expires now is useless)"
            );
            if mem.isolation != Isolation::Collection {
                eprintln!("note: --isolation is not `collection`, so this token will not match the shared collections. Run with --isolation collection (or `ostler config set isolation collection`) here and in the client.");
            }
            let tenant = for_tenant.unwrap_or_else(|| mem.tenant.clone());
            if days == 0 || days > 3650 {
                bail!("--days must be between 1 and 3650");
            }
            let exp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs()
                + days * 86_400;
            if admin.matches('.').count() == 2 && admin.starts_with("eyJ") {
                bail!("QDRANT_API_KEY looks like a token, not the admin key. Tokens are signed with the admin key, so a token minted from one would be rejected. Use the cluster's admin API key");
            }
            let token = ostler_memory::mint_tenant_token(&admin, &tenant, exp, read_only)?;
            eprintln!(
                "Token for tenant {tenant:?} ({}), valid {days} days. It only opens collections starting with {:?}.\n\
                 Use it as QDRANT_API_KEY together with --isolation collection --tenant {tenant}. Qdrant must run with service.jwt_rbac: true.\n\
                 The admin collections must exist first: run any `ostler memory list` once with the admin key and the same --tenant/--isolation.",
                if read_only { "read-only" } else { "read-write" },
                tenant_prefix(&tenant)
            );
            println!("{token}");
        }
        Cmd::Memory {
            cmd: MemoryCmd::Seal,
            mem,
        } => {
            if resolved_mode(&mem) != MemoryMode::Qdrant {
                bail!("memory is not stored in Qdrant, so there is nothing to seal. Run `ostler init` to set a Qdrant URL first");
            }
            if mem.seal == SealMode::Off {
                bail!("sealing is off (--seal off, or `seal = off` in the config), so there is nothing to seal. Turn it on with `ostler config set seal on` and run `ostler memory seal` again");
            }
            let raw: Arc<dyn Store> = Arc::new(qdrant_store(&mem)?);
            raw.ensure()
                .await
                .map_err(|e| anyhow::anyhow!(explain(&e.to_string())))?;
            key_preflight(&mem, Preflight::Quiet).await?;
            // Saved facts are copied first and checked by reading them back; only then are the old
            // unsealed points deleted. Interrupting this at any point leaves your facts intact: run it again.
            let rep = ostler_memory::seal_legacy(raw, config::memory_key()?, &mem.tenant)
                .await
                .map_err(|e| anyhow::anyhow!(explain(&e.to_string())))?;
            if rep.legacy_points_removed == 0 {
                println!(
                    "Nothing to do: no unsealed memory for tenant {:?}.",
                    mem.tenant
                );
            } else {
                println!(
                    "Sealed {} saved fact(s). Removed {} unsealed point(s) from Qdrant (that includes old learned skills and error episodes, which are relearned).\nBack up your sealing key ({}): if it is lost, sealed memory cannot be read.",
                    rep.facts_migrated,
                    rep.legacy_points_removed,
                    config::memory_key_path().display()
                );
            }
        }
        Cmd::Memory {
            cmd: MemoryCmd::Key { cmd },
            mem,
        } => key_cmd(cmd, &mem).await?,
        Cmd::Memory { cmd, mem } => {
            let Some(m) = build_memory(&mem)? else {
                bail!("memory is off. Run `ostler init` to save a Qdrant URL, or try it without Qdrant: `ostler memory --memory ephemeral list` (lost on exit)");
            };
            m.init()
                .await
                .map_err(|e| anyhow::anyhow!(explain(&e.to_string())))?;
            // `wipe` is the way out of a key mismatch, so it must not be blocked by one.
            if !matches!(cmd, MemoryCmd::Wipe { .. }) {
                key_preflight(
                    &mem,
                    if matches!(cmd, MemoryCmd::List) {
                        Preflight::Strict
                    } else {
                        Preflight::Quiet
                    },
                )
                .await?;
            }
            match cmd {
                MemoryCmd::List => {
                    let facts = m.facts(500).await?;
                    if facts.is_empty() {
                        println!("No saved facts yet. Add one: `ostler memory remember email you@example.com`");
                    }
                    for f in facts {
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
                        "no such fact (see `ostler memory list` for the exact keys)"
                    }
                ),
                MemoryCmd::Skills { limit } => {
                    let skills = m.skills(limit).await?;
                    if skills.is_empty() {
                        println!("Nothing learned yet. Skills appear after a form you filled is accepted.");
                    }
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    for p in skills {
                        let age_days = now.saturating_sub(p.verified_ts) / 86_400;
                        let mapped: Vec<String> = p
                            .fields
                            .iter()
                            .filter(|f| f.trusted())
                            .map(|f| {
                                format!("{} <- {}", f.label, f.profile_key.as_deref().unwrap_or(""))
                            })
                            .collect();
                        println!(
                            "{} [{}]  {} won / {} lost ({:.0}%), verified {}{}",
                            p.host,
                            &p.fingerprint[..p.fingerprint.len().min(8)],
                            p.wins.max(p.uses),
                            p.losses,
                            p.win_rate() * 100.0,
                            if p.verified_ts == 0 {
                                "never".to_string()
                            } else {
                                format!("{age_days}d ago")
                            },
                            p.learned_from
                                .as_deref()
                                .map(|f| format!(", learned from [{}]", &f[..f.len().min(8)]))
                                .unwrap_or_default()
                        );
                        for m in mapped {
                            println!("    {m}");
                        }
                    }
                }
                MemoryCmd::Token { .. } | MemoryCmd::Seal | MemoryCmd::Key { .. } => {
                    unreachable!("handled above")
                }
                MemoryCmd::Prune { days } => {
                    let n = m.prune(days.saturating_mul(86_400)).await?;
                    println!("pruned {n} stale records");
                }
                MemoryCmd::Wipe { yes } => {
                    if !yes {
                        bail!(
                            "this deletes ALL memory for tenant {:?} (facts, skills, episodes); to confirm, re-run: ostler memory wipe --yes",
                            m.tenant
                        );
                    }
                    m.wipe().await?;
                    println!("wiped");
                }
            }
        }
        Cmd::Fixtures { port } => {
            let _srv = ostler_fixtures::FixtureServer::start_on(
                &format!("127.0.0.1:{port}"),
                Duration::from_millis(250),
            );
            println!("Fixtures at http://127.0.0.1:{port}/  (ctrl-c to stop). Synthetic forms; nothing real is submitted.");
            println!("In your agent (after `ostler setup <client> --apply`), ask:");
            println!("  Open http://127.0.0.1:{port}/dependent.html and fill in the shipping form for Ada Lovelace in Germany, Bavaria.");
            tokio::signal::ctrl_c().await?;
        }
    }
    Ok(())
}

async fn key_cmd(cmd: KeyCmd, mem: &MemoryArgs) -> Result<()> {
    use std::io::Read;
    match cmd {
        KeyCmd::Export { yes_print } => {
            anyhow::ensure!(
                yes_print,
                "this prints the key that decrypts your memory. Anyone who sees it can read everything saved. Re-run with --yes-print and send it to a file or password manager, not a shared screen/log: ostler memory key export --yes-print > memory.key.backup"
            );
            let key = config::existing_memory_key()?.context(format!(
                "there is no memory key yet (looked in {}). One is created on first use of sealed memory",
                config::key_location()
            ))?;
            println!("{}", ostler_memory::SealKey::encode(&key));
            eprintln!(
                "ostler: key printed from {}. Keep the copy private.",
                config::key_location()
            );
        }
        KeyCmd::Import { force } => {
            let mut text = String::new();
            std::io::stdin().take(4096).read_to_string(&mut text)?;
            let key = ostler_memory::SealKey::decode(&text)
                .map_err(|e| anyhow::anyhow!("stdin does not hold a memory key: {e}"))?;
            println!("{}", config::import_memory_key(key, force)?);
            // Tell the user straight away whether the key fits the data in Qdrant.
            if resolved_mode(mem) == MemoryMode::Qdrant && mem.seal != SealMode::Off {
                let raw: Arc<dyn Store> = Arc::new(qdrant_store(mem)?);
                if raw.ensure().await.is_ok() {
                    match ostler_memory::probe_key(raw, key, &mem.tenant).await {
                        Ok(ostler_memory::KeyProbe::Ok) => println!("This key opens the sealed memory saved for tenant {:?}.", mem.tenant),
                        Ok(ostler_memory::KeyProbe::Empty) => println!("No sealed memory exists yet for tenant {:?}.", mem.tenant),
                        Ok(ostler_memory::KeyProbe::Mismatch { unopenable }) => println!("warning: {unopenable} sealed point(s) for tenant {:?} still cannot be opened with this key (wrong backup, or another tenant's data).", mem.tenant),
                        Err(e) => println!("(could not check against Qdrant: {e})"),
                    }
                }
            }
        }
    }
    Ok(())
}

async fn doctor(m: MemoryArgs) -> Result<()> {
    let mut ok = true;
    match ostler_cdp::find_chrome(None) {
        Ok(p) => {
            print!("Chrome: {} … ", p.display());
            match Browser::launch(LaunchOptions::default(), Policy::default()).await {
                Ok(b) => {
                    let t = std::time::Instant::now();
                    let r = b.page().navigate("about:blank").await;
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
                    let msg = e.to_string().to_lowercase();
                    if msg.contains("sandbox") || msg.contains("as root") {
                        println!("  fix: Chrome refuses its sandbox here (running as root or inside a container). Set OSTLER_NO_SANDBOX=1 only if the machine is isolated.");
                    } else if msg.contains("profile") || msg.contains("already") {
                        println!("  fix: another Chrome or Ostler is using the same profile. Close it, or give each server its own --profile-dir.");
                    } else if msg.contains("shared libraries")
                        || msg.contains("libnss")
                        || msg.contains("cannot open shared object")
                    {
                        println!("  fix: Chrome is missing system libraries. On Debian/Ubuntu install `chromium` from apt (it pulls them in), or the libraries named above.");
                    } else {
                        println!("  fix: run the Chrome binary above directly to see its own error; if it opens, retry with `--headed`. Point Ostler at a different build with OSTLER_CHROME=/path/to/chrome.");
                    }
                }
            }
        }
        Err(e) => {
            ok = false;
            println!("Chrome: {e}");
        }
    }
    memory_report(&m);
    match build_memory(&m) {
        Ok(None) => {}
        Ok(Some(mem)) => match mem.init().await {
            Ok(()) => {
                println!("  qdrant:     reachable, collections ready. OK");
                if let Err(e) = key_preflight(&m, Preflight::Strict).await {
                    ok = false;
                    println!("  key:        FAILED: {e}");
                }
            }
            Err(e) => {
                ok = false;
                println!("  qdrant:     FAILED: {}", explain(&e.to_string()));
            }
        },
        Err(e) => {
            ok = false;
            println!("  qdrant:     FAILED: {e}");
        }
    }
    if ok {
        println!("\nAll good. Next: `ostler setup claude --apply` (or codex | cursor | vscode | json) to connect your agent,\nthen try the safe demo: `ostler fixtures`.\nAfter rebuilding ostler, reconnect your MCP client (Claude Code: /mcp) to pick up the new binary.");
    } else {
        println!("\nFix the items above and run `ostler doctor` again. Chrome is required; memory is optional (unset QDRANT_URL to run without it).");
        std::process::exit(1);
    }
    Ok(())
}
