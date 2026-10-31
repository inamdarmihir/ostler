//! Tool catalogue. Descriptions are deliberately terse: every byte of schema is paid for
//! in the agent's context on every request.

use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Profile {
    /// navigation + interaction only
    Core,
    /// core + forms
    Forms,
    /// forms + memory + stats
    #[default]
    All,
}

impl std::str::FromStr for Profile {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "core" => Ok(Self::Core),
            "forms" => Ok(Self::Forms),
            "all" => Ok(Self::All),
            other => Err(format!("unknown tool profile {other:?} (core|forms|all)")),
        }
    }
}

struct Def {
    name: &'static str,
    desc: &'static str,
    props: Value,
    required: &'static [&'static str],
    min: Profile,
}

fn rank(p: Profile) -> u8 {
    match p {
        Profile::Core => 0,
        Profile::Forms => 1,
        Profile::All => 2,
    }
}

fn observe() -> Value {
    json!({"type":"string","enum":["delta","full","none"],"description":"snapshot returned after the action (default delta)"})
}

fn defs() -> Vec<Def> {
    vec![
        Def { name: "navigate", desc: "Open a URL. Returns page state with interactive elements.", min: Profile::Core, required: &["url"],
            props: json!({"url":{"type":"string"},"observe":{"type":"string","enum":["interactive","full","none"],"description":"default interactive"}}) },
        Def { name: "snapshot", desc: "Read the page as text with element refs (e1, e2…). Use delta to see only changes.", min: Profile::Core, required: &[],
            props: json!({"mode":{"type":"string","enum":["full","delta","interactive"]},"ref":{"type":"string","description":"limit to subtree"},"max_chars":{"type":"integer"}}) },
        Def { name: "click", desc: "Click an element by ref.", min: Profile::Core, required: &["ref"],
            props: json!({"ref":{"type":"string"},"observe":observe()}) },
        Def { name: "type", desc: "Type into an element by ref, replacing its content.", min: Profile::Core, required: &["ref","text"],
            props: json!({"ref":{"type":"string"},"text":{"type":"string"},"submit":{"type":"boolean","description":"press Enter after"},"observe":observe()}) },
        Def { name: "press", desc: "Press a key, e.g. Enter, Tab, Escape, Control+a.", min: Profile::Core, required: &["key"],
            props: json!({"key":{"type":"string"},"observe":observe()}) },
        Def { name: "wait_for", desc: "Wait until text appears/disappears or a CSS selector shows.", min: Profile::Core, required: &[],
            props: json!({"text":{"type":"string"},"text_gone":{"type":"string"},"selector":{"type":"string"},"timeout_ms":{"type":"integer"}}) },
        Def { name: "screenshot", desc: "Capture a PNG of the page or one element.", min: Profile::Core, required: &[],
            props: json!({"ref":{"type":"string"},"full_page":{"type":"boolean"}}) },
        Def { name: "back", desc: "Go back in history.", min: Profile::Core, required: &[], props: json!({}) },
        Def { name: "tabs", desc: "Manage tabs (they share cookies and the context tool). list | new [url] | switch tab | close tab. Popups open as new tabs.", min: Profile::Core, required: &["action"],
            props: json!({"action":{"type":"string","enum":["list","new","switch","close"]},"tab":{"type":"string","description":"tab id, e.g. t2"},"url":{"type":"string"}}) },
        Def { name: "context", desc: "Notes shared by every tab (and later calls). get|set|delete|list. Keep short facts, not page dumps.", min: Profile::Core, required: &["action"],
            props: json!({"action":{"type":"string","enum":["get","set","delete","list"]},"key":{"type":"string"},"value":{"type":"string"}}) },
        Def { name: "form_inspect", desc: "List the page's forms as typed fields (label, kind, required, options, errors) plus submit controls and what memory knows.", min: Profile::Forms, required: &[],
            props: json!({"form":{"type":"string","description":"form ref"}}) },
        Def { name: "form_fill", desc: "Fill many fields at once from {label|name|ref: value}. Handles selects, radios, checkbox groups, dates, files, comboboxes, dependent dropdowns; verifies each field. auto=required|all fills the rest from memory.", min: Profile::Forms, required: &[],
            props: json!({"values":{"type":"object","additionalProperties":true},"form":{"type":"string"},"auto":{"type":"string","enum":["off","required","all"]},"min_confidence":{"type":"number"}}) },
        Def { name: "form_submit", desc: "Click the form's submit/next control and report whether it was accepted, advanced to another step, or failed validation.", min: Profile::Forms, required: &[],
            props: json!({"form":{"type":"string"},"control":{"type":"string","description":"submit button ref"},"confirm":{"type":"boolean","description":"required when the server runs with --require-confirm-submit"}}) },
        Def { name: "fill_field", desc: "Set one field by ref (any kind, including custom widgets).", min: Profile::Forms, required: &["ref","value"],
            props: json!({"ref":{"type":"string"},"value":{}}) },
        Def { name: "memory_remember", desc: "Save a fact about the user for future forms (e.g. email, company). Secrets are refused.", min: Profile::All, required: &["key","value"],
            props: json!({"key":{"type":"string"},"value":{},"description":{"type":"string"},"autocomplete":{"type":"string","description":"HTML autocomplete token, e.g. given-name"}}) },
        Def { name: "memory_recall", desc: "Semantic search over saved facts.", min: Profile::All, required: &["query"],
            props: json!({"query":{"type":"string"},"limit":{"type":"integer"}}) },
        Def { name: "memory_list", desc: "List all saved facts.", min: Profile::All, required: &[], props: json!({}) },
        Def { name: "memory_forget", desc: "Delete a saved fact by key.", min: Profile::All, required: &["key"], props: json!({"key":{"type":"string"}}) },
        Def { name: "stats", desc: "Session cost/latency counters: calls, ms, estimated tokens, browser memory.", min: Profile::All, required: &[], props: json!({}) },
    ]
}

pub fn list(profile: Profile) -> Vec<Value> {
    defs()
        .into_iter()
        .filter(|d| rank(d.min) <= rank(profile))
        .map(|d| {
            json!({
                "name": d.name,
                "description": d.desc,
                "inputSchema": {"type":"object","properties": d.props, "required": d.required}
            })
        })
        .collect()
}

pub fn allowed(profile: Profile, name: &str) -> Option<bool> {
    defs()
        .into_iter()
        .find(|d| d.name == name)
        .map(|d| rank(d.min) <= rank(profile))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_nest() {
        let (c, f, a) = (
            list(Profile::Core).len(),
            list(Profile::Forms).len(),
            list(Profile::All).len(),
        );
        assert!(c < f && f < a);
    }

    #[test]
    fn schemas_are_small() {
        // A guard on the context cost of just *having* the server connected.
        let bytes = serde_json::to_string(&list(Profile::All)).unwrap().len();
        assert!(bytes < 7000, "tool schemas are {bytes} bytes");
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
