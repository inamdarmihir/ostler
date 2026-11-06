//! Source memory (page chunks, hybrid retrieval) against a real Qdrant server.
//! Skips unless QDRANT_URL or OSTLER_QDRANT_BIN is set (see qdrant_real.rs); fails instead with
//! OSTLER_REQUIRE_QDRANT=1.

mod common;

use std::sync::Arc;

use common::{start, store};
use ostler_memory::sparse::{doc_vector, TermHasher};
use ostler_memory::*;
use serde_json::{json, Value};

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn chunk(url: &str, id: &str, ordinal: u32, text: &str, hash: &str) -> SourceChunk {
    SourceChunk {
        url: url.into(),
        chunk_id: id.into(),
        ordinal,
        heading_path: vec!["Docs".into()],
        text: text.into(),
        fetched_at: now(),
        content_hash: hash.into(),
        title: Some("Page".into()),
    }
}

fn page(url: &str, hash: &str, texts: &[&str]) -> Vec<SourceChunk> {
    texts
        .iter()
        .enumerate()
        .map(|(i, t)| chunk(url, &format!("c{i}"), i as u32, t, hash))
        .collect()
}

const A: &str = "https://a.example/rust";
const B: &str = "https://b.example/cooking";
const C: &str = "https://c.example/parts";

const TEXTS_A: [&str; 3] = [
    "Rust ownership means each value has a single owner and borrowing rules",
    "The borrow checker rejects dangling references at compile time",
    "Cargo builds crates and resolves dependency versions",
];
const TEXTS_B: [&str; 2] = [
    "Whisk the eggs then fold in flour for a light sponge cake",
    "Preheat the oven to 180 degrees and grease the baking tin",
];
const TEXTS_C: [&str; 3] = [
    "The gasket kit ships in grey packaging with a printed manual",
    "Replacement part ZX-4471 fits the left hinge only",
    "Hinges are sold separately and come in several finishes",
];

async fn seed(m: &Memory) {
    m.save_sources(A, page(A, "h1", &TEXTS_A)).await.unwrap();
    m.save_sources(B, page(B, "h2", &TEXTS_B)).await.unwrap();
    m.save_sources(C, page(C, "h3", &TEXTS_C)).await.unwrap();
}

fn ids(hits: &[SourceHit]) -> Vec<String> {
    hits.iter()
        .map(|h| format!("{}#{}", h.chunk.url, h.chunk.chunk_id))
        .collect()
}

#[tokio::test]
async fn source_collection_has_named_vectors_and_the_others_are_unchanged() {
    let Some(srv) = start().await else { return };
    let s = store(&srv.url, 256);
    Memory::new(s.clone(), "alice").init().await.unwrap();
    let http = reqwest::Client::new();
    let info = |name: &str| {
        let url = format!("{}/collections/{name}", srv.url);
        let http = http.clone();
        async move {
            http.get(url)
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()
        }
    };
    let src = info("ostler_sources").await;
    let params = &src["result"]["config"]["params"];
    assert_eq!(params["vectors"]["dense"]["size"], 256);
    assert_eq!(params["sparse_vectors"]["bm25"]["modifier"], "idf");
    let prof = info("ostler_profile").await;
    assert_eq!(prof["result"]["config"]["params"]["vectors"]["size"], 256);
    assert!(prof["result"]["config"]["params"]["sparse_vectors"].is_null());
    // The tenant and url indexes exist on the new collection.
    let schema = &src["result"]["payload_schema"];
    assert_eq!(schema["tenant"]["data_type"], "keyword");
    assert_eq!(schema["url"]["data_type"], "keyword");
}

#[tokio::test]
async fn hybrid_retrieval_on_a_real_server() {
    let Some(srv) = start().await else { return };
    let s = store(&srv.url, 256);
    let m = Memory::new(s.clone(), "alice");
    m.init().await.unwrap();
    seed(&m).await;

    let opts = SourceRecallOptions::default();
    let hits = m
        .recall_sources("borrow checker dangling references", 3, &opts)
        .await
        .unwrap();
    assert_eq!(hits[0].chunk.chunk_id, "c1");
    assert_eq!(hits[0].chunk.url, A);
    assert!(
        hits[0].score > 0.9,
        "both rankings put it first: {}",
        hits[0].score
    );

    // The identifier is found by the sparse side alone (dense weight 0).
    let lexical = SourceRecallOptions {
        dense_weight: 0.0,
        max_per_url: 0,
        ..Default::default()
    };
    let hits = m.recall_sources("zx 4471", 3, &lexical).await.unwrap();
    assert_eq!(ids(&hits), [format!("{C}#c1")]);

    // url filter
    let only_b = SourceRecallOptions {
        url: Some(B.into()),
        ..Default::default()
    };
    let hits = m.recall_sources("oven", 5, &only_b).await.unwrap();
    assert!(!hits.is_empty() && hits.iter().all(|h| h.chunk.url == B));

    // tenant isolation
    let bob = Memory::new(s, "bob");
    assert!(bob
        .recall_sources("rust ownership", 5, &opts)
        .await
        .unwrap()
        .is_empty());
}

/// The Qdrant server fusion (one query, weights) and the client-side fusion used for older servers and by
/// the local stores must agree.
#[tokio::test]
async fn server_fusion_and_client_fusion_rank_alike() {
    let Some(srv) = start().await else { return };
    let server = store(&srv.url, 256);
    let m = Memory::new(server.clone(), "alice");
    m.init().await.unwrap();
    seed(&m).await;

    let client = store(&srv.url, 256);
    client.force_client_side_fusion();
    let mc = Memory::new(client, "alice");

    let in_memory = Memory::new(
        Arc::new(InMemoryStore::new(Arc::new(HashEmbedder::new(256)))),
        "alice",
    );
    seed(&in_memory).await;

    for (query, wd, ws) in [
        ("borrow checker", 1.0, 1.0),
        ("rust ownership borrowing", 1.0, 3.0),
        ("zx 4471 hinge", 2.0, 1.0),
        ("sponge cake oven", 1.0, 1.0),
        ("packaging manual", 0.5, 0.5),
    ] {
        let opts = SourceRecallOptions {
            dense_weight: wd,
            sparse_weight: ws,
            diversity: 0.0,
            max_per_url: 0,
            ..Default::default()
        };
        let a = m.recall_sources(query, 8, &opts).await.unwrap();
        let b = mc.recall_sources(query, 8, &opts).await.unwrap();
        assert_eq!(ids(&a), ids(&b), "server vs client fusion for {query:?}");
        for (x, y) in a.iter().zip(&b) {
            assert!(
                (x.score - y.score).abs() < 1e-5,
                "{query:?}: {} vs {}",
                x.score,
                y.score
            );
        }
        // The in-process store computes the same rankings (same IDF formula, exact scans).
        let c = in_memory.recall_sources(query, 8, &opts).await.unwrap();
        assert_eq!(ids(&a), ids(&c), "server vs in-process for {query:?}");
    }
}

#[tokio::test]
async fn replace_forget_retention_and_stats_on_a_real_server() {
    let Some(srv) = start().await else { return };
    let m = Memory::new(store(&srv.url, 256), "alice").with_source_retain_days(7);
    m.init().await.unwrap();
    seed(&m).await;
    assert_eq!(
        m.save_sources(A, page(A, "h1", &TEXTS_A))
            .await
            .unwrap()
            .unchanged,
        3
    );

    // A changed page replaces its chunks.
    let r = m
        .save_sources(
            A,
            page(A, "h1-v2", &["Lifetimes were rewritten in a new chapter"]),
        )
        .await
        .unwrap();
    assert_eq!((r.stored, r.removed), (1, 2));
    let st = m.source_stats().await.unwrap();
    assert_eq!((st.urls, st.chunks), (3, 6));

    // Retention: an old page expires, a refreshed one does not.
    let mut old = page(B, "h2", &TEXTS_B);
    for c in &mut old {
        c.fetched_at = now() - 10 * 86_400;
    }
    m.save_sources(B, old.clone()).await.unwrap();
    assert_eq!(m.source_stats().await.unwrap().expired_chunks, 2);
    for c in &mut old {
        c.fetched_at = now();
    }
    let r = m.save_sources(B, old).await.unwrap();
    assert_eq!(
        (r.stored, r.unchanged),
        (0, 2),
        "refresh is a payload update"
    );
    assert_eq!(m.source_stats().await.unwrap().expired_chunks, 0);
    let mut stale = page(C, "h3", &TEXTS_C);
    for c in &mut stale {
        c.fetched_at = now() - 9 * 86_400;
    }
    m.save_sources(C, stale).await.unwrap();
    assert_eq!(m.prune(180 * 86_400).await.unwrap(), 3);
    let left: Vec<String> = m
        .list_sources(10)
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.url)
        .collect();
    assert_eq!(left.len(), 2);
    assert!(!left.contains(&C.to_string()));

    assert_eq!(m.forget_sources(Some(A)).await.unwrap(), 1);
    // wipe takes the kind with it.
    m.wipe().await.unwrap();
    assert_eq!(m.source_stats().await.unwrap().chunks, 0);
}

#[tokio::test]
async fn sealed_sources_on_a_real_server_are_opaque_and_still_searchable() {
    let Some(srv) = start().await else { return };
    let raw = store(&srv.url, 256);
    let sealed: Arc<dyn Store> = Arc::new(SealedStore::new(raw.clone(), [9; 32]));
    let m = Memory::new(sealed, "alice");
    m.init().await.unwrap();
    let url = "https://clinic.example/records/ada-lovelace";
    let texts = [
        "Ada Lovelace cardiology appointment on Tuesday with invoice 40291",
        "Parking is limited near building seven",
    ];
    m.save_sources(url, page(url, "h", &texts)).await.unwrap();

    // Exact words still work, through keyed term ids and keyed url hashes.
    let lexical = SourceRecallOptions {
        dense_weight: 0.0,
        ..Default::default()
    };
    let hits = m.recall_sources("40291", 3, &lexical).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].chunk.text, texts[0]);
    let both = m
        .recall_sources("cardiology appointment", 3, &Default::default())
        .await
        .unwrap();
    assert_eq!(both[0].chunk.chunk_id, "c0");
    // A refreshed fetch time is written through a sealed payload overwrite and still opens.
    let mut again = page(url, "h", &texts);
    for c in &mut again {
        c.fetched_at = now() + 5;
    }
    assert_eq!(m.save_sources(url, again).await.unwrap().unchanged, 2);
    assert_eq!(m.list_sources(5).await.unwrap()[0].fetched_at, now() + 5);

    // The server's own view: payloads and sparse vectors.
    let http = reqwest::Client::new();
    let scroll: Value = http
        .post(format!(
            "{}/collections/ostler_sources/points/scroll",
            srv.url
        ))
        .json(&json!({"limit": 10, "with_payload": true, "with_vector": true}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let points = scroll["result"]["points"].as_array().unwrap();
    assert_eq!(points.len(), 2);
    let plain = doc_vector(&format!("Page\nDocs\n{}", texts[0]), &TermHasher::Plain);
    for p in points {
        let s = p["payload"].to_string();
        for leak in [
            "Lovelace",
            "cardiology",
            "clinic.example",
            "Parking",
            "Docs",
            "Page",
        ] {
            assert!(!s.contains(leak), "payload leaks {leak:?}: {s}");
        }
        let idx: Vec<u32> = serde_json::from_value(p["vector"]["bm25"]["indices"].clone()).unwrap();
        assert!(!idx.is_empty());
        assert!(
            idx.iter().all(|i| !plain.indices.contains(i)),
            "a stored term id equals the public hash of a word"
        );
    }

    // Another key finds nothing.
    let wrong: Arc<dyn Store> = Arc::new(SealedStore::new(raw, [8; 32]));
    let other = Memory::new(wrong, "alice");
    assert!(other
        .recall_sources("cardiology", 5, &Default::default())
        .await
        .unwrap()
        .is_empty());
    assert!(other.list_sources(5).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_source_collection_without_the_sparse_vector_is_refused_with_advice() {
    let Some(srv) = start().await else { return };
    // Someone created the collection by hand with a single unnamed vector.
    let http = reqwest::Client::new();
    let made = http
        .put(format!("{}/collections/ostler_sources", srv.url))
        .json(&json!({"vectors": {"size": 256, "distance": "Cosine"}}))
        .send()
        .await
        .unwrap();
    assert!(made.status().is_success());
    let err = Memory::new(store(&srv.url, 256), "alice")
        .init()
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("bm25") && err.contains("ostler_sources"),
        "{err}"
    );
}

#[tokio::test]
async fn local_file_store_persists_sources_across_restarts() {
    // No server needed; lives here with the other retrieval parity tests.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.local");
    let open = || {
        Arc::new(LocalStore::open(&path, Arc::new(HashEmbedder::new(256)), Some([3; 32])).unwrap())
    };
    let m = Memory::new(open(), "alice");
    seed(&m).await;
    drop(m);
    let m = Memory::new(open(), "alice");
    let hits = m
        .recall_sources(
            "zx 4471",
            2,
            &SourceRecallOptions {
                dense_weight: 0.0,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(ids(&hits), [format!("{C}#c1")]);
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(
        !raw.contains("ZX-4471") && !raw.contains("a.example"),
        "file is sealed"
    );
    assert_eq!(m.forget_sources(Some(C)).await.unwrap(), 3);
    drop(m);
    assert_eq!(
        Memory::new(open(), "alice")
            .source_stats()
            .await
            .unwrap()
            .urls,
        2
    );
}
