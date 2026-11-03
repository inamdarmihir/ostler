//! Adversarial tests for client-side sealing, filter semantics and key normalisation, on the in-process
//! store (no Qdrant needed). Deterministic: a small seeded PRNG, no extra crates.

use std::sync::Arc;

use ostler_memory::*;
use serde_json::{json, Map, Value};

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

const NOISE: &[&str] = &[
    "a", "Z", "9", " ", "-", "é", "e\u{301}", "😀", "ש", "日本", "\u{200b}", "\\", "\"", "\0",
    "\n", "/", ".", "@",
];

struct Gen {
    r: Rng,
    n: usize,
    markers: Vec<String>,
}

impl Gen {
    fn marker(&mut self) -> String {
        self.n += 1;
        let mut m = format!("MARK{}Q", self.n);
        for _ in 0..self.r.below(4) {
            m.push_str(NOISE[self.r.below(NOISE.len())]);
        }
        self.markers.push(format!("MARK{}Q", self.n));
        m
    }
    fn value(&mut self, depth: usize) -> Value {
        match self.r.below(if depth == 0 { 6 } else { 9 }) {
            0 => Value::Null,
            1 => json!(self.r.below(2) == 0),
            2 => json!(self.r.next() as i64),
            3 => json!(self.r.next()),
            4 => json!((self.r.next() % 1_000_000) as f64 / 8.0),
            5 => Value::String(self.marker()),
            6 | 7 => Value::Array(
                (0..self.r.below(5))
                    .map(|_| self.value(depth - 1))
                    .collect(),
            ),
            _ => {
                let mut m = Map::new();
                for _ in 0..self.r.below(5) {
                    let k = self.marker();
                    let v = self.value(depth - 1);
                    m.insert(k, v);
                }
                Value::Object(m)
            }
        }
    }
}

fn stores(key: [u8; 32]) -> (Arc<InMemoryStore>, Arc<dyn Store>) {
    let raw = Arc::new(InMemoryStore::new(Arc::new(HashEmbedder::default())));
    let sealed: Arc<dyn Store> = Arc::new(SealedStore::new(raw.clone(), key));
    (raw, sealed)
}

async fn raw_points(raw: &InMemoryStore, kind: Kind, tenant: &str) -> Vec<Hit> {
    raw.scroll(kind, &Filter::tenant(tenant), 1000)
        .await
        .unwrap()
}

// ------------------------------------------------------------------ round trip

#[tokio::test]
async fn random_payload_shapes_round_trip_and_leave_no_plaintext() {
    let mut g = Gen {
        r: Rng(0xC0FFEE),
        n: 0,
        markers: vec![],
    };
    let (raw, sealed) = stores([5; 32]);
    for case in 0..300 {
        let tenant = format!("fz{case}");
        g.markers.clear();
        let mut p = Map::new();
        p.insert("tenant".into(), json!(tenant));
        p.insert("wins".into(), json!(g.r.below(50)));
        p.insert("losses".into(), json!(g.r.below(50)));
        p.insert("verified_ts".into(), json!(g.r.next() >> 8));
        for _ in 0..g.r.below(6) {
            let k = g.marker();
            let v = g.value(4);
            p.insert(k, v);
        }
        // Sometimes also fill the hashed fields with arbitrary shapes.
        for f in [
            "host",
            "fingerprint",
            "labels_norm",
            "key_norm",
            "autocomplete",
            "id_hint",
        ] {
            if g.r.below(3) == 0 {
                let v = g.value(2);
                p.insert(f.into(), v);
            }
        }
        let payload = Value::Object(p);
        let id = format!("id-{case}-{}", g.marker());
        sealed
            .upsert(Kind::Profile, &id, &format!("text {case}"), payload.clone())
            .await
            .unwrap();

        // Round trip.
        let got = sealed
            .scroll(Kind::Profile, &Filter::tenant(&tenant), 10)
            .await
            .unwrap();
        assert_eq!(got.len(), 1, "case {case}");
        assert_eq!(got[0].payload, payload, "case {case}");
        let via_search = sealed
            .search(Kind::Profile, "text", &Filter::tenant(&tenant), 10)
            .await
            .unwrap();
        assert_eq!(via_search.len(), 1);
        assert_eq!(via_search[0].payload, payload);

        // The server's view: no marker (key or value) and nothing derived from the caller's id.
        for h in raw_points(&raw, Kind::Profile, &tenant).await {
            let stored = h.payload.to_string();
            for m in &g.markers {
                assert!(!stored.contains(m), "case {case}: {m} leaked in {stored}");
                assert!(!h.id.contains(m));
            }
            assert!(!h.id.contains("id-"), "id derived in clear: {}", h.id);
        }
    }
}

#[tokio::test]
async fn a_float_that_json_cannot_round_trip_is_refused_never_silently_altered() {
    // serde_json (without float_roundtrip) parses 98634.28571428572 as ...71: one ulp off.
    let (_raw, sealed) = stores([5; 32]);
    for x in [
        98634.28571428572f64,
        0.1 + 0.2,
        1e300 / 7.0,
        1.0 / 3.0,
        2.5,
        0.0,
        -1.5e-7,
    ] {
        let p = json!({"tenant": "fl", "x": x});
        let r = sealed.upsert(Kind::Profile, "id", "t", p.clone()).await;
        if r.is_ok() {
            let got = sealed
                .scroll(Kind::Profile, &Filter::tenant("fl"), 1)
                .await
                .unwrap();
            assert_eq!(got[0].payload, p, "{x} came back altered");
        }
    }
}

#[tokio::test]
async fn huge_values_round_trip() {
    let (_raw, sealed) = stores([5; 32]);
    let big = "x".repeat(3_000_000);
    let payload = json!({"tenant": "big", "value": big, "arr": vec![1; 50_000]});
    sealed
        .upsert(Kind::Profile, "id", "t", payload.clone())
        .await
        .unwrap();
    let got = sealed
        .scroll(Kind::Profile, &Filter::tenant("big"), 1)
        .await
        .unwrap();
    assert_eq!(got[0].payload, payload);
}

#[tokio::test]
async fn nesting_that_could_never_be_read_back_is_refused_not_silently_lost() {
    let (_raw, sealed) = stores([5; 32]);
    let mut v = json!("leaf");
    for _ in 0..300 {
        v = json!([v]);
    }
    let r = sealed
        .upsert(Kind::Profile, "id", "t", json!({"tenant": "deep", "v": v}))
        .await;
    assert!(
        r.is_err(),
        "stored a payload that serde_json cannot parse back (recursion limit): silent data loss"
    );
    // A depth that is fine still works.
    let mut v = json!("leaf");
    for _ in 0..100 {
        v = json!([v]);
    }
    let p = json!({"tenant": "deep", "v": v});
    sealed
        .upsert(Kind::Profile, "id2", "t", p.clone())
        .await
        .unwrap();
    let got = sealed
        .scroll(Kind::Profile, &Filter::tenant("deep"), 5)
        .await
        .unwrap();
    assert_eq!(got[0].payload, p);
}

// ------------------------------------------------------------------ tampering

async fn one_sealed(key: [u8; 32]) -> (Arc<InMemoryStore>, Arc<dyn Store>, Hit) {
    let (raw, sealed) = stores(key);
    sealed
        .upsert(
            Kind::Profile,
            "orig",
            "email address",
            json!({"tenant": "alice", "wins": 3, "losses": 1, "verified_ts": 1000,
                   "key": "email", "value": "ada@example.com", "key_norm": "email"}),
        )
        .await
        .unwrap();
    let hit = raw_points(&raw, Kind::Profile, "alice").await.remove(0);
    (raw, sealed, hit)
}

async fn visible(sealed: &Arc<dyn Store>, tenant: &str) -> usize {
    let a = sealed
        .scroll(Kind::Profile, &Filter::tenant(tenant), 50)
        .await
        .unwrap()
        .len();
    let b = sealed
        .search(Kind::Profile, "email", &Filter::tenant(tenant), 50)
        .await
        .unwrap()
        .len();
    assert_eq!(a, b, "scroll and search disagree");
    a
}

#[tokio::test]
async fn every_single_position_of_the_blob_is_authenticated() {
    let (raw, sealed, hit) = one_sealed([1; 32]).await;
    let blob = hit.payload["_sealed"].as_str().unwrap().to_string();
    assert_eq!(visible(&sealed, "alice").await, 1);
    let chars: Vec<char> = blob.chars().collect();
    for i in 0..chars.len() {
        for repl in ['A', 'B', '.', '=', '-', '_', '\u{ff}', '\0'] {
            if repl == chars[i] {
                continue;
            }
            let mut c = chars.clone();
            c[i] = repl;
            let mut p = hit.payload.clone();
            p["_sealed"] = json!(c.iter().collect::<String>());
            raw.upsert(Kind::Profile, &hit.id, "email", p)
                .await
                .unwrap();
            assert_eq!(
                visible(&sealed, "alice").await,
                0,
                "byte {i} -> {repl:?} was accepted"
            );
        }
    }
    // Restoring the original blob makes it visible again: nothing was destroyed.
    raw.upsert(Kind::Profile, &hit.id, "email", hit.payload.clone())
        .await
        .unwrap();
    assert_eq!(visible(&sealed, "alice").await, 1);
}

#[tokio::test]
async fn truncated_extended_and_malformed_blobs_are_invisible() {
    let (raw, sealed, hit) = one_sealed([1; 32]).await;
    let blob = hit.payload["_sealed"].as_str().unwrap().to_string();
    let mut variants: Vec<Value> = vec![];
    for n in 0..blob.len() {
        variants.push(json!(blob[..n]));
    }
    for tail in ["x", ".", ".x", "..", "=", "\0", "\u{1f600}"] {
        variants.push(json!(format!("{blob}{tail}")));
    }
    // wrong or missing version prefixes
    let rest = blob.split_once('.').unwrap().1;
    for v in ["v0", "v2x", "V1", "v", "", "v1 ", "vv1", "v01", "../v1"] {
        variants.push(json!(format!("{v}.{rest}")));
    }
    variants.push(json!(rest));
    variants.push(json!(format!("v1.{rest}.{rest}")));
    variants.push(json!("v1.."));
    variants.push(json!("v1...."));
    variants.push(json!("...."));
    // wrong JSON type
    variants.extend([
        json!(null),
        json!(7),
        json!(true),
        json!([]),
        json!({}),
        json!([blob]),
    ]);
    for v in variants {
        let mut p = hit.payload.clone();
        p["_sealed"] = v.clone();
        raw.upsert(Kind::Profile, &hit.id, "email", p)
            .await
            .unwrap();
        assert_eq!(visible(&sealed, "alice").await, 0, "accepted {v}");
    }
    // Missing field entirely.
    let mut p = hit.payload.clone();
    p.as_object_mut().unwrap().remove("_sealed");
    raw.upsert(Kind::Profile, &hit.id, "email", p)
        .await
        .unwrap();
    assert_eq!(visible(&sealed, "alice").await, 0);
    // Non-object payloads.
    for p in [json!(null), json!("str"), json!([1, 2]), json!(5)] {
        raw.upsert(Kind::Profile, &hit.id, "email", p)
            .await
            .unwrap();
        assert_eq!(visible(&sealed, "alice").await, 0);
    }
}

#[tokio::test]
async fn blobs_moved_swapped_or_replayed_elsewhere_do_not_open() {
    let (raw, sealed) = stores([1; 32]);
    for (id, key) in [("a", "email"), ("b", "city")] {
        sealed
            .upsert(
                Kind::Profile,
                id,
                key,
                json!({"tenant": "alice", "key": key, "value": key}),
            )
            .await
            .unwrap();
    }
    let pts = raw_points(&raw, Kind::Profile, "alice").await;
    assert_eq!(pts.len(), 2);
    let (a, b) = (&pts[0], &pts[1]);
    // swap payloads between the two ids
    raw.upsert(Kind::Profile, &a.id, "x", b.payload.clone())
        .await
        .unwrap();
    raw.upsert(Kind::Profile, &b.id, "x", a.payload.clone())
        .await
        .unwrap();
    assert_eq!(visible(&sealed, "alice").await, 0, "swapped ids opened");
    // put them back
    raw.upsert(Kind::Profile, &a.id, "x", a.payload.clone())
        .await
        .unwrap();
    raw.upsert(Kind::Profile, &b.id, "x", b.payload.clone())
        .await
        .unwrap();
    assert_eq!(visible(&sealed, "alice").await, 2);
    // replay a blob at a fresh id, and in another collection
    raw.upsert(Kind::Profile, "fresh-id", "x", a.payload.clone())
        .await
        .unwrap();
    raw.upsert(Kind::Playbook, &a.id, "x", a.payload.clone())
        .await
        .unwrap();
    assert_eq!(
        visible(&sealed, "alice").await,
        2,
        "replay at a fresh id opened"
    );
    assert!(sealed
        .scroll(Kind::Playbook, &Filter::tenant("alice"), 10)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn the_clear_fields_are_covered_by_the_seal() {
    // tenant, wins, losses and verified_ts stay readable to Qdrant (it filters and ranks on them), but the
    // seal must still notice if someone with write access to the server edits them: flipping `tenant`
    // would otherwise hand one tenant's facts to another; inflating `wins` would make prune keep junk.
    let (raw, sealed, hit) = one_sealed([1; 32]).await;
    for (field, evil) in [
        ("tenant", json!("bob")),
        ("wins", json!(999)),
        ("losses", json!(0)),
        ("verified_ts", json!(1)),
    ] {
        let mut p = hit.payload.clone();
        p[field] = evil.clone();
        raw.upsert(Kind::Profile, &hit.id, "email", p)
            .await
            .unwrap();
        let who = if field == "tenant" { "bob" } else { "alice" };
        assert_eq!(
            visible(&sealed, who).await,
            0,
            "edited clear field {field} was accepted"
        );
    }
    // Deleting a clear field is also an edit.
    let mut p = hit.payload.clone();
    p.as_object_mut().unwrap().remove("wins");
    raw.upsert(Kind::Profile, &hit.id, "email", p)
        .await
        .unwrap();
    assert_eq!(visible(&sealed, "alice").await, 0);
}

#[tokio::test]
async fn wrong_key_sees_nothing_and_cannot_delete_by_accident() {
    let (raw, a) = stores([1; 32]);
    let b: Arc<dyn Store> = Arc::new(SealedStore::new(raw.clone(), [2; 32]));
    a.upsert(
        Kind::Profile,
        "x",
        "email",
        json!({"tenant": "alice", "key": "email", "key_norm": "email"}),
    )
    .await
    .unwrap();
    assert_eq!(visible(&b, "alice").await, 0);
    // b filtering on the hashed field finds nothing either (different index key).
    let f = Filter::tenant("alice").and("key_norm", json!("email"));
    assert!(b.scroll(Kind::Profile, &f, 5).await.unwrap().is_empty());
    assert_eq!(a.scroll(Kind::Profile, &f, 5).await.unwrap().len(), 1);
    assert_eq!(
        probe_key(raw.clone(), [2; 32], "alice").await.unwrap(),
        KeyProbe::Mismatch { unopenable: 1 }
    );
}

// ------------------------------------------------------------------ reserved fields and types

#[tokio::test]
async fn payloads_that_collide_with_reserved_fields_are_refused() {
    let (raw, sealed) = stores([1; 32]);
    let bad = [
        json!({"tenant": "t", "_sealed": "v1.AAAA.AAAA"}),
        json!({"tenant": "t", "_sealed": {"nested": "ada@example.com"}}),
        // tenant/wins/losses/verified_ts stay in clear: they must be the plain types Qdrant needs, never a
        // way to smuggle text onto the server.
        json!({"tenant": 5}),
        json!({"tenant": ["ada@example.com"]}),
        json!({"tenant": "t", "wins": "ada@example.com"}),
        json!({"tenant": "t", "losses": {"x": "ada@example.com"}}),
        json!({"tenant": "t", "verified_ts": "secret"}),
        json!({"tenant": "t", "wins": -1}),
        json!({"tenant": "t", "wins": 1.5}),
        json!({"tenant": "t", "wins": null}),
        json!({"tenant": "t", "verified_ts": [1]}),
        json!([1, 2, 3]),
        json!("string"),
        json!(null),
    ];
    for p in bad {
        let r = sealed.upsert(Kind::Profile, "id", "t", p.clone()).await;
        assert!(r.is_err(), "accepted {p}");
    }
    assert!(raw_points(&raw, Kind::Profile, "t").await.is_empty());
    // And the legitimate shapes still work.
    sealed
        .upsert(
            Kind::Profile,
            "id",
            "t",
            json!({"tenant": "t", "wins": 0, "losses": 2, "verified_ts": 1_700_000_000u64, "k": "v"}),
        )
        .await
        .unwrap();
    sealed
        .upsert(Kind::Profile, "id2", "t", json!({"k": "no tenant at all"}))
        .await
        .unwrap();
}

// ------------------------------------------------------------------ filter semantics

async fn both_stores() -> Vec<(&'static str, Arc<dyn Store>)> {
    let plain: Arc<dyn Store> = Arc::new(InMemoryStore::new(Arc::new(HashEmbedder::default())));
    let (_raw, sealed) = stores([4; 32]);
    vec![("plain", plain), ("sealed", sealed)]
}

#[tokio::test]
async fn null_is_not_the_string_null() {
    for (name, s) in both_stores().await {
        s.upsert(
            Kind::Profile,
            "a",
            "x",
            json!({"tenant": "t", "autocomplete": null, "key_norm": "nullkey"}),
        )
        .await
        .unwrap();
        let f = Filter::tenant("t").and("autocomplete", json!("null"));
        assert!(
            s.scroll(Kind::Profile, &f, 5).await.unwrap().is_empty(),
            "{name}: the string \"null\" matched a null field"
        );
        assert_eq!(s.count(Kind::Profile, &f).await.unwrap(), 0, "{name}");
    }
}

#[tokio::test]
async fn memory_suggest_does_not_return_facts_with_no_autocomplete_for_the_token_null() {
    for (name, s) in both_stores().await {
        let m = Memory::new(s, "t");
        m.remember("zzz thing", json!("v"), None, None, "user")
            .await
            .unwrap();
        let q = FieldQuery {
            host: "h",
            fingerprint: "fp",
            label: "Whatever",
            name: None,
            kind: "text",
            placeholder: None,
            autocomplete: Some("null"),
        };
        let r = m.suggest(&q, 0.99).await.unwrap();
        assert!(r.is_none(), "{name}: autocomplete=\"null\" matched {r:?}");
    }
}

#[tokio::test]
async fn numbers_booleans_and_strings_stay_distinct_in_hashed_filters() {
    for (name, s) in both_stores().await {
        for (id, v) in [
            ("n", json!(5)),
            ("s", json!("5")),
            ("b", json!(true)),
            ("t", json!("true")),
            ("f", json!(5.5)),
        ] {
            s.upsert(
                Kind::Profile,
                id,
                "x",
                json!({"tenant": "t", "host": v, "id_hint": id}),
            )
            .await
            .unwrap();
        }
        let ids = |hits: Vec<Hit>| {
            let mut v: Vec<String> = hits
                .iter()
                .map(|h| h.payload["id_hint"].as_str().unwrap().to_string())
                .collect();
            v.sort();
            v
        };
        for (q, expect) in [
            (json!(5), vec!["n"]),
            (json!("5"), vec!["s"]),
            (json!(true), vec!["b"]),
            (json!("true"), vec!["t"]),
            (json!(5.5), vec!["f"]),
            (json!(6), vec![]),
            (json!(""), vec![]),
        ] {
            let got = ids(s
                .scroll(
                    Kind::Profile,
                    &Filter::tenant("t").and("host", q.clone()),
                    10,
                )
                .await
                .unwrap());
            assert_eq!(got, expect, "{name}: host == {q}");
        }
    }
}

#[tokio::test]
async fn an_array_filter_value_means_any_of_and_an_empty_one_matches_nothing() {
    for (name, s) in both_stores().await {
        s.upsert(
            Kind::Playbook,
            "p1",
            "x",
            json!({"tenant": "t", "host": "a.com", "labels_norm": ["email", "phone"], "id_hint": "p1"}),
        )
        .await
        .unwrap();
        s.upsert(
            Kind::Playbook,
            "p2",
            "x",
            json!({"tenant": "t", "host": "b.com", "labels_norm": ["city"], "id_hint": "p2"}),
        )
        .await
        .unwrap();
        let n = |v: Value| {
            let s = s.clone();
            async move {
                s.scroll(
                    Kind::Playbook,
                    &Filter::tenant("t").and("labels_norm", v),
                    10,
                )
                .await
                .unwrap()
                .len()
            }
        };
        // Equality on an array payload matches any element (Qdrant semantics).
        assert_eq!(n(json!("phone")).await, 1, "{name}");
        assert_eq!(n(json!("zip")).await, 0, "{name}");
        // A list of candidates matches a point holding any of them.
        assert_eq!(n(json!(["zip", "phone"])).await, 1, "{name}");
        assert_eq!(n(json!(["phone", "city"])).await, 2, "{name}");
        assert_eq!(
            n(json!([])).await,
            0,
            "{name}: empty any-of must match nothing"
        );
        // Scalar field, list of candidates.
        let h = |v: Value| {
            let s = s.clone();
            async move {
                s.scroll(Kind::Playbook, &Filter::tenant("t").and("host", v), 10)
                    .await
                    .unwrap()
                    .len()
            }
        };
        assert_eq!(h(json!(["a.com", "b.com"])).await, 2, "{name}");
        assert_eq!(h(json!(["c.com"])).await, 0, "{name}");
    }
}

#[tokio::test]
async fn empty_string_filters_match_only_empty_strings() {
    for (name, s) in both_stores().await {
        let m = Memory::new(s, "t");
        m.record_success("h", "", &[], vec![]).await.unwrap();
        m.record_success("h", "fp", &[], vec![]).await.unwrap();
        assert_eq!(
            m.playbook("").await.unwrap().unwrap().fingerprint,
            "",
            "{name}"
        );
        assert_eq!(m.playbook("fp").await.unwrap().unwrap().fingerprint, "fp");
        assert!(m.playbook("nope").await.unwrap().is_none());
        assert_eq!(m.episodes("", 5).await.unwrap().len(), 0);
    }
}

#[tokio::test]
async fn unscoped_bulk_delete_is_refused() {
    // Every Memory call carries the tenant; a Filter without one would wipe all tenants.
    for (name, s) in both_stores().await {
        for t in ["a", "b"] {
            s.upsert(Kind::Profile, t, "x", json!({"tenant": t, "key_norm": "k"}))
                .await
                .unwrap();
        }
        let r = s.delete_matching(Kind::Profile, &Filter::default()).await;
        let r2 = s
            .delete_matching(
                Kind::Profile,
                &Filter::default().and("key_norm", json!("k")),
            )
            .await;
        assert!(
            r.is_err() && r2.is_err(),
            "{name}: unscoped delete accepted"
        );
        assert_eq!(
            s.count(Kind::Profile, &Filter::tenant("a")).await.unwrap()
                + s.count(Kind::Profile, &Filter::tenant("b")).await.unwrap(),
            2,
            "{name}"
        );
    }
}

// ------------------------------------------------------------------ key normalisation

#[tokio::test]
async fn keys_differing_only_by_case_spacing_or_punctuation_are_one_fact() {
    for (name, s) in both_stores().await {
        let m = Memory::new(s, "t");
        for (i, k) in [
            "Work Email",
            "work_email",
            "WORK-EMAIL",
            " work.email ",
            "work  email",
        ]
        .iter()
        .enumerate()
        {
            m.remember(k, json!(format!("v{i}")), None, None, "user")
                .await
                .unwrap();
        }
        let facts = m.facts(10).await.unwrap();
        assert_eq!(facts.len(), 1, "{name}: {facts:?}");
        assert_eq!(facts[0].value, json!("v4"));
        assert!(m.fact("WORK EMAIL").await.unwrap().is_some());
        // ... but not a different word.
        m.remember("workemail", json!("other"), None, None, "user")
            .await
            .unwrap();
        assert_eq!(m.facts(10).await.unwrap().len(), 2, "{name}");
        assert!(m.forget("work-email").await.unwrap());
        assert_eq!(m.facts(10).await.unwrap().len(), 1, "{name}");
    }
}

#[tokio::test]
async fn keys_with_no_letters_or_digits_do_not_collapse_into_one() {
    for (name, s) in both_stores().await {
        let m = Memory::new(s, "t");
        for (k, v) in [
            ("😀", "smile"),
            ("🎉", "party"),
            ("***", "stars"),
            ("###", "hashes"),
        ] {
            m.remember(k, json!(v), None, None, "user").await.unwrap();
        }
        assert_eq!(
            m.facts(10).await.unwrap().len(),
            4,
            "{name}: keys overwrote each other"
        );
        assert_eq!(m.fact("🎉").await.unwrap().unwrap().value, json!("party"));
        assert!(m.forget("🎉").await.unwrap());
        assert_eq!(m.fact("😀").await.unwrap().unwrap().value, json!("smile"));
        assert_eq!(m.facts(10).await.unwrap().len(), 3);
        // An empty / blank key is still refused.
        assert!(m
            .remember("  ", json!("x"), None, None, "user")
            .await
            .is_err());
    }
}

#[tokio::test]
async fn decomposed_accents_do_not_merge_with_the_bare_letter() {
    for (name, s) in both_stores().await {
        let m = Memory::new(s, "t");
        m.remember("cafe", json!("plain"), None, None, "user")
            .await
            .unwrap();
        m.remember("cafe\u{301}", json!("accented"), None, None, "user")
            .await
            .unwrap();
        m.remember("İstanbul", json!("tr"), None, None, "user")
            .await
            .unwrap();
        m.remember("istanbul", json!("ascii"), None, None, "user")
            .await
            .unwrap();
        assert_eq!(m.facts(10).await.unwrap().len(), 4, "{name}");
        assert_eq!(m.fact("cafe").await.unwrap().unwrap().value, json!("plain"));
        assert_eq!(
            m.fact("cafe\u{301}").await.unwrap().unwrap().value,
            json!("accented")
        );
    }
}

#[tokio::test]
async fn playbook_labels_made_only_of_symbols_keep_separate_histories() {
    for (name, s) in both_stores().await {
        let m = Memory::new(s, "t");
        let ok = |l: &str| FieldOutcome {
            label: l.into(),
            kind: "text".into(),
            profile_key: None,
            ok: true,
        };
        m.record_outcome("h", "fp", &[], vec![ok("*"), ok("#"), ok("★")], true, None)
            .await
            .unwrap();
        let pb = m.playbook("fp").await.unwrap().unwrap();
        assert_eq!(pb.fields.len(), 3, "{name}: {:?}", pb.fields);
    }
}

#[tokio::test]
async fn ids_cannot_collide_across_tenants_through_separator_characters() {
    // Point ids are hashes of (tenant, kind, ...parts) joined by U+001F. A tenant or fingerprint containing
    // that character must not be able to land on another tenant's point.
    for (name, s) in both_stores().await {
        let victim = Memory::new(s.clone(), "t\u{1f}playbook\u{1f}x");
        let attacker = Memory::new(s.clone(), "t");
        victim
            .record_success("victim.example", "y", &[], vec![])
            .await
            .unwrap();
        attacker
            .record_success("evil.example", "x\u{1f}playbook\u{1f}y", &[], vec![])
            .await
            .unwrap();
        let v = victim.playbook("y").await.unwrap();
        assert_eq!(
            v.map(|p| p.host),
            Some("victim.example".to_string()),
            "{name}: another tenant overwrote this point"
        );
        assert!(attacker
            .playbook("x\u{1f}playbook\u{1f}y")
            .await
            .unwrap()
            .is_some());

        // Same for episodes: boundaries between fingerprint, field, problem and detail.
        let m = Memory::new(s, "e");
        m.record_episode("h", "a\u{1f}b", "c", "p", "d")
            .await
            .unwrap();
        m.record_episode("h", "a", "b", "c\u{1f}p", "d")
            .await
            .unwrap();
        assert_eq!(m.episodes("a\u{1f}b", 5).await.unwrap().len(), 1, "{name}");
        assert_eq!(m.episodes("a", 5).await.unwrap().len(), 1, "{name}");
    }
}

// ------------------------------------------------------------------ unopenable points must not crowd out real ones

#[tokio::test]
async fn foreign_or_legacy_points_do_not_hide_the_ones_we_can_open() {
    let raw = Arc::new(InMemoryStore::new(Arc::new(HashEmbedder::default())));
    let mine: Arc<dyn Store> = Arc::new(SealedStore::new(raw.clone(), [1; 32]));
    let foreign: Arc<dyn Store> = Arc::new(SealedStore::new(raw.clone(), [2; 32]));
    // Older, plaintext points for the same tenant come first and score best for the query.
    let legacy = Memory::new(raw.clone(), "t");
    for i in 0..20 {
        legacy.put_fact_for_test(&format!("email {i}")).await;
    }
    for i in 0..20 {
        foreign
            .upsert(
                Kind::Profile,
                &format!("f{i}"),
                "email address",
                json!({"tenant": "t", "key": format!("k{i}"), "value": "x"}),
            )
            .await
            .unwrap();
    }
    let m = Memory::new(mine, "t");
    for k in ["email address", "city", "company"] {
        m.remember(k, json!(k), None, None, "user").await.unwrap();
    }
    assert_eq!(
        m.facts(3).await.unwrap().len(),
        3,
        "scroll limit consumed by unopenable points"
    );
    let hits = m.recall("email address", 2).await.unwrap();
    assert_eq!(hits.len(), 2, "search limit consumed by unopenable points");
    assert_eq!(hits[0].0.key, "email address");
}

trait LegacyExt {
    async fn put_fact_for_test(&self, key: &str);
}
impl LegacyExt for Memory {
    async fn put_fact_for_test(&self, key: &str) {
        self.remember(key, json!("legacy"), None, None, "user")
            .await
            .unwrap();
    }
}

// ------------------------------------------------------------------ nothing is silently truncated

#[tokio::test]
async fn prune_reaches_past_the_first_ten_thousand_stale_playbooks() {
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new(Arc::new(HashEmbedder::new(8))));
    let m = Memory::new(store.clone(), "t");
    // 10 050 unproven, long-unverified skills plus one proven one.
    for i in 0..10_050 {
        store
            .upsert(
                Kind::Playbook,
                &format!("p{i}"),
                "x",
                json!({"tenant": "t", "wins": 0, "losses": 1, "verified_ts": 1, "fingerprint": format!("f{i}")}),
            )
            .await
            .unwrap();
    }
    store
        .upsert(
            Kind::Playbook,
            "proven",
            "x",
            json!({"tenant": "t", "wins": 5, "losses": 0, "verified_ts": 1, "fingerprint": "proven"}),
        )
        .await
        .unwrap();
    assert_eq!(m.prune(3600).await.unwrap(), 10_050);
    assert_eq!(
        store
            .count(Kind::Playbook, &Filter::tenant("t"))
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn probe_key_sees_every_point_not_just_the_first_page() {
    let raw = Arc::new(InMemoryStore::new(Arc::new(HashEmbedder::new(8))));
    let mine: Arc<dyn Store> = Arc::new(SealedStore::new(raw.clone(), [1; 32]));
    let foreign: Arc<dyn Store> = Arc::new(SealedStore::new(raw.clone(), [2; 32]));
    for i in 0..10_020 {
        mine.upsert(
            Kind::Profile,
            &format!("m{i}"),
            "x",
            json!({"tenant": "t", "k": i}),
        )
        .await
        .unwrap();
    }
    // The one point this key cannot open sorts after the first 10 000.
    foreign
        .upsert(Kind::Profile, "odd", "x", json!({"tenant": "t"}))
        .await
        .unwrap();
    assert_eq!(
        probe_key(raw, [1; 32], "t").await.unwrap(),
        KeyProbe::Mismatch { unopenable: 1 }
    );
}

#[tokio::test]
async fn a_card_number_cannot_be_smuggled_in_by_script_nesting_or_text() {
    let m = Memory::new(
        Arc::new(InMemoryStore::new(Arc::new(HashEmbedder::default()))),
        "t",
    );
    for (k, v) in [
        ("note", json!("４１１１ １１１１ １１１１ １１１１")),
        ("note", json!("٤١١١١١١١١١١١١١١١")),
        ("note", json!("4111.1111.1111.1111")),
        ("note", json!("4111_1111_1111_1111")),
        ("note", json!({"cc": "4111111111111111"})),
        ("note", json!(["4111 1111 1111 1111"])),
        ("note", json!(4111111111111111u64)),
        ("note", json!("my card is 4111 1111 1111 1111, thanks")),
        ("note", json!({"nested": {"password": "x"}})),
        ("note", json!("4111\u{200b}1111\u{200b}1111\u{200b}1111")),
    ] {
        assert!(
            m.remember(k, v.clone(), None, None, "user").await.is_err(),
            "stored {v}"
        );
        assert!(m.learn(k, v.clone()).await.is_err(), "learned {v}");
    }
    // Ordinary numbers and text are fine.
    for v in [
        json!("5551234567"),
        json!("order 1234567890123 shipped"),
        json!({"zip": "411029", "floor": 3}),
        json!(["a", "b"]),
        json!(1234567890),
        json!("2024-05-17 10:30"),
    ] {
        m.remember("note", v.clone(), None, None, "user")
            .await
            .unwrap_or_else(|e| panic!("{v}: {e}"));
    }
}
