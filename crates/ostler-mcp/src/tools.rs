//! Tool catalogue. Descriptions are deliberately terse: every byte of schema is paid for
//! in the agent's context on every request.

use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Profile {
    /// navigation, interaction and `read`
    Core,
    /// core + forms
    Forms,
    /// core + research (`cite`, `evidence`, `search`, `open_many`, `extract`, `links`)
    Research,
    /// everything: forms + memory + stats + research
    All,
    /// Lists only the core tools until a page with form fields is seen (then adds forms and memory) or
    /// the first `read`/`search` (then adds the research tools), and sends `tools/list_changed`. Every
    /// tool stays callable, so a client that never refreshes its list is told the names in the result.
    #[default]
    Auto,
}

impl std::str::FromStr for Profile {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "core" => Ok(Self::Core),
            "forms" => Ok(Self::Forms),
            "research" => Ok(Self::Research),
            "all" => Ok(Self::All),
            "auto" => Ok(Self::Auto),
            other => Err(format!(
                "unknown tool profile {other:?} (auto|core|forms|research|all)"
            )),
        }
    }
}

/// What a tool is for. A profile (and, for `auto`, what the session has seen) selects groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    Core,
    Forms,
    Memory,
    Research,
}

struct Def {
    name: &'static str,
    desc: &'static str,
    props: Value,
    required: &'static [&'static str],
    group: Group,
}

/// Groups listed for `profile`. `forms_seen` / `research_seen` only matter for `auto`.
fn shows(profile: Profile, group: Group, forms_seen: bool, research_seen: bool) -> bool {
    match (profile, group) {
        (_, Group::Core) | (Profile::All, _) => true,
        (Profile::Forms, Group::Forms) | (Profile::Research, Group::Research) => true,
        (Profile::Auto, Group::Forms | Group::Memory) => forms_seen,
        (Profile::Auto, Group::Research) => research_seen,
        _ => false,
    }
}

fn observe() -> Value {
    json!({"type":"string","enum":["delta","full","none"],"description":"snapshot returned after the action (default delta)"})
}

fn defs() -> Vec<Def> {
    use Group::{Core, Forms, Memory, Research};
    vec![
        Def { name: "navigate", desc: "Open a URL. Returns page state with interactive elements.", group: Core, required: &["url"],
            props: json!({"url":{"type":"string"},"observe":{"type":"string","enum":["interactive","full","none"],"description":"default interactive"}}) },
        Def { name: "snapshot", desc: "Read the page as text with element refs (e1, e2…). Use delta to see only changes. Pass the ref of a heading to read just that section.", group: Core, required: &[],
            props: json!({"mode":{"type":"string","enum":["full","delta","interactive"]},"ref":{"type":"string","description":"limit to subtree"},"max_chars":{"type":"integer"}}) },
        Def { name: "read", desc: "Read a page for research (opens url if given). With query: just the best passages. Without: the start. Only pages you read can be cited. Flags paywalls, bot walls, changed pages, text aimed at AI.", group: Core, required: &[],
            props: json!({"url":{"type":"string"},"query":{"type":"string"},"k":{"type":"integer","description":"passages, default 5"},"max_chars":{"type":"integer","description":"default 8000"}}) },
        Def { name: "click", desc: "Click an element by ref.", group: Core, required: &["ref"],
            props: json!({"ref":{"type":"string"},"observe":observe()}) },
        Def { name: "type", desc: "Type into an element by ref, replacing its content.", group: Core, required: &["ref","text"],
            props: json!({"ref":{"type":"string"},"text":{"type":"string"},"submit":{"type":"boolean","description":"press Enter after"},"observe":observe()}) },
        Def { name: "press", desc: "Press a key, e.g. Enter, Tab, Escape, Control+a.", group: Core, required: &["key"],
            props: json!({"key":{"type":"string"},"observe":observe()}) },
        Def { name: "hover", desc: "Move the pointer over an element by ref (opens menus and tooltips).", group: Core, required: &["ref"],
            props: json!({"ref":{"type":"string"},"observe":observe()}) },
        Def { name: "scroll", desc: "Scroll an element into view (ref) or the page (direction up|down|top|bottom, optional amount in px). Reports position as scrolled/max.", group: Core, required: &[],
            props: json!({"ref":{"type":"string"},"direction":{"type":"string","enum":["up","down","top","bottom"]},"amount":{"type":"integer"},"observe":observe()}) },
        Def { name: "wait_for", desc: "Wait until text appears/disappears or a CSS selector shows, or just pause for ms milliseconds (max 10000).", group: Core, required: &[],
            props: json!({"text":{"type":"string"},"text_gone":{"type":"string"},"selector":{"type":"string"},"timeout_ms":{"type":"integer"},"ms":{"type":"integer","description":"plain pause"}}) },
        Def { name: "wait_for_human", desc: "A CAPTCHA/verification wall is blocking the page: wait for the user to complete it in the browser window (needs --headed). Ostler never solves these itself.", group: Core, required: &[],
            props: json!({"timeout_s":{"type":"integer","description":"default 120, max 600"}}) },
        Def { name: "screenshot", desc: "Capture a PNG of the page or one element.", group: Core, required: &[],
            props: json!({"ref":{"type":"string"},"full_page":{"type":"boolean"}}) },
        Def { name: "look", desc: "See where to click: a screenshot with every clickable element boxed and numbered by its ref, plus a legend saying what each does (type here, submits the form, error…). Use refs with click/type/fill_field.", group: Core, required: &[],
            props: json!({"max":{"type":"integer"}}) },
        Def { name: "search_tools", desc: "Find Ostler tools without listing them all. detail: names | brief (default) | full (with input schema). Every tool is callable whether or not it is listed.", group: Core, required: &[],
            props: json!({"query":{"type":"string"},"detail":{"type":"string","enum":["names","brief","full"]}}) },
        Def { name: "back", desc: "Go back in history.", group: Core, required: &[], props: json!({}) },
        Def { name: "tabs", desc: "Manage tabs (they share cookies and the context tool). list | new [url] | switch tab | close tab. Popups open as new tabs.", group: Core, required: &["action"],
            props: json!({"action":{"type":"string","enum":["list","new","switch","close"]},"tab":{"type":"string","description":"tab id, e.g. t2"},"url":{"type":"string"}}) },
        Def { name: "context", desc: "Notes shared by every tab (and later calls). get|set|delete|list. Keep short facts, not page dumps.", group: Core, required: &["action"],
            props: json!({"action":{"type":"string","enum":["get","set","delete","list"]},"key":{"type":"string"},"value":{"type":"string"}}) },
        Def { name: "cite", desc: "Check a quote against a page you read. VERIFIED returns the page's exact words and a link to check them; anything else means do not use that claim. Quote the page, not your summary.", group: Research, required: &["url","quote"],
            props: json!({"url":{"type":"string"},"quote":{"type":"string","description":"words from the page (12+ chars); ... marks an omission"}}) },
        Def { name: "evidence", desc: "What you have read and may state: verified citations, stale ones, and gaps (pages you could not read: paywall, bot wall, robots, errors). Call before answering.", group: Research, required: &[], props: json!({}) },
        Def { name: "search", desc: "Web search (needs OSTLER_SEARXNG_URL or a Brave key). Returns titles, URLs and snippets; read a result before relying on it.", group: Research, required: &["query"],
            props: json!({"query":{"type":"string"},"limit":{"type":"integer","description":"default 8, max 20"},"recency":{"type":"string","enum":["day","week","month","year"]},"site":{"type":"string","description":"host name, e.g. example.com"}}) },
        Def { name: "open_many", desc: "Read several URLs (max 10) in a scratch tab, respecting robots.txt and per-host limits. With query: the best passages from each. Reports each page's outcome; your current tab is untouched.", group: Research, required: &["urls"],
            props: json!({"urls":{"type":"array","items":{"type":"string"}},"query":{"type":"string"},"k":{"type":"integer","description":"passages per page (default 2)"},"force":{"type":"boolean","description":"re-read pages already read"}}) },
        Def { name: "extract", desc: "Structured data from the current page: tables (rows) or lists (repeated cards, with pagination hints and whether the list looks complete).", group: Research, required: &["kind"],
            props: json!({"kind":{"type":"string","enum":["tables","lists"]},"max_rows":{"type":"integer","description":"per table or list (default 30)"},"where":{"type":"string","description":"keep only rows containing any of a|b|c"},"select":{"type":"array","items":{},"description":"table columns to keep (header name or index)"}}) },
        Def { name: "links", desc: "Links on the current page with anchor text and context, optionally filtered.", group: Research, required: &[],
            props: json!({"same_origin":{"type":"boolean"},"text_contains":{"type":"string"},"href_contains":{"type":"string"},"limit":{"type":"integer","description":"default 30"},"where":{"type":"string","description":"keep only lines containing any of a|b|c"}}) },
        Def { name: "run", desc: "Run a short Rhai script for multi-page jobs; only its result returns. tool(\"name\", #{args}) runs an Ostler tool and returns its text; also grep(text, word), lines(text), print(x). No files or network; 60 calls max.", group: Research, required: &["script"],
            props: json!({"script":{"type":"string"},"max_chars":{"type":"integer","description":"default 8000"}}) },
        Def { name: "form_inspect", desc: "List the page's forms as typed fields (label, kind, required, options, errors) plus submit controls and what memory knows.", group: Forms, required: &[],
            props: json!({"form":{"type":"string","description":"form ref"}}) },
        Def { name: "form_fill", desc: "Fill many fields at once from {label|name|ref: value}; a value {{fact:KEY}} uses that saved fact without you seeing it. Handles selects, radios, checkbox groups, dates, files, comboboxes, dependent dropdowns; verifies each field. auto=required|all fills the rest from memory.", group: Forms, required: &[],
            props: json!({"values":{"type":"object","additionalProperties":true},"form":{"type":"string"},"auto":{"type":"string","enum":["off","required","all"]},"min_confidence":{"type":"number"}}) },
        Def { name: "form_submit", desc: "Click the form's submit/next control and report whether it was accepted, advanced to another step, or failed validation.", group: Forms, required: &[],
            props: json!({"form":{"type":"string"},"control":{"type":"string","description":"submit button ref"},"confirm":{"type":"boolean","description":"required when the server runs with --require-confirm-submit"},"again":{"type":"boolean","description":"resubmit a form submitted moments ago"}}) },
        Def { name: "fill_field", desc: "Set one field by ref (any kind, including custom widgets).", group: Forms, required: &["ref","value"],
            props: json!({"ref":{"type":"string"},"value":{}}) },
        Def { name: "memory_remember", desc: "Save a fact about the user for future forms (e.g. email, company). Secrets are refused.", group: Memory, required: &["key","value"],
            props: json!({"key":{"type":"string"},"value":{},"description":{"type":"string"},"autocomplete":{"type":"string","description":"HTML autocomplete token, e.g. given-name"}}) },
        Def { name: "memory_recall", desc: "Semantic search over saved facts.", group: Memory, required: &["query"],
            props: json!({"query":{"type":"string"},"limit":{"type":"integer"}}) },
        Def { name: "memory_list", desc: "List all saved facts.", group: Memory, required: &[], props: json!({}) },
        Def { name: "memory_forget", desc: "Delete a saved fact by key.", group: Memory, required: &["key"], props: json!({"key":{"type":"string"}}) },
        Def { name: "stats", desc: "Session cost/latency counters: calls, ms, estimated tokens, browser memory.", group: Memory, required: &[], props: json!({}) },
    ]
}

pub fn list(profile: Profile) -> Vec<Value> {
    list_revealed(profile, false, false)
}

/// Tools to show now. Only `Auto` depends on what the session has seen: a page with form fields, and a
/// first `read` or `search`.
pub fn list_revealed(profile: Profile, forms_seen: bool, research_seen: bool) -> Vec<Value> {
    defs()
        .into_iter()
        .filter(|d| shows(profile, d.group, forms_seen, research_seen))
        .map(|d| {
            json!({
                "name": d.name,
                "description": d.desc,
                "inputSchema": {"type":"object","properties": d.props, "required": d.required}
            })
        })
        .collect()
}

/// `search_tools`: tools whose name or description mention every word of `query` (all tools when
/// empty), at the requested level of detail. Searches the whole catalogue, not just what is listed.
pub fn search(query: &str, detail: &str) -> String {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let hits: Vec<Def> = defs()
        .into_iter()
        .filter(|d| {
            let hay = format!("{} {}", d.name, d.desc).to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .collect();
    if hits.is_empty() {
        return format!(
            "No tool matches {query:?}. Try search_tools with no query to list every name."
        );
    }
    let mut out = format!("{} tool(s):\n", hits.len());
    for d in hits {
        match detail {
            "names" => {
                out.push_str(d.name);
                out.push('\n');
            }
            "full" => {
                let schema = json!({"type":"object","properties": d.props, "required": d.required});
                out.push_str(&format!("{}: {}\n  input: {}\n", d.name, d.desc, schema));
            }
            _ => out.push_str(&format!("{}: {}\n", d.name, d.desc)),
        }
    }
    out.trim_end().to_string()
}

/// Whether `name` may be called under `profile`. `None` for an unknown tool. `auto` allows everything
/// (listing is what it defers), so a client that never refreshes its tool list still works.
pub fn allowed(profile: Profile, name: &str) -> Option<bool> {
    defs()
        .into_iter()
        .find(|d| d.name == name)
        .map(|d| profile == Profile::Auto || shows(profile, d.group, true, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: Vec<Value>) -> Vec<String> {
        v.iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn profiles_nest() {
        let (c, f, a) = (
            list(Profile::Core).len(),
            list(Profile::Forms).len(),
            list(Profile::All).len(),
        );
        assert!(c < f && f < a);
        let r = list(Profile::Research).len();
        assert!(c < r && r < a);
    }

    #[test]
    fn read_is_always_visible_but_the_citation_tools_are_not_core() {
        let core = names(list(Profile::Core));
        assert!(core.contains(&"read".to_string()));
        assert!(!core.contains(&"cite".to_string()));
        let auto = names(list_revealed(Profile::Auto, false, false));
        assert_eq!(auto, core);
    }

    #[test]
    fn auto_reveals_each_group_on_its_own_trigger() {
        let both = list_revealed(Profile::Auto, true, true).len();
        assert_eq!(both, list(Profile::All).len());
        let forms = names(list_revealed(Profile::Auto, true, false));
        assert!(forms.contains(&"form_fill".to_string()) && !forms.contains(&"cite".to_string()));
        let research = names(list_revealed(Profile::Auto, false, true));
        assert!(
            research.contains(&"cite".to_string()) && !research.contains(&"form_fill".to_string())
        );
        assert_eq!(allowed(Profile::Auto, "form_fill"), Some(true));
        assert_eq!(allowed(Profile::Auto, "cite"), Some(true));
        assert_eq!(allowed(Profile::Core, "cite"), Some(false));
        assert_eq!(allowed(Profile::Research, "cite"), Some(true));
        assert_eq!(allowed(Profile::Research, "form_fill"), Some(false));
    }

    #[test]
    fn schemas_are_small() {
        // A guard on the context cost of just *having* the server connected.
        let all = serde_json::to_string(&list(Profile::All)).unwrap().len();
        assert!(all < 11_000, "all tool schemas are {all} bytes");
        let auto = serde_json::to_string(&list(Profile::Auto)).unwrap().len();
        assert!(auto < 5_400, "the starting tool list is {auto} bytes");
    }

    #[test]
    fn search_tools_finds_hidden_tools_at_each_detail_level() {
        let names = search("citation", "names");
        assert!(names.contains("evidence"), "{names}");
        assert!(names.lines().skip(1).all(|l| !l.contains(':')), "{names}");
        let full = search("form_fill", "full");
        assert!(
            full.contains("input: {") && full.contains("values"),
            "{full}"
        );
        let brief = search("", "brief");
        assert!(brief.contains("form_submit:") && brief.contains("search_tools:"));
        assert!(search("zeppelin", "brief").starts_with("No tool matches"));
    }

    #[test]
    fn names_unique_and_required_exist_in_props() {
        let mut seen = std::collections::HashSet::new();
        for d in defs() {
            assert!(seen.insert(d.name));
            for r in d.required {
                assert!(d.props.get(r).is_some(), "{} missing prop {}", d.name, r);
            }
        }
    }
}
