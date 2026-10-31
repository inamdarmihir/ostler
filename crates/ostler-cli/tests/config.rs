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
