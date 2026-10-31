//! Learning-loop evaluation: does what Ostler learned on a site still help after the site changes?
//!
//! A scripted site with three forms is "visited" and learned. Then the site ships redesigns (new
//! fingerprints: reordered fields, a removed field, a new field, one renamed label) plus a sibling
//! form, and an unrelated site uses the same labels. For every field we ask memory for a value and
//! grade it against the ground truth: correct, wrong, or empty.
//!
//! * **baseline**: facts only, which is what any new or redesigned form got before the learning
//!   loop (the exact-fingerprint playbook misses).
//! * **learned**: the same facts plus skills from the first visit.
//!
//! Wrong fills are the metric that matters most: an empty field costs the agent one turn, a wrong
//! value typed confidently can cost the user. Runs on the in-memory store always, and on a real
//! Qdrant server (score-boosting formula in the engine) when QDRANT_URL or OSTLER_QDRANT_BIN is set.

mod common;

use std::sync::Arc;

use ostler_memory::*;
use serde_json::json;

const FACTS: &[(&str, &str)] = &[
    ("name", "Ada Lovelace"),
    ("email", "ada@example.com"),
    ("phone", "+44 20 7946 0000"),
    ("company", "Analytical Engines"),
    ("job_title", "Engineer"),
    ("city", "London"),
];

/// (label, kind, ground truth profile key or None when the field must stay empty)
type Form = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str, Option<&'static str>)],
);

const FIRST_VISIT: &[Form] = &[
    (
        "checkout-v1",
        "shop.example",
        &[
            ("Ship to", "text", Some("name")),
            ("Reach me at", "tel", Some("phone")),
            ("Org", "text", Some("company")),
            ("Town", "text", Some("city")),
        ],
    ),
    (
        "account-v1",
        "shop.example",
        &[
            ("Display name", "text", Some("name")),
            ("Work mail", "email", Some("email")),
            ("Role", "text", Some("job_title")),
        ],
    ),
    (
        "support-v1",
        "shop.example",
        &[
            ("Callback number", "tel", Some("phone")),
            ("Org", "text", Some("company")),
        ],
    ),
];

const LATER: &[Form] = &[
    // Redesign: reordered, "Town" removed, a new field with no fact behind it.
    (
        "checkout-v2",
        "shop.example",
        &[
            ("Org", "text", Some("company")),
            ("Promo code", "text", None),
            ("Ship to", "text", Some("name")),
            ("Reach me at", "tel", Some("phone")),
        ],
    ),
    // Redesign with one renamed label: "Work mail" became "Work e-mail address".
    (
        "account-v2",
        "shop.example",
        &[
            ("Display name", "text", Some("name")),
            ("Work e-mail address", "email", Some("email")),
            ("Role", "text", Some("job_title")),
        ],
    ),
    // A sibling form never seen before, sharing some labels.
    (
        "returns-v1",
        "shop.example",
        &[
            ("Ship to", "text", Some("name")),
            ("Callback number", "tel", Some("phone")),
            ("Friend's name", "text", None),
            ("Order number", "text", None),
        ],
    ),
    // Another site with the same idiosyncratic labels: must not borrow shop.example's skills.
    (
        "bank-v1",
        "bank.example",
        &[
            ("Org", "text", Some("company")),
            ("Reach me at", "tel", Some("phone")),
        ],
    ),
];

#[derive(Default, Debug, Clone, Copy)]
struct Score {
    correct: u32,
    wrong: u32,
    empty: u32,
}

async fn grade(m: &Memory, forms: &[Form]) -> Score {
    let mut s = Score::default();
    for (fp, host, fields) in forms {
        for (label, kind, truth) in *fields {
            let q = FieldQuery {
                host,
                fingerprint: fp,
                label,
                name: None,
                kind,
                placeholder: None,
                autocomplete: None,
            };
            // Same floor and default min_confidence as the MCP server.
            let got = m
                .suggest(&q, 0.3)
                .await
                .unwrap()
                .filter(|r| r.confidence >= 0.55)
                .map(|r| r.fact.key);
            match (got.as_deref(), truth) {
                (None, _) => s.empty += 1,
                (Some(g), Some(t)) if g == *t => s.correct += 1,
                _ => s.wrong += 1,
            }
        }
    }
    s
}

async fn seed_facts(m: &Memory) {
    for (k, v) in FACTS {
        m.remember(k, json!(v), None, None, "user").await.unwrap();
    }
}

async fn learn_first_visit(m: &Memory) {
    for (fp, host, fields) in FIRST_VISIT {
        let outcomes = fields
            .iter()
            .map(|(label, kind, truth)| FieldOutcome {
                label: label.to_string(),
                kind: kind.to_string(),
                profile_key: truth.map(String::from),
                ok: true,
            })
            .collect();
        m.record_outcome(host, fp, &[], outcomes, true, None)
            .await
            .unwrap();
    }
}

async fn run(name: &str, make: impl Fn(&str) -> Arc<dyn Store>) -> (Score, Score) {
    let base = Memory::new(make("baseline"), format!("{name}-baseline"));
    base.init().await.unwrap();
    seed_facts(&base).await;
    let baseline = grade(&base, LATER).await;

    let learned_mem = Memory::new(make("learned"), format!("{name}-learned"));
    learned_mem.init().await.unwrap();
    seed_facts(&learned_mem).await;
    learn_first_visit(&learned_mem).await;
    let learned = grade(&learned_mem, LATER).await;

    let total = LATER.iter().map(|f| f.2.len()).sum::<usize>();
    eprintln!(
        "\nlearning eval [{name}] over {total} fields on redesigned / sibling / foreign forms"
    );
    eprintln!(
        "  {:<10} {:>8} {:>6} {:>6}",
        "", "correct", "wrong", "empty"
    );
    for (label, s) in [("baseline", baseline), ("learned", learned)] {
        eprintln!(
            "  {label:<10} {:>8} {:>6} {:>6}",
            s.correct, s.wrong, s.empty
        );
    }
    (baseline, learned)
}

fn check(baseline: Score, learned: Score) {
    assert!(
        learned.correct > baseline.correct,
        "learning should fill more fields correctly: {baseline:?} -> {learned:?}"
    );
    assert!(
        learned.wrong <= baseline.wrong,
        "learning must not add wrong fills: {baseline:?} -> {learned:?}"
    );
}

#[tokio::test]
async fn learning_transfers_across_redesigns_in_memory() {
    let (b, l) = run("in-memory", |_| {
        Arc::new(InMemoryStore::new(Arc::new(HashEmbedder::default())))
    })
    .await;
    check(b, l);
}

#[tokio::test]
async fn learning_transfers_across_redesigns_on_real_qdrant() {
    let Some(server) = common::start().await else {
        return;
    };
    let s: Arc<dyn Store> = common::store(&server.url, 256);
    // Tenants keep the two conditions apart inside the same collections.
    let (b, l) = run("qdrant", move |_| s.clone()).await;
    check(b, l);
}
