use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use url::{Host, Url};

/// What the browser is allowed to do. Enforced on navigation and again after
/// every action (redirects and link clicks included), and for every request the page makes.
#[derive(Debug, Clone, Default)]
pub struct Policy {
    /// Empty means "any http(s) host". Entries match the host and its subdomains, optionally pinned to one
    /// port (`example.com:8443`, `https://example.com:8443`, `[::1]:8080`). Without a port, any port matches.
    pub allowed_domains: Vec<String>,
    pub allow_file: bool,
    /// Directories a file field may be filled from. Empty means uploads are refused: a page (or a prompt
    /// injected into the agent) must not be able to make it send `~/.ssh/id_rsa`.
    pub upload_dirs: Vec<PathBuf>,
    /// With no allowlist, refuse loopback, private, link-local (cloud metadata) and internal-looking hosts.
    /// An explicit allowlist already decides, so this only matters for "any host".
    pub block_private_networks: bool,
    /// Answer `confirm()` and `prompt()` with "OK". Off by default: a page's "Delete everything?" must not be
    /// confirmed by a browser nobody is watching. `alert()` and `beforeunload` are always accepted.
    pub accept_dialogs: bool,
}

struct Entry {
    host: String,
    port: Option<u16>,
}

fn parse_entry(raw: &str) -> Option<Entry> {
    let e = raw.trim().to_lowercase();
    if e.is_empty() {
        return None;
    }
    if e.contains("://") {
        let u = Url::parse(&e).ok()?;
        return Some(Entry {
            host: u.host_str()?.trim_start_matches("*.").to_string(),
            port: u.port(),
        });
    }
    let e = e.trim_start_matches("*.");
    if let Some(rest) = e.strip_prefix('[') {
        // [v6] or [v6]:port
        let (v6, tail) = rest.split_once(']')?;
        let port = tail.strip_prefix(':').and_then(|p| p.parse().ok());
        return Some(Entry {
            host: format!("[{v6}]"),
            port,
        });
    }
    match e.matches(':').count() {
        0 => Some(Entry {
            host: e.to_string(),
            port: None,
        }),
        1 => {
            let (h, p) = e.split_once(':')?;
            Some(Entry {
                host: h.to_string(),
                port: p.parse().ok(),
            })
        }
        _ => Some(Entry {
            host: format!("[{e}]"),
            port: None,
        }), // bare IPv6
    }
}

fn private_v4(a: Ipv4Addr) -> bool {
    let o = a.octets();
    a.is_loopback()
        || a.is_private()
        || a.is_link_local() // 169.254.0.0/16, includes the cloud metadata address
        || a.is_unspecified()
        || a.is_broadcast()
        || a.is_multicast()
        || (o[0] == 100 && (64..=127).contains(&o[1])) // CGNAT 100.64.0.0/10
        || (o[0] == 192 && o[1] == 0 && o[2] == 0) // 192.0.0.0/24
        || (o[0] == 198 && (o[1] == 18 || o[1] == 19)) // benchmarking
        || o[0] >= 240 // reserved
        || o[0] == 0
}

fn private_v6(a: Ipv6Addr) -> bool {
    if let Some(v4) = a.to_ipv4_mapped() {
        return private_v4(v4);
    }
    let s = a.segments();
    // NAT64 64:ff9b::/96 embeds an IPv4 address in the last 32 bits.
    if s[0] == 0x64 && s[1] == 0xff9b && s[2..6].iter().all(|x| *x == 0) {
        return private_v4(Ipv4Addr::new(
            (s[6] >> 8) as u8,
            s[6] as u8,
            (s[7] >> 8) as u8,
            s[7] as u8,
        ));
    }
    a.is_loopback()
        || a.is_unspecified()
        || a.is_multicast()
        || (s[0] & 0xfe00) == 0xfc00 // unique local fc00::/7
        || (s[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
}

fn private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => private_v4(a),
        IpAddr::V6(a) => private_v6(a),
    }
}

/// Names that only make sense inside a network: refused without needing a DNS answer.
fn internal_name(d: &str) -> bool {
    let d = d.trim_end_matches('.');
    d == "localhost"
        || !d.contains('.') // single label: resolved through a search domain on the inside
        || d.ends_with(".localhost")
        || d.ends_with(".local")
        || d.ends_with(".internal")
        || d.ends_with(".localdomain")
        || d.ends_with(".lan")
        || d.ends_with(".home.arpa")
}

type DnsCache = Mutex<HashMap<String, (Instant, bool)>>;

static DNS_CACHE: LazyLock<DnsCache> = LazyLock::new(Default::default);

/// True when `host` resolves to any private address. Cached briefly: every sub-resource asks.
async fn resolves_private(host: &str, port: u16) -> bool {
    const TTL: Duration = Duration::from_secs(30);
    if let Some((t, v)) = DNS_CACHE.lock().unwrap().get(host) {
        if t.elapsed() < TTL {
            return *v;
        }
    }
    let private = match tokio::net::lookup_host((host, port)).await {
        Ok(addrs) => addrs.into_iter().any(|a| private_ip(a.ip())),
        Err(_) => false, // Chrome will fail to resolve it too
    };
    DNS_CACHE
        .lock()
        .unwrap()
        .insert(host.to_string(), (Instant::now(), private));
    private
}

impl Policy {
    /// True when an allowlist is in force (so network-level enforcement is worth its cost).
    pub fn restricts(&self) -> bool {
        !self.allowed_domains.is_empty()
    }

    /// Synchronous checks that need no DNS: scheme, allowlist, and (with no allowlist) literal private hosts.
    pub fn check(&self, raw: &str) -> Result<(), String> {
        if raw == "about:blank" {
            return Ok(());
        }
        let url = Url::parse(raw).map_err(|e| format!("invalid URL {raw:?}: {e}"))?;
        match url.scheme() {
            "http" | "https" => {}
            "file" if self.allow_file => return Ok(()),
            other => {
                return Err(format!(
                    "scheme {other:?} is not allowed (start with --allow-file for file://)"
                ))
            }
        }
        if self.allowed_domains.is_empty() {
            if self.block_private_networks {
                let private = match url.host() {
                    Some(Host::Ipv4(a)) => private_v4(a),
                    Some(Host::Ipv6(a)) => private_v6(a),
                    Some(Host::Domain(d)) => internal_name(d),
                    None => true,
                };
                if private {
                    return Err(format!(
                        "{} is a private or internal address (blocked by --block-private-networks)",
                        url.host_str().unwrap_or("")
                    ));
                }
            }
            return Ok(());
        }
        let host = url.host_str().unwrap_or("").to_lowercase();
        let port = url.port_or_known_default();
        let ok = self
            .allowed_domains
            .iter()
            .filter_map(|d| parse_entry(d))
            .any(|d| {
                (host == d.host || host.ends_with(&format!(".{}", d.host)))
                    && d.port.is_none_or(|p| Some(p) == port)
            });
        if ok {
            Ok(())
        } else {
            Err(format!(
                "{host}{} is not in the allowed domains: {}",
                port.map(|p| format!(":{p}")).unwrap_or_default(),
                self.allowed_domains.join(", ")
            ))
        }
    }

    /// [`check`](Self::check) plus, with no allowlist and `block_private_networks`, a DNS lookup so a public
    /// name that points at a private address is refused too. (Chrome resolves again when it connects, so a
    /// DNS-rebinding server can still answer differently the second time: use an allowlist where that matters.)
    pub async fn check_resolved(&self, raw: &str) -> Result<(), String> {
        self.check(raw)?;
        if self.restricts() || !self.block_private_networks {
            return Ok(());
        }
        let Ok(url) = Url::parse(raw) else {
            return Ok(());
        };
        if let Some(Host::Domain(d)) = url.host() {
            if resolves_private(d, url.port_or_known_default().unwrap_or(80)).await {
                return Err(format!(
                    "{d} resolves to a private or internal address (blocked by --block-private-networks)"
                ));
            }
        }
        Ok(())
    }

    /// Resolve `path` for upload: it must be an existing regular file inside one of `upload_dirs`
    /// (after resolving symlinks and `..`). Returns the canonical path to hand to the browser.
    pub fn check_upload(&self, path: &str) -> Result<String, String> {
        if self.upload_dirs.is_empty() {
            return Err("file uploads are disabled: start Ostler with --upload-dir <directory> to allow files from one".into());
        }
        let real = std::fs::canonicalize(path).map_err(|_| format!("file not found: {path}"))?;
        if !real.is_file() {
            return Err(format!("not a regular file: {path}"));
        }
        let inside = self
            .upload_dirs
            .iter()
            .filter_map(|d| std::fs::canonicalize(d).ok())
            .any(|d| real.starts_with(d));
        if !inside {
            return Err(format!("{path} is outside the allowed upload directories"));
        }
        Ok(real.to_string_lossy().into_owned())
    }

    /// Value for Chrome's `--host-resolver-rules`: every host except the allowed ones (and their
    /// subdomains) becomes unresolvable. Unlike per-tab request interception this binds the whole
    /// browser process, so WebSockets, images, stylesheets and the first request of a popup (which is
    /// attached only after it has started) cannot reach a forbidden host either. Ports are not part of
    /// the rule; they are enforced per request.
    ///
    /// None when there is no allowlist, or when it names an IPv6 literal (the rule syntax for those is
    /// not something we have verified, and a wrong rule would break the allowed host): interception still
    /// applies then.
    pub fn host_resolver_rules(&self) -> Option<String> {
        if !self.restricts() {
            return None;
        }
        let mut hosts: Vec<String> = Vec::new();
        for e in self.allowed_domains.iter().filter_map(|d| parse_entry(d)) {
            if e.host.starts_with('[') {
                return None;
            }
            if !hosts.contains(&e.host) {
                hosts.push(e.host);
            }
        }
        let mut rules = vec!["MAP * ~NOTFOUND".to_string()];
        for h in hosts {
            rules.push(format!("EXCLUDE {h}"));
            rules.push(format!("EXCLUDE *.{h}"));
        }
        Some(rules.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allow(d: &[&str]) -> Policy {
        Policy {
            allowed_domains: d.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn open_policy_allows_http_blocks_odd_schemes() {
        let p = Policy::default();
        assert!(p.check("https://example.com/x").is_ok());
        assert!(p.check("about:blank").is_ok());
        assert!(p.check("javascript:alert(1)").is_err());
        assert!(p.check("data:text/html,hi").is_err());
        assert!(p.check("file:///etc/passwd").is_err());
    }

    #[test]
    fn domain_allowlist_matches_subdomains_not_suffix_tricks() {
        let p = allow(&["example.com"]);
        assert!(p.check("https://example.com").is_ok());
        assert!(p.check("https://a.b.example.com/x").is_ok());
        assert!(p.check("https://evilexample.com").is_err());
        assert!(p.check("https://example.com.evil.io").is_err());
        assert!(p.check("https://user@evil.io/").is_err());
    }

    #[test]
    fn file_needs_opt_in() {
        let p = Policy {
            allow_file: true,
            ..Default::default()
        };
        assert!(p.check("file:///tmp/a.html").is_ok());
    }

    #[test]
    fn a_port_in_the_entry_pins_the_port_and_no_port_means_any() {
        let p = allow(&["example.com:8443", "other.org"]);
        assert!(p.check("https://example.com:8443/x").is_ok());
        assert!(
            p.check("https://example.com/x").is_err(),
            "default 443 is not 8443"
        );
        assert!(p.check("https://example.com:9000/").is_err());
        assert!(
            p.check("https://other.org:1234/").is_ok(),
            "no port in the entry: any port"
        );
        // Pasted URLs and IPv6 literals work too.
        let q = allow(&["https://api.example.com:8443", "[::1]:8080"]);
        assert!(q.check("https://api.example.com:8443/").is_ok());
        assert!(q.check("https://api.example.com:443/").is_err());
        assert!(q.check("http://[::1]:8080/").is_ok());
        assert!(q.check("http://[::1]:9/").is_err());
    }

    #[test]
    fn private_networks_are_blocked_by_literal_and_by_name_when_asked() {
        let p = Policy {
            block_private_networks: true,
            ..Default::default()
        };
        for bad in [
            "http://127.0.0.1/",
            "http://localhost:8787/",
            "http://10.1.2.3/",
            "http://192.168.0.9/",
            "http://172.16.0.1/",
            "http://169.254.169.254/latest/meta-data/",
            "http://100.64.0.1/",
            "http://[::1]/",
            "http://[fd00::1]/",
            "http://[fe80::1]/",
            "http://[::ffff:10.0.0.1]/",
            "http://0x7f.1/",
            "http://2130706433/",
            "http://intranet/",
            "http://db.internal/",
            "http://printer.local/",
            "http://app.localhost/",
        ] {
            assert!(p.check(bad).is_err(), "{bad} should be refused");
        }
        for ok in [
            "https://example.com/",
            "http://8.8.8.8/",
            "https://sub.example.co.uk:8443/",
        ] {
            assert!(p.check(ok).is_ok(), "{ok} should be allowed");
        }
        // Off by default, and an explicit allowlist always wins.
        assert!(Policy::default().check("http://127.0.0.1/").is_ok());
        let q = Policy {
            block_private_networks: true,
            allowed_domains: vec!["127.0.0.1".into()],
            ..Default::default()
        };
        assert!(q.check("http://127.0.0.1/").is_ok());
    }

    #[tokio::test]
    async fn a_public_looking_name_that_resolves_private_is_refused() {
        let p = Policy {
            block_private_networks: true,
            ..Default::default()
        };
        // `localtest.me` and friends are public names that resolve to 127.0.0.1; offline it simply fails to
        // resolve, so the test only asserts the refusal when the lookup works.
        if tokio::net::lookup_host(("localtest.me", 80)).await.is_ok() {
            assert!(p.check_resolved("http://localtest.me/").await.is_err());
        }
        assert!(p.check_resolved("http://127.0.0.1/").await.is_err());
    }

    #[test]
    fn uploads_are_denied_by_default_and_confined_to_their_directory() {
        let none = Policy::default();
        let dir = tempfile::tempdir().unwrap();
        let inside = dir.path().join("cv.txt");
        std::fs::write(&inside, "hi").unwrap();
        assert!(none
            .check_upload(inside.to_str().unwrap())
            .unwrap_err()
            .contains("disabled"));

        let p = Policy {
            upload_dirs: vec![dir.path().to_path_buf()],
            ..Default::default()
        };
        assert!(p.check_upload(inside.to_str().unwrap()).is_ok());
        // `..` and symlinks must not escape.
        let secret_dir = tempfile::tempdir().unwrap();
        let secret = secret_dir.path().join("id_rsa");
        std::fs::write(&secret, "key").unwrap();
        let dotdot = format!(
            "{}/../{}/id_rsa",
            dir.path().display(),
            secret_dir.path().file_name().unwrap().to_string_lossy()
        );
        assert!(p.check_upload(&dotdot).is_err(), "{dotdot}");
        #[cfg(unix)]
        {
            let link = dir.path().join("link");
            std::os::unix::fs::symlink(&secret, &link).unwrap();
            assert!(p
                .check_upload(link.to_str().unwrap())
                .unwrap_err()
                .contains("outside"));
        }
        assert!(p
            .check_upload(dir.path().to_str().unwrap())
            .unwrap_err()
            .contains("regular file"));
        assert!(p
            .check_upload("/definitely/not/here")
            .unwrap_err()
            .contains("not found"));
    }

    #[test]
    fn resolver_rules_block_everything_but_the_allowlist() {
        assert_eq!(Policy::default().host_resolver_rules(), None);
        let p = allow(&["example.com:8443", " *.Docs.Rs ", "", "example.com"]);
        assert_eq!(
            p.host_resolver_rules().unwrap(),
            "MAP * ~NOTFOUND, EXCLUDE example.com, EXCLUDE *.example.com, EXCLUDE docs.rs, EXCLUDE *.docs.rs"
        );
        // An IPv6 literal in the allowlist: no browser-wide rule (interception still enforces).
        assert_eq!(
            allow(&["[::1]:8080", "example.com"]).host_resolver_rules(),
            None
        );
    }
}
