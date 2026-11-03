//! Server-enforced tenant isolation with scoped JWTs. Needs a Qdrant started with `service.api_key` and
//! `service.jwt_rbac: true`: set QDRANT_URL and OSTLER_QDRANT_ADMIN_KEY. Skips otherwise.

use std::sync::Arc;
use std::time::Duration;

use ostler_memory::*;
use serde_json::json;

fn client(url: &str, key: &str, prefix: &str) -> Arc<QdrantStore> {
    Arc::new(
        QdrantStore::new(QdrantConfig {
            url: url.into(),
            api_key: Some(key.into()),
            vectors: VectorSource::Client(Arc::new(HashEmbedder::new(256))),
            timeout: Duration::from_secs(10),
            on_disk: false,
            allow_insecure: false,
            collection_prefix: prefix.into(),
        })
        .unwrap(),
    )
}

#[tokio::test]
async fn a_tenant_token_cannot_touch_another_tenants_collections() {
    let (Ok(url), Ok(admin)) = (
        std::env::var("QDRANT_URL"),
        std::env::var("OSTLER_QDRANT_ADMIN_KEY"),
    ) else {
        eprintln!("SKIPPED: set QDRANT_URL and OSTLER_QDRANT_ADMIN_KEY (JWT RBAC enabled)");
        return;
    };
    let exp = 4_000_000_000;
    let (pa, pb) = (tenant_prefix("alice"), tenant_prefix("bob"));

    // The admin creates each tenant's collections once.
    let bob_admin = Memory::new(client(&url, &admin, &pb), "bob");
    bob_admin.init().await.unwrap();
    bob_admin.wipe().await.unwrap();
    bob_admin
        .remember("email", json!("bob@example.com"), None, None, "user")
        .await
        .unwrap();
    let alice_admin = Memory::new(client(&url, &admin, &pa), "alice");
    alice_admin.init().await.unwrap();
    alice_admin.wipe().await.unwrap();

    // Alice's token works on Alice's collections.
    let alice_tok = mint_tenant_token(&admin, "alice", exp, false).unwrap();
    let alice = Memory::new(client(&url, &alice_tok, &pa), "alice");
    alice
        .remember("email", json!("alice@example.com"), None, None, "user")
        .await
        .expect("alice may write her own collections");
    assert_eq!(alice.facts(10).await.unwrap().len(), 1);

    // ...and is refused (by the server) on Bob's, for reads and writes, even if the client lies about the tenant.
    let sneaky = Memory::new(client(&url, &alice_tok, &pb), "bob");
    assert!(
        sneaky.facts(10).await.is_err(),
        "alice's token read bob's collection"
    );
    assert!(sneaky
        .remember("email", json!("evil@example.com"), None, None, "user")
        .await
        .is_err());
    assert_eq!(
        bob_admin.fact("email").await.unwrap().unwrap().value,
        json!("bob@example.com")
    );

    // Read-only tokens cannot write; bad tokens cannot do anything.
    let ro = mint_tenant_token(&admin, "alice", exp, true).unwrap();
    let ro_mem = Memory::new(client(&url, &ro, &pa), "alice");
    assert_eq!(ro_mem.facts(10).await.unwrap().len(), 1);
    assert!(ro_mem
        .remember("city", json!("Pune"), None, None, "user")
        .await
        .is_err());
    let expired = mint_tenant_token(&admin, "alice", 1, false).unwrap();
    assert!(Memory::new(client(&url, &expired, &pa), "alice")
        .facts(10)
        .await
        .is_err());
    let forged = mint_tenant_token("not-the-admin-key", "alice", exp, false).unwrap();
    assert!(Memory::new(client(&url, &forged, &pa), "alice")
        .facts(10)
        .await
        .is_err());
}
