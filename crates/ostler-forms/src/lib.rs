//! Form intermediate representation and the pure logic around it.
//!
//! The browser crate extracts a [`FormIr`] from a live page; this crate knows
//! how to match an agent's `{key: value}` input to fields, normalise values
//! (options, dates, booleans), spot sensitive fields, and fingerprint a form
//! so memory can recognise it again.

mod resolve;
mod values;

pub use resolve::{resolve_values, Candidate, Resolution, Unmatched};
pub use values::{coerce_bool, match_option, normalize_date, normalize_time, ValueError};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    Text,
    Email,
    Tel,
    Url,
    Search,
    Password,
    Number,
    Textarea,
    Select,
    MultiSelect,
    Checkbox,
    Radio,
    Date,
    Time,
    DatetimeLocal,
    Month,
    Week,
    Range,
    Color,
    File,
    Combobox,
    Contenteditable,
    Switch,
}

impl FieldKind {
    /// Kinds filled by typing characters into a focused element.
    pub fn is_textual(self) -> bool {
        use FieldKind::*;
        matches!(
            self,
            Text | Email | Tel | Url | Search | Password | Number | Textarea | Contenteditable
        )
    }
    /// Kinds that take a native value string set programmatically.
    pub fn is_native_value(self) -> bool {
        use FieldKind::*;
        matches!(
            self,
            Date | Time | DatetimeLocal | Month | Week | Range | Color
        )
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FieldOption {
    pub value: String,
    pub label: String,
    #[serde(default)]
    pub selected: bool,
    #[serde(default)]
    pub disabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Field {
    #[serde(rename = "ref")]
    pub r#ref: String,
    pub kind: FieldKind,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    pub readonly: bool,
    #[serde(default)]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub autocomplete: Option<String>,
    #[serde(default)]
    pub pattern: Option<String>,
    #[serde(default)]
    pub min: Option<String>,
    #[serde(default)]
    pub max: Option<String>,
    #[serde(default)]
    pub step: Option<String>,
    #[serde(default)]
    pub minlength: Option<i64>,
    #[serde(default)]
    pub maxlength: Option<i64>,
    #[serde(default)]
    pub options: Vec<FieldOption>,
    #[serde(default)]
    pub value: Value,
    #[serde(default)]
    pub section: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    /// Not currently visible/interactable (e.g. collapsed wizard step).
    #[serde(default)]
    pub hidden: bool,
    /// A radio/checkbox group whose options are separately clickable controls (not a native `<select>`).
    #[serde(default)]
    pub grouped: bool,
    /// For a text input that wants a date in a particular shape: that shape, e.g. `mm/dd/yyyy`.
    #[serde(default)]
    pub format: Option<String>,
}

impl Field {
    pub fn display(&self) -> String {
        let pick = |o: &Option<String>| o.clone().filter(|t| !t.trim().is_empty());
        if !self.label.is_empty() {
            self.label.clone()
        } else if let Some(n) = pick(&self.name).or_else(|| pick(&self.placeholder)) {
            n
        } else if let Some(i) = pick(&self.id) {
            i
        } else {
            // Nothing on the page names it: say what it is, not an opaque ref (the ref is shown beside it).
            let kind = serde_json::to_value(self.kind)
                .ok()
                .and_then(|v| v.as_str().map(|s| s.replace('_', " ")))
                .unwrap_or_default();
            format!("unlabelled {kind}")
        }
    }

    /// Fields whose values must never be stored in memory or auto-filled from it.
    pub fn is_sensitive(&self) -> bool {
        if self.kind == FieldKind::Password {
            return true;
        }
        if let Some(ac) = &self.autocomplete {
            let ac = ac.to_lowercase();
            if ac.contains("password") || ac.contains("cc-") || ac.contains("one-time-code") {
                return true;
            }
        }
        let hay = format!(
            "{} {} {}",
            self.label.to_lowercase(),
            self.name.as_deref().unwrap_or("").to_lowercase(),
            self.id.as_deref().unwrap_or("").to_lowercase()
        );
        const MARKERS: &[&str] = &[
            "password",
            "passcode",
            "card number",
            "cardnumber",
            "card-number",
            "cvv",
            "cvc",
            "security code",
            "ssn",
            "social security",
            "pin code",
            "secret",
            "iban",
            "routing number",
            "account number",
            "otp",
            "one-time",
        ];
        MARKERS.iter().any(|m| hay.contains(m))
    }

    pub fn is_empty_value(&self) -> bool {
        match &self.value {
            Value::Null => true,
            Value::String(s) => s.is_empty(),
            Value::Bool(b) => !*b,
            Value::Array(a) => a.is_empty(),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitControl {
    #[serde(rename = "ref")]
    pub r#ref: String,
    pub label: String,
    /// "submit" | "next" | "button"
    #[serde(default)]
    pub role: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StepInfo {
    pub index: u32,
    pub total: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Form {
    #[serde(rename = "ref")]
    pub r#ref: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub headings: Vec<String>,
    pub fields: Vec<Field>,
    #[serde(default)]
    pub submit: Vec<SubmitControl>,
    #[serde(default)]
    pub step: Option<StepInfo>,
    /// Page-level validation/alert messages near this form.
    #[serde(default)]
    pub errors: Vec<String>,
    /// Labels of controls that exist but are not showing right now (a collapsed step, a branch another
    /// answer reveals). Lets a miss say "hidden" instead of "not found".
    #[serde(default)]
    pub concealed: Vec<String>,
}

impl Form {
    /// Stable identity for memory lookups: host + the shape of the visible fields.
    pub fn fingerprint(&self, host: &str) -> String {
        let mut parts: Vec<String> = self
            .fields
            .iter()
            .filter(|f| !f.hidden)
            .map(|f| {
                format!(
                    "{:?}|{}|{}",
                    f.kind,
                    resolve::normalize(&f.label),
                    resolve::normalize(f.name.as_deref().unwrap_or(""))
                )
            })
            .collect();
        parts.sort();
        let mut h = Sha256::new();
        h.update(host.to_lowercase().as_bytes());
        for p in parts {
            h.update(b"\n");
            h.update(p.as_bytes());
        }
        let digest = h.finalize();
        digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormIr {
    pub url: String,
    pub title: String,
    pub forms: Vec<Form>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(label: &str, kind: FieldKind) -> Field {
        serde_json::from_value(serde_json::json!({"ref": "e1", "kind": kind, "label": label}))
            .unwrap()
    }

    #[test]
    fn sensitive_detection() {
        assert!(field("Password", FieldKind::Password).is_sensitive());
        assert!(field("Card number", FieldKind::Text).is_sensitive());
        assert!(field("CVV", FieldKind::Text).is_sensitive());
        assert!(!field("Email", FieldKind::Email).is_sensitive());
        assert!(!field("First name", FieldKind::Text).is_sensitive());
    }

    #[test]
    fn fingerprint_is_order_independent_and_host_scoped() {
        let mk = |labels: &[&str]| Form {
            r#ref: "f1".into(),
            name: None,
            action: None,
            method: None,
            headings: vec![],
            fields: labels.iter().map(|l| field(l, FieldKind::Text)).collect(),
            submit: vec![],
            step: None,
            errors: vec![],
            concealed: vec![],
        };
        let a = mk(&["Name", "Email"]);
        let b = mk(&["Email", "Name"]);
        assert_eq!(a.fingerprint("example.com"), b.fingerprint("EXAMPLE.com"));
        assert_ne!(a.fingerprint("example.com"), a.fingerprint("other.com"));
        assert_ne!(
            a.fingerprint("example.com"),
            mk(&["Name"]).fingerprint("example.com")
        );
    }
}

#[cfg(test)]
mod prop_tests;
