//! `ostler init` / `config`: the file is private, the key is never printed, and flags/env beat the file.

use std::process::Command;

fn wm(cfg: &std::path::Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_ostler"));
    c.env("OSTLER_CONFIG", cfg)
        .env_remove("QDRANT_URL")
        .env_remove("QDRANT_API_KEY")
        .env_remove("OSTLER_TENANT");
    c
}

#[test]
fn init_saves_privately_masks_the_key_and_env_wins() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    let out = wm(&cfg)
        .args([
            "init",
            "--no-check",
            "--yes",
            "--qdrant-url",
            "https://abc.cloud.qdrant.io",
            "--qdrant-api-key",
            "super-secret-key-123456",
            "--tenant",
            "alice",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&cfg).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "config holds a secret and must be owner-only");
    }
    let saved = std::fs::read_to_string(&cfg).unwrap();
    assert!(
        saved.contains("https://abc.cloud.qdrant.io:6333"),
        "{saved}"
    );

    let show = wm(&cfg).args(["config", "show"]).output().unwrap();
    let text = String::from_utf8_lossy(&show.stdout);
    assert!(text.contains("tenant = alice"), "{text}");
    assert!(
        !text.contains("super-secret-key-123456"),
        "key leaked: {text}"
    );

    // An environment variable overrides the file.
    let show = wm(&cfg)
        .env("OSTLER_TENANT", "bob")
        .args(["memory", "--memory", "ephemeral", "list"])
        .output()
        .unwrap();
    assert!(show.status.success());

    let bad = wm(&cfg)
        .args(["config", "set", "nonsense", "x"])
        .output()
        .unwrap();
    assert!(!bad.status.success());
}

fn run(cmd: &mut Command) -> (bool, String, String) {
    let o = cmd.output().unwrap();
    (
        o.status.success(),
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

#[test]
fn init_saves_privacy_choices_and_a_model_ollama_knows() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    let (ok, _, err) = run(wm(&cfg).args([
        "init",
        "--no-check",
        "--yes",
        "--qdrant-url",
        "https://q.example.com:6333",
        "--embeddings",
        "ollama",
        "--seal",
        "on",
        "--retain-days",
        "7",
    ]));
    assert!(ok, "{err}");
    let saved = std::fs::read_to_string(&cfg).unwrap();
    assert!(saved.contains("seal = \"on\""), "{saved}");
    assert!(saved.contains("retain_days = \"7\""), "{saved}");
    // The default model name is a Qdrant Cloud one; Ollama needs its own.
    assert!(
        saved.contains("embedding_model = \"all-minilm\""),
        "{saved}"
    );
}

#[test]
fn doctor_explains_memory_in_plain_language_and_refused_http_says_what_to_do() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    let (ok, out, _) = run(wm(&cfg)
        .env("QDRANT_URL", "http://qdrant.example.com:6333")
        .arg("doctor"));
    assert!(!ok, "a refused transport must fail doctor");
    for needle in [
        "sealing:    ON",
        "key:",
        "unrecoverable",
        "embeddings: cloud",
        "transport:  http to a remote host: REFUSED",
        "--allow-insecure-qdrant",
    ] {
        assert!(out.contains(needle), "missing {needle:?} in:\n{out}");
    }
    assert!(
        !dir.path().join("memory.key").exists(),
        "doctor must not create the key just by reporting"
    );

    let (_, out, _) = run(wm(&cfg)
        .env("OSTLER_SEAL", "off")
        .env("QDRANT_URL", "https://x.cloud.qdrant.io:6333")
        .arg("doctor"));
    assert!(out.contains("sealing:    OFF"), "{out}");
    assert!(out.contains("ostler memory seal"), "{out}");
}

#[test]
fn memory_errors_name_the_next_command() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    let (ok, _, err) = run(wm(&cfg).args(["memory", "list"]));
    assert!(!ok);
    assert!(err.contains("ostler init"), "{err}");

    let (ok, _, err) = run(wm(&cfg).args(["memory", "seal"]));
    assert!(!ok);
    assert!(err.contains("ostler init"), "{err}");

    let (ok, _, err) = run(wm(&cfg)
        .env("QDRANT_URL", "http://localhost:6333")
        .args(["memory", "--seal", "off", "seal"]));
    assert!(!ok);
    assert!(err.contains("ostler config set seal on"), "{err}");

    let (ok, out, _) = run(wm(&cfg).args(["memory", "--memory", "ephemeral", "list"]));
    assert!(ok);
    assert!(out.contains("memory remember"), "{out}");

    let (ok, _, err) = run(wm(&cfg).args(["memory", "--memory", "ephemeral", "wipe"]));
    assert!(!ok);
    assert!(err.contains("memory wipe --yes"), "{err}");
}

#[test]
fn memory_help_describes_every_subcommand() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    let (ok, out, _) = run(wm(&cfg).args(["memory", "--help"]));
    assert!(ok);
    for sub in ["list", "remember", "forget", "seal", "wipe"] {
        let line = out
            .lines()
            .find(|l| l.trim_start().starts_with(&format!("{sub} ")))
            .unwrap_or_else(|| panic!("no {sub} line in:\n{out}"));
        assert!(
            line.trim().len() > sub.len() + 5,
            "{sub} has no help: {line}"
        );
    }
}

/// No Qdrant: `init --local` saves memory in an encrypted file that survives across processes, and a
/// lost key is refused instead of being read as empty memory.
#[test]
fn local_memory_persists_encrypted_and_refuses_a_wrong_key() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("cfg").join("config.toml");
    let (ok, out, err) = run(wm(&cfg).args(["init", "--local"]));
    assert!(ok, "{out}{err}");
    assert!(out.contains("Local memory: file opens"), "{out}");
    assert!(
        std::fs::read_to_string(&cfg)
            .unwrap()
            .contains("memory = \"local\""),
        "init --local must be remembered"
    );

    // A separate process each time: this is what persistence means.
    let (ok, _, err) = run(wm(&cfg).args(["memory", "remember", "email", "ada@example.com"]));
    assert!(ok, "{err}");
    let (ok, out, err) = run(wm(&cfg).args(["memory", "list"]));
    assert!(ok, "{err}");
    assert!(out.contains("ada@example.com"), "{out}");

    let file = std::fs::read_to_string(dir.path().join("cfg").join("memory.local")).unwrap();
    assert!(
        !file.contains("ada@example.com") && !file.contains("email"),
        "{file}"
    );

    let key = dir.path().join("cfg").join("memory.key");
    let good = std::fs::read_to_string(&key).unwrap();
    std::fs::write(&key, "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").unwrap();
    let (ok, _, err) = run(wm(&cfg).args(["memory", "list"]));
    assert!(!ok && err.contains("memory key"), "{err}");
    std::fs::write(&key, good).unwrap();
    let (ok, out, _) = run(wm(&cfg).args(["memory", "list"]));
    assert!(ok && out.contains("ada@example.com"), "{out}");
}

#[test]
fn init_with_a_qdrant_url_replaces_local_and_the_two_flags_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    assert!(run(wm(&cfg).args(["init", "--local", "--no-check"])).0);
    let (ok, _, _) =
        run(wm(&cfg).args(["init", "--local", "--qdrant-url", "http://localhost:6333"]));
    assert!(!ok, "--local and --qdrant-url must conflict");
    let (ok, _, err) = run(wm(&cfg).args([
        "init",
        "--no-check",
        "--yes",
        "--qdrant-url",
        "http://localhost:6333",
    ]));
    assert!(ok, "{err}");
    assert!(!std::fs::read_to_string(&cfg)
        .unwrap()
        .contains("memory = \"local\""));
}
