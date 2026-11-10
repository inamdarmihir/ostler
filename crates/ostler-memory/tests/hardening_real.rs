//! Adversarial tests against a real Qdrant: concurrency, odd tenant names, filter semantics and tampering.
//! Skips unless QDRANT_URL or OSTLER_QDRANT_BIN is set (see qdrant_real.rs); fails if OSTLER_REQUIRE_QDRANT=1.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{start, store as qdrant_store};
use ostler_memory::*;
use serde_json::{json, Value};

fn ok_outcome(label: &str, key: &str) -> FieldOutcome {
    FieldOutcome {
        label: label.into(),
        kind: "text".into(),
        profile_key: Some(key.into()),
        ok: true,
    }
}

async fn hammer(m: Memory, tasks: usize) {
    // Every task does a bit of everything on the same tenant, including the same episode and playbook.
    let mut hs = vec![];
    for i in 0..tasks {
        let m = m.clone();
        hs.push(tokio::spawn(async move {
            m.remember(
                &format!("key {}", i % 5),
                json!(format!("v{i}")),
                None,
                None,
                "user",
            )
            .await
            .unwrap();
            m.record_episode(
                "h",
                "fp-shared",
                "ZIP",
                "validation_error",
                "must be 5 digits",
            )
            .await
            .unwrap();
            m.record_outcome(
                "h",
                "fp-shared",
                &["Heading".into()],
                vec![ok_outcome("Name", "name")],
                true,
                None,
            )
            .await
            .unwrap();
            // Reads and housekeeping racing the writes.
            let _ = m.facts(100).await.unwrap();
            let _ = m.skills(100).await.unwrap();
            m.prune(86_400).await.unwrap();
            if i % 7 == 0 {
                let _ = m.forget("key 9").await.unwrap();
            }
            m.learn(&format!("learned {}", i % 3), json!(i))
                .await
                .unwrap();
        }));
    }
    for h in hs {
        h.await.expect("a task panicked");
    }
}

#[tokio::test]
async fn concurrent_writers_lose_no_counter_updates_and_never_panic() {
    for sealed in [false, true] {
        let Some(srv) = start().await else { return };
        let raw = qdrant_store(&srv.url, 64);
        let s: Arc<dyn Store> = if sealed {
            Arc::new(SealedStore::new(raw.clone(), [6; 32]))
        } else {
            raw.clone()
        };
        let m = Memory::new(s, "conc");
        m.init().await.unwrap();
        const N: usize = 40;
        hammer(m.clone(), N).await;

        let eps = m.episodes("fp-shared", 10).await.unwrap();
        assert_eq!(eps.len(), 1, "sealed={sealed}");
        assert_eq!(
            eps[0].count, N as u64,
            "sealed={sealed}: episode increments were lost"
        );
        let pb = m.playbook("fp-shared").await.unwrap().unwrap();
        assert_eq!(pb.uses, N as u64, "sealed={sealed}: uses lost");
        assert_eq!(pb.wins, N as u64, "sealed={sealed}: wins lost");
        assert_eq!(pb.fields.len(), 1);
        assert_eq!(
            pb.fields[0].wins, N as u32,
            "sealed={sealed}: field wins lost"
        );
        // 5 profile keys + 3 learned keys; "key 9" never existed.
        let facts = m.facts(100).await.unwrap();
        assert_eq!(facts.len(), 8, "sealed={sealed}: {facts:?}");
        assert!(m.degraded().is_none());
    }
}

#[tokio::test]
async fn concurrent_user_remember_always_beats_learn() {
    let Some(srv) = start().await else { return };
    let m = Memory::new(qdrant_store(&srv.url, 64), "t");
    m.init().await.unwrap();
    for round in 0..15 {
        let key = format!("company {round}");
        let (a, b) = (m.clone(), m.clone());
        let k1 = key.clone();
        let learn = tokio::spawn(async move { a.learn(&k1, json!("from a page")).await });
        let k2 = key.clone();
        let user = tokio::spawn(async move {
            b.remember(&k2, json!("from the user"), None, None, "user")
                .await
        });
        learn.await.unwrap().unwrap();
        user.await.unwrap().unwrap();
        let f = m.fact(&key).await.unwrap().unwrap();
        // Whichever order they ran in, the user's value must be the one left standing.
        assert_eq!(
            (f.source.as_str(), f.value),
            ("user", json!("from the user")),
            "round {round}"
        );
    }
}

#[tokio::test]
async fn prune_counts_exactly_and_forget_is_idempotent_under_races() {
    let Some(srv) = start().await else { return };
    let m = Memory::new(qdrant_store(&srv.url, 64), "t");
    m.init().await.unwrap();
    for i in 0..30 {
        // never accepted: losses 1, wins 0, verified_ts 0 -> stale and unproven
        m.record_outcome("h", &format!("lost-{i}"), &[], vec![], false, None)
            .await
            .unwrap();
    }
    m.record_success("h", "proven", &[], vec![]).await.unwrap();
    for i in 0..10 {
        m.record_episode("h", "proven", "f", "p", &format!("detail {i}"))
            .await
            .unwrap();
    }
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let (a, b) = (m.clone(), m.clone());
    let (ra, rb) = tokio::join!(a.prune(0), b.prune(0));
    // Two racing prunes together remove everything stale exactly once or twice-counted-at-worst, never panic.
    let total = ra.unwrap() + rb.unwrap();
    assert!(total >= 40, "{total}");
    assert_eq!(m.skills(100).await.unwrap().len(), 1);
    assert!(m.episodes("proven", 100).await.unwrap().is_empty());
    // A sequential prune afterwards finds nothing.
    assert_eq!(m.prune(0).await.unwrap(), 0);
}

// ------------------------------------------------------------------ filters on the real server

#[tokio::test]
async fn filter_semantics_match_the_in_process_store_on_real_qdrant() {
    for sealed in [false, true] {
        let Some(srv) = start().await else { return };
        let raw = qdrant_store(&srv.url, 64);
        raw.ensure().await.unwrap();
        let s: Arc<dyn Store> = if sealed {
            Arc::new(SealedStore::new(raw.clone(), [3; 32]))
        } else {
            raw.clone()
        };
        let mem: Arc<dyn Store> = Arc::new(InMemoryStore::new(Arc::new(HashEmbedder::new(64))));
        let sealed_mem: Arc<dyn Store> = Arc::new(SealedStore::new(
            Arc::new(InMemoryStore::new(Arc::new(HashEmbedder::new(64)))),
            [3; 32],
        ));
        let pts = [
            (
                "p1",
                json!({"host": "a.com", "labels_norm": ["email", "phone"], "id_hint": "p1", "key_norm": "k"}),
            ),
            (
                "p2",
                json!({"host": "b.com", "labels_norm": ["city"], "id_hint": "p2", "key_norm": ""}),
            ),
            (
                "p3",
                json!({"host": "c.com", "labels_norm": [], "id_hint": "p3", "autocomplete": null}),
            ),
            (
                "p4",
                json!({"host": 5, "labels_norm": ["x"], "id_hint": "p4", "autocomplete": "null"}),
            ),
            (
                "p5",
                json!({"host": "5", "labels_norm": ["Zip Code"], "id_hint": "p5", "autocomplete": "tel"}),
            ),
        ];
        for st in [&s, &mem, &sealed_mem] {
            for (id, p) in &pts {
                let mut p = p.clone();
                p["tenant"] = json!("t");
                st.upsert(
                    Kind::Playbook,
                    &format!("00000000-0000-0000-0000-00000000000{}", &id[1..]),
                    "x",
                    p,
                )
                .await
                .unwrap();
            }
        }
        let conds: Vec<(&str, Value)> = vec![
            ("host", json!("a.com")),
            ("host", json!(["a.com", "b.com"])),
            ("host", json!(["nope"])),
            ("host", json!([])),
            ("host", json!(5)),
            ("host", json!("5")),
            ("labels_norm", json!("phone")),
            ("labels_norm", json!(["city", "phone"])),
            ("labels_norm", json!([])),
            ("labels_norm", json!("Zip Code")),
            ("key_norm", json!("")),
            ("key_norm", json!("k")),
            ("autocomplete", json!("null")),
            ("autocomplete", json!("tel")),
            ("id_hint", json!(["p1", "p5", "p9"])),
        ];
        let ids = |hits: Vec<Hit>| {
            let mut v: Vec<String> = hits
                .iter()
                .map(|h| h.payload["id_hint"].as_str().unwrap().to_string())
                .collect();
            v.sort();
            v
        };
        for (k, v) in conds {
            let f = Filter::tenant("t").and(k, v.clone());
            let want = ids(mem.scroll(Kind::Playbook, &f, 50).await.unwrap());
            assert_eq!(
                ids(sealed_mem.scroll(Kind::Playbook, &f, 50).await.unwrap()),
                want,
                "in-process sealed differs from plain for {k} = {v}"
            );
            // Real Qdrant cannot hold a numeric `host` in a keyword-indexed field the way the in-process
            // store does, so compare it only where the value types are valid for the index.
            let got = s.scroll(Kind::Playbook, &f, 50).await;
            let n = s.count(Kind::Playbook, &f).await;
            if sealed || !(k == "host" && v == json!(5)) {
                let got = ids(got.unwrap_or_else(|e| panic!("{k} = {v}: {e}")));
                assert_eq!(
                    got, want,
                    "sealed={sealed}: real Qdrant differs for {k} = {v}"
                );
                assert_eq!(n.unwrap() as usize, want.len(), "count for {k} = {v}");
            }
        }
    }
}

#[tokio::test]
async fn unscoped_bulk_delete_is_refused_by_the_server_store_too() {
    let Some(srv) = start().await else { return };
    let raw = qdrant_store(&srv.url, 64);
    raw.ensure().await.unwrap();
    for t in ["a", "b"] {
        raw.upsert(
            Kind::Profile,
            &format!("00000000-0000-0000-0000-00000000000{}", u8::from(t == "b")),
            "x",
            json!({"tenant": t}),
        )
        .await
        .unwrap();
    }
    assert!(raw
        .delete_matching(Kind::Profile, &Filter::default())
        .await
        .is_err());
    assert!(raw
        .delete_matching(Kind::Profile, &Filter::default().and("id_hint", json!("x")))
        .await
        .is_err());
    // Count per tenant: `ensure()` also stores one embedding-model marker point in the collection, so a
    // count over the whole collection is 3, not 2. Both tenants' rows must still be there.
    for t in ["a", "b"] {
        assert_eq!(
            raw.count(Kind::Profile, &Filter::tenant(t)).await.unwrap(),
            1,
            "tenant {t} lost its row to a refused bulk delete"
        );
    }
}

// ------------------------------------------------------------------ tampering on the real server

#[tokio::test]
async fn tampering_with_what_the_server_holds_is_noticed() {
    let Some(srv) = start().await else { return };
    let raw = qdrant_store(&srv.url, 64);
    let sealed: Arc<dyn Store> = Arc::new(SealedStore::new(raw.clone(), [8; 32]));
    let alice = Memory::new(sealed.clone(), "alice");
    let bob = Memory::new(sealed.clone(), "bob");
    alice.init().await.unwrap();
    alice
        .remember("email", json!("alice@example.com"), None, None, "user")
        .await
        .unwrap();
    for _ in 0..2 {
        alice
            .record_outcome("h", "fp", &[], vec![ok_outcome("Name", "name")], true, None)
            .await
            .unwrap();
    }
    // What a hostile server operator can do: rewrite stored payload fields in place.
    let hits = raw
        .scroll(Kind::Profile, &Filter::tenant("alice"), 10)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    let h = &hits[0];
    let mut flipped = h.payload.clone();
    flipped["tenant"] = json!("bob");
    raw.upsert(Kind::Profile, &h.id, "email", flipped)
        .await
        .unwrap();
    assert!(
        bob.facts(10).await.unwrap().is_empty(),
        "flipping `tenant` handed alice's fact to bob"
    );
    assert!(bob.fact("email").await.unwrap().is_none());
    // Restore: alice sees it again (nothing was destroyed).
    raw.upsert(Kind::Profile, &h.id, "email", h.payload.clone())
        .await
        .unwrap();
    assert_eq!(alice.facts(10).await.unwrap().len(), 1);

    // Inflating wins on a playbook so prune keeps it.
    let pbs = raw
        .scroll(Kind::Playbook, &Filter::tenant("alice"), 10)
        .await
        .unwrap();
    let mut p = pbs[0].payload.clone();
    p["wins"] = json!(1_000_000);
    raw.upsert(Kind::Playbook, &pbs[0].id, "x", p)
        .await
        .unwrap();
    assert!(
        alice.playbook("fp").await.unwrap().is_none(),
        "edited `wins` was accepted"
    );
}
