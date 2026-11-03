//! Memory-key safety: refusing a key that cannot open existing sealed data, the legacy-plaintext warning,
//! key export/import, the OS-keychain store (against fake `security` / `secret-tool`), and config validation.

use std::io::Write;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const KEY_A: &str = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE";
const KEY_B: &str = "AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI";

fn ostler(cfg: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_ostler"));
    c.env("OSTLER_CONFIG", cfg)
        .env_remove("QDRANT_URL")
        .env_remove("QDRANT_API_KEY")
        .env_remove("OSTLER_TENANT")
        .env_remove("OSTLER_MEMORY_KEY")
        .env_remove("OSTLER_KEY_STORE")
        .env_remove("OSTLER_MEMORY")
        .env_remove("OSTLER_SEAL")
        .env_remove("OSTLER_ISOLATION")
        .env_remove("OSTLER_EMBEDDINGS")
        .stdin(Stdio::null());
    c
}

struct Out {
    ok: bool,
    stdout: String,
    stderr: String,
}

fn run(c: &mut Command) -> Out {
    let o = c.output().unwrap();
    Out {
        ok: o.status.success(),
        stdout: String::from_utf8_lossy(&o.stdout).into(),
        stderr: String::from_utf8_lossy(&o.stderr).into(),
    }
}

fn run_with_stdin(c: &mut Command, input: &str) -> Out {
    let mut child = c
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let o = child.wait_with_output().unwrap();
    Out {
        ok: o.status.success(),
        stdout: String::from_utf8_lossy(&o.stdout).into(),
        stderr: String::from_utf8_lossy(&o.stderr).into(),
    }
}

// ---------- config validation and option order (no Qdrant) ----------

#[test]
fn config_set_rejects_bad_values_with_the_allowed_ones() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    for (k, v, want) in [
        ("seal", "banana", "auto, on, off"),
        ("isolation", "everyone", "shared, collection"),
        ("embeddings", "openai", "cloud, ollama, dev-hash"),
        ("retain_days", "soon", "whole number"),
        ("retain_days", "3.5", "whole number"),
        ("embedding_dim", "big", "positive whole number"),
        ("embedding_dim", "0", "positive whole number"),
        ("key_store", "cloud", "file, keychain"),
        ("verbose", "maybe", "true or false"),
        ("qdrant_url", "localhost:6333", "http://"),
    ] {
        let o = run(ostler(&cfg).args(["config", "set", k, v]));
        assert!(!o.ok, "{k}={v} was accepted");
        assert!(o.stderr.contains(want), "{k}={v}: {}", o.stderr);
    }
    assert!(!cfg.exists(), "a rejected value must not create the file");
    let o = run(ostler(&cfg).args(["config", "set", "seal", "on"]));
    assert!(o.ok, "{}", o.stderr);
    let o = run(ostler(&cfg).args(["config", "set", "verbose", "yes"]));
    assert!(o.ok && o.stdout.contains("verbose = true"), "{}", o.stdout);
    let saved = std::fs::read_to_string(&cfg).unwrap();
    assert!(saved.contains("seal = \"on\"") && saved.contains("verbose = true"));
}

#[test]
fn memory_options_work_before_and_after_the_subcommand() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    for args in [
        ["memory", "--memory", "ephemeral", "--tenant", "a", "list"],
        ["memory", "list", "--memory", "ephemeral", "--tenant", "a"],
    ] {
        let o = run(ostler(&cfg).args(args));
        assert!(o.ok, "{args:?}: {}", o.stderr);
        assert!(o.stdout.contains("No saved facts"), "{}", o.stdout);
    }
    // Subcommand flags that share a name with nothing global still work in either position.
    let o = run(ostler(&cfg).args(["memory", "--memory", "ephemeral", "skills", "--limit", "3"]));
    assert!(o.ok, "{}", o.stderr);
}

// ---------- key export / import (file store) ----------

#[test]
fn key_export_needs_confirmation_and_import_round_trips_and_protects_the_old_key() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    let keyfile = dir.path().join("memory.key");

    let o = run(ostler(&cfg).args(["memory", "key", "export", "--yes-print"]));
    assert!(
        !o.ok && o.stderr.contains("no memory key yet"),
        "{}",
        o.stderr
    );
    assert!(!keyfile.exists(), "export must never create a key");

    let o = run_with_stdin(ostler(&cfg).args(["memory", "key", "import"]), KEY_A);
    assert!(o.ok, "{}", o.stderr);
    assert_eq!(std::fs::read_to_string(&keyfile).unwrap().trim(), KEY_A);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&keyfile).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    let o = run(ostler(&cfg).args(["memory", "key", "export"]));
    assert!(!o.ok && o.stderr.contains("--yes-print"), "{}", o.stderr);
    assert!(!o.stdout.contains(KEY_A));
    let o = run(ostler(&cfg).args(["memory", "key", "export", "--yes-print"]));
    assert!(o.ok && o.stdout.trim() == KEY_A, "{}", o.stdout);

    // A different key is not swapped in silently.
    let o = run_with_stdin(ostler(&cfg).args(["memory", "key", "import"]), KEY_B);
    assert!(!o.ok && o.stderr.contains("--force"), "{}", o.stderr);
    assert_eq!(std::fs::read_to_string(&keyfile).unwrap().trim(), KEY_A);
    let o = run_with_stdin(
        ostler(&cfg).args(["memory", "key", "import", "--force"]),
        KEY_B,
    );
    assert!(o.ok, "{}", o.stderr);
    assert_eq!(std::fs::read_to_string(&keyfile).unwrap().trim(), KEY_B);
    let backups: Vec<PathBuf> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("memory.key.old-")
        })
        .collect();
    assert_eq!(backups.len(), 1);
    assert_eq!(std::fs::read_to_string(&backups[0]).unwrap().trim(), KEY_A);

    // Garbage is refused.
    let o = run_with_stdin(
        ostler(&cfg).args(["memory", "key", "import", "--force"]),
        "not a key",
    );
    assert!(
        !o.ok && o.stderr.contains("does not hold a memory key"),
        "{}",
        o.stderr
    );
}

// ---------- OS keychain store, against fake tools ----------

fn fake_tools(dir: &Path) -> (PathBuf, PathBuf) {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let store = dir.join("kc-item");
    let log = dir.join("argv.log");
    // macOS `security`: `-i` reads commands on stdin; `find-generic-password ... -w` prints; 44 = not found.
    let security = format!(
        r#"#!/bin/bash
echo "security $*" >> "{log}"
case "$1" in
  help) exit 0;;
  -i) read -r line
      key=$(echo "$line" | sed -n 's/.*-w "\([^"]*\)".*/\1/p')
      if [ -f "{store}" ] && ! echo "$line" | grep -q -- ' -U '; then echo "already exists"; exit 0; fi
      printf '%s' "$key" > "{store}"; exit 0;;
  find-generic-password) [ -f "{store}" ] && cat "{store}" && exit 0; echo "not found" >&2; exit 44;;
esac
exit 1
"#,
        log = log.display(),
        store = store.display()
    );
    let secret = format!(
        r#"#!/bin/bash
echo "secret-tool $*" >> "{log}"
case "$1" in
  --version) echo 0.20; exit 0;;
  store) cat > "{store}"; exit 0;;
  lookup) [ -f "{store}" ] && cat "{store}" && exit 0; exit 1;;
esac
exit 1
"#,
        log = log.display(),
        store = store.display()
    );
    for (name, body) in [("security", security), ("secret-tool", secret)] {
        let p = bin.join(name);
        std::fs::write(&p, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    (bin, store)
}

fn with_fake_path(c: &mut Command, bin: &Path) {
    let old = std::env::var("PATH").unwrap_or_default();
    c.env("PATH", format!("{}:{old}", bin.display()))
        .env("OSTLER_KEYCHAIN_ACCOUNT", "test-account");
}

#[cfg(unix)]
#[test]
fn keychain_store_holds_the_key_off_the_command_line_and_migrates_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    let (bin, item) = fake_tools(dir.path());
    let keyfile = dir.path().join("memory.key");
    let log = dir.path().join("argv.log");
    let mk = |args: &[&str]| {
        let mut c = ostler(&cfg);
        with_fake_path(&mut c, &bin);
        c.args(args);
        c
    };

    // Existing file key is carried into the keychain when switching.
    std::fs::write(&keyfile, KEY_A).unwrap();
    let o = run(&mut mk(&["config", "set", "key_store", "keychain"]));
    assert!(o.ok, "{}", o.stderr);
    assert!(
        o.stderr.contains("copied your existing key"),
        "{}",
        o.stderr
    );
    assert_eq!(std::fs::read_to_string(&item).unwrap().trim(), KEY_A);

    // Export now reads from the keychain, not the file.
    std::fs::remove_file(&keyfile).unwrap();
    let o = run(&mut mk(&["memory", "key", "export", "--yes-print"]));
    assert!(
        o.ok && o.stdout.trim() == KEY_A,
        "{} / {}",
        o.stdout,
        o.stderr
    );

    // Import replaces it (with --force), keeps the old key on disk as a backup, and never creates memory.key.
    let o = run_with_stdin(&mut mk(&["memory", "key", "import", "--force"]), KEY_B);
    assert!(o.ok, "{}", o.stderr);
    assert_eq!(std::fs::read_to_string(&item).unwrap().trim(), KEY_B);
    assert!(!keyfile.exists());

    // Neither key ever appeared on a command line.
    let argv = std::fs::read_to_string(&log).unwrap();
    assert!(!argv.contains(KEY_A) && !argv.contains(KEY_B), "{argv}");
    assert!(
        argv.contains("security -i") || argv.contains("secret-tool store"),
        "{argv}"
    );
}

#[cfg(unix)]
#[test]
fn keychain_selected_but_tool_missing_fails_with_advice_and_does_not_fall_back() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    let empty = dir.path().join("empty-bin");
    std::fs::create_dir_all(&empty).unwrap();
    // A PATH with only the test binary's own directory of nothing: no security/secret-tool anywhere.
    let o = run(ostler(&cfg)
        .env("PATH", &empty)
        .args(["config", "set", "key_store", "keychain"]));
    assert!(!o.ok, "{}", o.stdout);
    assert!(o.stderr.contains("not found on PATH"), "{}", o.stderr);
    assert!(!cfg.exists());
}

// ---------- real Qdrant ----------

struct Qdrant {
    url: String,
    child: Option<Child>,
    _dir: Option<tempfile::TempDir>,
}

impl Drop for Qdrant {
    fn drop(&mut self) {
        if let Some(c) = &mut self.child {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn ready(port: u16) -> bool {
    use std::io::Read;
    let Ok(mut s) = std::net::TcpStream::connect(("127.0.0.1", port)) else {
        return false;
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = s.write_all(b"GET /readyz HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    let mut buf = String::new();
    let _ = s.read_to_string(&mut buf);
    buf.starts_with("HTTP/1.1 200")
}

fn qdrant() -> Option<Qdrant> {
    if let Ok(url) = std::env::var("QDRANT_URL") {
        return Some(Qdrant {
            url,
            child: None,
            _dir: None,
        });
    }
    let Ok(bin) = std::env::var("OSTLER_QDRANT_BIN") else {
        assert!(
            std::env::var("OSTLER_REQUIRE_QDRANT").is_err(),
            "Qdrant required but not configured"
        );
        eprintln!("SKIPPED: set QDRANT_URL or OSTLER_QDRANT_BIN");
        return None;
    };
    let dir = tempfile::tempdir().unwrap();
    let (http, grpc) = (free_port(), free_port());
    let child = Command::new(bin)
        .current_dir(dir.path())
        .env("QDRANT__SERVICE__HTTP_PORT", http.to_string())
        .env("QDRANT__SERVICE__GRPC_PORT", grpc.to_string())
        .env("QDRANT__STORAGE__STORAGE_PATH", dir.path().join("storage"))
        .env("QDRANT__STORAGE__SNAPSHOTS_PATH", dir.path().join("snaps"))
        .env("QDRANT__TELEMETRY_DISABLED", "true")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn qdrant");
    let q = Qdrant {
        url: format!("http://127.0.0.1:{http}"),
        child: Some(child),
        _dir: Some(dir),
    };
    for _ in 0..200 {
        if ready(http) {
            return Some(q);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("qdrant did not become ready");
}

fn mem(cfg: &Path, q: &Qdrant, tenant: &str) -> Command {
    let mut c = ostler(cfg);
    c.args([
        "memory",
        "--qdrant-url",
        &q.url,
        "--embeddings",
        "dev-hash",
        "--tenant",
        tenant,
    ]);
    c
}

#[test]
fn a_replaced_key_is_refused_with_recovery_commands_and_wipe_or_import_fixes_it() {
    let Some(q) = qdrant() else { return };
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    let keyfile = dir.path().join("memory.key");

    // 1. Save a sealed fact with key A (file store).
    std::fs::write(&keyfile, KEY_A).unwrap();
    let o = run(mem(&cfg, &q, "kt").args(["remember", "email", "ada@example.com"]));
    assert!(o.ok, "{}", o.stderr);
    let o = run(mem(&cfg, &q, "kt").arg("list"));
    assert!(
        o.ok && o.stdout.contains("ada@example.com"),
        "{}{}",
        o.stdout,
        o.stderr
    );
    assert!(!o.stderr.contains("unencrypted"), "{}", o.stderr);

    // 2. The key file is lost: a new one gets generated. The data must not be shadowed.
    std::fs::remove_file(&keyfile).unwrap();
    let o = run(mem(&cfg, &q, "kt").arg("list"));
    assert!(!o.ok, "list must refuse; got {}", o.stdout);
    for want in [
        "refusing to start",
        "1 sealed point(s)",
        "just created",
        "ostler memory key import --force",
        "ostler memory wipe --yes --tenant kt",
    ] {
        assert!(o.stderr.contains(want), "missing {want:?} in {}", o.stderr);
    }
    // Writes are refused too, and nothing was overwritten.
    let o = run(mem(&cfg, &q, "kt").args(["remember", "email", "evil@example.com"]));
    assert!(
        !o.ok && o.stderr.contains("refusing to start"),
        "{}",
        o.stderr
    );
    // So does `serve`, before it reports ready.
    let o = run(ostler(&cfg).args([
        "serve",
        "--qdrant-url",
        &q.url,
        "--embeddings",
        "dev-hash",
        "--tenant",
        "kt",
    ]));
    assert!(
        !o.ok && o.stderr.contains("refusing to start"),
        "{}",
        o.stderr
    );
    assert!(!o.stderr.contains("MCP over stdio"), "{}", o.stderr);
    // The key that now exists is not "just created" on the next run, but still refused.
    let o = run(mem(&cfg, &q, "kt").arg("list"));
    assert!(!o.ok && !o.stderr.contains("just created"), "{}", o.stderr);

    // Another tenant is unaffected.
    let o = run(mem(&cfg, &q, "other").arg("list"));
    assert!(o.ok, "{}", o.stderr);

    // 3a. Recovery: import the backup, data is back.
    let o = run_with_stdin(
        mem(&cfg, &q, "kt").args(["key", "import", "--force"]),
        KEY_A,
    );
    assert!(
        o.ok && o.stdout.contains("opens the sealed memory"),
        "{}{}",
        o.stdout,
        o.stderr
    );
    let o = run(mem(&cfg, &q, "kt").arg("list"));
    assert!(
        o.ok && o.stdout.contains("ada@example.com"),
        "{}{}",
        o.stdout,
        o.stderr
    );

    // 3b. Or the key is gone for good: wipe is not blocked by the mismatch, then everything works.
    let o = run_with_stdin(
        mem(&cfg, &q, "kt").args(["key", "import", "--force"]),
        KEY_B,
    );
    assert!(
        o.ok && o.stdout.contains("cannot be opened"),
        "{}{}",
        o.stdout,
        o.stderr
    );
    let o = run(mem(&cfg, &q, "kt").arg("list"));
    assert!(!o.ok);
    let o = run(mem(&cfg, &q, "kt").args(["wipe", "--yes"]));
    assert!(o.ok, "{}", o.stderr);
    let o = run(mem(&cfg, &q, "kt").args(["remember", "email", "new@example.com"]));
    assert!(o.ok, "{}", o.stderr);
    let o = run(mem(&cfg, &q, "kt").arg("list"));
    assert!(o.ok && o.stdout.contains("new@example.com"), "{}", o.stdout);
}

#[test]
fn legacy_plaintext_is_warned_about_on_list_and_serve_until_sealed() {
    let Some(q) = qdrant() else { return };
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    std::fs::write(dir.path().join("memory.key"), KEY_A).unwrap();

    let o = run(mem(&cfg, &q, "lg").args(["--seal", "off", "remember", "city", "Pune"]));
    assert!(o.ok, "{}", o.stderr);

    let o = run(mem(&cfg, &q, "lg").arg("list"));
    assert!(o.ok, "{}", o.stderr);
    let warn: Vec<&str> = o
        .stderr
        .lines()
        .filter(|l| l.contains("unencrypted"))
        .collect();
    assert_eq!(warn.len(), 1, "one warning line expected: {}", o.stderr);
    assert!(warn[0].contains("ostler memory seal") && warn[0].contains("1 saved memory point"));

    let serve = || {
        let mut c = ostler(&cfg);
        c.args([
            "serve",
            "--qdrant-url",
            &q.url,
            "--embeddings",
            "dev-hash",
            "--tenant",
            "lg",
        ]);
        c
    };
    let o = run(&mut serve());
    assert!(o.stderr.contains("ostler memory seal"), "{}", o.stderr);

    // Off means the user chose it: no nagging.
    let o = run(mem(&cfg, &q, "lg").args(["--seal", "off", "list"]));
    assert!(!o.stderr.contains("unencrypted"), "{}", o.stderr);

    let o = run(mem(&cfg, &q, "lg").arg("seal"));
    assert!(o.ok, "{}", o.stderr);
    let o = run(mem(&cfg, &q, "lg").arg("list"));
    assert!(
        o.ok && o.stdout.contains("Pune") && !o.stderr.contains("unencrypted"),
        "{}{}",
        o.stdout,
        o.stderr
    );
    let o = run(&mut serve());
    assert!(!o.stderr.contains("unencrypted"), "{}", o.stderr);
}
