//! The sealed store against a real Qdrant: the same Memory API works, and what the server holds is opaque.
//! Skips unless QDRANT_URL or OSTLER_QDRANT_BIN is set (see qdrant_real.rs).

mod common;

use std::sync::Arc;

use common::{start, store};
use ostler_memory::*;
use serde_json::json;

#[tokio::test]
async fn sealed_memory_roundtrips_and_the_server_sees_no_plaintext() {
    let Some(srv) = start().await else { return };
    let raw = store(&srv.url, 256);
    let sealed: Arc<dyn Store> = Arc::new(SealedStore::new(raw.clone(), [9; 32]));
    let m = Memory::new(sealed, "alice");
    m.init().await.unwrap();

    m.remember(
        "email",
        json!("ada@example.com"),
        Some("work email"),
        None,
        "user",
    )
    .await
    .unwrap();
    m.remember("city", json!("Pune"), None, None, "user")
        .await
        .unwrap();
    for _ in 0..2 {
        m.record_success(
            "shop.example",
            "fp-secret",
            &["Contact".into()],
            vec![PlaybookField::new("Contact", "email", Some("email".into()))],
        )
        .await
        .unwrap();
        m.record_episode(
            "shop.example",
            "fp-secret",
            "ZIP",
            "validation_error",
            "ZIP must be 5 digits",
        )
        .await
        .unwrap();
    }

    // Everything works through the filters, which now run on hashes.
    assert_eq!(m.facts(10).await.unwrap().len(), 2);
    assert_eq!(
        m.fact("email").await.unwrap().unwrap().value,
        json!("ada@example.com")
    );
    assert_eq!(
        m.recall("email address", 2).await.unwrap()[0].0.key,
        "email"
    );
    assert_eq!(m.playbook("fp-secret").await.unwrap().unwrap().uses, 2);
    let eps = m.episodes("fp-secret", 10).await.unwrap();
    assert_eq!((eps.len(), eps[0].count), (1, 2));
    let q = FieldQuery {
        host: "shop.example",
        fingerprint: "other-fp",
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

    // The server's own view of every collection.
    for kind in Kind::ALL {
        let all = raw.scroll(kind, &Filter::default(), 100).await.unwrap();
        assert!(!all.is_empty(), "{kind:?} should hold points");
        for h in all {
            let s = h.payload.to_string();
            for leak in [
                "ada@example.com",
                "work email",
                "Pune",
                "shop.example",
                "fp-secret",
                "Contact",
                "ZIP",
                "clinic.example",
                "Lovelace",
                "Cardiology",
            ] {
                assert!(!s.contains(leak), "{kind:?} leaks {leak:?}: {s}");
            }
        }
    }

    // Another tenant, and another key, see nothing.
    let sealed: Arc<dyn Store> = Arc::new(SealedStore::new(raw.clone(), [9; 32]));
    assert!(Memory::new(sealed, "bob")
        .facts(10)
        .await
        .unwrap()
        .is_empty());
    let wrong: Arc<dyn Store> = Arc::new(SealedStore::new(raw, [8; 32]));
    assert!(Memory::new(wrong, "alice")
        .facts(10)
        .await
        .unwrap()
        .is_empty());

    assert!(m.forget("city").await.unwrap());
    assert_eq!(m.facts(10).await.unwrap().len(), 1);
    m.wipe().await.unwrap();
    assert!(m.facts(10).await.unwrap().is_empty());
}

#[tokio::test]
async fn v1_points_on_a_real_qdrant_are_refused_then_migrated_to_v2() {
    let Some(srv) = start().await else { return };
    let raw = store(&srv.url, 256);
    let sealed: Arc<dyn Store> = Arc::new(SealedStore::new(raw.clone(), [4; 32]));
    let m = Memory::new(sealed, "mig");
    m.init().await.unwrap();
    // A fact written by an older build (`v1`), saved under the id that build derived.
    write_v1_point_for_tests(
        raw.clone(),
        [4; 32],
        Kind::Profile,
        &Memory::fact_id_for_tests("mig", "email"),
        "email",
        json!({"tenant": "mig", "key": "email", "key_norm": "email", "value": "old@example.com",
               "description": null, "autocomplete": "email", "source": "user", "updated_at": 11}),
    )
    .await
    .unwrap();
    assert!(
        m.fact("email").await.unwrap().is_none(),
        "a v1 point was read"
    );
    assert_eq!(
        probe_key(raw.clone(), [4; 32], "mig").await.unwrap(),
        KeyProbe::NeedsUpgrade {
            v1: 1,
            stale_keys: 0
        }
    );
    let rep = migrate_sealed(raw.clone(), [4; 32], "mig").await.unwrap();
    assert_eq!(rep.v1_upgraded, 1);
    let f = m.fact("email").await.unwrap().unwrap();
    assert_eq!((f.value, f.updated_at), (json!("old@example.com"), 11));
    assert_eq!(probe_key(raw, [4; 32], "mig").await.unwrap(), KeyProbe::Ok);
}
