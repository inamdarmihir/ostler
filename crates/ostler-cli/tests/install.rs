//! `ostler install`, zero-config `serve`, `doctor` fixes and `--cdp-url`, through the real binary.
//! Every test points OSTLER_INSTALL_HOME and OSTLER_CONFIG at temp dirs: no real user config is read or written.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{json, Value};

fn ostler(home: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_ostler"));
    c.env("OSTLER_INSTALL_HOME", home)
        .env("OSTLER_CONFIG", home.join("cfg").join("config.toml"))
        .env("XDG_CONFIG_HOME", home.join("xdg"))
        .env("HOME", home)
        .env_remove("APPDATA")
        .env_remove("QDRANT_URL")
        .env_remove("QDRANT_API_KEY")
        .env_remove("OSTLER_MEMORY")
        .env_remove("OSTLER_MEMORY_KEY")
        .env_remove("OSTLER_CDP_URL")
        .env_remove("OSTLER_CHROME")
        .stdin(Stdio::null());
    c
}

fn run(c: &mut Command) -> (bool, String, String) {
    let o = c.output().unwrap();
    (
        o.status.success(),
        String::from_utf8_lossy(&o.stdout).into(),
        String::from_utf8_lossy(&o.stderr).into(),
    )
}

#[test]
fn install_cursor_roundtrip_is_idempotent_and_reversible() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join(".cursor/mcp.json");
    std::fs::create_dir_all(f.parent().unwrap()).unwrap();
    std::fs::write(&f, r#"{"mcpServers":{"keep":{"command":"k"}}}"#).unwrap();

    let (ok, out, _) = run(ostler(d.path()).args(["install", "--client", "cursor", "--dry-run"]));
    assert!(
        ok && out.contains("dry run") && out.contains("Added"),
        "{out}"
    );
    assert!(!std::fs::read_to_string(&f).unwrap().contains("ostler"));

    let (ok, out, err) =
        run(ostler(d.path()).args(["install", "--client", "cursor", "--", "--lean"]));
    assert!(ok, "{out}{err}");
    assert!(
        out.contains("Backup of the previous file")
            && out.contains("Other entries left untouched: 1"),
        "{out}"
    );
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
    assert_eq!(v["mcpServers"]["keep"]["command"], "k");
    assert_eq!(
        v["mcpServers"]["ostler"]["args"],
        json!(["serve", "--lean"])
    );
    assert!(v["mcpServers"]["ostler"]["command"]
        .as_str()
        .unwrap()
        .contains("ostler"));

    let (_, out, _) = run(ostler(d.path()).args(["install", "--client", "cursor", "--", "--lean"]));
    assert!(out.contains("Already installed"), "{out}");

    let (ok, out, _) = run(ostler(d.path()).args(["install", "--client", "cursor", "--uninstall"]));
    assert!(ok && out.contains("Removed"), "{out}");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
    assert_eq!(v, json!({"mcpServers":{"keep":{"command":"k"}}}));
}

#[test]
fn install_refuses_malformed_files_and_leaves_them_alone() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join(".claude.json");
    std::fs::write(&f, "{ oops").unwrap();
    let (ok, _, err) = run(ostler(d.path()).args(["install", "--client", "claude-code"]));
    assert!(!ok && err.contains("not valid JSON"), "{err}");
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "{ oops");

    let t = d.path().join(".codex/config.toml");
    std::fs::create_dir_all(t.parent().unwrap()).unwrap();
    std::fs::write(&t, "model = ").unwrap();
    let (ok, _, err) = run(ostler(d.path()).args(["install", "--client", "codex"]));
    assert!(!ok && err.contains("not valid TOML"), "{err}");
    assert_eq!(std::fs::read_to_string(&t).unwrap(), "model = ");
}

#[test]
fn install_codex_writes_toml_and_list_reports_state() {
    let d = tempfile::tempdir().unwrap();
    let (ok, _, err) = run(ostler(d.path()).args(["install", "--client", "codex"]));
    assert!(ok, "{err}");
    let t: toml::Table = std::fs::read_to_string(d.path().join(".codex/config.toml"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        t["mcp_servers"]["ostler"]["args"].as_array().unwrap().len(),
        1
    );
    let (ok, out, _) = run(ostler(d.path()).args(["install", "--list"]));
    assert!(ok);
    for name in [
        "claude-code",
        "claude-desktop",
        "cursor",
        "vscode",
        "windsurf",
        "codex",
    ] {
        assert!(out.contains(name), "{out}");
    }
    assert!(out.contains("installed (matches this binary)"), "{out}");
}

#[test]
fn install_without_client_says_what_to_do_and_unsupported_paths_are_not_guessed() {
    let d = tempfile::tempdir().unwrap();
    let (ok, _, err) = run(ostler(d.path()).args(["install"]));
    assert!(!ok && err.contains("choose a client"), "{err}");
    if cfg!(target_os = "linux") {
        let (ok, _, err) = run(ostler(d.path()).args(["install", "--client", "claude-desktop"]));
        assert!(
            !ok && err.contains("no official Linux config location"),
            "{err}"
        );
        assert!(!d.path().join("Library").exists() && !d.path().join(".config").exists());
    }
}

#[test]
fn doctor_prints_fix_commands_and_fix_only_does_safe_things() {
    let d = tempfile::tempdir().unwrap();
    let cfgdir = d.path().join("cfg");
    let missing = d.path().join("no-such-chrome");
    let (ok, out, _) = run(ostler(d.path())
        .env("OSTLER_CHROME", &missing)
        .arg("doctor"));
    assert!(!ok, "{out}");
    assert!(
        out.contains("fix: ") && out.contains("OSTLER_CHROME"),
        "{out}"
    );
    assert!(out.contains("mkdir -p") && !cfgdir.exists(), "{out}");
    assert!(out.contains("ostler install --client claude-code"), "{out}");
    assert!(out.contains("in-process"), "{out}");

    let (_, out, _) = run(ostler(d.path())
        .env("OSTLER_CHROME", &missing)
        .args(["doctor", "--fix"]));
    assert!(cfgdir.is_dir() && out.contains("created"), "{out}");
    // --fix never makes keys, config files or client entries.
    assert!(!cfgdir.join("memory.key").exists() && !cfgdir.join("config.toml").exists());
    assert!(!d.path().join(".claude.json").exists());
}

#[test]
fn doctor_cdp_url_failure_explains_how_to_start_chrome() {
    let d = tempfile::tempdir().unwrap();
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let (ok, out, _) = run(ostler(d.path())
        .env("OSTLER_CHROME", d.path().join("none"))
        .args(["doctor", "--cdp-url", &format!("http://127.0.0.1:{port}")]));
    assert!(!ok);
    assert!(
        out.contains("--remote-debugging-port") && out.contains("--user-data-dir"),
        "{out}"
    );
}

#[test]
fn serve_refuses_remote_cdp_url_at_startup() {
    let d = tempfile::tempdir().unwrap();
    let (ok, _, err) =
        run(ostler(d.path()).args(["serve", "--cdp-url", "http://example.com:9222"]));
    assert!(!ok && err.contains("non-loopback"), "{err}");
}

/// Zero config: no Qdrant, key, flags or config file. The server starts, says what is on/off, and memory works.
#[tokio::test]
async fn serve_with_no_config_uses_in_process_memory() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let d = tempfile::tempdir().unwrap();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_ostler"))
        .arg("serve")
        .env("OSTLER_INSTALL_HOME", d.path())
        .env("OSTLER_CONFIG", d.path().join("cfg/config.toml"))
        .env("HOME", d.path())
        .env_remove("QDRANT_URL")
        .env_remove("OSTLER_MEMORY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut err = BufReader::new(child.stderr.take().unwrap()).lines();
    let mut banner = String::new();
    let read = async {
        while let Some(l) = err.next_line().await.unwrap() {
            banner.push_str(&l);
            banner.push('\n');
            if l.contains("Opt in with") {
                break;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(20), read)
        .await
        .expect("no banner");
    assert!(
        banner.contains("memory:  in-process") && banner.contains("Qdrant"),
        "{banner}"
    );

    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap()).lines();
    let send = |v: Value| format!("{v}\n");
    stdin.write_all(send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}})).as_bytes()).await.unwrap();
    let l = tokio::time::timeout(Duration::from_secs(20), out.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(l.contains("\"ostler\""), "{l}");
    stdin
        .write_all(send(json!({"jsonrpc":"2.0","method":"notifications/initialized"})).as_bytes())
        .await
        .unwrap();
    stdin.write_all(send(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"memory_remember","arguments":{"key":"email","value":"a@example.com"}}})).as_bytes()).await.unwrap();
    let l = tokio::time::timeout(Duration::from_secs(20), out.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let v: Value = serde_json::from_str(&l).unwrap();
    assert_ne!(v["result"]["isError"], json!(true), "{l}");
    // Nothing was created on disk: no key, no config.
    assert!(
        !d.path().join("cfg").exists(),
        "zero-config must not write files"
    );
    let _ = child.kill().await;
}

/// Attach to a Chrome that the test starts itself (loopback, throwaway profile). Skipped without Chrome.
#[tokio::test]
async fn attach_to_an_existing_chrome_leaves_it_running() {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let Ok(chrome) = ostler_cdp::find_chrome(None) else {
        eprintln!("SKIPPED: no Chrome");
        return;
    };
    let prof = tempfile::tempdir().unwrap();
    let mut browser = tokio::process::Command::new(chrome)
        .args([
            "--headless=new",
            "--remote-debugging-port=0",
            "--remote-debugging-address=127.0.0.1",
            "--no-first-run",
            "--disable-extensions",
            "--no-sandbox",
            "about:blank",
        ])
        .arg(format!("--user-data-dir={}", prof.path().display()))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(browser.stderr.take().unwrap()).lines();
    let ws = tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(l) = lines.next_line().await.unwrap() {
            if let Some(r) = l.split("DevTools listening on ").nth(1) {
                return r.trim().to_string();
            }
        }
        panic!("chrome never announced DevTools");
    })
    .await
    .unwrap();
    tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
    let port = ws
        .split(':')
        .nth(2)
        .unwrap()
        .split('/')
        .next()
        .unwrap()
        .to_string();

    let d = tempfile::tempdir().unwrap();
    let url = format!("http://127.0.0.1:{port}");
    let out = tokio::process::Command::new(env!("CARGO_BIN_EXE_ostler"))
        .args(["doctor", "--cdp-url", &url])
        .env("OSTLER_CONFIG", d.path().join("c.toml"))
        .env("OSTLER_INSTALL_HOME", d.path())
        .env_remove("QDRANT_URL")
        .output()
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("attached and opened a blank tab OK"),
        "{text}"
    );
    assert!(
        browser.try_wait().unwrap().is_none(),
        "ostler must not kill a browser it did not start"
    );
    let _ = browser.kill().await;
}

#[test]
fn start_connects_detected_apps_only_and_is_idempotent() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join(".cursor")).unwrap();
    let (ok, out, _) = run(ostler(d.path()).arg("start"));
    assert!(ok, "{out}");
    assert!(out.contains("cursor: connected"), "{out}");
    assert!(!out.contains("codex"), "{out}");
    let v: Value =
        serde_json::from_str(&std::fs::read_to_string(d.path().join(".cursor/mcp.json")).unwrap())
            .unwrap();
    assert_eq!(v["mcpServers"]["ostler"]["args"], json!(["serve"]));
    let (ok, out, _) = run(ostler(d.path()).arg("start"));
    assert!(ok && out.contains("already connected"), "{out}");
}

#[test]
fn start_with_no_apps_explains_instead_of_failing() {
    let d = tempfile::tempdir().unwrap();
    let (ok, out, _) = run(ostler(d.path()).arg("start"));
    assert!(ok, "{out}");
    assert!(out.contains("No supported AI app found"), "{out}");
}

#[test]
fn start_cautious_registers_the_confirm_flag() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join(".cursor")).unwrap();
    let (ok, out, _) = run(ostler(d.path()).args(["start", "--cautious"]));
    assert!(ok, "{out}");
    let v: Value =
        serde_json::from_str(&std::fs::read_to_string(d.path().join(".cursor/mcp.json")).unwrap())
            .unwrap();
    assert_eq!(
        v["mcpServers"]["ostler"]["args"],
        json!(["serve", "--require-confirm-submit"])
    );
}
