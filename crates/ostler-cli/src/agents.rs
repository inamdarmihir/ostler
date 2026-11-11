//! Subagent definitions for the coding agents that support them.
//!
//! `ostler agents` copies the files under `agents/` into the folder a client reads its subagents from.
//! A subagent keeps page text and tool descriptions out of the main conversation, and (Claude Code) gets its
//! own Ostler process, so parallel subagents never share a browser profile.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::ValueEnum;

#[derive(Clone, Copy, PartialEq, Eq, Debug, ValueEnum)]
pub enum AgentClient {
    ClaudeCode,
    Codex,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, ValueEnum)]
pub enum Scope {
    /// Your home folder: every project.
    User,
    /// The current folder: this project only (commit it to share it).
    Project,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Created,
    Updated,
    Unchanged,
    /// A different file is already there; kept (use `--force` to replace it).
    Kept,
    Removed,
    Absent,
    /// Not removed on uninstall because you edited it.
    Edited,
}

impl AgentClient {
    fn files(self) -> &'static [(&'static str, &'static str)] {
        match self {
            AgentClient::ClaudeCode => &[
                (
                    "ostler-researcher.md",
                    include_str!("../agents/claude-code/ostler-researcher.md"),
                ),
                (
                    "ostler-browser.md",
                    include_str!("../agents/claude-code/ostler-browser.md"),
                ),
                (
                    "ostler-form-filler.md",
                    include_str!("../agents/claude-code/ostler-form-filler.md"),
                ),
            ],
            AgentClient::Codex => &[
                (
                    "ostler-researcher.toml",
                    include_str!("../agents/codex/ostler-researcher.toml"),
                ),
                (
                    "ostler-browser.toml",
                    include_str!("../agents/codex/ostler-browser.toml"),
                ),
                (
                    "ostler-form-filler.toml",
                    include_str!("../agents/codex/ostler-form-filler.toml"),
                ),
            ],
        }
    }

    fn folder(self) -> &'static str {
        match self {
            AgentClient::ClaudeCode => ".claude/agents",
            AgentClient::Codex => ".codex/agents",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            AgentClient::ClaudeCode => "claude-code",
            AgentClient::Codex => "codex",
        }
    }
}

/// The folder a client reads subagents from: under `home` for the user scope, under `cwd` for the project scope.
pub fn target_dir(client: AgentClient, scope: Scope, home: &Path, cwd: &Path) -> PathBuf {
    let base = match scope {
        Scope::User => home,
        Scope::Project => cwd,
    };
    base.join(client.folder())
}

/// Copy the subagent files into `dir`. Never overwrites a file that differs unless `force`; writes nothing on `dry_run`.
pub fn install(
    client: AgentClient,
    dir: &Path,
    force: bool,
    dry_run: bool,
) -> Result<Vec<(&'static str, Outcome)>> {
    let mut out = vec![];
    for (name, text) in client.files() {
        let path = dir.join(name);
        let outcome = match std::fs::read_to_string(&path) {
            Ok(have) if have == *text => Outcome::Unchanged,
            Ok(_) if !force => Outcome::Kept,
            Ok(_) => Outcome::Updated,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Outcome::Created,
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        if !dry_run && matches!(outcome, Outcome::Created | Outcome::Updated) {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
            std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
        }
        out.push((*name, outcome));
    }
    Ok(out)
}

/// Remove the files this version installed. A file you edited is left alone.
pub fn uninstall(
    client: AgentClient,
    dir: &Path,
    dry_run: bool,
) -> Result<Vec<(&'static str, Outcome)>> {
    let mut out = vec![];
    for (name, text) in client.files() {
        let path = dir.join(name);
        let outcome = match std::fs::read_to_string(&path) {
            Ok(have) if have == *text => {
                if !dry_run {
                    std::fs::remove_file(&path)
                        .with_context(|| format!("removing {}", path.display()))?;
                }
                Outcome::Removed
            }
            Ok(_) => Outcome::Edited,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Outcome::Absent,
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        out.push((*name, outcome));
    }
    Ok(out)
}

pub fn report(client: AgentClient, dir: &Path, results: &[(&str, Outcome)], dry_run: bool) {
    let would = if dry_run { "would be " } else { "" };
    for (name, o) in results {
        let what = match o {
            Outcome::Created => format!("{would}created"),
            Outcome::Updated => format!("{would}replaced"),
            Outcome::Unchanged => "already up to date".into(),
            Outcome::Kept => {
                "kept: a different file is already there (use --force to replace it)".into()
            }
            Outcome::Removed => format!("{would}removed"),
            Outcome::Absent => "not installed".into(),
            Outcome::Edited => "kept: you changed it".into(),
        };
        println!("  {name}: {what}");
    }
    println!("{} subagents in {}", client.name(), dir.display());
    if results
        .iter()
        .any(|(_, o)| matches!(o, Outcome::Created | Outcome::Updated))
        && !dry_run
    {
        match client {
            AgentClient::ClaudeCode => println!(
                "Claude Code picks them up within seconds. Each subagent starts its own `ostler` (it must be on your PATH) with its own browser."
            ),
            AgentClient::Codex => println!(
                "Enable multi-agent in Codex if it is not already on, and register Ostler first: `ostler install --client codex`."
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_dirs_follow_each_clients_convention() {
        let (home, cwd) = (Path::new("/h"), Path::new("/p"));
        assert_eq!(
            target_dir(AgentClient::ClaudeCode, Scope::User, home, cwd),
            Path::new("/h/.claude/agents")
        );
        assert_eq!(
            target_dir(AgentClient::Codex, Scope::Project, home, cwd),
            Path::new("/p/.codex/agents")
        );
    }

    #[test]
    fn install_is_idempotent_and_never_overwrites_an_edited_file() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("agents");
        let first = install(AgentClient::ClaudeCode, &dir, false, false).unwrap();
        assert!(first.iter().all(|(_, o)| *o == Outcome::Created));
        let again = install(AgentClient::ClaudeCode, &dir, false, false).unwrap();
        assert!(again.iter().all(|(_, o)| *o == Outcome::Unchanged));

        let file = dir.join("ostler-browser.md");
        std::fs::write(&file, "mine").unwrap();
        let kept = install(AgentClient::ClaudeCode, &dir, false, false).unwrap();
        assert!(kept.contains(&("ostler-browser.md", Outcome::Kept)));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "mine");
        let forced = install(AgentClient::ClaudeCode, &dir, true, false).unwrap();
        assert!(forced.contains(&("ostler-browser.md", Outcome::Updated)));
        assert_ne!(std::fs::read_to_string(&file).unwrap(), "mine");
    }

    #[test]
    fn dry_run_writes_nothing_and_uninstall_keeps_edits() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("agents");
        let plan = install(AgentClient::Codex, &dir, false, true).unwrap();
        assert!(plan.iter().all(|(_, o)| *o == Outcome::Created));
        assert!(!dir.exists());

        install(AgentClient::Codex, &dir, false, false).unwrap();
        std::fs::write(dir.join("ostler-researcher.toml"), "edited").unwrap();
        let gone = uninstall(AgentClient::Codex, &dir, false).unwrap();
        assert!(gone.contains(&("ostler-researcher.toml", Outcome::Edited)));
        assert!(gone.contains(&("ostler-browser.toml", Outcome::Removed)));
        assert!(dir.join("ostler-researcher.toml").exists());
        assert!(!dir.join("ostler-browser.toml").exists());
    }

    #[test]
    fn shipped_files_are_well_formed() {
        for (name, text) in AgentClient::ClaudeCode.files() {
            let body = text
                .strip_prefix("---\n")
                .unwrap_or_else(|| panic!("{name}: no frontmatter"));
            let (front, prompt) = body
                .split_once("\n---\n")
                .unwrap_or_else(|| panic!("{name}: unclosed frontmatter"));
            let n = name.trim_end_matches(".md");
            assert!(front.contains(&format!("name: {n}")), "{name}");
            assert!(front.contains("description: "), "{name}");
            assert!(front.contains("command: ostler"), "{name}");
            assert!(
                prompt.contains("untrusted"),
                "{name}: missing the prompt-injection rule"
            );
        }
        for (name, text) in AgentClient::Codex.files() {
            let v: toml::Table = text.parse().unwrap_or_else(|e| panic!("{name}: {e}"));
            for key in ["name", "description", "developer_instructions"] {
                assert!(
                    v.get(key).and_then(|x| x.as_str()).is_some(),
                    "{name}: {key}"
                );
            }
            assert!(
                v["developer_instructions"]
                    .as_str()
                    .unwrap()
                    .contains("untrusted"),
                "{name}"
            );
        }
    }
}
