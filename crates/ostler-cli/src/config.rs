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

/// Where `--memory local` keeps its encrypted file: next to the config, like `memory.key`.
pub fn local_memory_path() -> PathBuf {
    path().with_file_name("memory.local")
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
///
/// Every call goes through a `Runner`, so the Linux behaviour (which cannot run on a Mac) is tested with a
/// scripted fake, on any host.
pub mod keychain {
    use std::io::Write;
    use std::path::Path;
    use std::process::{Command, Stdio};

    use anyhow::{Context, Result};

    pub const SERVICE: &str = "ostler-memory-key";

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Os {
        Mac,
        Linux,
    }

    impl Os {
        pub fn host() -> Os {
            if cfg!(target_os = "macos") {
                Os::Mac
            } else {
                Os::Linux
            }
        }
        fn tool(self) -> &'static str {
            match self {
                Os::Mac => "security",
                Os::Linux => "secret-tool",
            }
        }
    }

    /// One invocation of the platform tool.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Cmd {
        pub args: Vec<String>,
        pub stdin: Option<String>,
    }

    /// What the tool did.
    #[derive(Clone, Debug, PartialEq, Eq, Default)]
    pub struct Out {
        pub code: Option<i32>,
        pub stdout: String,
        pub stderr: String,
    }

    impl Out {
        fn success(&self) -> bool {
            self.code == Some(0)
        }
    }

    /// Runs the platform tool. `Err` means it could not be run at all (not installed, hung, killed).
    pub trait Runner {
        fn run(&self, os: Os, cmd: &Cmd) -> Result<Out>;
    }

    /// The real thing: spawn the tool, never wait more than 20 s.
    pub struct System;

    impl Runner for System {
        fn run(&self, os: Os, cmd: &Cmd) -> Result<Out> {
            let mut c = Command::new(os.tool());
            c.args(&cmd.args)
                .stdin(if cmd.stdin.is_some() {
                    Stdio::piped()
                } else {
                    Stdio::null()
                })
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut child = c.spawn().map_err(|e| missing_tool(os, e))?;
            if let Some(input) = &cmd.stdin {
                child
                    .stdin
                    .take()
                    .context("no stdin")?
                    .write_all(input.as_bytes())?;
            }
            let o = wait_timeout(os, child)?;
            Ok(Out {
                code: o.status.code(),
                stdout: String::from_utf8_lossy(&o.stdout).into(),
                stderr: String::from_utf8_lossy(&o.stderr).into(),
            })
        }
    }

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

    fn fallback(os: Os) -> &'static str {
        match os {
            Os::Mac => "Or use `ostler config set key_store file`.",
            Os::Linux => "No Secret Service is available here (typical on servers, containers and SSH sessions without a desktop keyring). Use the file store instead: `ostler config set key_store file`. Ostler never falls back to it by itself, so the key never ends up half in a keyring and half on disk.",
        }
    }

    fn missing_tool(os: Os, e: std::io::Error) -> anyhow::Error {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!(
                "key_store is `keychain` but `{}` was not found on PATH. {} {}",
                os.tool(),
                if os == Os::Mac {
                    "It ships with macOS (/usr/bin/security)."
                } else {
                    "Install libsecret-tools (Debian/Ubuntu: `apt install libsecret-tools`) and run a Secret Service such as gnome-keyring."
                },
                fallback(os)
            )
        } else {
            anyhow::Error::new(e).context(format!("running {}", os.tool()))
        }
    }

    /// Wait for the tool, but not forever: a locked keyring or a missing D-Bus session can make
    /// `security`/`secret-tool` block on a prompt nobody can answer, which would hang startup silently.
    fn wait_timeout(os: Os, child: std::process::Child) -> Result<std::process::Output> {
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
                    "{} did not answer within 20 s (a locked keychain or no desktop session?). Unlock it. {}",
                    os.tool(),
                    fallback(os)
                )
            }
        }
    }

    /// A credential store reached through `runner`, as seen on `os`.
    pub struct Keychain<'a> {
        pub os: Os,
        pub account: String,
        pub runner: &'a dyn Runner,
    }

    static SYSTEM: System = System;

    impl Keychain<'static> {
        /// This machine's own store.
        pub fn system() -> Self {
            Keychain {
                os: Os::host(),
                account: account(),
                runner: &SYSTEM,
            }
        }
    }

    impl Keychain<'_> {
        pub fn describe(&self) -> String {
            match self.os {
                Os::Mac => format!(
                    "the macOS Keychain (service {SERVICE:?}, account {:?})",
                    self.account
                ),
                Os::Linux => format!(
                    "the system Secret Service (service {SERVICE:?}, account {:?})",
                    self.account
                ),
            }
        }

        /// Fail early, with advice, when the platform tool is not there.
        pub fn check_available(&self) -> Result<()> {
            let arg = if self.os == Os::Mac {
                "help"
            } else {
                "--version"
            };
            self.runner
                .run(
                    self.os,
                    &Cmd {
                        args: vec![arg.into()],
                        stdin: None,
                    },
                )
                .map(|_| ())
        }

        /// `Ok(None)` only when the tool says the item does not exist; any other failure (locked keychain, no
        /// session bus, no Secret Service, denied) is an error, so a new key is never generated over a key we
        /// merely could not read.
        pub fn get(&self) -> Result<Option<String>> {
            let args: Vec<String> = if self.os == Os::Mac {
                [
                    "find-generic-password",
                    "-s",
                    SERVICE,
                    "-a",
                    &self.account,
                    "-w",
                ]
                .map(String::from)
                .to_vec()
            } else {
                ["lookup", "service", SERVICE, "account", &self.account]
                    .map(String::from)
                    .to_vec()
            };
            let out = self.runner.run(self.os, &Cmd { args, stdin: None })?;
            let text = out.stdout.trim().to_string();
            if out.success() && !text.is_empty() {
                return Ok(Some(text));
            }
            let err = out.stderr.trim().to_string();
            // macOS: exit 44 = errSecItemNotFound. secret-tool: exit 1 with nothing on stderr.
            let not_found = if self.os == Os::Mac {
                out.code == Some(44)
            } else {
                out.code == Some(1) && err.is_empty()
            };
            if not_found || (out.success() && text.is_empty()) {
                return Ok(None);
            }
            anyhow::bail!(
                "reading the key from {} failed: {}. {}",
                self.describe(),
                if err.is_empty() {
                    format!("exit status {:?}", out.code)
                } else {
                    err
                },
                fallback(self.os)
            )
        }

        /// Store `b64` (replacing an existing entry when `replace`). Verified by reading it back.
        pub fn set(&self, b64: &str, replace: bool) -> Result<()> {
            anyhow::ensure!(
                b64.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "+/=".contains(c)),
                "refusing to store a key with unexpected characters"
            );
            if self.os == Os::Mac {
                let line = format!(
                    "add-generic-password {}-s \"{SERVICE}\" -a \"{}\" -l \"Ostler memory key\" -w \"{b64}\"\n",
                    if replace { "-U " } else { "" },
                    self.account
                );
                let _ = self.runner.run(
                    self.os,
                    &Cmd {
                        args: vec!["-i".into()],
                        stdin: Some(line),
                    },
                )?;
            } else {
                let out = self.runner.run(
                    self.os,
                    &Cmd {
                        args: [
                            "store",
                            "--label=Ostler memory key",
                            "service",
                            SERVICE,
                            "account",
                            &self.account,
                        ]
                        .map(String::from)
                        .to_vec(),
                        stdin: Some(b64.into()),
                    },
                )?;
                anyhow::ensure!(
                    out.success(),
                    "secret-tool could not store the key: {}. {}",
                    out.stderr.trim(),
                    fallback(self.os)
                );
            }
            match self.get()? {
                Some(v) if v == b64 => Ok(()),
                Some(_) if !replace => Ok(()), // someone else created it first; the caller re-reads
                _ => anyhow::bail!(
                    "the key was not stored in {} (tool reported no error but reading it back failed). {}",
                    self.describe(),
                    fallback(self.os)
                ),
            }
        }

        /// Create the item with `b64` unless one exists; either way return what is stored afterwards, and
        /// whether this call created it.
        ///
        /// `secret-tool store` always replaces, so "check, then store" would let two first starts each
        /// believe they created the key, and the loser's data would be sealed with a key that is gone.
        /// Creators therefore queue on an exclusive lock file (`lock`), and the check and the store happen
        /// inside it. Other programs writing the same item are not covered.
        pub fn create_if_absent(&self, b64: &str, lock: &Path) -> Result<(String, bool)> {
            let _guard = LockGuard::acquire(lock)?;
            if let Some(v) = self.get()? {
                return Ok((v, false));
            }
            self.set(b64, false)?;
            let stored = self
                .get()?
                .context("the key vanished right after it was stored")?;
            Ok((stored.clone(), stored == b64))
        }
    }

    /// An exclusive lock file; removed on drop. A lock older than a minute belongs to a crashed starter and is
    /// taken over (the keychain call it protects is bounded by a 20 s timeout).
    struct LockGuard(std::path::PathBuf);

    impl LockGuard {
        fn acquire(path: &Path) -> Result<Self> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let start = std::time::Instant::now();
            loop {
                match std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)
                {
                    Ok(mut f) => {
                        let _ = write!(f, "{}", std::process::id());
                        return Ok(LockGuard(path.to_path_buf()));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        let age = std::fs::metadata(path)
                            .and_then(|m| m.modified())
                            .ok()
                            .and_then(|t| t.elapsed().ok());
                        if age.is_some_and(|a| a > std::time::Duration::from_secs(60)) {
                            let _ = std::fs::remove_file(path);
                            continue;
                        }
                        anyhow::ensure!(
                            start.elapsed() < std::time::Duration::from_secs(45),
                            "another ostler is creating the memory key ({} exists). If none is running, delete that file",
                            path.display()
                        );
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                    Err(e) => {
                        return Err(e).with_context(|| format!("creating {}", path.display()))
                    }
                }
            }
        }
    }

    impl Drop for LockGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    // Thin wrappers over this machine's own store, for callers that do not need to inject a runner.
    pub fn describe() -> String {
        Keychain::system().describe()
    }
    pub fn check_available() -> Result<()> {
        Keychain::system().check_available()
    }
    pub fn get() -> Result<Option<String>> {
        Keychain::system().get()
    }
    pub fn set(b64: &str, replace: bool) -> Result<()> {
        Keychain::system().set(b64, replace)
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
            // Selected the keychain (env var, hand-edited config) but the key is still in memory.key:
            // creating a second key here would split it in two.
            None if memory_key_path().exists() => anyhow::bail!(
                "key_store is `keychain` but the keychain holds no key, while {} does. Use `ostler config set key_store keychain` to copy the key across, or `ostler config set key_store file` to keep using the file",
                memory_key_path().display()
            ),
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

/// The key, which must already exist. Starting never creates one by itself: see [`create_memory_key`].
pub fn memory_key() -> Result<[u8; 32]> {
    existing_memory_key()?.with_context(|| {
        format!(
            "there is no memory key yet (looked in {}). It is created on the first start once Qdrant has been checked for existing sealed memory; restore a backup with `ostler memory key import`",
            key_location()
        )
    })
}

/// What a first start found out about the server before it may create a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreState {
    /// Could not be reached or read (the reason). Whether sealed memory exists there is unknown.
    Unreachable(String),
    /// Reachable and holds no sealed points for this tenant: a new key cannot orphan anything.
    Empty,
    /// Reachable and holds this many sealed points that the (absent) key would have to open.
    Sealed(usize),
}

/// Decide whether a first start may create a key. A new key over existing sealed memory would shadow it
/// for good, so the server must first be seen to hold none. `allow_new_key` (`--allow-new-key`) overrides
/// only the "could not look" case, for a first-time setup where Qdrant is not up yet; it never overrides
/// "there is sealed memory here".
pub fn authorize_new_key(state: &StoreState, allow_new_key: bool, tenant: &str) -> Result<()> {
    match state {
        StoreState::Empty => Ok(()),
        StoreState::Unreachable(_) if allow_new_key => {
            eprintln!("ostler: warning: creating a new memory key without being able to check Qdrant (--allow-new-key). If sealed memory already exists there, this key cannot open it; `ostler doctor` will say so once Qdrant is reachable.");
            Ok(())
        }
        StoreState::Unreachable(why) => anyhow::bail!(
            "refusing to create a new memory key: no key exists here yet and Qdrant could not be checked ({why}).\n  A new key would silently orphan any sealed memory already saved there under an earlier key.\n  - Start Qdrant (or fix qdrant_url) and run this again: the key is created once Qdrant is seen to hold no sealed memory for tenant {tenant:?}.\n  - Restoring an earlier key? `ostler memory key import < your-backup.key` (or set OSTLER_MEMORY_KEY).\n  - Really a first-time setup with Qdrant not up yet? Pass --allow-new-key (OSTLER_ALLOW_NEW_KEY=1)."
        ),
        StoreState::Sealed(n) => anyhow::bail!(
            "refusing to create a new memory key: Qdrant already holds {n} sealed point(s) for tenant {tenant:?} and no key exists here to open them.\n  Restore the original key: `ostler memory key import < your-backup.key` (or set OSTLER_MEMORY_KEY).\n  Or, if that key is gone for good, start over: `ostler memory wipe --yes --tenant {tenant}` (deletes this tenant's memory; irreversible). --allow-new-key does not override this."
        ),
    }
}

/// Create the sealing key in the configured store. Callers must have passed [`authorize_new_key`] first.
/// Safe against two first starts at once, and never writes to a second location.
pub fn create_memory_key() -> Result<[u8; 32]> {
    let store = key_store()?;
    let lock = memory_key_path().with_file_name("memory.key.lock");
    let (key, created) = create_key_in(
        store,
        &keychain::Keychain::system(),
        &memory_key_path(),
        &lock,
    )?;
    if created {
        KEY_CREATED.store(true, std::sync::atomic::Ordering::SeqCst);
        match store {
            KeyStore::Keychain => eprintln!(
                "ostler: created the key that seals your saved memory in {}.\n  Back it up with `ostler memory key export --yes-print > memory.key.backup`. If it is lost, sealed memory can never be read again;\n  the only recovery is `ostler memory wipe --yes` and starting over. Do not share it.",
                keychain::describe()
            ),
            KeyStore::File => eprintln!(
                "ostler: created the key that seals your saved memory at {}.\n  Back it up (`ostler memory key export --yes-print`, or a password manager or encrypted drive). If it is lost, sealed memory can never be read again;\n  the only recovery is `ostler memory wipe --yes` and starting over. Do not share it.",
                memory_key_path().display()
            ),
        }
    }
    Ok(key)
}

/// [`create_memory_key`] with the store, runner and paths injected (this is what the tests drive).
/// Returns the key now in force and whether this call created it (false: a concurrent starter did).
pub fn create_key_in(
    store: KeyStore,
    kc: &keychain::Keychain<'_>,
    file: &std::path::Path,
    lock: &std::path::Path,
) -> Result<([u8; 32], bool)> {
    use ostler_memory::SealKey;
    let key = SealKey::generate().map_err(|e| anyhow::anyhow!("{e}"))?;
    if store == KeyStore::Keychain {
        // A key file next to the config means the key lives there; a second, different key in the
        // keychain would split the key in two with no way to tell which one sealed the data.
        if file.exists() {
            anyhow::bail!(
                "key_store is `keychain` but {} exists, so a key was kept in a file. Creating another key in {} would split it across two places. Move it across with `ostler config set key_store keychain` (copies it), or keep the file with `ostler config set key_store file`",
                file.display(),
                kc.describe()
            );
        }
        let (stored, created) = kc.create_if_absent(&SealKey::encode(&key), lock)?;
        let k = SealKey::decode(&stored).map_err(|e| anyhow::anyhow!("{}: {e}", kc.describe()))?;
        return Ok((k, created));
    }
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Write a private temp file completely, then hard-link it into place. The link fails if the key
    // appeared meanwhile (another ostler starting at the same time), so two starters can never read a
    // half-written key or overwrite each other's: the loser reads the winner's.
    let tmp = file.with_file_name(format!(".memory.key.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    write_private(&tmp, &SealKey::encode(&key))?;
    let linked = std::fs::hard_link(&tmp, file);
    let _ = std::fs::remove_file(&tmp);
    match linked {
        Ok(()) => Ok((key, true)),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let s = std::fs::read_to_string(file)
                .with_context(|| format!("reading {}", file.display()))?;
            let k = SealKey::decode(&s).map_err(|e| anyhow::anyhow!("{}: {e}", file.display()))?;
            Ok((k, false))
        }
        Err(e) => Err(e).with_context(|| format!("creating {}", file.display())),
    }
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
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        // Two replacements in one second must not clobber (or fail on) the earlier backup.
        let mut backup = path().with_file_name(format!("memory.key.old-{stamp}"));
        for n in 1.. {
            if !backup.exists() {
                break;
            }
            backup = path().with_file_name(format!("memory.key.old-{stamp}-{n}"));
        }
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
        "memory" => choice(&["auto", "qdrant", "local", "ephemeral", "off"]),
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

    // ---- sealing-key creation: first-start policy and the Linux Secret Service path, against fakes ----

    use keychain::{Cmd, Keychain, Os, Out, Runner};
    use std::sync::Mutex;

    /// A scripted `secret-tool` / `security`: one stored item, optional failures, and a log of every call.
    #[derive(Default)]
    struct Fake {
        item: Mutex<Option<String>>,
        log: Mutex<Vec<String>>,
        /// Every call fails with this stderr (exit 1), as when no Secret Service is running.
        broken: Option<String>,
        /// `store` is accepted but silently drops the secret.
        drops_store: bool,
        /// The tool is not installed.
        missing: bool,
        /// Sleep inside `lookup`, to widen the race window between check and store.
        slow_lookup: bool,
    }

    impl Runner for Fake {
        fn run(&self, os: Os, cmd: &Cmd) -> anyhow::Result<Out> {
            self.log.lock().unwrap().push(cmd.args.join(" "));
            if self.missing {
                anyhow::bail!("key_store is `keychain` but `secret-tool` was not found on PATH.");
            }
            if let Some(e) = &self.broken {
                return Ok(Out {
                    code: Some(1),
                    stderr: e.clone(),
                    ..Out::default()
                });
            }
            let first = cmd.args.first().map(String::as_str).unwrap_or("");
            match (os, first) {
                (_, "--version") | (_, "help") => Ok(Out {
                    code: Some(0),
                    ..Out::default()
                }),
                (Os::Linux, "lookup") => {
                    if self.slow_lookup {
                        std::thread::sleep(std::time::Duration::from_millis(30));
                    }
                    Ok(match &*self.item.lock().unwrap() {
                        Some(v) => Out {
                            code: Some(0),
                            stdout: v.clone(),
                            ..Out::default()
                        },
                        // secret-tool: exit 1 and nothing on stderr when the item does not exist.
                        None => Out {
                            code: Some(1),
                            ..Out::default()
                        },
                    })
                }
                // secret-tool store replaces unconditionally: that is why creation needs the lock.
                (Os::Linux, "store") => {
                    if !self.drops_store {
                        *self.item.lock().unwrap() = cmd.stdin.clone();
                    }
                    Ok(Out {
                        code: Some(0),
                        ..Out::default()
                    })
                }
                _ => anyhow::bail!("unscripted call {:?}", cmd.args),
            }
        }
    }

    fn linux(f: &Fake) -> Keychain<'_> {
        Keychain {
            os: Os::Linux,
            account: "test".into(),
            runner: f,
        }
    }

    #[test]
    fn a_first_start_that_cannot_see_qdrant_must_not_create_a_key() {
        let down = StoreState::Unreachable("connection refused".into());
        let e = authorize_new_key(&down, false, "default")
            .unwrap_err()
            .to_string();
        assert!(e.contains("refusing to create a new memory key"), "{e}");
        assert!(
            e.contains("--allow-new-key") && e.contains("key import"),
            "{e}"
        );
        // The explicit flag lets a first-time setup through...
        assert!(authorize_new_key(&down, true, "default").is_ok());
        // ...but never over sealed memory that is there.
        let sealed = StoreState::Sealed(3);
        for allow in [false, true] {
            let e = authorize_new_key(&sealed, allow, "work")
                .unwrap_err()
                .to_string();
            assert!(
                e.contains("3 sealed point(s)") && e.contains("wipe --yes --tenant work"),
                "{e}"
            );
        }
        assert!(authorize_new_key(&StoreState::Empty, false, "default").is_ok());
    }

    #[test]
    fn linux_creates_the_key_once_and_reads_it_back() {
        let dir = tempfile::tempdir().unwrap();
        let f = Fake::default();
        let (k, created) = create_key_in(
            KeyStore::Keychain,
            &linux(&f),
            &dir.path().join("memory.key"),
            &dir.path().join("memory.key.lock"),
        )
        .unwrap();
        assert!(created);
        assert_eq!(
            f.item.lock().unwrap().as_deref(),
            Some(ostler_memory::SealKey::encode(&k).as_str())
        );
        // Never a second copy on disk, and the lock is released.
        assert!(!dir.path().join("memory.key").exists());
        assert!(!dir.path().join("memory.key.lock").exists());
        // A second start finds the same key and creates nothing.
        let (k2, created2) = create_key_in(
            KeyStore::Keychain,
            &linux(&f),
            &dir.path().join("memory.key"),
            &dir.path().join("memory.key.lock"),
        )
        .unwrap();
        assert!(!created2 && k2 == k);
        assert_eq!(
            f.log
                .lock()
                .unwrap()
                .iter()
                .filter(|c| c.starts_with("store"))
                .count(),
            1
        );
    }

    #[test]
    fn linux_concurrent_first_starts_agree_on_one_key() {
        let dir = tempfile::tempdir().unwrap();
        let f = Fake {
            slow_lookup: true,
            ..Fake::default()
        };
        let (file, lock) = (
            dir.path().join("memory.key"),
            dir.path().join("memory.key.lock"),
        );
        let results: Vec<([u8; 32], bool)> = std::thread::scope(|s| {
            let hs: Vec<_> = (0..6)
                .map(|_| {
                    s.spawn(|| create_key_in(KeyStore::Keychain, &linux(&f), &file, &lock).unwrap())
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        // One creator, everyone holds the key that is actually stored: without the lock `store` (which
        // replaces) would let each of them keep its own random key.
        assert_eq!(results.iter().filter(|r| r.1).count(), 1);
        assert!(results.iter().all(|r| r.0 == results[0].0));
        let stored = f.item.lock().unwrap().clone().unwrap();
        assert_eq!(
            ostler_memory::SealKey::decode(&stored).unwrap(),
            results[0].0
        );
        assert_eq!(
            f.log
                .lock()
                .unwrap()
                .iter()
                .filter(|c| c.starts_with("store"))
                .count(),
            1
        );
    }

    #[test]
    fn linux_without_a_secret_service_fails_with_the_fallback_and_creates_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (file, lock) = (
            dir.path().join("memory.key"),
            dir.path().join("memory.key.lock"),
        );
        let f = Fake {
            broken: Some("secret-tool: Cannot autolaunch D-Bus without X11 $DISPLAY".into()),
            ..Fake::default()
        };
        let e = create_key_in(KeyStore::Keychain, &linux(&f), &file, &lock)
            .unwrap_err()
            .to_string();
        assert!(e.contains("Cannot autolaunch D-Bus"), "{e}");
        assert!(
            e.contains("key_store file") && e.contains("never falls back"),
            "{e}"
        );
        assert!(f.item.lock().unwrap().is_none());
        assert!(!file.exists(), "fell back to a key file");
        assert!(!lock.exists(), "lock left behind");
        // A tool that is not installed at all.
        let f = Fake {
            missing: true,
            ..Fake::default()
        };
        assert!(create_key_in(KeyStore::Keychain, &linux(&f), &file, &lock).is_err());
        assert!(!file.exists());
    }

    #[test]
    fn linux_store_that_silently_drops_the_secret_is_an_error_not_a_phantom_key() {
        let dir = tempfile::tempdir().unwrap();
        let f = Fake {
            drops_store: true,
            ..Fake::default()
        };
        let e = create_key_in(
            KeyStore::Keychain,
            &linux(&f),
            &dir.path().join("memory.key"),
            &dir.path().join("memory.key.lock"),
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("was not stored"), "{e}");
    }

    #[test]
    fn keychain_mode_with_a_key_file_present_refuses_instead_of_splitting_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("memory.key");
        std::fs::write(&file, "x").unwrap();
        let f = Fake::default();
        let e = create_key_in(KeyStore::Keychain, &linux(&f), &file, &dir.path().join("l"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("split it across two places"), "{e}");
        assert!(
            f.log.lock().unwrap().is_empty(),
            "touched the keychain anyway"
        );
    }

    #[test]
    fn the_file_store_creates_one_private_key_and_a_second_start_reuses_it() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sub").join("memory.key");
        let f = Fake::default();
        let (k, created) =
            create_key_in(KeyStore::File, &linux(&f), &file, &dir.path().join("l")).unwrap();
        assert!(created);
        let (k2, created2) =
            create_key_in(KeyStore::File, &linux(&f), &file, &dir.path().join("l")).unwrap();
        assert!(!created2 && k == k2);
        assert!(
            f.log.lock().unwrap().is_empty(),
            "file mode must not touch the keychain"
        );
    }
}
