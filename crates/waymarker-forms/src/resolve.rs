use std::collections::{BTreeMap, HashSet};

use serde::Serialize;
use serde_json::Value;

use crate::Field;

pub(crate) fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = true;
    for c in s.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
            last_space = false;
        } else if !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    out.trim_end().to_string()
}

fn tokens(s: &str) -> HashSet<String> {
    normalize(s)
        .split(' ')
        .filter(|t| !t.is_empty())
        .map(String::from)
        .collect()
}

/// Words that carry no identity in a form label.
const STOP: &[&str] = &[
    "your", "the", "a", "an", "of", "please", "enter", "required", "optional",
];

fn content_tokens(s: &str) -> HashSet<String> {
    tokens(s)
        .into_iter()
        .filter(|t| !STOP.contains(&t.as_str()))
        .collect()
}

/// How well does `key` identify `field`? 0.0 = not at all, 1.0 = exact ref.
fn score(key: &str, field: &Field) -> f32 {
    let k = key.trim().trim_start_matches('@');
    if k == field.r#ref {
        return 1.0;
    }
    let nk = normalize(k);
    if nk.is_empty() {
        return 0.0;
    }
    let exact = |o: &Option<String>| o.as_deref().map(normalize).is_some_and(|n| n == nk);
    if exact(&field.name) || exact(&field.id) {
        return 0.95;
    }
    let nl = normalize(&field.label);
    if !nl.is_empty() && nl == nk {
        return 0.9;
    }
    let kt = content_tokens(k);
    let lt = content_tokens(&field.label);
    if kt.is_empty() || lt.is_empty() {
        return 0.0;
    }
    let inter = kt.intersection(&lt).count() as f32;
    if inter == 0.0 {
        return 0.0;
    }
    // Key fully contained in label (or vice versa) is a strong, but not exact, signal.
    if kt.is_subset(&lt) || lt.is_subset(&kt) {
        let cover = inter / kt.len().max(lt.len()) as f32;
        return 0.62 + 0.25 * cover;
    }
    let union = kt.union(&lt).count() as f32;
    0.55 * inter / union
}

#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    #[serde(rename = "ref")]
    pub r#ref: String,
    pub label: String,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Unmatched {
    pub key: String,
    pub reason: String,
    pub candidates: Vec<Candidate>,
}

#[derive(Debug, Default)]
pub struct Resolution {
    /// (index into the fields slice, original key, value)
    pub matched: Vec<(usize, String, Value)>,
    pub unmatched: Vec<Unmatched>,
}

const MIN_SCORE: f32 = 0.6;
const MARGIN: f32 = 0.08;

/// Match each agent-supplied key to at most one field; a field is claimed once.
pub fn resolve_values(fields: &[Field], values: &BTreeMap<String, Value>) -> Resolution {
    let mut res = Resolution::default();
    let mut claimed: HashSet<usize> = HashSet::new();
    for (key, value) in values {
        let mut scored: Vec<(usize, f32)> = fields
            .iter()
            .enumerate()
            .filter(|(_, f)| !f.disabled)
            .map(|(i, f)| (i, score(key, f)))
            .filter(|(_, s)| *s > 0.0)
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        let cands = || {
            scored
                .iter()
                .take(3)
                .map(|(i, s)| Candidate {
                    r#ref: fields[*i].r#ref.clone(),
                    label: fields[*i].display(),
                    score: (*s * 100.0).round() / 100.0,
                })
                .collect::<Vec<_>>()
        };
        match scored.first() {
            None => res.unmatched.push(Unmatched {
                key: key.clone(),
                reason: "no field on the page matches this key".into(),
                candidates: vec![],
            }),
            Some(&(idx, s)) if s >= MIN_SCORE => {
                let runner_up = scored.get(1).map(|x| x.1).unwrap_or(0.0);
                if s < 0.9 && s - runner_up < MARGIN {
                    res.unmatched.push(Unmatched {
                        key: key.clone(),
                        reason: "ambiguous: several fields match equally well; use a ref".into(),
                        candidates: cands(),
                    });
                } else if claimed.contains(&idx) {
                    res.unmatched.push(Unmatched {
                        key: key.clone(),
                        reason: format!(
                            "field {} already targeted by another key",
                            fields[idx].r#ref
                        ),
                        candidates: cands(),
                    });
                } else {
                    claimed.insert(idx);
                    res.matched.push((idx, key.clone(), value.clone()));
                }
            }
            Some(_) => res.unmatched.push(Unmatched {
                key: key.clone(),
                reason: "no confident match".into(),
                candidates: cands(),
            }),
        }
    }
    // Fill in document order, not map order, so dependent fields behave predictably.
    res.matched.sort_by_key(|(i, _, _)| *i);
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FieldKind;
    use serde_json::json;

    fn f(r: &str, label: &str, name: Option<&str>) -> Field {
        serde_json::from_value(
            json!({"ref": r, "kind": FieldKind::Text, "label": label, "name": name}),
        )
        .unwrap()
    }

    fn vals(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn matches_by_ref_name_label() {
        let fields = vec![
            f("e1", "First name", Some("fname")),
            f("e2", "Email address", Some("email")),
        ];
        let r = resolve_values(
            &fields,
            &vals(&[("@e1", json!("Ada")), ("email", json!("a@b.c"))]),
        );
        assert_eq!(r.matched.len(), 2);
        assert!(r.unmatched.is_empty());
        let r = resolve_values(
            &fields,
            &vals(&[("first name", json!("Ada")), ("Email", json!("x"))]),
        );
        assert_eq!(r.matched.len(), 2);
    }

    #[test]
    fn partial_label_matches_when_unique() {
        let fields = vec![
            f("e1", "Your first name (required)", None),
            f("e2", "Last name", None),
        ];
        let r = resolve_values(&fields, &vals(&[("first name", json!("Ada"))]));
        assert_eq!(r.matched.len(), 1);
        assert_eq!(r.matched[0].0, 0);
    }

    #[test]
    fn ambiguity_is_reported_not_guessed() {
        let fields = vec![
            f("e1", "Billing phone number", None),
            f("e2", "Shipping phone number", None),
        ];
        let r = resolve_values(&fields, &vals(&[("phone", json!("1"))]));
        assert!(r.matched.is_empty());
        assert_eq!(r.unmatched.len(), 1);
        assert_eq!(r.unmatched[0].candidates.len(), 2);
    }

    #[test]
    fn unknown_key_is_unmatched() {
        let fields = vec![f("e1", "Email", Some("email"))];
        let r = resolve_values(&fields, &vals(&[("favourite colour", json!("blue"))]));
        assert!(r.matched.is_empty());
        assert_eq!(r.unmatched.len(), 1);
    }

    #[test]
    fn field_claimed_once_and_document_order() {
        let fields = vec![
            f("e1", "Email", Some("email")),
            f("e2", "Name", Some("name")),
        ];
        let r = resolve_values(
            &fields,
            &vals(&[
                ("name", json!("A")),
                ("Email", json!("B")),
                ("e1", json!("C")),
            ]),
        );
        // "Email" and "e1" both target e1; one is rejected.
        assert_eq!(r.matched.len(), 2);
        assert_eq!(r.unmatched.len(), 1);
        assert!(r.matched[0].0 < r.matched[1].0);
    }

    #[test]
    fn disabled_fields_are_skipped() {
        let mut d = f("e1", "Email", Some("email"));
        d.disabled = true;
        let r = resolve_values(&[d], &vals(&[("email", json!("x"))]));
        assert!(r.matched.is_empty());
    }
}
