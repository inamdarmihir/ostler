//! `RoutedStore` over a real Qdrant: pages go to Qdrant, saved facts stay in the other store.
//! Skips unless QDRANT_URL or OSTLER_QDRANT_BIN is set (see qdrant_real.rs).

mod common;

use std::sync::Arc;

use common::{start, store};
use ostler_memory::*;
use serde_json::json;

#[tokio::test]
async fn only_source_chunks_reach_qdrant_and_facts_stay_local() {
    let Some(srv) = start().await else { return };
    let qdrant = store(&srv.url, 64);
    let local: Arc<dyn Store> = Arc::new(InMemoryStore::new(Arc::new(HashEmbedder::new(64))));
    let routed: Arc<dyn Store> = Arc::new(RoutedStore::new(qdrant.clone(), local.clone()));
    let mem = Memory::new(routed, "t");
    mem.init().await.unwrap();

    mem.remember("email", json!("ada@example.com"), None, None, "test")
        .await
        .unwrap();
    mem.save_sources(
        "https://docs.example/a",
        vec![SourceChunk {
            url: "https://docs.example/a".into(),
            chunk_id: "c0".into(),
            ordinal: 0,
            heading_path: vec![],
            text: "The borrow checker rejects dangling references at compile time".into(),
            fetched_at: 0,
            content_hash: "h".into(),
            title: None,
        }],
    )
    .await
    .unwrap();

    // The fact is in the local half and in no Qdrant collection; the page is in Qdrant and not local.
    // Qdrant keeps one bookkeeping point of its own (the embedding-model marker), so count this tenant's records.
    let all = Filter::tenant("t");
    assert_eq!(local.count(Kind::Profile, &all).await.unwrap(), 1);
    assert_eq!(qdrant.count(Kind::Profile, &all).await.unwrap(), 0);
    assert_eq!(qdrant.count(Kind::Source, &all).await.unwrap(), 1);
    assert_eq!(local.count(Kind::Source, &all).await.unwrap(), 0);

    // Both still answer through `Memory`.
    let facts = mem.facts(10).await.unwrap();
    assert_eq!(facts.len(), 1, "{facts:?}");
    let hits = mem
        .recall_sources("dangling references", 3, &SourceRecallOptions::default())
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
}
