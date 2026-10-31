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
    ("tenant", "WAYMARKER_TENANT"),
    ("memory", "WAYMARKER_MEMORY"),
    ("embeddings", "WAYMARKER_EMBEDDINGS"),
    ("embedding_model", "WAYMARKER_EMBEDDING_MODEL"),
    ("qdrant_on_disk", "WAYMARKER_QDRANT_ON_DISK"),
    ("learn", "WAYMARKER_LEARN"),
    ("lean", "WAYMARKER_LEAN"),
    ("tools", "WAYMARKER_TOOLS"),
];

const SECRET: &str = "qdrant_api_key";

pub fn path() -> PathBuf {
    if let Ok(p) = std::env::var("WAYMARKER_CONFIG") {
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
    base.join("waymarker").join("config.toml")
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
        "# Waymarker settings. Flags and environment variables override this file.\n# Contains a secret when qdrant_api_key is set: keep it private.\n{}",
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

pub fn env_for(key: &str) -> Option<&'static str> {
    KEYS.iter().find(|(k, _)| *k == key).map(|(_, e)| *e)
}

pub fn mask(key: &str, value: &str) -> String {
    if key != SECRET {
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
    fn secrets_are_masked_but_other_values_are_not() {
        assert_eq!(mask("tenant", "alice"), "alice");
        assert_eq!(mask("qdrant_api_key", "short"), "********");
        let m = mask("qdrant_api_key", "abcdefghijklmnop");
        assert!(!m.contains("defghijklmn"), "{m}");
    }
}
