//! `ostler install`: register (or remove) the MCP server entry in an MCP client's own config file.
//!
//! Safety rules, all enforced here and covered by tests:
//! - the file is parsed first; malformed JSON/TOML is refused, never overwritten;
//! - every other entry in the file is preserved (JSON is re-serialised in key order; TOML is edited as text
//!   so comments and layout survive, then re-parsed to prove nothing else moved);
//! - an existing file is copied to a timestamped backup before it is changed;
//! - re-running is a no-op ("already installed"), and `--dry-run` writes nothing;
//! - paths for an OS we do not know are reported as unsupported, never guessed.
//!
//! `OSTLER_INSTALL_HOME` replaces the home directory (tests, sandboxes).

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::ValueEnum;
use serde_json::{json, Map, Value};

#[derive(Clone, Copy, PartialEq, Eq, Debug, ValueEnum)]
pub enum InstallClient {
    ClaudeCode,
    ClaudeDesktop,
    Cursor,
    Vscode,
    Windsurf,
    Codex,
}

impl InstallClient {
    pub const ALL: [InstallClient; 6] = [
        Self::ClaudeCode,
        Self::ClaudeDesktop,
        Self::Cursor,
        Self::Vscode,
        Self::Windsurf,
        Self::Codex,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::ClaudeDesktop => "claude-desktop",
            Self::Cursor => "cursor",
            Self::Vscode => "vscode",
            Self::Windsurf => "windsurf",
            Self::Codex => "codex",
        }
    }

    /// The restart hint printed after a change.
    fn restart(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Restart Claude Code (or run /mcp), then ask it to use ostler.",
            Self::ClaudeDesktop => "Quit and reopen Claude Desktop.",
            Self::Cursor => "Reload Cursor (Settings > MCP shows ostler).",
            Self::Vscode => "Reload VS Code; start the server from the MCP: List Servers command if it is not running.",
            Self::Windsurf => "Refresh MCP servers in Windsurf (Cascade > MCP) or restart it.",
            Self::Codex => "Start a new Codex session.",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Os {
    Mac,
    Linux,
    Windows,
}

impl Os {
    pub fn current() -> Option<Os> {
        if cfg!(target_os = "macos") {
            Some(Os::Mac)
        } else if cfg!(target_os = "linux") {
            Some(Os::Linux)
        } else if cfg!(windows) {
            Some(Os::Windows)
        } else {
            None
        }
    }
}

/// Where config files live. Built from the environment, or by hand in tests.
#[derive(Clone, Debug)]
pub struct Paths {
    pub os: Option<Os>,
    pub home: PathBuf,
    /// `%APPDATA%` on Windows.
    pub appdata: Option<PathBuf>,
}

impl Paths {
    pub fn from_env() -> Result<Paths> {
        let overridden = std::env::var_os("OSTLER_INSTALL_HOME").filter(|v| !v.is_empty());
        let home = match &overridden {
            Some(h) => PathBuf::from(h),
            None => std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .context("cannot find your home directory: set HOME, or OSTLER_INSTALL_HOME to the directory to use")?,
        };
        let appdata = if overridden.is_some() {
            Some(home.join("AppData").join("Roaming"))
        } else {
            std::env::var_os("APPDATA").map(PathBuf::from)
        };
        Ok(Paths {
            os: Os::current(),
            home,
            appdata,
        })
    }

    /// The config file for a client, or why there is none on this OS.
    pub fn config_path(&self, c: InstallClient) -> std::result::Result<PathBuf, String> {
        let os = self
            .os
            .ok_or_else(|| "this operating system is not supported by `ostler install`; use `ostler setup <client>` and paste the snippet".to_string())?;
        let app_support = |app: &str, file: &str| -> std::result::Result<PathBuf, String> {
            match os {
                Os::Mac => Ok(self.home.join("Library/Application Support").join(app).join(file)),
                Os::Windows => self
                    .appdata
                    .clone()
                    .map(|a| a.join(app).join(file))
                    .ok_or_else(|| "APPDATA is not set".to_string()),
                Os::Linux => Err(format!("{app} has no official Linux config location; use `ostler setup` and paste the snippet")),
            }
        };
        match c {
            InstallClient::ClaudeCode => Ok(self.home.join(".claude.json")),
            InstallClient::ClaudeDesktop => app_support("Claude", "claude_desktop_config.json"),
            InstallClient::Cursor => Ok(self.home.join(".cursor").join("mcp.json")),
            InstallClient::Windsurf => Ok(self
                .home
                .join(".codeium")
                .join("windsurf")
                .join("mcp_config.json")),
            InstallClient::Codex => Ok(self.home.join(".codex").join("config.toml")),
            InstallClient::Vscode => match os {
                Os::Linux => Ok(self.home.join(".config/Code/User/mcp.json")),
                _ => app_support("Code", "User/mcp.json"),
            },
        }
    }
}

/// What to register: the absolute path of this binary and the `serve` arguments.
#[derive(Clone, Debug)]
pub struct Spec {
    pub exe: String,
    pub args: Vec<String>,
}

impl Spec {
    fn args_json(&self) -> Value {
        Value::Array(self.args.iter().map(|a| Value::String(a.clone())).collect())
    }

    /// Fields we own in the client's JSON entry. Other fields the user added (env, disabled) are kept.
    fn json_fields(&self, c: InstallClient) -> Map<String, Value> {
        let mut m = Map::new();
        if matches!(c, InstallClient::ClaudeCode | InstallClient::Vscode) {
            m.insert("type".into(), json!("stdio"));
        }
        m.insert("command".into(), json!(self.exe));
        m.insert("args".into(), self.args_json());
        m
    }
}

fn json_container(c: InstallClient) -> &'static str {
    if c == InstallClient::Vscode {
        "servers"
    } else {
        "mcpServers"
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Install,
    Uninstall,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Added,
    Updated,
    Removed,
    /// Install found the exact entry already there.
    Unchanged,
    /// Uninstall found no entry (or no file).
    NotPresent,
}

#[derive(Debug)]
pub struct Outcome {
    pub path: PathBuf,
    pub action: Action,
    pub before: Option<String>,
    pub after: Option<String>,
    /// Other entries in the same container, left alone.
    pub preserved: usize,
    pub backup: Option<PathBuf>,
    pub created_file: bool,
    pub wrote: bool,
}

struct Plan {
    action: Action,
    before: Option<String>,
    after: Option<String>,
    preserved: usize,
    new_text: Option<String>,
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap()
}

fn plan_json(c: InstallClient, text: Option<&str>, spec: &Spec, mode: Mode) -> Result<Plan> {
    let mut root: Value = match text {
        None => json!({}),
        Some(t) if t.trim().is_empty() => json!({}),
        Some(t) => serde_json::from_str(t).map_err(|e| {
            anyhow::anyhow!("the file is not valid JSON ({e}); it may contain comments or a typo. Nothing was changed. Fix or move it, or add the entry by hand (see `ostler setup {}`)", c.name())
        })?,
    };
    let key = json_container(c);
    let obj = root
        .as_object_mut()
        .context("the file's top level is not a JSON object; refusing to touch it")?;
    if let Some(v) = obj.get(key) {
        if !v.is_object() {
            bail!("`{key}` in the file is not an object; refusing to touch it");
        }
    }
    let existing = obj.get(key).and_then(|v| v.get("ostler")).cloned();
    let preserved = obj
        .get(key)
        .and_then(Value::as_object)
        .map_or(0, |m| m.keys().filter(|k| *k != "ostler").count());
    match mode {
        Mode::Install => {
            let mut entry = existing
                .clone()
                .filter(Value::is_object)
                .unwrap_or_else(|| json!({}));
            let ours = spec.json_fields(c);
            let same = ours.iter().all(|(k, v)| entry.get(k) == Some(v));
            if existing.is_some() && same {
                return Ok(Plan {
                    action: Action::Unchanged,
                    before: existing.as_ref().map(pretty),
                    after: existing.as_ref().map(pretty),
                    preserved,
                    new_text: None,
                });
            }
            for (k, v) in ours {
                entry[&k] = v;
            }
            let container = obj
                .entry(key.to_string())
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .unwrap();
            container.insert("ostler".into(), entry.clone());
            Ok(Plan {
                action: if existing.is_some() {
                    Action::Updated
                } else {
                    Action::Added
                },
                before: existing.as_ref().map(pretty),
                after: Some(pretty(&entry)),
                preserved,
                new_text: Some(pretty(&root) + "\n"),
            })
        }
        Mode::Uninstall => {
            let Some(old) = existing else {
                return Ok(Plan {
                    action: Action::NotPresent,
                    before: None,
                    after: None,
                    preserved,
                    new_text: None,
                });
            };
            obj.get_mut(key)
                .and_then(Value::as_object_mut)
                .unwrap()
                .shift_remove("ostler");
            Ok(Plan {
                action: Action::Removed,
                before: Some(pretty(&old)),
                after: None,
                preserved,
                new_text: Some(pretty(&root) + "\n"),
            })
        }
    }
}

// ---- TOML (Codex) ----

fn is_header(line: &str) -> bool {
    // A table header line: `[a.b]` or `[[a.b]]`, optionally followed by a comment.
    let t = line.split('#').next().unwrap_or("").trim();
    t.len() > 2 && t.starts_with('[') && t.ends_with(']') && !t.contains('=')
}

fn header_name(line: &str) -> String {
    line.trim()
        .split('#')
        .next()
        .unwrap_or("")
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim()
        .to_string()
}

/// Line ranges of `[mcp_servers.ostler]` (and with `with_subtables` also `[mcp_servers.ostler.*]`).
fn toml_spans(text: &str, with_subtables: bool) -> Vec<(usize, usize)> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if is_header(lines[i]) {
            let name = header_name(lines[i]);
            let ours = name == "mcp_servers.ostler"
                || (with_subtables && name.starts_with("mcp_servers.ostler."));
            if ours && !lines[i].trim().starts_with("[[") {
                let mut j = i + 1;
                while j < lines.len() && !is_header(lines[j]) {
                    j += 1;
                }
                spans.push((i, j));
                i = j;
                continue;
            }
        }
        i += 1;
    }
    spans
}

fn toml_block(spec: &Spec) -> String {
    let s = |v: &str| toml::Value::String(v.to_string()).to_string();
    format!(
        "[mcp_servers.ostler]\ncommand = {}\nargs = [{}]\n",
        s(&spec.exe),
        spec.args
            .iter()
            .map(|a| s(a))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn plan_toml(text: Option<&str>, spec: &Spec, mode: Mode) -> Result<Plan> {
    let src = text.unwrap_or("");
    let doc: toml::Table = src.parse().map_err(|e| {
        anyhow::anyhow!("the file is not valid TOML ({e}). Nothing was changed. Fix or move it, or add the entry by hand (see `ostler setup codex`)")
    })?;
    let servers = match doc.get("mcp_servers") {
        None => None,
        Some(toml::Value::Table(t)) => Some(t),
        Some(_) => bail!("`mcp_servers` in the file is not a table; refusing to touch it"),
    };
    let existing = servers.and_then(|t| t.get("ostler")).cloned();
    let preserved = servers.map_or(0, |t| t.keys().filter(|k| *k != "ostler").count());
    let show = |v: &toml::Value| {
        let mut t = toml::Table::new();
        t.insert("ostler".into(), v.clone());
        let mut root = toml::Table::new();
        root.insert("mcp_servers".into(), toml::Value::Table(t));
        toml::to_string(&root)
            .unwrap_or_default()
            .trim_end()
            .to_string()
    };
    let spans = toml_spans(src, mode == Mode::Uninstall);
    if existing.is_some() && toml_spans(src, false).is_empty() {
        bail!("an `ostler` entry exists but is written in a form this tool will not edit (inline table or dotted keys). Edit {} by hand, or remove it and re-run", "the file");
    }
    let lines: Vec<&str> = src.split_inclusive('\n').collect();
    let want_args: Vec<toml::Value> = spec
        .args
        .iter()
        .map(|a| toml::Value::String(a.clone()))
        .collect();
    let (action, new_text) = match mode {
        Mode::Install => {
            let same = existing.as_ref().is_some_and(|e| {
                e.get("command").and_then(|v| v.as_str()) == Some(&spec.exe)
                    && e.get("args").and_then(|v| v.as_array()) == Some(&want_args)
            });
            if same {
                (Action::Unchanged, None)
            } else if existing.is_some() {
                // Replace the `command`/`args` lines inside the main table; keep any other keys and comments.
                let (a, b) = spans[0];
                let mut keep: Vec<&str> = lines[a + 1..b]
                    .iter()
                    .copied()
                    .filter(|l| {
                        let k = l.trim_start();
                        !(k.starts_with("command") && k[7..].trim_start().starts_with('='))
                            && !(k.starts_with("args") && k[4..].trim_start().starts_with('='))
                    })
                    .collect();
                // multi-line `args = [` arrays would leave stray lines; verification below catches that.
                let block = toml_block(spec);
                let mut out: Vec<String> = lines[..a].iter().map(|s| s.to_string()).collect();
                out.push(block);
                out.extend(keep.drain(..).map(|s| s.to_string()));
                out.extend(lines[b..].iter().map(|s| s.to_string()));
                (Action::Updated, Some(out.concat()))
            } else {
                let mut t = src.to_string();
                if !t.is_empty() && !t.ends_with('\n') {
                    t.push('\n');
                }
                if !t.is_empty() {
                    t.push('\n');
                }
                t.push_str(&toml_block(spec));
                (Action::Added, Some(t))
            }
        }
        Mode::Uninstall => {
            if existing.is_none() {
                (Action::NotPresent, None)
            } else {
                let mut out = String::new();
                'l: for (i, l) in lines.iter().enumerate() {
                    for (a, b) in &spans {
                        if i >= *a && i < *b {
                            continue 'l;
                        }
                    }
                    out.push_str(l);
                }
                (
                    Action::Removed,
                    Some(
                        out.trim_end().to_string() + if out.trim().is_empty() { "" } else { "\n" },
                    ),
                )
            }
        }
    };
    // Prove the edit did what we said and nothing else.
    if let Some(nt) = &new_text {
        let after: toml::Table = nt.parse().context(
            "internal check failed: the edited file does not parse; nothing was written",
        )?;
        let mut a = after.clone();
        let mut b = doc.clone();
        let strip = |t: &mut toml::Table| {
            if let Some(toml::Value::Table(s)) = t.get_mut("mcp_servers") {
                s.remove("ostler");
                if s.is_empty() {
                    t.remove("mcp_servers");
                }
            }
        };
        strip(&mut a);
        strip(&mut b);
        if a != b {
            bail!("internal check failed: the edit would change other settings; nothing was written. Edit the file by hand (see `ostler setup codex`)");
        }
        let got = after.get("mcp_servers").and_then(|v| v.get("ostler"));
        let ok = match mode {
            Mode::Install => got.is_some_and(|e| {
                e.get("command").and_then(|v| v.as_str()) == Some(&spec.exe)
                    && e.get("args").and_then(|v| v.as_array()) == Some(&want_args)
            }),
            Mode::Uninstall => got.is_none(),
        };
        if !ok {
            bail!("internal check failed: the edited entry is not what was intended; nothing was written. Edit the file by hand (see `ostler setup codex`)");
        }
    }
    let after_entry = new_text
        .as_deref()
        .and_then(|t| t.parse::<toml::Table>().ok())
        .and_then(|t| t.get("mcp_servers").and_then(|v| v.get("ostler")).cloned())
        .map(|v| show(&v));
    Ok(Plan {
        action,
        before: existing.as_ref().map(show),
        after: if action == Action::Unchanged {
            existing.as_ref().map(show)
        } else {
            after_entry
        },
        preserved,
        new_text,
    })
}

// ---- files ----

fn backup_path(path: &Path) -> PathBuf {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut n = 0;
    loop {
        let suffix = if n == 0 {
            String::new()
        } else {
            format!("-{n}")
        };
        let p = path.with_file_name(format!("{name}.ostler-backup-{secs}{suffix}"));
        if !p.exists() {
            return p;
        }
        n += 1;
    }
}

/// Write `text` next to `path` and rename over it, keeping the original's permissions.
fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let dir = path
        .parent()
        .context("config path has no parent directory")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let tmp = path.with_file_name(format!(
        ".{}.ostler-tmp-{}",
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        std::process::id()
    ));
    std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("replacing {}", path.display()));
    }
    Ok(())
}

/// Install or uninstall for one client under `paths`. With `dry_run` nothing is written.
pub fn apply(
    c: InstallClient,
    paths: &Paths,
    spec: &Spec,
    mode: Mode,
    dry_run: bool,
) -> Result<Outcome> {
    let path = paths.config_path(c).map_err(anyhow::Error::msg)?;
    // Follow a symlinked config (dotfile managers) so the link survives.
    let target = if path.exists() {
        std::fs::canonicalize(&path).unwrap_or(path.clone())
    } else {
        path.clone()
    };
    let existed = target.exists();
    let text = if existed {
        Some(
            std::fs::read_to_string(&target)
                .with_context(|| format!("reading {}", target.display()))?,
        )
    } else {
        None
    };
    let plan = if c == InstallClient::Codex {
        plan_toml(text.as_deref(), spec, mode)
    } else {
        plan_json(c, text.as_deref(), spec, mode)
    }
    .with_context(|| format!("{}", path.display()))?;
    let mut out = Outcome {
        path: path.clone(),
        action: plan.action,
        before: plan.before,
        after: plan.after,
        preserved: plan.preserved,
        backup: None,
        created_file: false,
        wrote: false,
    };
    if let (Some(new_text), false) = (plan.new_text, dry_run) {
        if existed {
            let b = backup_path(&target);
            std::fs::copy(&target, &b).with_context(|| format!("backing up to {}", b.display()))?;
            out.backup = Some(b);
        } else {
            out.created_file = true;
        }
        write_atomic(&target, &new_text)?;
        out.wrote = true;
    }
    Ok(out)
}

/// Whether `ostler` is registered for a client, for `--list`.
fn status(c: InstallClient, paths: &Paths, spec: &Spec) -> String {
    let path = match paths.config_path(c) {
        Ok(p) => p,
        Err(why) => return format!("unsupported here: {why}"),
    };
    if !path.exists() {
        return format!(
            "{}  (no config file yet; `install` will create it)",
            path.display()
        );
    }
    match apply(c, paths, spec, Mode::Install, true) {
        Ok(o) => {
            let s = match o.action {
                Action::Unchanged => "installed (matches this binary)",
                Action::Updated => {
                    "installed, but differs from this binary (install will update it)"
                }
                _ => "not installed",
            };
            format!("{}  [{s}]", path.display())
        }
        Err(e) => format!("{}  [cannot read safely: {e:#}]", path.display()),
    }
}

pub fn list(paths: &Paths, spec: &Spec) {
    println!("Clients `ostler install --client <name>` can register with:\n");
    for c in InstallClient::ALL {
        println!("  {:<15} {}", c.name(), status(c, paths, spec));
    }
    println!(
        "\nAdd --dry-run to preview, --uninstall to remove. Other clients: `ostler setup json`."
    );
}

pub fn report(c: InstallClient, o: &Outcome, mode: Mode, dry_run: bool) {
    let dry = if dry_run {
        " (dry run: nothing written)"
    } else {
        ""
    };
    let verb = match o.action {
        Action::Added => "Added the ostler entry",
        Action::Updated => "Updated the ostler entry",
        Action::Removed => "Removed the ostler entry",
        Action::Unchanged => "Already installed, nothing to change",
        Action::NotPresent => "No ostler entry found, nothing to remove",
    };
    println!("{verb}: {}{dry}", o.path.display());
    if let Some(b) = &o.before {
        println!("--- before\n{b}");
    }
    if let Some(a) = &o.after {
        if o.action != Action::Unchanged {
            println!("+++ after\n{a}");
        }
    }
    if o.action != Action::NotPresent {
        println!("Other entries left untouched: {}", o.preserved);
    }
    if let Some(b) = &o.backup {
        println!("Backup of the previous file: {}", b.display());
    } else if o.created_file {
        println!("The file did not exist, so it was created (no backup needed).");
    }
    if o.wrote {
        println!("\n{}", c.restart());
        if mode == Mode::Install {
            println!("Zero-config: memory is in-process and nothing needs Qdrant. First demo: `ostler fixtures`.");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(dir: &Path, os: Os) -> Paths {
        Paths {
            os: Some(os),
            home: dir.to_path_buf(),
            appdata: Some(dir.join("AppData/Roaming")),
        }
    }
    fn spec() -> Spec {
        Spec {
            exe: "/opt/ostler".into(),
            args: vec!["serve".into()],
        }
    }
    fn read(p: &Path) -> String {
        std::fs::read_to_string(p).unwrap()
    }
    fn backups(dir: &Path) -> usize {
        std::fs::read_dir(dir)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains("ostler-backup")
            })
            .count()
    }

    #[test]
    fn paths_per_os() {
        let d = Path::new("/h");
        let m = paths(d, Os::Mac);
        assert_eq!(
            m.config_path(InstallClient::ClaudeDesktop).unwrap(),
            Path::new("/h/Library/Application Support/Claude/claude_desktop_config.json")
        );
        assert_eq!(
            m.config_path(InstallClient::Vscode).unwrap(),
            Path::new("/h/Library/Application Support/Code/User/mcp.json")
        );
        let l = paths(d, Os::Linux);
        assert!(l
            .config_path(InstallClient::ClaudeDesktop)
            .unwrap_err()
            .contains("Linux"));
        assert_eq!(
            l.config_path(InstallClient::Vscode).unwrap(),
            Path::new("/h/.config/Code/User/mcp.json")
        );
        let w = paths(d, Os::Windows);
        assert_eq!(
            w.config_path(InstallClient::ClaudeDesktop).unwrap(),
            Path::new("/h/AppData/Roaming/Claude/claude_desktop_config.json")
        );
        let unknown = Paths {
            os: None,
            home: d.into(),
            appdata: None,
        };
        assert!(unknown
            .config_path(InstallClient::Cursor)
            .unwrap_err()
            .contains("not supported"));
        assert_eq!(
            l.config_path(InstallClient::Codex).unwrap(),
            Path::new("/h/.codex/config.toml")
        );
    }

    #[test]
    fn json_install_is_idempotent_and_preserves_others() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path(), Os::Linux);
        let f = d.path().join(".cursor/mcp.json");
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(
            &f,
            r#"{"theme":"dark","mcpServers":{"other":{"command":"x","args":["1"]}}}"#,
        )
        .unwrap();
        let o = apply(InstallClient::Cursor, &p, &spec(), Mode::Install, false).unwrap();
        assert_eq!(o.action, Action::Added);
        assert_eq!(o.preserved, 1);
        assert!(o.backup.as_ref().unwrap().exists());
        let v: Value = serde_json::from_str(&read(&f)).unwrap();
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["mcpServers"]["other"]["command"], "x");
        assert_eq!(v["mcpServers"]["ostler"]["command"], "/opt/ostler");
        assert_eq!(v["mcpServers"]["ostler"]["args"], json!(["serve"]));
        // second run: no change, no new backup
        let before = read(&f);
        let o = apply(InstallClient::Cursor, &p, &spec(), Mode::Install, false).unwrap();
        assert_eq!(o.action, Action::Unchanged);
        assert!(!o.wrote);
        assert_eq!(read(&f), before);
        assert_eq!(backups(f.parent().unwrap()), 1);
    }

    #[test]
    fn json_update_keeps_user_fields_and_dry_run_writes_nothing() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path(), Os::Linux);
        let f = d.path().join(".claude.json");
        std::fs::write(&f, r#"{"mcpServers":{"ostler":{"type":"stdio","command":"/old","args":["serve"],"env":{"A":"1"}}}}"#).unwrap();
        let o = apply(InstallClient::ClaudeCode, &p, &spec(), Mode::Install, true).unwrap();
        assert_eq!(o.action, Action::Updated);
        assert!(!o.wrote && o.backup.is_none());
        assert!(read(&f).contains("/old"));
        apply(InstallClient::ClaudeCode, &p, &spec(), Mode::Install, false).unwrap();
        let v: Value = serde_json::from_str(&read(&f)).unwrap();
        assert_eq!(v["mcpServers"]["ostler"]["command"], "/opt/ostler");
        assert_eq!(v["mcpServers"]["ostler"]["env"]["A"], "1");
    }

    #[test]
    fn json_creates_missing_file_and_uninstalls() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path(), Os::Mac);
        let o = apply(InstallClient::Vscode, &p, &spec(), Mode::Install, false).unwrap();
        assert!(o.created_file && o.backup.is_none());
        let v: Value = serde_json::from_str(&read(&o.path)).unwrap();
        assert_eq!(v["servers"]["ostler"]["type"], "stdio");
        let o = apply(InstallClient::Vscode, &p, &spec(), Mode::Uninstall, false).unwrap();
        assert_eq!(o.action, Action::Removed);
        assert!(o.backup.is_some());
        let v: Value = serde_json::from_str(&read(&o.path)).unwrap();
        assert!(v["servers"].get("ostler").is_none());
        let o = apply(InstallClient::Vscode, &p, &spec(), Mode::Uninstall, false).unwrap();
        assert_eq!(o.action, Action::NotPresent);
        assert!(!o.wrote);
    }

    #[test]
    fn malformed_or_odd_json_is_refused_untouched() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path(), Os::Linux);
        let f = d.path().join(".codeium/windsurf/mcp_config.json");
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        for bad in [
            "{ not json",
            "// comment\n{}",
            "[1,2]",
            r#"{"mcpServers":[1]}"#,
        ] {
            std::fs::write(&f, bad).unwrap();
            let e = apply(InstallClient::Windsurf, &p, &spec(), Mode::Install, false).unwrap_err();
            assert!(!format!("{e:#}").is_empty());
            assert_eq!(read(&f), bad);
            assert_eq!(backups(f.parent().unwrap()), 0);
        }
        std::fs::write(&f, "  \n").unwrap();
        assert_eq!(
            apply(InstallClient::Windsurf, &p, &spec(), Mode::Install, false)
                .unwrap()
                .action,
            Action::Added
        );
    }

    #[cfg(unix)]
    #[test]
    fn permissions_survive() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path(), Os::Linux);
        let f = d.path().join(".claude.json");
        std::fs::write(&f, "{}").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600)).unwrap();
        apply(InstallClient::ClaudeCode, &p, &spec(), Mode::Install, false).unwrap();
        assert_eq!(
            std::fs::metadata(&f).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn toml_add_update_remove_keeps_comments() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path(), Os::Linux);
        let f = d.path().join(".codex/config.toml");
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        let original =
            "# my codex\nmodel = \"o3\"\n\n[mcp_servers.other]\ncommand = \"x\" # keep\n";
        std::fs::write(&f, original).unwrap();
        let o = apply(InstallClient::Codex, &p, &spec(), Mode::Install, false).unwrap();
        assert_eq!(o.action, Action::Added);
        let t = read(&f);
        assert!(t.starts_with(original), "{t}");
        assert!(t.contains("[mcp_servers.ostler]\ncommand = \"/opt/ostler\"\nargs = [\"serve\"]"));
        assert_eq!(
            apply(InstallClient::Codex, &p, &spec(), Mode::Install, false)
                .unwrap()
                .action,
            Action::Unchanged
        );
        // update after the binary moved
        let moved = Spec {
            exe: "/new/ostler".into(),
            args: vec!["serve".into(), "--lean".into()],
        };
        let o = apply(InstallClient::Codex, &p, &moved, Mode::Install, false).unwrap();
        assert_eq!(o.action, Action::Updated);
        let t = read(&f);
        assert!(t.contains("/new/ostler") && !t.contains("/opt/ostler") && t.contains("# keep"));
        let o = apply(InstallClient::Codex, &p, &moved, Mode::Uninstall, false).unwrap();
        assert_eq!(o.action, Action::Removed);
        assert_eq!(read(&f).trim_end(), original.trim_end());
    }

    #[test]
    fn toml_refuses_malformed_and_inline_forms() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path(), Os::Linux);
        let f = d.path().join(".codex/config.toml");
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        for bad in [
            "model = ",
            "[mcp_servers]\nostler = { command = \"x\" }\n",
            "mcp_servers = 3\n",
        ] {
            std::fs::write(&f, bad).unwrap();
            assert!(
                apply(InstallClient::Codex, &p, &spec(), Mode::Install, false).is_err(),
                "{bad}"
            );
            assert_eq!(read(&f), bad);
        }
    }

    #[test]
    fn toml_uninstall_removes_subtables_too() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path(), Os::Linux);
        let f = d.path().join(".codex/config.toml");
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(&f, "[mcp_servers.ostler]\ncommand = \"a\"\nargs = []\n[mcp_servers.ostler.env]\nA = \"1\"\n[mcp_servers.keep]\ncommand = \"k\"\n").unwrap();
        apply(InstallClient::Codex, &p, &spec(), Mode::Uninstall, false).unwrap();
        assert_eq!(read(&f), "[mcp_servers.keep]\ncommand = \"k\"\n");
    }

    #[test]
    fn list_status_text() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path(), Os::Linux);
        assert!(status(InstallClient::ClaudeDesktop, &p, &spec()).starts_with("unsupported"));
        assert!(status(InstallClient::Cursor, &p, &spec()).contains("no config file yet"));
        apply(InstallClient::Cursor, &p, &spec(), Mode::Install, false).unwrap();
        assert!(status(InstallClient::Cursor, &p, &spec()).contains("installed (matches"));
    }
}
