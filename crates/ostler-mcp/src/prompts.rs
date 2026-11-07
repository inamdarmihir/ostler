//! MCP prompt templates: ready-made tasks a client can show in a menu, so people click instead of writing prompts.
//! Each prompt only produces instructions for the assistant; it runs nothing by itself.

use serde_json::{json, Value};

struct Arg {
    name: &'static str,
    description: &'static str,
    required: bool,
}

struct Prompt {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    args: &'static [Arg],
    /// `{name}` placeholders are replaced with the argument values.
    template: &'static str,
}

const PROMPTS: &[Prompt] = &[
    Prompt {
        name: "summarize_page",
        title: "Summarize a web page",
        description: "Open a page and summarize it in plain language.",
        args: &[Arg { name: "url", description: "The page to open", required: true }],
        template: "Open {url} with Ostler and read it. Summarize it in a few short paragraphs in plain language, then list anything on the page you could not read.",
    },
    Prompt {
        name: "research_with_sources",
        title: "Research a topic with checked sources",
        description: "Research a question and cite only pages that were actually read, with every quote verified.",
        args: &[Arg { name: "question", description: "What you want to find out", required: true }],
        template: "Research this question: {question}\n\nUse Ostler's search and read tools. Support every claim with a quote checked by the cite tool, and call evidence before you answer. Only use pages you actually read. In your answer, list each source with its link, and say clearly which pages could not be read (paywall, blocked, error) so I know what is missing.",
    },
    Prompt {
        name: "fill_form_review_first",
        title: "Fill in a web form (review before sending)",
        description: "Fill a form from details you give, show what was entered, and wait for your approval before submitting.",
        args: &[
            Arg { name: "url", description: "The page with the form", required: true },
            Arg { name: "details", description: "What to enter, e.g. name, email, address", required: true },
        ],
        template: "Open {url} and fill in the form using these details: {details}\n\nUse form_inspect and form_fill. Show me the value read back for every field, and list any field you could not match or that is still required. Do NOT submit. Wait until I say to submit.",
    },
    Prompt {
        name: "check_my_site",
        title: "Check that my web page works",
        description: "Load a page, look for visible problems, and try its main action.",
        args: &[
            Arg { name: "url", description: "The page to check", required: true },
            Arg { name: "what_to_try", description: "The main thing a visitor should be able to do", required: false },
        ],
        template: "Open {url} with Ostler and check that it works. Report anything that looks broken (errors, missing content, dead links). Then try this as a visitor would: {what_to_try}. Say plainly what worked and what did not. Do not submit real data or make purchases.",
    },
    Prompt {
        name: "compare_pages",
        title: "Compare information across pages",
        description: "Read several pages and compare them side by side.",
        args: &[
            Arg { name: "urls", description: "The pages to compare, separated by spaces or commas", required: true },
            Arg { name: "what_to_compare", description: "What matters to you, e.g. price, features", required: true },
        ],
        template: "Read each of these pages with Ostler: {urls}\n\nCompare them on: {what_to_compare}. Present a short table, quote the exact text each figure came from using the cite tool, and note any page you could not read.",
    },
];

pub fn list() -> Value {
    Value::Array(
        PROMPTS
            .iter()
            .map(|p| {
                json!({
                    "name": p.name,
                    "title": p.title,
                    "description": p.description,
                    "arguments": p.args.iter().map(|a| json!({
                        "name": a.name, "description": a.description, "required": a.required
                    })).collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}

/// The prompt text for `name`, or a message to return as an invalid-params error.
pub fn get(name: &str, arguments: &Value) -> Result<Value, String> {
    let p = PROMPTS
        .iter()
        .find(|p| p.name == name)
        .ok_or_else(|| format!("unknown prompt {name:?}"))?;
    let mut text = p.template.to_string();
    for a in p.args {
        let value = arguments
            .get(a.name)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty());
        match (value, a.required) {
            (Some(v), _) => text = text.replace(&format!("{{{}}}", a.name), v),
            (None, true) => return Err(format!("prompt {name:?} needs the argument {:?}", a.name)),
            (None, false) => {
                text = text.replace(&format!("{{{}}}", a.name), "the page's main purpose")
            }
        }
    }
    Ok(json!({
        "description": p.description,
        "messages": [{"role": "user", "content": {"type": "text", "text": text}}],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_placeholder_has_an_argument() {
        for p in PROMPTS {
            let mut t = p.template.to_string();
            for a in p.args {
                t = t.replace(&format!("{{{}}}", a.name), "x");
            }
            assert!(!t.contains('{'), "{} has an undeclared placeholder", p.name);
        }
    }

    #[test]
    fn get_fills_and_validates() {
        let r = get("summarize_page", &json!({"url": "https://example.com"})).unwrap();
        assert!(r["messages"][0]["content"]["text"]
            .as_str()
            .unwrap()
            .contains("https://example.com"));
        assert!(get("summarize_page", &json!({})).is_err());
        assert!(get("nope", &json!({})).is_err());
        let r = get("check_my_site", &json!({"url": "https://a.test"})).unwrap();
        assert!(!r["messages"][0]["content"]["text"]
            .as_str()
            .unwrap()
            .contains('{'));
    }
}
