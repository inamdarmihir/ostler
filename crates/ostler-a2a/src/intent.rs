//! Turn an A2A message into a form task. Ostler is a tool, not a language model: it understands
//! structured requests (a `data` part) and a bare URL in text, and says so plainly otherwise.

use std::collections::BTreeMap;

use ostler_mcp::FormTask;
use serde_json::Value;

#[derive(Debug, Default)]
pub struct Intent {
    pub task: FormTask,
    /// The message actually carried something to act on.
    pub understood: bool,
}

pub const HELP: &str = "I fill and submit web forms. Send a data part like \
{\"url\": \"https://…\", \"values\": {\"Email\": \"a@b.co\"}, \"submit\": true}. \
Omit values to just inspect the form. \"auto\": \"required\" fills the rest from memory. \
When I need something I answer input-required; reply on the same task with more values or {\"confirm\": true}.";

fn first_url(text: &str) -> Option<String> {
    text.split_whitespace()
        .map(|w| {
            w.trim_start_matches(|c: char| "<([\"'".contains(c))
                .trim_end_matches(|c: char| ">)]\"',.;:!?".contains(c))
        })
        .find(|w| w.starts_with("http://") || w.starts_with("https://"))
        .map(String::from)
}

fn merge(task: &mut FormTask, o: &serde_json::Map<String, Value>) -> bool {
    let mut any = false;
    if let Some(u) = o.get("url").and_then(Value::as_str) {
        task.url = Some(u.to_string());
        any = true;
    }
    for key in ["values", "fields"] {
        if let Some(Value::Object(m)) = o.get(key) {
            let vals: BTreeMap<String, Value> =
                m.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            task.values.extend(vals);
            any = true;
        }
    }
    if let Some(a) = o.get("auto").and_then(Value::as_str) {
        task.auto = Some(a.to_string());
        any = true;
    }
    if let Some(b) = o.get("submit").and_then(Value::as_bool) {
        task.submit = b;
        any = true;
    }
    if let Some(b) = o.get("confirm").and_then(Value::as_bool) {
        task.confirm = b;
        any = true;
    }
    any
}

pub fn parse(message: &Value) -> Intent {
    let mut task = FormTask::default();
    let mut understood = false;
    let Some(parts) = message.get("parts").and_then(Value::as_array) else {
        return Intent::default();
    };
    for p in parts {
        if let Some(Value::Object(o)) = p.get("data") {
            understood |= merge(&mut task, o);
        } else if let Some(t) = p.get("text").and_then(Value::as_str) {
            // Some clients put the JSON in a text part.
            if let Ok(Value::Object(o)) = serde_json::from_str::<Value>(t.trim()) {
                understood |= merge(&mut task, &o);
            } else if task.url.is_none() {
                if let Some(u) = first_url(t) {
                    task.url = Some(u);
                    understood = true;
                }
            }
        }
    }
    Intent { task, understood }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn data_part_is_read() {
        let m = json!({"parts":[{"data":{"url":"https://x.test/f","values":{"Email":"a@b.co"},"submit":true}}]});
        let i = parse(&m);
        assert!(i.understood);
        assert_eq!(i.task.url.as_deref(), Some("https://x.test/f"));
        assert_eq!(i.task.values["Email"], json!("a@b.co"));
        assert!(i.task.submit && !i.task.confirm);
    }

    #[test]
    fn bare_url_in_text_means_inspect() {
        let i = parse(&json!({"parts":[{"text":"please look at <https://x.test/form>."}]}));
        assert!(i.understood && i.task.values.is_empty());
        assert_eq!(i.task.url.as_deref(), Some("https://x.test/form"));
    }

    #[test]
    fn json_in_text_is_accepted_and_confirm_alone_is_understood() {
        let i = parse(&json!({"parts":[{"text":"{\"confirm\": true}"}]}));
        assert!(i.understood && i.task.confirm);
    }

    #[test]
    fn chatter_is_not_understood() {
        let i = parse(&json!({"parts":[{"text":"hello, can you book me a flight?"}]}));
        assert!(!i.understood);
        assert!(!parse(&json!({})).understood);
    }
}
