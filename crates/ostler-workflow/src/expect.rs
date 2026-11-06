//! Postconditions ("expect"): a URL pattern, text or element that must be present or gone after an
//! action. The browser layer fills an [`Observation`]; [`evaluate`] says which expectations held.
//!
//! # Schema agents pass
//!
//! Terse object form (every key optional, each value a string or an array of strings):
//!
//! ```json
//! {"url":"*/thanks*", "text":"Order confirmed", "gone":"Checkout", "element":"button:Continue"}
//! ```
//!
//! | key | meaning |
//! |---|---|
//! | `url` | URL must match; a `*` makes it a glob over the whole URL, otherwise a substring |
//! | `url_contains` / `url_matches` | explicit forms of the above |
//! | `text` | text must be on the page (body or title) |
//! | `gone` | text must be absent (body and title) |
//! | `element` / `no_element` | `"role:name"`, `"name"` or `{"role":..,"name":..}` must be present / absent |
//!
//! Explicit list form: an array of such objects, `[{"url":"*/thanks*"},{"text":"Order confirmed"}]`.
//!
//! Matching is case- and whitespace-insensitive. Element names match as a substring, roles exactly.
//!
//! Limits: at most [`MAX_EXPECTS`] expectations, each string at most [`MAX_LEN`] characters.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{json, Map, Value};

use crate::one_line;

/// Most expectations accepted in one call.
pub const MAX_EXPECTS: usize = 32;
/// Longest string accepted in an expectation, in characters.
pub const MAX_LEN: usize = 2000;

const KEYS: &str = "url, url_contains, url_matches, text, gone, element, no_element";

/// One postcondition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expect {
    /// The URL contains the string (case-insensitive).
    UrlContains(String),
    /// The whole URL matches a glob where `*` matches any run of characters (case-insensitive).
    UrlMatches(String),
    /// The text appears in the page body or title.
    TextPresent(String),
    /// The text appears in neither the page body nor the title.
    TextAbsent(String),
    /// An element with this role and name exists. `None` means "any".
    ElementPresent {
        /// Exact (case-insensitive) role, e.g. `button`.
        role: Option<String>,
        /// Substring of the accessible name.
        name: Option<String>,
    },
    /// No element with this role and name exists.
    ElementAbsent {
        /// Exact (case-insensitive) role.
        role: Option<String>,
        /// Substring of the accessible name.
        name: Option<String>,
    },
}

impl std::fmt::Display for Expect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let el = |role: &Option<String>, name: &Option<String>| match (role, name) {
            (Some(r), Some(n)) => format!("{r}:\"{}\"", one_line(n, 80)),
            (Some(r), None) => r.clone(),
            (None, Some(n)) => format!("\"{}\"", one_line(n, 80)),
            (None, None) => "*".into(),
        };
        match self {
            Expect::UrlContains(s) => write!(f, "url contains \"{}\"", one_line(s, 80)),
            Expect::UrlMatches(s) => write!(f, "url matches \"{}\"", one_line(s, 80)),
            Expect::TextPresent(s) => write!(f, "text \"{}\"", one_line(s, 80)),
            Expect::TextAbsent(s) => write!(f, "gone \"{}\"", one_line(s, 80)),
            Expect::ElementPresent { role, name } => write!(f, "element {}", el(role, name)),
            Expect::ElementAbsent { role, name } => write!(f, "no element {}", el(role, name)),
        }
    }
}

impl Expect {
    /// Terse JSON form (single-key object), accepted back by [`parse_expect`].
    pub fn to_json(&self) -> Value {
        let el = |role: &Option<String>, name: &Option<String>| {
            let mut m = Map::new();
            if let Some(r) = role {
                m.insert("role".into(), json!(r));
            }
            if let Some(n) = name {
                m.insert("name".into(), json!(n));
            }
            Value::Object(m)
        };
        match self {
            Expect::UrlContains(s) => json!({"url_contains": s}),
            Expect::UrlMatches(s) => json!({"url_matches": s}),
            Expect::TextPresent(s) => json!({"text": s}),
            Expect::TextAbsent(s) => json!({"gone": s}),
            Expect::ElementPresent { role, name } => json!({"element": el(role, name)}),
            Expect::ElementAbsent { role, name } => json!({"no_element": el(role, name)}),
        }
    }

    /// Rebuild with every string passed through `f` (used by the trace redactor).
    pub(crate) fn map_strings(&self, f: &dyn Fn(&str) -> String) -> Expect {
        let o = |x: &Option<String>| x.as_deref().map(f);
        match self {
            Expect::UrlContains(s) => Expect::UrlContains(f(s)),
            Expect::UrlMatches(s) => Expect::UrlMatches(f(s)),
            Expect::TextPresent(s) => Expect::TextPresent(f(s)),
            Expect::TextAbsent(s) => Expect::TextAbsent(f(s)),
            Expect::ElementPresent { role, name } => Expect::ElementPresent {
                role: o(role),
                name: o(name),
            },
            Expect::ElementAbsent { role, name } => Expect::ElementAbsent {
                role: o(role),
                name: o(name),
            },
        }
    }
}

impl Serialize for Expect {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.to_json().serialize(s)
    }
}

impl<'de> Deserialize<'de> for Expect {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        let mut all = parse_expect(&v).map_err(serde::de::Error::custom)?;
        if all.len() == 1 {
            Ok(all.remove(0))
        } else {
            Err(serde::de::Error::custom("expected exactly one expectation"))
        }
    }
}

/// Why an expect value could not be parsed. The message names the path, e.g.
/// `expect[1].text: must be a non-empty string`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{path}: {msg}")]
pub struct ExpectError {
    /// Where in the input the problem is.
    pub path: String,
    /// What is wrong.
    pub msg: String,
}

fn err<T>(path: &str, msg: impl Into<String>) -> Result<T, ExpectError> {
    Err(ExpectError {
        path: path.to_string(),
        msg: msg.into(),
    })
}

/// Parse the agent-facing `expect` value (terse object, array of objects, or null) into a list.
pub fn parse_expect(v: &Value) -> Result<Vec<Expect>, ExpectError> {
    let mut out = Vec::new();
    match v {
        Value::Null => {}
        Value::Object(m) => parse_object(m, "expect", &mut out)?,
        Value::Array(items) => {
            for (i, it) in items.iter().enumerate() {
                let path = format!("expect[{i}]");
                match it {
                    Value::Object(m) => parse_object(m, &path, &mut out)?,
                    other => {
                        return err(
                            &path,
                            format!(
                                "must be an object like {{\"text\":\"..\"}}, got {}",
                                kind(other)
                            ),
                        )
                    }
                }
            }
        }
        other => {
            return err(
                "expect",
                format!(
                    "must be an object or an array of objects, got {}",
                    kind(other)
                ),
            )
        }
    }
    if out.len() > MAX_EXPECTS {
        return err(
            "expect",
            format!("too many expectations ({}, max {MAX_EXPECTS})", out.len()),
        );
    }
    Ok(out)
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

fn parse_object(
    m: &Map<String, Value>,
    path: &str,
    out: &mut Vec<Expect>,
) -> Result<(), ExpectError> {
    for (k, v) in m {
        let p = format!("{path}.{k}");
        match k.as_str() {
            "url" => {
                for s in strings(v, &p)? {
                    out.push(if s.contains('*') {
                        Expect::UrlMatches(s)
                    } else {
                        Expect::UrlContains(s)
                    });
                }
            }
            "url_contains" => out.extend(strings(v, &p)?.into_iter().map(Expect::UrlContains)),
            "url_matches" => out.extend(strings(v, &p)?.into_iter().map(Expect::UrlMatches)),
            "text" => out.extend(strings(v, &p)?.into_iter().map(Expect::TextPresent)),
            "gone" => out.extend(strings(v, &p)?.into_iter().map(Expect::TextAbsent)),
            "element" | "no_element" => {
                let present = k == "element";
                let items: Vec<&Value> = match v {
                    Value::Array(a) => a.iter().collect(),
                    one => vec![one],
                };
                for (i, it) in items.into_iter().enumerate() {
                    let ip = if v.is_array() {
                        format!("{p}[{i}]")
                    } else {
                        p.clone()
                    };
                    let (role, name) = element(it, &ip)?;
                    out.push(if present {
                        Expect::ElementPresent { role, name }
                    } else {
                        Expect::ElementAbsent { role, name }
                    });
                }
            }
            other => return err(&p, format!("unknown key `{other}` (valid: {KEYS})")),
        }
    }
    Ok(())
}

fn check_str(s: &str, p: &str) -> Result<String, ExpectError> {
    let t = s.trim();
    if t.is_empty() {
        return err(p, "must be a non-empty string");
    }
    if t.chars().count() > MAX_LEN {
        return err(p, format!("too long (max {MAX_LEN} characters)"));
    }
    Ok(t.to_string())
}

fn strings(v: &Value, p: &str) -> Result<Vec<String>, ExpectError> {
    match v {
        Value::String(s) => Ok(vec![check_str(s, p)?]),
        Value::Array(a) => {
            let mut out = Vec::new();
            for (i, x) in a.iter().enumerate() {
                match x {
                    Value::String(s) => out.push(check_str(s, &format!("{p}[{i}]"))?),
                    other => {
                        return err(
                            &format!("{p}[{i}]"),
                            format!("must be a non-empty string, got {}", kind(other)),
                        )
                    }
                }
            }
            Ok(out)
        }
        other => err(
            p,
            format!(
                "must be a non-empty string or array of strings, got {}",
                kind(other)
            ),
        ),
    }
}

type RoleName = (Option<String>, Option<String>);

fn element(v: &Value, p: &str) -> Result<RoleName, ExpectError> {
    match v {
        Value::String(s) => {
            let s = check_str(s, p)?;
            if let Some((r, n)) = s.split_once(':') {
                let r = r.trim();
                let plain_role =
                    !r.is_empty() && r.chars().all(|c| c.is_ascii_lowercase() || c == '-');
                if plain_role {
                    let n = n.trim();
                    return Ok((
                        Some(r.to_string()),
                        if n.is_empty() {
                            None
                        } else {
                            Some(n.to_string())
                        },
                    ));
                }
            }
            Ok((None, Some(s)))
        }
        Value::Object(m) => {
            let mut role = None;
            let mut name = None;
            for (k, x) in m {
                let kp = format!("{p}.{k}");
                match (k.as_str(), x) {
                    ("role", Value::String(s)) => role = Some(check_str(s, &kp)?.to_lowercase()),
                    ("name", Value::String(s)) => name = Some(check_str(s, &kp)?),
                    ("role" | "name", other) => {
                        return err(
                            &kp,
                            format!("must be a non-empty string, got {}", kind(other)),
                        )
                    }
                    (other, _) => {
                        return err(&kp, format!("unknown key `{other}` (valid: role, name)"))
                    }
                }
            }
            if role.is_none() && name.is_none() {
                return err(p, "needs a role or a name");
            }
            Ok((role, name))
        }
        other => err(
            p,
            format!(
                "must be \"role:name\" or {{\"role\",\"name\"}}, got {}",
                kind(other)
            ),
        ),
    }
}

/// An element as the browser layer sees it: accessibility role and accessible name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElementRef {
    /// Role, e.g. `button`, `link`, `textbox`.
    pub role: String,
    /// Accessible name.
    pub name: String,
}

/// What the page looks like after an action. Plain data; the browser layer fills it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Observation {
    /// Current URL.
    pub url: String,
    /// Document title.
    pub title: String,
    /// Visible page text.
    pub text: String,
    /// Interactive/named elements.
    pub elements: Vec<ElementRef>,
}

/// One unmet expectation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    /// The expectation that did not hold.
    pub expect: Expect,
    /// Short reason. May quote the observed URL (control characters stripped, truncated).
    pub why: String,
}

/// Outcome of [`evaluate`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpectResult {
    /// True when every expectation held (vacuously true for an empty list).
    pub ok: bool,
    /// Expectations that held.
    pub passed: Vec<Expect>,
    /// Expectations that did not.
    pub failed: Vec<Failure>,
}

impl ExpectResult {
    /// Number of expectations evaluated.
    pub fn total(&self) -> usize {
        self.passed.len() + self.failed.len()
    }

    /// Terse text for tool results: `EXPECT OK`, or `EXPECT FAILED: n of m` followed by one
    /// `- <expectation>: <why>` line per failure. Values are single-line and truncated; the
    /// `why` may quote page-derived URLs, so callers should still place it in the untrusted wrapper.
    pub fn render(&self) -> String {
        if self.ok {
            return "EXPECT OK".to_string();
        }
        let mut s = format!("EXPECT FAILED: {} of {}", self.failed.len(), self.total());
        for f in &self.failed {
            s.push_str(&format!("\n- {}: {}", f.expect, one_line(&f.why, 160)));
        }
        s
    }
}

/// Collapse whitespace and lowercase.
pub(crate) fn norm(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Glob match where `*` matches any run (including empty); everything else is literal.
/// Case-insensitive. Anchored at both ends.
pub(crate) fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);
    while ti < t.len() {
        if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

fn element_found(obs: &Observation, role: &Option<String>, name: &Option<String>) -> bool {
    let role = role.as_deref().map(norm);
    let name = name.as_deref().map(norm);
    obs.elements.iter().any(|e| {
        role.as_ref().is_none_or(|r| norm(&e.role) == *r)
            && name
                .as_ref()
                .is_none_or(|n| norm(&e.name).contains(n.as_str()))
    })
}

/// Check every expectation against the observation.
pub fn evaluate(expects: &[Expect], obs: &Observation) -> ExpectResult {
    let text = norm(&obs.text);
    let title = norm(&obs.title);
    let url = obs.url.to_lowercase();
    let mut res = ExpectResult::default();
    for e in expects {
        let why: Option<String> = match e {
            Expect::UrlContains(s) => (!url.contains(&s.to_lowercase()))
                .then(|| format!("url is \"{}\"", one_line(&obs.url, 100))),
            Expect::UrlMatches(g) => (!glob_match(g, &obs.url))
                .then(|| format!("url is \"{}\"", one_line(&obs.url, 100))),
            Expect::TextPresent(s) => {
                let n = norm(s);
                (!(text.contains(&n) || title.contains(&n))).then(|| "text not found".to_string())
            }
            Expect::TextAbsent(s) => {
                let n = norm(s);
                (text.contains(&n) || title.contains(&n)).then(|| "text still present".to_string())
            }
            Expect::ElementPresent { role, name } => {
                (!element_found(obs, role, name)).then(|| "no such element".to_string())
            }
            Expect::ElementAbsent { role, name } => {
                element_found(obs, role, name).then(|| "element still present".to_string())
            }
        };
        match why {
            None => res.passed.push(e.clone()),
            Some(why) => res.failed.push(Failure {
                expect: e.clone(),
                why,
            }),
        }
    }
    res.ok = res.failed.is_empty();
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs() -> Observation {
        Observation {
            url: "https://shop.example.com/checkout/thanks?id=7".into(),
            title: "Thanks".into(),
            text: "Order   confirmed\nYour order #A1234 is on its way".into(),
            elements: vec![
                ElementRef {
                    role: "button".into(),
                    name: "Continue shopping".into(),
                },
                ElementRef {
                    role: "link".into(),
                    name: "View order".into(),
                },
            ],
        }
    }

    #[test]
    fn terse_form() {
        let v = json!({"url":"*/thanks*","text":"Order confirmed","gone":"Checkout"});
        let e = parse_expect(&v).unwrap();
        assert!(e.contains(&Expect::UrlMatches("*/thanks*".into())));
        assert!(e.contains(&Expect::TextPresent("Order confirmed".into())));
        assert!(e.contains(&Expect::TextAbsent("Checkout".into())));
        let r = evaluate(&e, &obs());
        assert!(r.ok, "{}", r.render());
        assert_eq!(r.render(), "EXPECT OK");
    }

    #[test]
    fn list_form_and_arrays() {
        let v = json!([{"url":"thanks"},{"text":["a","b"]},{"element":["button:Continue","View"]}]);
        let e = parse_expect(&v).unwrap();
        assert_eq!(e.len(), 5);
        assert_eq!(e[0], Expect::UrlContains("thanks".into()));
        assert_eq!(
            e[3],
            Expect::ElementPresent {
                role: Some("button".into()),
                name: Some("Continue".into())
            }
        );
        assert_eq!(
            e[4],
            Expect::ElementPresent {
                role: None,
                name: Some("View".into())
            }
        );
    }

    #[test]
    fn null_and_empty_are_empty() {
        assert!(parse_expect(&Value::Null).unwrap().is_empty());
        assert!(parse_expect(&json!({})).unwrap().is_empty());
        assert!(parse_expect(&json!([])).unwrap().is_empty());
        assert!(evaluate(&[], &obs()).ok);
    }

    #[test]
    fn precise_errors() {
        let e = parse_expect(&json!({"txt":"x"})).unwrap_err();
        assert!(
            e.to_string().starts_with("expect.txt: unknown key `txt`"),
            "{e}"
        );
        let e = parse_expect(&json!([{"text":"ok"},{"text":""}])).unwrap_err();
        assert_eq!(e.path, "expect[1].text");
        let e = parse_expect(&json!({"text":5})).unwrap_err();
        assert!(e.msg.contains("got a number"), "{e}");
        let e = parse_expect(&json!({"text":["a",null]})).unwrap_err();
        assert_eq!(e.path, "expect.text[1]");
        let e = parse_expect(&json!("hello")).unwrap_err();
        assert!(e.msg.contains("got a string"));
        let e = parse_expect(&json!([1])).unwrap_err();
        assert_eq!(e.path, "expect[0]");
        let e = parse_expect(&json!({"element":{}})).unwrap_err();
        assert!(e.msg.contains("role or a name"));
        let e = parse_expect(&json!({"element":{"role":"b","x":1}})).unwrap_err();
        assert!(e.msg.contains("unknown key `x`"));
        let e = parse_expect(&json!({"text":"   "})).unwrap_err();
        assert!(e.msg.contains("non-empty"));
    }

    #[test]
    fn limits() {
        let long = "x".repeat(MAX_LEN + 1);
        assert!(parse_expect(&json!({"text":long})).is_err());
        let many: Vec<Value> = (0..=MAX_EXPECTS)
            .map(|i| json!({"text":format!("t{i}")}))
            .collect();
        assert!(parse_expect(&Value::Array(many)).is_err());
    }

    #[test]
    fn role_name_split_is_conservative() {
        // Capitalised prefix is a name, not a role.
        let e = parse_expect(&json!({"element":"Note: saved"})).unwrap();
        assert_eq!(
            e[0],
            Expect::ElementPresent {
                role: None,
                name: Some("Note: saved".into())
            }
        );
        let e = parse_expect(&json!({"no_element":"dialog:"})).unwrap();
        assert_eq!(
            e[0],
            Expect::ElementAbsent {
                role: Some("dialog".into()),
                name: None
            }
        );
    }

    #[test]
    fn failures_render_one_line_each() {
        let e = parse_expect(&json!({"url":"*/nope*","text":"Missing","gone":"Order confirmed"}))
            .unwrap();
        let r = evaluate(&e, &obs());
        assert!(!r.ok);
        assert_eq!(r.failed.len(), 3);
        let s = r.render();
        assert!(s.starts_with("EXPECT FAILED: 3 of 3"), "{s}");
        assert_eq!(s.lines().count(), 4);
        assert!(s.contains("text still present"));
    }

    #[test]
    fn text_is_case_and_whitespace_insensitive() {
        let r = evaluate(&[Expect::TextPresent("ORDER CONFIRMED".into())], &obs());
        assert!(r.ok);
        let r = evaluate(&[Expect::TextAbsent("order   confirmed".into())], &obs());
        assert!(!r.ok);
        let r = evaluate(&[Expect::TextPresent("thanks".into())], &obs());
        assert!(r.ok, "title counts");
    }

    #[test]
    fn globs() {
        assert!(glob_match("*", ""));
        assert!(glob_match("a*b*c", "aXXbYYc"));
        assert!(!glob_match("a*b", "abc"));
        assert!(glob_match("**x", "x"));
        assert!(glob_match("HTTPS://*/Thanks*", "https://a.b/thanks?x"));
        assert!(!glob_match("*thanks", "thanks!"));
        assert!(glob_match("a*a*a*a*a*b", &format!("{}b", "a".repeat(40))));
        // pathological input stays fast and correct
        assert!(!glob_match("a*a*a*a*a*c", &"a".repeat(2000)));
    }

    #[test]
    fn elements() {
        let r = evaluate(
            &[
                Expect::ElementPresent {
                    role: Some("BUTTON".into()),
                    name: Some("continue".into()),
                },
                Expect::ElementAbsent {
                    role: Some("dialog".into()),
                    name: None,
                },
                Expect::ElementPresent {
                    role: Some("link".into()),
                    name: Some("Continue".into()),
                },
                Expect::ElementAbsent {
                    role: None,
                    name: Some("view order".into()),
                },
            ],
            &obs(),
        );
        assert_eq!(r.passed.len(), 2);
        assert_eq!(r.failed.len(), 2);
    }

    #[test]
    fn url_failure_is_sanitised() {
        let mut o = obs();
        o.url = format!("https://x.test/\n{}", "a".repeat(500));
        let r = evaluate(&[Expect::UrlContains("zzz".into())], &o);
        let s = r.render();
        assert_eq!(s.lines().count(), 2, "{s}");
        assert!(s.len() < 400);
    }

    #[test]
    fn serde_roundtrip() {
        let e = parse_expect(&json!({"url":"*/x*","text":"a","gone":"b","element":"button:Go","no_element":{"role":"dialog"}})).unwrap();
        let s = serde_json::to_string(&e).unwrap();
        let back: Vec<Expect> = serde_json::from_str(&s).unwrap();
        assert_eq!(e, back);
        let r = evaluate(&e, &obs());
        let s = serde_json::to_string(&r).unwrap();
        let back: ExpectResult = serde_json::from_str(&s).unwrap();
        assert_eq!(r, back);
    }
}
