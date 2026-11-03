//! Shared helpers: a real Qdrant server for integration tests.
#![allow(dead_code)]

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use ostler_memory::*;

pub struct Server {
    pub url: String,
    child: Option<Child>,
    _dir: Option<tempfile::TempDir>,
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(c) = &mut self.child {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

pub async fn start() -> Option<Server> {
    if let Ok(url) = std::env::var("QDRANT_URL") {
        return Some(Server {
            url,
            child: None,
            _dir: None,
        });
    }
    let Ok(bin) = std::env::var("OSTLER_QDRANT_BIN") else {
        assert!(
            std::env::var("OSTLER_REQUIRE_QDRANT").is_err(),
            "Qdrant required but not configured"
        );
        eprintln!("SKIPPED: set QDRANT_URL or OSTLER_QDRANT_BIN");
        return None;
    };
    let dir = tempfile::tempdir().unwrap();
    let (http, grpc) = (free_port(), free_port());
    let child = Command::new(bin)
        .current_dir(dir.path())
        .env("QDRANT__SERVICE__HTTP_PORT", http.to_string())
        .env("QDRANT__SERVICE__GRPC_PORT", grpc.to_string())
        .env("QDRANT__STORAGE__STORAGE_PATH", dir.path().join("storage"))
        .env("QDRANT__STORAGE__SNAPSHOTS_PATH", dir.path().join("snaps"))
        .env("QDRANT__TELEMETRY_DISABLED", "true")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn qdrant");
    let url = format!("http://127.0.0.1:{http}");
    // Owning the child in `Server` first guarantees it is killed and reaped even if we panic below.
    let server = Server {
        url: url.clone(),
        child: Some(child),
        _dir: Some(dir),
    };
    let client = reqwest::Client::new();
    for _ in 0..100 {
        if client
            .get(format!("{url}/readyz"))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
        {
            return Some(server);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("qdrant did not become ready");
}

pub fn store(url: &str, dim: usize) -> Arc<QdrantStore> {
    Arc::new(
        QdrantStore::new(QdrantConfig {
            url: url.into(),
            api_key: None,
            vectors: VectorSource::Client(Arc::new(HashEmbedder::new(dim))),
            timeout: Duration::from_secs(10),
            on_disk: false,
            allow_insecure: false,
            collection_prefix: String::new(),
        })
        .unwrap(),
    )
}
