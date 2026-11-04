//! Property tests for the URL / domain / private-network policy. Pure and offline: no browser, no DNS.
use ostler_browser::Policy;
use proptest::prelude::*;

fn blocking() -> Policy {
    Policy {
        block_private_networks: true,
        ..Default::default()
    }
}

fn allow(domains: &[&str]) -> Policy {
    Policy {
        allowed_domains: domains.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

/// A private/loopback/link-local IPv4 address as octets.
fn private_v4() -> impl Strategy<Value = [u8; 4]> {
    prop_oneof![
        (any::<u8>(), any::<u8>(), any::<u8>()).prop_map(|(a, b, c)| [127, a, b, c]),
        (any::<u8>(), any::<u8>(), any::<u8>()).prop_map(|(a, b, c)| [10, a, b, c]),
        (16u8..=31, any::<u8>(), any::<u8>()).prop_map(|(a, b, c)| [172, a, b, c]),
        (any::<u8>(), any::<u8>()).prop_map(|(a, b)| [192, 168, a, b]),
        (any::<u8>(), any::<u8>()).prop_map(|(a, b)| [169, 254, a, b]),
        (64u8..=127, any::<u8>(), any::<u8>()).prop_map(|(a, b, c)| [100, a, b, c]),
        (any::<u8>(), any::<u8>(), any::<u8>()).prop_map(|(a, b, c)| [0, a, b, c]),
    ]
}

/// The same address spelled the ways a URL parser accepts.
fn spellings(o: [u8; 4]) -> Vec<String> {
    let n = u32::from_be_bytes(o);
    vec![
        format!("{}.{}.{}.{}", o[0], o[1], o[2], o[3]),
        format!("{}.{}.{}.{}.", o[0], o[1], o[2], o[3]),
        n.to_string(),
        format!("0x{n:x}"),
        format!("0X{n:X}"),
        format!("0{:o}.0{:o}.0{:o}.0{:o}", o[0], o[1], o[2], o[3]),
        format!("0x{:x}.0x{:x}.0x{:x}.0x{:x}", o[0], o[1], o[2], o[3]),
        format!("{}.{}", o[0], u32::from_be_bytes([0, o[1], o[2], o[3]])),
        format!("[::ffff:{}.{}.{}.{}]", o[0], o[1], o[2], o[3]),
        format!("[::ffff:{:x}:{:x}]", n >> 16, n & 0xffff),
    ]
}

proptest! {
    #[test]
    fn private_ipv4_in_any_spelling_is_blocked(
        o in private_v4(),
        scheme in prop_oneof!["http", "https"],
        port in prop::option::of(1u16..),
        userinfo in prop::option::of("[a-z]{1,8}(:[a-z]{1,8})?"),
        path in "(/[a-z0-9]{0,6}){0,3}",
    ) {
        let p = blocking();
        for host in spellings(o) {
            let auth = userinfo.as_ref().map(|u| format!("{u}@")).unwrap_or_default();
            let port = port.map(|p| format!(":{p}")).unwrap_or_default();
            let url = format!("{scheme}://{auth}{host}{port}{path}");
            prop_assert!(p.check(&url).is_err(), "allowed {url}");
        }
    }

    #[test]
    fn private_ipv6_is_blocked(
        kind in 0u8..5, a in any::<u16>(), b in any::<u16>(), c in any::<u16>(),
    ) {
        let host = match kind {
            0 => "::1".to_string(),
            1 => format!("fe{:02x}::{a:x}:{b:x}", 0x80 + (c & 0x3f)), // fe80::/10
            2 => format!("fc{:02x}::{a:x}:{b:x}", c & 0xff),
            3 => format!("fd{:02x}::{a:x}:{b:x}", c & 0xff),
            _ => format!("::ffff:{}.{}.{}.{}", 10, a & 0xff, b & 0xff, c & 0xff),
        };
        let url = format!("http://[{host}]/");
        prop_assert!(blocking().check(&url).is_err(), "allowed {url}");
    }

    #[test]
    fn metadata_addresses_are_blocked_even_without_the_flag(
        a in any::<u8>(), b in any::<u8>(), upper in any::<bool>(), dot in any::<bool>(),
    ) {
        let p = Policy::default();
        let d = if dot { "." } else { "" };
        for url in [
            format!("http://169.254.{a}.{b}{d}/latest/meta-data/"),
            format!("http://[::ffff:169.254.{a}.{b}]/"),
            "http://[fd00:ec2::254]/".to_string(),
            "http://[fe80::1]/".to_string(),
            format!("http://metadata.google.internal{d}/"),
            format!("http://metadata{d}/"),
            format!("http://instance-data{d}/"),
            format!("http://{}@169.254.169.254/", "allowed.com"),
        ] {
            let url = if upper { url.to_uppercase() } else { url };
            prop_assert!(p.check(&url).is_err(), "allowed {url}");
        }
    }

    #[test]
    fn internal_names_are_blocked(label in "[a-z][a-z0-9]{0,10}", suffix in prop::sample::select(vec![
        "localhost", "local", "internal", "localdomain", "lan", "home.arpa"]), dot in any::<bool>())
    {
        let d = if dot { "." } else { "" };
        let p = blocking();
        let named = format!("http://{label}.{suffix}{d}/");
        let single = format!("http://{label}{d}/");
        prop_assert!(p.check(&named).is_err());
        prop_assert!(p.check(&single).is_err(), "single label");
    }

    #[test]
    fn allowlist_never_matches_lookalikes(
        prefix in "[a-z0-9]{1,8}", evil in "[a-z]{1,8}\\.(com|net|org)", port in 1u16..,
    ) {
        let p = allow(&["allowed.com"]);
        for url in [
            format!("http://{prefix}allowed.com/"),
            format!("http://allowed.com.{evil}/"),
            format!("http://allowed.com@{evil}/"),
            format!("http://allowed.com:{port}@{evil}/"),
            format!("http://{evil}/allowed.com"),
            format!("http://{evil}/?u=allowed.com"),
            format!("http://{evil}/#allowed.com"),
            format!("http://{evil}\\@allowed.com/"),
            format!("http://allowed.com%2e{evil}/"),
            format!("http://allowed-com.{evil}/"),
            format!("http://allowed.com{prefix}.{evil}/"),
            "ftp://allowed.com/".to_string(),
            "javascript:alert('allowed.com')".to_string(),
            "data:text/html,allowed.com".to_string(),
            "file:///allowed.com".to_string(),
        ] {
            prop_assert!(p.check(&url).is_err(), "allowed {url}");
        }
    }

    #[test]
    fn allowlist_accepts_host_and_subdomains_only_on_pinned_port(
        sub in prop::option::of("[a-z0-9]{1,8}"), port in 1u16..,
    ) {
        let host = match &sub { Some(s) => format!("{s}.allowed.com"), None => "allowed.com".into() };
        let url = format!("https://{host}:{port}/x");
        prop_assert!(allow(&["allowed.com"]).check(&url).is_ok());
        let pinned = allow(&["allowed.com:8443"]);
        prop_assert_eq!(pinned.check(&url).is_ok(), port == 8443);
        // Case-insensitive host.
        let upper = format!("http://{}/", host.to_uppercase());
        prop_assert!(allow(&["ALLOWED.com"]).check(&upper).is_ok());
    }

    #[test]
    fn allowlist_entry_only_matches_its_own_port_and_host(
        other in "[a-z]{1,8}\\.(com|net)", port in 1u16..,
    ) {
        prop_assume!(other != "allowed.com");
        let p = allow(&["http://allowed.com:8080"]);
        let url = format!("http://{other}:{port}/");
        prop_assert!(p.check(&url).is_err());
    }

    #[test]
    fn never_panics_on_arbitrary_input(s in "\\PC{0,200}", entry in "\\PC{0,40}") {
        for p in [Policy::default(), blocking(), allow(&[&entry]),
                  Policy { allow_file: true, allowed_domains: vec![entry.clone()], ..Default::default() }] {
            let _ = p.check(&s);
            let _ = p.restricts();
            let _ = p.host_resolver_rules();
        }
    }

    #[test]
    fn never_panics_on_urlish_input(s in "(https?|file|ftp)?:?/{0,3}[\\[\\]a-z0-9.:@%\\\\/?#*-]{0,60}") {
        let _ = blocking().check(&s);
        let _ = allow(&["a.com", "[::1]:80", "*.b.com", "::1", "x:y:z"]).check(&s);
    }
}

#[test]
fn non_http_schemes_are_refused() {
    for u in [
        "javascript:1",
        "data:text/html,x",
        "ftp://a.com/",
        "chrome://settings",
        "file:///etc/passwd",
        "view-source:http://a.com",
    ] {
        assert!(Policy::default().check(u).is_err(), "{u}");
    }
    assert!(Policy::default().check("about:blank").is_ok());
}

/// Deprecated IPv4-compatible IPv6 (`::a.b.c.d`) is not unwrapped by `private_v6`/`is_metadata_host`, so these
/// pass. Most OS stacks do not route them to IPv4, so this is hardening rather than a known exploit.
#[test]
fn ipv4_compatible_ipv6_is_blocked() {
    for u in ["http://[::127.0.0.1]/", "http://[::10.0.0.1]/"] {
        assert!(blocking().check(u).is_err(), "{u}");
    }
    assert!(Policy::default()
        .check("http://[::169.254.169.254]/")
        .is_err());
}
