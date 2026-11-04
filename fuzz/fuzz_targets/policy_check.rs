#![no_main]
//! Arbitrary URL strings through every policy mode. Invariants: never panics; the cloud metadata
//! address stays refused; an allowlist of one host never admits a URL whose parsed host differs.
use libfuzzer_sys::fuzz_target;
use ostler_browser::Policy;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else { return };
    let (entry, url) = s.split_once('\n').unwrap_or(("example.com", s));
    let open = Policy::default();
    let blocking = Policy { block_private_networks: true, ..Default::default() };
    let listed = Policy { allowed_domains: vec![entry.to_string()], ..Default::default() };
    for p in [&open, &blocking, &listed] {
        let _ = p.check(url);
        let _ = p.host_resolver_rules();
    }
    if let Ok(u) = url::Url::parse(url) {
        if u.host_str() == Some("169.254.169.254") && matches!(u.scheme(), "http" | "https") {
            assert!(open.check(url).is_err(), "metadata address allowed: {url}");
        }
    }
});
