//! Runs the memory layer against a real Qdrant server.
//!
//! Either set QDRANT_URL (an already-running server) or OSTLER_QDRANT_BIN (we spawn it
//! on free ports with a temp storage dir). With neither set the tests skip, unless
//! OSTLER_REQUIRE_QDRANT=1 (CI), in which case they fail.

mod common;

use common::{start, store};
use ostler_memory::*;
use serde_json::json;

#[tokio::test]
async fn memory_roundtrip_on_real_qdrant() {
    let Some(srv) = start().await else { return };
    let s = store(&srv.url, 256);
    let m = Memory::new(s.clone(), "alice");
    m.init().await.unwrap();
    m.init().await.unwrap(); // idempotent

    m.remember("email", json!("ada@example.com"), None, None, "user")
        .await
        .unwrap();
    m.remember(
        "company",
        json!("Analytical Engines"),
        Some("employer"),
        None,
        "user",
    )
    .await
    .unwrap();
    m.remember("email", json!("ada@new.example"), None, None, "user")
        .await
        .unwrap(); // upsert
    let facts = m.facts(50).await.unwrap();
    assert_eq!(facts.len(), 2, "{facts:?}");
    assert_eq!(
        m.fact("email").await.unwrap().unwrap().value,
        json!("ada@new.example")
    );

    // Semantic search returns the right fact first.
    let top = m.recall("email address", 2).await.unwrap();
    assert_eq!(top[0].0.key, "email", "{top:?}");

    // Field suggestion via the deterministic autocomplete path and the semantic path.
    let q = FieldQuery {
        host: "h",
        fingerprint: "fp",
        label: "Contact",
        name: None,
        kind: "email",
        placeholder: None,
        autocomplete: Some("email"),
    };
    assert_eq!(
        m.suggest(&q, 0.3).await.unwrap().unwrap().via,
        "autocomplete"
    );

    // Playbooks and episodes persist and aggregate.
    for _ in 0..2 {
        m.record_success(
            "h",
            "fp",
            &["Contact".into()],
            vec![PlaybookField::new("Contact", "email", Some("email".into()))],
        )
        .await
        .unwrap();
        m.record_episode("h", "fp", "ZIP", "validation_error", "ZIP must be 5 digits")
            .await
            .unwrap();
    }
    assert_eq!(m.playbook("fp").await.unwrap().unwrap().uses, 2);
    let eps = m.episodes("fp", 10).await.unwrap();
    assert_eq!((eps.len(), eps[0].count), (1, 2));

    // Tenant isolation holds on the real server, including under semantic search.
    let bob = Memory::new(s.clone(), "bob");
    assert!(bob.facts(50).await.unwrap().is_empty());
    assert!(bob.recall("email address", 5).await.unwrap().is_empty());
    assert!(bob.playbook("fp").await.unwrap().is_none());

    // Forget + wipe.
    assert!(m.forget("company").await.unwrap());
    assert_eq!(m.facts(50).await.unwrap().len(), 1);
    m.wipe().await.unwrap();
    assert!(m.facts(50).await.unwrap().is_empty());
    assert!(m.playbook("fp").await.unwrap().is_none());
}

#[tokio::test]
async fn dimension_mismatch_detected_on_real_qdrant() {
    let Some(srv) = start().await else { return };
    store(&srv.url, 256).ensure().await.unwrap();
    let err = store(&srv.url, 384).ensure().await.unwrap_err().to_string();
    assert!(err.contains("256") && err.contains("384"), "{err}");
}

#[tokio::test]
async fn many_tenants_and_concurrent_writers() {
    let Some(srv) = start().await else { return };
    let s = store(&srv.url, 256);
    Memory::new(s.clone(), "init").init().await.unwrap();
    let mut tasks = vec![];
    for t in 0..20 {
        let s = s.clone();
        tasks.push(tokio::spawn(async move {
            let m = Memory::new(s, format!("tenant{t}"));
            for k in 0..5 {
                m.remember(
                    &format!("fact_{k}"),
                    json!(format!("v{t}-{k}")),
                    None,
                    None,
                    "user",
                )
                .await
                .unwrap();
            }
            m.facts(100).await.unwrap().len()
        }));
    }
    for t in tasks {
        assert_eq!(t.await.unwrap(), 5);
    }
    // Per tenant: the collection also holds one embedding-model marker point, so a whole-collection count is 101.
    let mut n = 0;
    for t in 0..20 {
        n += s
            .count(Kind::Profile, &Filter::tenant(&format!("tenant{t}")))
            .await
            .unwrap();
    }
    assert_eq!(n, 100);
}
