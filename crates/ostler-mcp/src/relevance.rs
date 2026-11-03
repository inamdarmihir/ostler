//! Detects search-result pages whose visible text does not match the query, a sign the engine is
//! degrading results for an automated browser. Pure functions: no I/O, no page text is returned.

use url::Url;

const ENGINES: [&str; 7] = [
    "google.",
    "bing.com",
    "duckduckgo.com",
    "search.brave.com",
    "yahoo.com",
    "ecosia.org",
    "startpage.com",
];

const STOP: [&str; 40] = [
    "the", "and", "for", "with", "that", "this", "from", "are", "was", "were", "what", "which",
    "who", "how", "why", "when", "where", "can", "does", "you", "your", "not", "but", "has",
    "have", "had", "its", "into", "about", "near", "best", "vs", "any", "all", "out", "off",
    "than", "then", "too", "get",
];

/// The user's query if `url` is a known search-results page with a query parameter.
pub fn search_query(url: &str) -> Option<String> {
    let u = Url::parse(url).ok()?;
    let host = u.host_str()?.to_ascii_lowercase();
    if !ENGINES.iter().any(|e| host.contains(e)) {
        return None;
    }
    let keys: &[&str] = if host.contains("yahoo.com") {
        &["p", "q"]
    } else {
        &["q", "query"]
    };
    u.query_pairs()
        .find(|(k, v)| keys.contains(&k.as_ref()) && !v.trim().is_empty())
        .map(|(_, v)| v.into_owned())
}

fn words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect()
}

/// Significant query words: at least 3 chars and not a stop word.
fn significant(query: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for w in words(query) {
        if w.chars().count() < 3 || STOP.contains(&w.as_str()) || out.contains(&w) {
            continue;
        }
        out.push(w);
    }
    out
}

/// True when fewer than half of the query's significant words occur in `text`.
/// Queries with no significant words are never flagged.
pub fn mismatch(query: &str, text: &str) -> bool {
    let sig = significant(query);
    if sig.is_empty() {
        return false;
    }
    let have = words(text);
    let hit = sig.iter().filter(|w| have.contains(w)).count();
    hit * 2 < sig.len()
}

/// The query to warn about, or None when the page is fine or not a search page.
pub fn check(url: &str, visible_text: &str) -> Option<String> {
    let q = search_query(url)?;
    mismatch(&q, visible_text).then_some(q)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_engines_and_params() {
        assert_eq!(
            search_query("https://www.bing.com/search?q=rust+mcp"),
            Some("rust mcp".into())
        );
        assert_eq!(
            search_query("https://www.google.de/search?q=x1y"),
            Some("x1y".into())
        );
        assert_eq!(
            search_query("https://search.yahoo.com/search?p=cats"),
            Some("cats".into())
        );
        assert_eq!(search_query("https://example.com/search?q=rust"), None);
        assert_eq!(search_query("https://www.bing.com/search?q="), None);
        assert_eq!(search_query("https://www.bing.com/"), None);
    }

    #[test]
    fn matching_results_pass() {
        let t = "Rust MCP browser server - GitHub. A browser for agents written in Rust.";
        assert!(!mismatch("rust mcp browser", t));
    }

    #[test]
    fn unrelated_results_flag() {
        let t = "Best pizza recipes. Some results have been removed. Cooking tips";
        assert!(mismatch("rust mcp browser server", t));
    }

    #[test]
    fn stop_words_and_short_tokens_ignored() {
        assert!(!mismatch("what is the kubernetes of 42", "Kubernetes docs"));
        assert!(mismatch("what is the kubernetes", "pizza"));
        assert!(!mismatch("what is a 42", "anything"));
    }

    #[test]
    fn half_matches_is_enough() {
        assert!(!mismatch("alpha beta gamma delta", "alpha delta"));
        assert!(mismatch("alpha beta gamma delta", "alpha"));
    }

    #[test]
    fn check_combines() {
        assert_eq!(
            check("https://duckduckgo.com/?q=quantum+widgets", "cats dogs"),
            Some("quantum widgets".into())
        );
        assert_eq!(
            check(
                "https://duckduckgo.com/?q=quantum+widgets",
                "Quantum widgets inc"
            ),
            None
        );
        assert_eq!(check("https://example.com/?q=zzz", "nothing"), None);
    }
}
