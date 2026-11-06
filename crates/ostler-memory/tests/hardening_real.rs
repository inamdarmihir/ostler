//! Adversarial tests against a real Qdrant: concurrency, odd tenant names, filter semantics and tampering.
//! Skips unless QDRANT_URL or OSTLER_QDRANT_BIN is set (see qdrant_real.rs); fails if OSTLER_REQUIRE_QDRANT=1.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::start;
use ostler_memory::*;
use serde_json::{json, Value};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn prefixed(url: &str, prefix: &str) -> Arc<QdrantStore> {
    Arc::new(
        QdrantStore::new(QdrantConfig {
            url: url.into(),
            api_key: None,
            vectors: VectorSource::Client(Arc::new(HashEmbedder::new(64))),
            timeout: Duration::from_secs(20),
            on_disk: false,
            allow_insecure: false,
            collection_prefix: prefix.into(),
        })
        .unwrap(),
    )
}

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
    let Some(srv) = start().await else { return };
    for sealed in [false, true] {
        let raw = prefixed(&srv.url, &format!("conc{}_", u8::from(sealed)));
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
    let m = Memory::new(prefixed(&srv.url, "learnrace_"), "t");
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
    let m = Memory::new(prefixed(&srv.url, "prune_"), "t");
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

// ------------------------------------------------------------------ tenants

fn tenant_cases() -> Vec<String> {
    let mut v: Vec<String> = [
        "",
        " ",
        "  alice",
        "alice ",
        "Alice",
        "alice",
        "ALICE",
        "a b",
        "a_b",
        "a-b",
        "a.b",
        "a/b",
        "a\\b",
        "../../etc",
        "ostler_x_",
        "x_ostler_profile",
        "tenant with spaces and ünïcödé",
        "日本語",
        "😀",
        "😀😀",
        "🎉",
        "a\0b",
        "a\u{1f}b",
        "\n",
        "%2e%2e",
        "a?b=c",
        "a#b",
        "a\"b",
        "{}",
        "null",
        "0",
        "007",
        "7",
        "con",
        "nul",
        "-",
        "_",
        "~",
        "ß",
        "SS",
        "ss",
        "İ",
        "i",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    v.push("x".repeat(40));
    v.push("x".repeat(41));
    v.push("x".repeat(300));
    v.push(format!("{}a", "x".repeat(40)));
    v.push(format!("{}b", "x".repeat(40)));
    v.push("é".repeat(60));
    let mut r = Rng(0xABCDEF);
    for _ in 0..300 {
        let n = r.below(50);
        let s: String = (0..n)
            .map(|_| match r.below(6) {
                0 => char::from(b'a' + r.below(26) as u8),
                1 => char::from(b'A' + r.below(26) as u8),
                2 => char::from(b'0' + r.below(10) as u8),
                3 => ['_', '-', ' ', '.', '/'][r.below(5)],
                4 => char::from_u32(0x80 + r.below(0x2000) as u32).unwrap_or('?'),
                _ => ['😀', '🎉', 'ß', 'İ'][r.below(4)],
            })
            .collect();
        v.push(s);
    }
    v
}

#[test]
fn tenant_prefixes_are_valid_collection_names_distinct_and_bounded() {
    let tenants = tenant_cases();
    let mut seen = std::collections::HashMap::new();
    for t in &tenants {
        let p = tenant_prefix(t);
        assert!(p.len() <= 64, "{t:?} -> {p} is {} chars", p.len());
        assert!(p.starts_with("ostler_") && p.ends_with('_'), "{p}");
        assert!(
            p.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "{t:?} -> {p}"
        );
        for k in Kind::ALL {
            let name = format!("{p}{}", k.collection());
            assert!(name.len() <= 100, "{name}");
        }
        if let Some(prev) = seen.insert(p.clone(), t.clone()) {
            assert_eq!(&prev, t, "{prev:?} and {t:?} share the prefix {p}");
        }
    }
    // Collections of different tenants never overlap either: no tenant's prefix+kind equals another's.
    let mut names = std::collections::HashSet::new();
    for t in tenants.iter().collect::<std::collections::BTreeSet<_>>() {
        for k in Kind::ALL {
            assert!(
                names.insert(format!("{}{}", tenant_prefix(t), k.collection())),
                "collection name collision for {t:?}"
            );
        }
    }
}

#[test]
fn minted_tokens_scope_exactly_the_tenants_collections() {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
    use base64::Engine;
    for t in tenant_cases() {
        if t.trim().is_empty() {
            assert!(mint_tenant_token("k", &t, 1, false).is_err(), "{t:?}");
            continue;
        }
        for ro in [false, true] {
            let tok = mint_tenant_token("admin-\u{e9}-key", &t, 4_000_000_000, ro).unwrap();
            let parts: Vec<&str> = tok.split('.').collect();
            assert_eq!(parts.len(), 3);
            assert!(parts.iter().all(|p| p
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')));
            let claims: Value = serde_json::from_slice(&B64URL.decode(parts[1]).unwrap()).unwrap();
            assert_eq!(claims["exp"], 4_000_000_000u64);
            let access = claims["access"].as_array().unwrap();
            assert_eq!(access.len(), Kind::ALL.len());
            for (a, k) in access.iter().zip(Kind::ALL) {
                assert_eq!(
                    a["collection"],
                    format!("{}{}", tenant_prefix(&t), k.collection()),
                    "{t:?}"
                );
                assert_eq!(a["access"], if ro { "r" } else { "rw" });
            }
        }
    }
    assert!(mint_tenant_token("", "alice", 1, false).is_err());
}

#[tokio::test]
async fn every_odd_tenant_name_gets_working_isolated_collections_on_real_qdrant() {
    let Some(srv) = start().await else { return };
    // Each tenant costs three collections and nine indexes each (slow on a laptop disk), so a curated set:
    // padding, case, separator look-alikes, emoji, NUL, path traversal and overlong names.
    let tenants: Vec<String> = [
        "  alice",
        "Alice",
        "alice",
        "a b",
        "a_b",
        "😀",
        "a\0b",
        "../../etc",
        "ß",
        "tenant-ünï",
    ]
    .iter()
    .map(|s| s.to_string())
    .chain(["x".repeat(41), "x".repeat(300)])
    .collect();
    for t in &tenants {
        let m = Memory::new(prefixed(&srv.url, &tenant_prefix(t)), t.clone());
        m.init()
            .await
            .unwrap_or_else(|e| panic!("tenant {t:?}: {e}"));
        m.remember("name", json!(t), None, None, "user")
            .await
            .unwrap_or_else(|e| panic!("tenant {t:?}: {e}"));
    }
    for t in &tenants {
        let m = Memory::new(prefixed(&srv.url, &tenant_prefix(t)), t.clone());
        let f = m.fact("name").await.unwrap().unwrap();
        assert_eq!(f.value, json!(t), "tenant {t:?} read someone else's fact");
        assert_eq!(m.facts(10).await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn concurrent_first_start_creates_collections_once_without_error() {
    let Some(srv) = start().await else { return };
    let s = prefixed(&srv.url, "race_");
    let mut hs = vec![];
    for _ in 0..8 {
        let s = s.clone();
        hs.push(tokio::spawn(async move { s.ensure().await }));
    }
    for h in hs {
        h.await.unwrap().expect("ensure raced with another ensure");
    }
}

// ------------------------------------------------------------------ filters on the real server

#[tokio::test]
async fn filter_semantics_match_the_in_process_store_on_real_qdrant() {
    let Some(srv) = start().await else { return };
    for sealed in [false, true] {
        let raw = prefixed(&srv.url, &format!("filt{}_", u8::from(sealed)));
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
    let raw = prefixed(&srv.url, "unscoped_");
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
    let raw = prefixed(&srv.url, "tamper_");
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
