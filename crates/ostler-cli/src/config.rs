//! User config file. Lowest-precedence source of settings: flag > environment variable > config file > default.
//!
//! The file maps onto the same environment variables the flags already read, so every setting has exactly one path.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};

/// (config key, environment variable)
pub const KEYS: &[(&str, &str)] = &[
    ("qdrant_url", "QDRANT_URL"),
    ("qdrant_api_key", "QDRANT_API_KEY"),
    ("tenant", "OSTLER_TENANT"),
    ("memory", "OSTLER_MEMORY"),
    ("embeddings", "OSTLER_EMBEDDINGS"),
    ("embedding_model", "OSTLER_EMBEDDING_MODEL"),
    ("embedding_url", "OSTLER_EMBEDDING_URL"),
    ("embedding_dim", "OSTLER_EMBEDDING_DIM"),
    ("seal", "OSTLER_SEAL"),
    ("key_store", "OSTLER_KEY_STORE"),
    ("isolation", "OSTLER_ISOLATION"),
    ("allow_insecure_qdrant", "OSTLER_ALLOW_INSECURE_QDRANT"),
    ("retain_days", "OSTLER_RETAIN_DAYS"),
    ("qdrant_on_disk", "OSTLER_QDRANT_ON_DISK"),
    ("learn", "OSTLER_LEARN"),
    ("lean", "OSTLER_LEAN"),
    ("verbose", "OSTLER_VERBOSE"),
    ("a2a_bind", "OSTLER_A2A_BIND"),
    ("a2a_token", "OSTLER_A2A_TOKEN"),
    ("tools", "OSTLER_TOOLS"),
];

const SECRETS: &[&str] = &["qdrant_api_key", "a2a_token"];

pub fn path() -> PathBuf {
    if let Ok(p) = std::env::var("OSTLER_CONFIG") {
        return PathBuf::from(p);
    }
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .ok()
        .filter(|p| p.is_absolute())
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".config"))
        })
        .unwrap_or_else(|| PathBuf::from(".config"));
    base.join("ostler").join("config.toml")
}

/// The sealing key for memory. Order: `OSTLER_MEMORY_KEY` (base64, 32 bytes) > the store chosen by
/// `key_store` (`file`, the default: an owner-only `memory.key` next to the config file; or `keychain`: the
/// macOS Keychain / Linux Secret Service). If none holds a key a random one is created and its location is
/// reported once. Losing the key makes sealed memory unreadable by design; `ostler memory wipe` starts over.
pub fn memory_key_path() -> PathBuf {
    path().with_file_name("memory.key")
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyStore {
    File,
    Keychain,
}

pub fn key_store() -> Result<KeyStore> {
    match std::env::var("OSTLER_KEY_STORE").ok().as_deref() {
        None | Some("") | Some("file") => Ok(KeyStore::File),
        Some("keychain") => Ok(KeyStore::Keychain),
        Some(other) => anyhow::bail!(
            "key_store {other:?} is not valid; use `file` or `keychain` (`ostler config set key_store file`)"
        ),
    }
}

static KEY_CREATED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// True when this process had to generate a new key because none existed.
pub fn key_just_created() -> bool {
    KEY_CREATED.load(std::sync::atomic::Ordering::SeqCst)
}

/// Human description of where the key is read from, for messages.
pub fn key_location() -> String {
    if std::env::var_os("OSTLER_MEMORY_KEY").is_some() {
        return "the OSTLER_MEMORY_KEY environment variable".into();
    }
    match key_store() {
        Ok(KeyStore::Keychain) => keychain::describe(),
        _ => memory_key_path().display().to_string(),
    }
}

/// OS credential store access through the platform's own CLI. No crate dependencies.
///
/// The key never appears on a command line: macOS `security` is driven through its interactive mode
/// (`security -i`, commands on stdin) and Linux `secret-tool store` reads the secret from stdin. Reading
/// uses `-w` / `lookup`, which print to stdout.
pub mod keychain {
    use std::io::Write;
    use std::process::{Command, Stdio};

    use anyhow::{Context, Result};

    const SERVICE: &str = "ostler-memory-key";

    pub fn account() -> String {
        std::env::var("OSTLER_KEYCHAIN_ACCOUNT")
            .ok()
            .filter(|a| {
                !a.is_empty()
                    && a.chars()
                        .all(|c| c.is_ascii_graphic() && c != '"' && c != '\\')
            })
            .unwrap_or_else(|| "memory".into())
    }

    #[cfg(target_os = "macos")]
    const TOOL: &str = "security";
    #[cfg(not(target_os = "macos"))]
    const TOOL: &str = "secret-tool";

    pub fn describe() -> String {
        if cfg!(target_os = "macos") {
            format!(
                "the macOS Keychain (service {SERVICE:?}, account {:?})",
                account()
            )
        } else {
            format!(
                "the system Secret Service (service {SERVICE:?}, account {:?})",
                account()
            )
        }
    }

    fn missing_tool(e: std::io::Error) -> anyhow::Error {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!(
                "key_store is `keychain` but `{TOOL}` was not found on PATH. {} Or use `ostler config set key_store file`.",
                if cfg!(target_os = "macos") {
                    "It ships with macOS (/usr/bin/security)."
                } else {
                    "Install libsecret-tools (Debian/Ubuntu: `apt install libsecret-tools`) and run a Secret Service such as gnome-keyring."
                }
            )
        } else {
            anyhow::Error::new(e).context(format!("running {TOOL}"))
        }
    }

    /// Wait for the tool, but not forever: a locked keyring or a missing D-Bus session can make
    /// `security`/`secret-tool` block on a prompt nobody can answer, which would hang startup silently.
    fn wait_timeout(child: std::process::Child) -> Result<std::process::Output> {
        let pid = child.id();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(child.wait_with_output());
        });
        match rx.recv_timeout(std::time::Duration::from_secs(20)) {
            Ok(r) => Ok(r?),
            Err(_) => {
                let _ = Command::new("kill").arg(pid.to_string()).status();
                anyhow::bail!(
                    "{} did not answer within 20 s (a locked keychain or no desktop session?). Unlock it, or use `ostler config set key_store file`",
                    TOOL
                )
            }
        }
    }

    fn run(c: &mut Command) -> Result<std::process::Output> {
        let child = c
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(missing_tool)?;
        wait_timeout(child)
    }

    /// Fail early, with advice, when the platform tool is not there.
    pub fn check_available() -> Result<()> {
        let mut c = Command::new(TOOL);
        c.arg(if cfg!(target_os = "macos") {
            "help"
        } else {
            "--version"
        });
        run(&mut c).map(|_| ())
    }

    /// `Ok(None)` only when the tool says the item does not exist; any other failure (locked keychain, no
    /// session bus, denied) is an error, so a new key is never generated over a key we merely could not read.
    pub fn get() -> Result<Option<String>> {
        let mut c = Command::new(TOOL);
        if cfg!(target_os = "macos") {
            c.args([
                "find-generic-password",
                "-s",
                SERVICE,
                "-a",
                &account(),
                "-w",
            ]);
        } else {
            c.args(["lookup", "service", SERVICE, "account", &account()]);
        }
        let out = run(&mut c)?;
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if out.status.success() && !text.is_empty() {
            return Ok(Some(text));
        }
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        // macOS: exit 44 = errSecItemNotFound. secret-tool: exit 1 with nothing on stderr.
        let not_found = if cfg!(target_os = "macos") {
            out.status.code() == Some(44)
        } else {
            out.status.code() == Some(1) && err.is_empty()
        };
        if not_found || (out.status.success() && text.is_empty()) {
            return Ok(None);
        }
        anyhow::bail!(
            "reading the key from {} failed: {}",
            describe(),
            if err.is_empty() {
                format!("{}", out.status)
            } else {
                err
            }
        )
    }

    /// Store `b64` (replacing an existing entry when `replace`). Verified by reading it back.
    pub fn set(b64: &str, replace: bool) -> Result<()> {
        anyhow::ensure!(
            b64.chars()
                .all(|c| c.is_ascii_alphanumeric() || "+/=".contains(c)),
            "refusing to store a key with unexpected characters"
        );
        if cfg!(target_os = "macos") {
            let mut child = Command::new(TOOL)
                .arg("-i")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(missing_tool)?;
            let line = format!(
                "add-generic-password {}-s \"{SERVICE}\" -a \"{}\" -l \"Ostler memory key\" -w \"{b64}\"\n",
                if replace { "-U " } else { "" },
                account()
            );
            child
                .stdin
                .take()
                .context("no stdin")?
                .write_all(line.as_bytes())?;
            let _ = wait_timeout(child)?;
        } else {
            let mut child = Command::new(TOOL)
                .args([
                    "store",
                    "--label=Ostler memory key",
                    "service",
                    SERVICE,
                    "account",
                    &account(),
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(missing_tool)?;
            child
                .stdin
                .take()
                .context("no stdin")?
                .write_all(b64.as_bytes())?;
            let out = wait_timeout(child)?;
            anyhow::ensure!(
                out.status.success(),
                "secret-tool could not store the key: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        match get()? {
            Some(v) if v == b64 => Ok(()),
            Some(_) if !replace => Ok(()), // someone else created it first; the caller re-reads
            _ => anyhow::bail!(
                "the key was not stored in {} (tool reported no error but reading it back failed)",
                describe()
            ),
        }
    }
}

fn write_private(p: &std::path::Path, text: &str) -> Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(p)
        .with_context(|| format!("creating {}", p.display()))?;
    f.write_all(text.as_bytes())?;
    f.sync_all()?;
    Ok(())
}

/// Where the sealing key comes from right now, without creating it.
pub enum KeyStatus {
    /// `OSTLER_MEMORY_KEY` is set.
    Env,
    /// `memory.key` exists; `private` is false when other users can read it (unix only).
    File { path: PathBuf, private: bool },
    /// `key_store = keychain`: the key is in the OS credential store (`present`) or not yet created.
    Keychain { present: Option<bool> },
    /// No key yet: one is created the first time sealed memory is used.
    Missing { path: PathBuf },
}

pub fn memory_key_status() -> KeyStatus {
    if std::env::var_os("OSTLER_MEMORY_KEY").is_some() {
        return KeyStatus::Env;
    }
    if matches!(key_store(), Ok(KeyStore::Keychain)) {
        return KeyStatus::Keychain {
            present: keychain::get().map(|k| k.is_some()).ok(),
        };
    }
    let path = memory_key_path();
    match std::fs::metadata(&path) {
        Ok(_m) => {
            #[cfg(unix)]
            let private = {
                use std::os::unix::fs::PermissionsExt;
                _m.permissions().mode() & 0o077 == 0
            };
            #[cfg(not(unix))]
            let private = true;
            KeyStatus::File { path, private }
        }
        Err(_) => KeyStatus::Missing { path },
    }
}

/// True for `http://` URLs to a host other than this machine (the case the store refuses by default).
pub fn is_remote_http(url: &str) -> bool {
    let Some(rest) = url.trim().strip_prefix("http://") else {
        return false;
    };
    let authority = rest.split(['/', '?']).next().unwrap_or("");
    let host = match authority.rsplit_once('@') {
        Some((_, h)) => h,
        None => authority,
    };
    let host = if let Some(h) = host.strip_prefix('[') {
        h.split(']').next().unwrap_or("")
    } else {
        host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host)
    };
    !(host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1")
}

/// The key from the environment or the configured store; `None` when there is none yet (nothing is created).
pub fn existing_memory_key() -> Result<Option<[u8; 32]>> {
    use ostler_memory::SealKey;
    if let Ok(v) = std::env::var("OSTLER_MEMORY_KEY") {
        return SealKey::decode(&v)
            .map(Some)
            .map_err(|e| anyhow::anyhow!("OSTLER_MEMORY_KEY: {e}"));
    }
    match key_store()? {
        KeyStore::Keychain => match keychain::get()? {
            Some(s) => SealKey::decode(&s)
                .map(Some)
                .map_err(|e| anyhow::anyhow!("{}: {e}", keychain::describe())),
            None => Ok(None),
        },
        KeyStore::File => {
            let p = memory_key_path();
            match std::fs::read_to_string(&p) {
                Ok(s) if s.trim().is_empty() => anyhow::bail!(
                    "{} is empty (a previous start was interrupted?). Delete it to create a new key, but only if no saved memory needs the old one.",
                    p.display()
                ),
                Ok(s) => SealKey::decode(&s)
                    .map(Some)
                    .map_err(|e| anyhow::anyhow!("{}: {e}", p.display())),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e).with_context(|| format!("reading {}", p.display())),
            }
        }
    }
}

pub fn memory_key() -> Result<[u8; 32]> {
    use ostler_memory::SealKey;
    if let Some(k) = existing_memory_key()? {
        return Ok(k);
    }
    let key = SealKey::generate().map_err(|e| anyhow::anyhow!("{e}"))?;
    if key_store()? == KeyStore::Keychain {
        // No atomic create-if-absent exists across both tools; re-read so a concurrent starter's key wins.
        keychain::set(&SealKey::encode(&key), false)?;
        let stored =
            existing_memory_key()?.context("the key vanished right after it was stored")?;
        if stored == key {
            KEY_CREATED.store(true, std::sync::atomic::Ordering::SeqCst);
            eprintln!(
                "ostler: created the key that seals your saved memory in {}.\n  Back it up with `ostler memory key export --yes-print > memory.key.backup`. If it is lost, sealed memory can never be read again;\n  the only recovery is `ostler memory wipe --yes` and starting over. Do not share it.",
                keychain::describe()
            );
        }
        return Ok(stored);
    }
    let p = memory_key_path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Write a private temp file completely, then hard-link it into place. The link fails if the key
    // appeared meanwhile (another ostler starting at the same time), so two starters can never read a
    // half-written key or overwrite each other's: the loser reads the winner's.
    let tmp = p.with_file_name(format!(".memory.key.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    write_private(&tmp, &SealKey::encode(&key))?;
    let linked = std::fs::hard_link(&tmp, &p);
    let _ = std::fs::remove_file(&tmp);
    match linked {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let s =
                std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
            return SealKey::decode(&s).map_err(|e| anyhow::anyhow!("{}: {e}", p.display()));
        }
        Err(e) => return Err(e).with_context(|| format!("creating {}", p.display())),
    }
    KEY_CREATED.store(true, std::sync::atomic::Ordering::SeqCst);
    eprintln!(
        "ostler: created the key that seals your saved memory at {}.\n  Back it up (`ostler memory key export --yes-print`, or a password manager or encrypted drive). If it is lost, sealed memory can never be read again;\n  the only recovery is `ostler memory wipe --yes` and starting over. Do not share it.",
        p.display()
    );
    Ok(key)
}

/// Save `key` in the configured store (for `ostler memory key import`). Replacing a different existing key
/// needs `force`, and the old one is first kept in `memory.key.old-<unix time>` (owner-only).
pub fn import_memory_key(key: [u8; 32], force: bool) -> Result<String> {
    use ostler_memory::SealKey;
    anyhow::ensure!(
        std::env::var_os("OSTLER_MEMORY_KEY").is_none(),
        "OSTLER_MEMORY_KEY is set and always wins over stored keys. Unset it (or put the key there instead), then import"
    );
    let b64 = SealKey::encode(&key);
    let store = key_store()?;
    if store == KeyStore::Keychain {
        keychain::check_available()?;
    }
    // A corrupt or empty key file must not block replacing it: with --force it is simply overwritten.
    let existing = match existing_memory_key() {
        Ok(k) => k,
        Err(e) if force => {
            eprintln!("ostler: the existing key could not be read ({e:#}); replacing it");
            None
        }
        Err(e) => {
            return Err(
                e.context("the existing key is unreadable; re-run with --force to replace it")
            )
        }
    };
    if let Some(old) = existing {
        if old == key {
            return Ok(format!(
                "that key is already in place ({}); nothing changed",
                key_location()
            ));
        }
        anyhow::ensure!(
            force,
            "a different key already exists in {}. Replacing it makes anything sealed with it unreadable unless you have a backup. If you are sure, re-run with --force (the old key is saved next to the config file first)",
            key_location()
        );
        let backup = path().with_file_name(format!(
            "memory.key.old-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        ));
        if let Some(dir) = backup.parent() {
            std::fs::create_dir_all(dir)?;
        }
        write_private(&backup, &SealKey::encode(&old))?;
        eprintln!("ostler: the replaced key was saved to {}", backup.display());
    }
    match store {
        KeyStore::Keychain => keychain::set(&b64, true)?,
        KeyStore::File => {
            let p = memory_key_path();
            if let Some(dir) = p.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = p.with_file_name(format!(".memory.key.{}.import", std::process::id()));
            let _ = std::fs::remove_file(&tmp);
            write_private(&tmp, &b64)?;
            std::fs::rename(&tmp, &p).with_context(|| format!("writing {}", p.display()))?;
        }
    }
    Ok(format!("imported the memory key into {}", key_location()))
}

pub fn read() -> Result<BTreeMap<String, String>> {
    let p = path();
    let text = match std::fs::read_to_string(&p) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", p.display())),
    };
    let table: toml::Table = text
        .parse()
        .with_context(|| format!("{} is not valid TOML", p.display()))?;
    let mut out = BTreeMap::new();
    for (k, v) in table {
        let s = match v {
            toml::Value::String(s) => s,
            toml::Value::Boolean(b) => b.to_string(),
            toml::Value::Integer(i) => i.to_string(),
            other => anyhow::bail!("{}: unsupported value for {k}: {other}", p.display()),
        };
        out.insert(k, s);
    }
    Ok(out)
}

pub fn write(map: &BTreeMap<String, String>) -> Result<PathBuf> {
    let p = path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut table = toml::Table::new();
    for (k, v) in map {
        let value = match v.as_str() {
            "true" => toml::Value::Boolean(true),
            "false" => toml::Value::Boolean(false),
            _ => toml::Value::String(v.clone()),
        };
        table.insert(k.clone(), value);
    }
    let body = format!(
        "# Ostler settings. Flags and environment variables override this file.\n# Contains a secret when qdrant_api_key is set: keep it private.\n{}",
        toml::to_string(&table)?
    );
    // Create with owner-only permissions from the start so the key is never world-readable.
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&p)?;
        f.write_all(body.as_bytes())?;
    }
    #[cfg(not(unix))]
    std::fs::write(&p, body)?;
    Ok(p)
}

/// Export config values as environment variables unless the user already set them.
/// Called before argument parsing, so clap's `env =` attributes see them. A broken file is reported, not fatal.
pub fn load_into_env() {
    match read() {
        Ok(map) => {
            for (k, env) in KEYS {
                if let Some(v) = map.get(*k) {
                    if std::env::var_os(env).is_none() {
                        std::env::set_var(env, v);
                    }
                }
            }
        }
        Err(e) => eprintln!("warning: ignoring config file: {e:#}"),
    }
}

/// Called by `config set key_store <new>` before saving: makes sure the new store works and carries an
/// existing key across, so switching never leaves the key behind. Returns lines to show the user.
pub fn switch_key_store(new: KeyStore) -> Result<Vec<String>> {
    use ostler_memory::SealKey;
    let mut notes = vec![];
    let p = memory_key_path();
    let file_key = match std::fs::read_to_string(&p) {
        Ok(s) if !s.trim().is_empty() => {
            Some(SealKey::decode(&s).map_err(|e| anyhow::anyhow!("{}: {e}", p.display()))?)
        }
        _ => None,
    };
    match new {
        KeyStore::Keychain => {
            keychain::check_available()?;
            let kc = keychain::get()?
                .map(|s| SealKey::decode(&s))
                .transpose()
                .map_err(|e| anyhow::anyhow!("{}: {e}", keychain::describe()))?;
            match (file_key, kc) {
                (Some(f), None) => {
                    keychain::set(&SealKey::encode(&f), false)?;
                    notes.push(format!("copied your existing key from {} into {}. The file was left in place: once `ostler doctor` is happy, delete it so the key lives in one place.", p.display(), keychain::describe()));
                }
                (Some(f), Some(k)) if f != k => anyhow::bail!(
                    "{} and {} hold DIFFERENT keys. Decide which one opened your saved memory, then make them agree (`ostler memory key export --yes-print` under the current setting, then `ostler memory key import --force` after switching) and retry",
                    p.display(),
                    keychain::describe()
                ),
                _ => {}
            }
        }
        KeyStore::File => {
            if file_key.is_none() && keychain::get().ok().flatten().is_some() {
                let k = keychain::get()?.unwrap_or_default();
                let key = SealKey::decode(&k)
                    .map_err(|e| anyhow::anyhow!("{}: {e}", keychain::describe()))?;
                if let Some(dir) = p.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                write_private(&p, &SealKey::encode(&key))?;
                notes.push(format!(
                    "copied your key from {} to {} (owner-only). Back it up.",
                    keychain::describe(),
                    p.display()
                ));
            }
        }
    }
    Ok(notes)
}

/// Check a value at `config set` time so a typo fails now, not as an obscure error at the next start.
/// Returns the value to store (booleans are normalised).
pub fn validate(key: &str, value: &str) -> Result<String> {
    let choice = |allowed: &[&str]| -> Result<String> {
        if allowed.contains(&value) {
            Ok(value.to_string())
        } else {
            anyhow::bail!(
                "{key} must be one of: {} (got {value:?})",
                allowed.join(", ")
            )
        }
    };
    let boolean = || -> Result<String> {
        match value.to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" | "1" => Ok("true".into()),
            "false" | "no" | "off" | "0" => Ok("false".into()),
            _ => anyhow::bail!("{key} must be true or false (got {value:?})"),
        }
    };
    match key {
        "seal" => choice(&["auto", "on", "off"]),
        "isolation" => choice(&["shared", "collection"]),
        "embeddings" => choice(&["cloud", "ollama", "dev-hash"]),
        "memory" => choice(&["auto", "qdrant", "ephemeral", "off"]),
        "key_store" => choice(&["file", "keychain"]),
        "tools" => choice(&["core", "forms", "all"]),
        "retain_days" => value.parse::<u64>().map(|n| n.to_string()).map_err(|_| {
            anyhow::anyhow!(
                "retain_days must be a whole number of days, 0 = keep forever (got {value:?})"
            )
        }),
        "embedding_dim" => match value.parse::<usize>() {
            Ok(n) if n > 0 => Ok(n.to_string()),
            _ => anyhow::bail!(
                "embedding_dim must be a positive whole number such as 384 (got {value:?})"
            ),
        },
        "allow_insecure_qdrant" | "qdrant_on_disk" | "learn" | "lean" | "verbose" => boolean(),
        "qdrant_url" | "embedding_url" => {
            anyhow::ensure!(
                value.starts_with("http://") || value.starts_with("https://"),
                "{key} must start with http:// or https:// (got {value:?})"
            );
            Ok(value.to_string())
        }
        _ => Ok(value.to_string()),
    }
}

pub fn env_for(key: &str) -> Option<&'static str> {
    KEYS.iter().find(|(k, _)| *k == key).map(|(_, e)| *e)
}

pub fn mask(key: &str, value: &str) -> String {
    if !SECRETS.contains(&key) {
        return value.to_string();
    }
    match value.len() {
        0 => String::new(),
        1..=8 => "********".into(),
        n => format!("{}…{} ({n} chars)", &value[..3], &value[n - 2..]),
    }
}

/// Qdrant Cloud serves the REST API on :6333; people paste the bare cluster URL.
pub fn normalize_qdrant_url(url: &str) -> String {
    let u = url.trim().trim_end_matches('/').to_string();
    let after_scheme = u.split("://").nth(1).unwrap_or(&u);
    let host_part = after_scheme.split('/').next().unwrap_or("");
    if host_part.ends_with("cloud.qdrant.io") && !host_part.contains(':') {
        return u.replacen(host_part, &format!("{host_part}:6333"), 1);
    }
    u
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_urls_get_the_rest_port() {
        assert_eq!(
            normalize_qdrant_url("https://abc.eu-west.aws.cloud.qdrant.io/"),
            "https://abc.eu-west.aws.cloud.qdrant.io:6333"
        );
        assert_eq!(
            normalize_qdrant_url("https://abc.cloud.qdrant.io:6333"),
            "https://abc.cloud.qdrant.io:6333"
        );
        assert_eq!(
            normalize_qdrant_url("http://localhost:6333"),
            "http://localhost:6333"
        );
    }

    #[test]
    fn only_non_loopback_http_counts_as_remote() {
        assert!(is_remote_http("http://qdrant.example.com:6333"));
        assert!(is_remote_http("http://10.0.0.5:6333"));
        assert!(!is_remote_http("http://localhost:6333"));
        assert!(!is_remote_http("http://127.0.0.1:6333/x"));
        assert!(!is_remote_http("http://[::1]:6333"));
        assert!(!is_remote_http("https://qdrant.example.com"));
    }

    #[test]
    fn secrets_are_masked_but_other_values_are_not() {
        assert_eq!(mask("tenant", "alice"), "alice");
        assert_eq!(mask("qdrant_api_key", "short"), "********");
        let m = mask("qdrant_api_key", "abcdefghijklmnop");
        assert!(!m.contains("defghijklmn"), "{m}");
    }
}
