#![no_main]
//! Fields and keys derived from arbitrary text through matching and value normalisation.
use libfuzzer_sys::fuzz_target;
use ostler_forms::*;
use serde_json::json;
use std::collections::BTreeMap;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else { return };
    let parts: Vec<&str> = s.split('\n').collect();
    let fields: Vec<Field> = parts
        .iter()
        .enumerate()
        .filter_map(|(i, l)| {
            serde_json::from_value(json!({"ref": format!("e{i}"), "kind": "text", "label": l, "name": l})).ok()
        })
        .collect();
    let values: BTreeMap<String, serde_json::Value> =
        parts.iter().rev().map(|k| (k.to_string(), json!(k))).collect();
    let r = resolve_values(&fields, &values);
    assert_eq!(r.matched.len() + r.unmatched.len(), values.len());
    let options: Vec<FieldOption> = parts
        .iter()
        .map(|l| FieldOption { value: l.to_string(), label: l.to_string(), selected: false, disabled: false })
        .collect();
    for p in &parts {
        let _ = match_option(&options, &json!(p));
        let _ = normalize_date(p);
        let _ = normalize_time(p);
        let _ = coerce_bool(&json!(p));
    }
});
