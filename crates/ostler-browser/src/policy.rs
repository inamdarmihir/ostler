use url::Url;

/// What the browser is allowed to visit. Enforced on navigation and again after
/// every action (redirects and link clicks included).
#[derive(Debug, Clone, Default)]
pub struct Policy {
    /// Empty means "any http(s) host". Entries match the host and its subdomains.
    pub allowed_domains: Vec<String>,
    pub allow_file: bool,
}

impl Policy {
    /// True when an allowlist is in force (so network-level enforcement is worth its cost).
    pub fn restricts(&self) -> bool {
        !self.allowed_domains.is_empty()
    }

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
            return Ok(());
        }
        let host = url.host_str().unwrap_or("").to_lowercase();
        let ok = self.allowed_domains.iter().any(|d| {
            let d = d.trim().trim_start_matches("*.").to_lowercase();
            host == d || host.ends_with(&format!(".{d}"))
        });
        if ok {
            Ok(())
        } else {
            Err(format!(
                "{host} is not in the allowed domains: {}",
                self.allowed_domains.join(", ")
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let p = Policy {
            allowed_domains: vec!["example.com".into()],
            allow_file: false,
        };
        assert!(p.check("https://example.com").is_ok());
        assert!(p.check("https://app.example.com/a").is_ok());
        assert!(p.check("https://evilexample.com").is_err());
        assert!(p.check("https://example.com.evil.io").is_err());
    }

    #[test]
    fn file_needs_opt_in() {
        let p = Policy {
            allowed_domains: vec![],
            allow_file: true,
        };
        assert!(p.check("file:///tmp/a.html").is_ok());
    }
}
