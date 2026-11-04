//! Property tests for matching and normalisation. In-crate so they can reach `normalize`.
use std::collections::{BTreeMap, HashSet};

use proptest::prelude::*;
use serde_json::{json, Value};

use crate::resolve::normalize;
use crate::*;

fn field(r: &str, label: &str, name: Option<String>, disabled: bool) -> Field {
    serde_json::from_value(json!({
        "ref": r, "kind": "text", "label": label, "name": name, "disabled": disabled
    }))
    .unwrap()
}

fn opt(value: &str, label: &str, disabled: bool) -> FieldOption {
    FieldOption {
        value: value.into(),
        label: label.into(),
        selected: false,
        disabled,
    }
}

fn any_json() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        "\\PC{0,20}".prop_map(Value::from),
        prop::collection::vec("\\PC{0,5}", 0..3).prop_map(|v| json!(v)),
    ]
}

proptest! {
    #[test]
    fn normalize_is_idempotent_and_canonical(s in "\\PC{0,80}") {
        let n = normalize(&s);
        prop_assert_eq!(normalize(&n), n.clone());
        prop_assert!(!n.starts_with(' ') && !n.ends_with(' ') && !n.contains("  "));
    }

    #[test]
    fn date_normalisation_never_panics_and_is_idempotent(s in "\\PC{0,30}") {
        if let Ok(iso) = normalize_date(&s) {
            prop_assert_eq!(normalize_date(&iso), Ok(iso.clone()));
            prop_assert_eq!(iso.len(), 10);
        }
    }

    #[test]
    fn numeric_dates_never_silently_guess_day_vs_month(
        a in 1u32..=12, b in 1u32..=12, y in 1900i32..=2100, sep in prop::sample::select(vec!['/', '-', '.']),
    ) {
        prop_assume!(a != b);
        let s = format!("{a}{sep}{b}{sep}{y}");
        prop_assert!(matches!(normalize_date(&s), Err(ValueError::Ambiguous(_))), "{s}");
    }

    #[test]
    fn unambiguous_numeric_dates_resolve_correctly(
        d in 13u32..=28, m in 1u32..=12, y in 1900i32..=2100,
    ) {
        let want = format!("{y:04}-{m:02}-{d:02}");
        prop_assert_eq!(normalize_date(&format!("{d}/{m}/{y}")), Ok(want.clone()));
        prop_assert_eq!(normalize_date(&format!("{m}/{d}/{y}")), Ok(want));
    }

    #[test]
    fn time_normalisation_never_panics_and_is_idempotent(s in "\\PC{0,20}") {
        if let Ok(t) = normalize_time(&s) {
            prop_assert_eq!(normalize_time(&t), Ok(t.clone()));
            let (h, m) = t.split_once(':').unwrap();
            prop_assert!(h.parse::<u32>().unwrap() < 24 && m.parse::<u32>().unwrap() < 60);
        }
    }

    #[test]
    fn coerce_bool_never_panics(v in any_json()) {
        let _ = coerce_bool(&v);
    }

    #[test]
    fn match_option_never_panics_and_never_picks_disabled(
        opts in prop::collection::vec(("\\PC{0,8}", "\\PC{0,8}", any::<bool>()), 0..6),
        wanted in any_json(),
    ) {
        let options: Vec<_> = opts.iter().map(|(v, l, d)| opt(v, l, *d)).collect();
        if let Ok(i) = match_option(&options, &wanted) {
            prop_assert!(i < options.len());
            prop_assert!(!options[i].disabled);
        }
    }

    #[test]
    fn match_option_never_resolves_a_shared_prefix_between_options(
        w in "[a-z]{3,8}", x in "[a-z]{2,5}", y in "[a-z]{2,5}",
    ) {
        prop_assume!(x != y);
        let options = vec![opt("1", &format!("{w} {x}"), false), opt("2", &format!("{w} {y}"), false)];
        prop_assume!(w != x && w != y);
        prop_assert!(matches!(match_option(&options, &json!(w)), Err(ValueError::Ambiguous(_))));
    }

    #[test]
    fn match_option_finds_an_exact_label(
        labels in prop::collection::hash_set("[A-Za-z]{4,10}", 1..6), pick in any::<prop::sample::Index>(),
    ) {
        let labels: Vec<_> = labels.into_iter().collect();
        let norm: HashSet<_> = labels.iter().map(|l| normalize(l)).collect();
        prop_assume!(norm.len() == labels.len());
        let options: Vec<_> = labels.iter().enumerate().map(|(i, l)| opt(&format!("v{i}"), l, false)).collect();
        let i = pick.index(options.len());
        prop_assert_eq!(match_option(&options, &json!(labels[i])), Ok(i));
    }

    #[test]
    fn resolve_never_panics_and_accounts_for_every_key(
        specs in prop::collection::vec(("[a-z0-9]{1,4}", "\\PC{0,12}", prop::option::of("[a-z_]{0,8}"), any::<bool>()), 0..6),
        keys in prop::collection::btree_map("\\PC{0,12}", any_json(), 0..5),
    ) {
        let fields: Vec<_> = specs.iter().enumerate()
            .map(|(i, (r, l, n, d))| field(&format!("{r}{i}"), l, n.clone(), *d)).collect();
        let res = resolve_values(&fields, &keys);
        prop_assert_eq!(res.matched.len() + res.unmatched.len(), keys.len());
        let mut seen = HashSet::new();
        for (i, k, _) in &res.matched {
            prop_assert!(*i < fields.len());
            prop_assert!(seen.insert(*i), "field {i} claimed twice");
            prop_assert!(!fields[*i].disabled, "matched a disabled field via {k:?}");
        }
        let sorted: Vec<_> = res.matched.iter().map(|m| m.0).collect();
        prop_assert!(sorted.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn resolve_never_guesses_between_identical_labels(label in "[a-z]{4,10}( [a-z]{3,8})?") {
        let fields = vec![
            field("e1", &label, None, false),
            field("e2", &label, None, false),
        ];
        let keys: BTreeMap<String, Value> = [(label.clone(), json!("x"))].into();
        let res = resolve_values(&fields, &keys);
        prop_assert!(res.matched.is_empty(), "silently picked one of two identical labels: {res:?}");
    }

    #[test]
    fn resolve_by_unique_ref_always_matches(n in 1usize..6, pick in any::<prop::sample::Index>()) {
        let fields: Vec<_> = (0..n).map(|i| field(&format!("e{i}"), "same", None, false)).collect();
        let i = pick.index(n);
        let keys: BTreeMap<String, Value> = [(format!("e{i}"), json!("x"))].into();
        let res = resolve_values(&fields, &keys);
        prop_assert_eq!(res.matched.len(), 1);
        prop_assert_eq!(res.matched[0].0, i);
    }

    #[test]
    fn password_fields_are_always_sensitive(label in "\\PC{0,20}", name in prop::option::of("\\PC{0,10}")) {
        let mut f = field("e1", &label, name, false);
        f.kind = FieldKind::Password;
        prop_assert!(f.is_sensitive());
        let _ = f.display();
    }

    #[test]
    fn fingerprint_ignores_order_hidden_fields_and_host_case(
        labels in prop::collection::vec("[a-z ]{1,10}", 1..5), host in "[a-z]{1,8}\\.com",
    ) {
        let fields: Vec<Field> = labels.iter().enumerate()
            .map(|(i, l)| field(&format!("e{i}"), l, None, false)).collect();
        let form = |fields: Vec<Field>| Form {
            r#ref: "f".into(), name: None, action: None, method: None, headings: vec![],
            fields, submit: vec![], step: None, errors: vec![], concealed: vec![],
        };
        let mut rev = fields.clone();
        rev.reverse();
        let mut extra = fields.clone();
        let mut hidden = field("h", "secret hidden", None, false);
        hidden.hidden = true;
        extra.push(hidden);
        let base = form(fields).fingerprint(&host);
        prop_assert_eq!(form(rev).fingerprint(&host.to_uppercase()), base.clone());
        prop_assert_eq!(form(extra).fingerprint(&host), base);
    }
}
