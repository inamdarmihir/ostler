//! Web research building blocks for Ostler. Async, network allowed, no browser dependency.
//!
//! * [`search`]: search providers (SearXNG, Brave) behind one trait.
//! * [`robots`]: RFC 9309 robots.txt parsing, matching and caching.
//! * [`budget`]: crawl budgets and per-host politeness limits.
//! * [`ledger`]: the source ledger (what was read, what could not be, and why).
//! * [`fetch_plan`]: a pure planner for fetching many URLs under a budget.
//!
//! Nothing here bypasses bot walls, paywalls or robots rules; those outcomes are recorded as gaps.

pub mod budget;
pub mod fetch_plan;
pub mod ledger;
pub mod robots;
pub mod search;
#[cfg(test)]
mod testutil;

/// Lowercase hex of a byte slice.
pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Strip control and invisible/bidi characters, collapse whitespace runs to single spaces, trim, cap length.
pub(crate) fn sanitise_text(s: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut last_space = true;
    for c in s.chars() {
        let invisible = matches!(
            c,
            '\u{200B}'..='\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{2069}'
                | '\u{FEFF}'
        );
        if invisible {
            continue;
        }
        if c.is_whitespace() || c.is_control() {
            if c.is_whitespace() && !last_space {
                out.push(' ');
                last_space = true;
            }
            continue;
        }
        out.push(c);
        last_space = false;
    }
    out.trim_end().chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitise_strips_controls_and_caps() {
        assert_eq!(sanitise_text("a\u{0}b\n\n c\u{202E}d  ", 100), "ab cd");
        assert_eq!(sanitise_text("abcdef", 3), "abc");
    }
}
