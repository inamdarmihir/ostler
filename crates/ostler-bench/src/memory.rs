//! Memory at scale on a real Qdrant: how does filtered, multi-tenant retrieval behave
//! as users and facts grow, and what does the footprint look like?

use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use ostler_memory::{
    FieldQuery, HashEmbedder, Kind, Memory, QdrantConfig, QdrantStore, Store, VectorSource,
};
use serde_json::{json, Value};

pub struct Server {
    pub url: String,
    pub pid: Option<u32>,
    child: Option<Child>,
    pub dir: Option<tempfile::TempDir>,
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
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

pub async fn start_server() -> Result<Server> {
    if let Ok(url) = std::env::var("QDRANT_URL") {
        return Ok(Server {
            url,
            pid: None,
            child: None,
            dir: None,
        });
    }
    let bin = std::env::var("OSTLER_QDRANT_BIN")
        .context("set OSTLER_QDRANT_BIN (path to a qdrant binary) or QDRANT_URL")?;
    let dir = tempfile::tempdir()?;
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
        .spawn()?;
    let pid = child.id();
    let url = format!("http://127.0.0.1:{http}");
    let server = Server {
        url: url.clone(),
        pid: Some(pid),
        child: Some(child),
        dir: Some(dir),
    };
    let client = reqwest::Client::new();
    for _ in 0..200 {
        if client
            .get(format!("{url}/readyz"))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
        {
            return Ok(server);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!("qdrant did not become ready")
}

fn ps_rss_kb(pid: u32) -> Option<u64> {
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

fn du_kb(path: &std::path::Path) -> Option<u64> {
    let out = Command::new("du").args(["-sk"]).arg(path).output().ok()?;
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    if v.is_empty() {
        return 0.0;
    }
    v[(((v.len() - 1) as f64) * p).round() as usize]
}

const KEYS: [&str; 20] = [
    "first_name",
    "last_name",
    "email",
    "phone",
    "address",
    "city",
    "state",
    "postal_code",
    "country",
    "company",
    "job_title",
    "website",
    "username",
    "birthday",
    "full_name",
    "linkedin",
    "github",
    "twitter",
    "timezone",
    "language",
];

fn lcg(seed: &mut u64) -> u64 {
    *seed = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *seed >> 33
}

pub async fn run(sizes: &str, queries: usize, on_disk: bool) -> Result<Value> {
    println!(
        "Memory scale on real Qdrant (vectors {}). Vectors:",
        if on_disk { "ON DISK" } else { "in RAM" }
    );
    println!("Embeddings: 256-dim client-side hash embeddings (a real 384-dim model is ~1.5x the vector footprint).");
    println!("Latencies are client-observed over localhost HTTP, sequential unless noted; add network RTT for a remote cluster.\n");
    println!(
        "{:<14} {:>9} {:>9} | {:>8} {:>8} {:>8} | {:>8} {:>8} | {:>9} {:>9} {:>10}",
        "tenants x facts",
        "points",
        "load s",
        "sem p50",
        "sem p95",
        "sem p99",
        "key p50",
        "key p95",
        "QPS x16",
        "RSS MiB",
        "disk B/pt"
    );
    let mut results = vec![];
    for spec in sizes.split(',') {
        let (t, f) = spec.split_once('x').context("sizes look like 1000x20")?;
        let (tenants, facts): (usize, usize) = (t.trim().parse()?, f.trim().parse()?);
        let srv = start_server().await?;
        let store = Arc::new(QdrantStore::new(QdrantConfig {
            url: srv.url.clone(),
            api_key: None,
            vectors: VectorSource::Client(Arc::new(HashEmbedder::new(256))),
            timeout: Duration::from_secs(30),
            on_disk,
        })?);
        store.ensure().await?;

        let t0 = Instant::now();
        let mut batch = Vec::with_capacity(500);
        for ti in 0..tenants {
            for key in KEYS.iter().cycle().take(facts) {
                let id = uuid::Uuid::new_v4().to_string();
                let text = key.replace('_', " ");
                batch.push((id, text, json!({"tenant": format!("t{ti}"), "key": key, "key_norm": key.replace('_', " "), "value": format!("{key}-value-{ti}"), "autocomplete": Value::Null, "source": "bench", "updated_at": 0})));
                if batch.len() == 500 {
                    store
                        .upsert_batch(Kind::Profile, std::mem::take(&mut batch))
                        .await?;
                }
            }
        }
        if !batch.is_empty() {
            store.upsert_batch(Kind::Profile, batch).await?;
        }
        let load_s = t0.elapsed().as_secs_f64();
        let points = store.count(Kind::Profile, &Default::default()).await?;

        let mut seed = 42u64;
        let query = |label: &'static str| FieldQuery {
            host: "bench",
            fingerprint: "none",
            label,
            name: None,
            kind: "email",
            placeholder: None,
            autocomplete: None,
        };
        for _ in 0..20 {
            let m = Memory::new(
                store.clone(),
                format!("t{}", lcg(&mut seed) as usize % tenants),
            );
            let _ = m.suggest(&query("E-mail address"), 0.3).await;
        }
        let (mut sem, mut key) = (vec![], vec![]);
        for _ in 0..queries {
            let m = Memory::new(
                store.clone(),
                format!("t{}", lcg(&mut seed) as usize % tenants),
            );
            let t = Instant::now();
            let r = m.suggest(&query("E-mail address"), 0.3).await?;
            sem.push(t.elapsed().as_secs_f64() * 1000.0);
            anyhow::ensure!(r.is_some(), "semantic query returned nothing");
            let t = Instant::now();
            let f = m.fact("email").await?;
            key.push(t.elapsed().as_secs_f64() * 1000.0);
            anyhow::ensure!(f.is_some(), "key lookup returned nothing");
        }
        // Throughput: 16 concurrent clients.
        let t = Instant::now();
        let per = queries.max(16) / 16;
        let mut tasks = vec![];
        for w in 0..16u64 {
            let store = store.clone();
            tasks.push(tokio::spawn(async move {
                let mut seed = 1000 + w;
                for _ in 0..per {
                    let m = Memory::new(
                        store.clone(),
                        format!("t{}", lcg(&mut seed) as usize % tenants),
                    );
                    let _ = m
                        .suggest(
                            &FieldQuery {
                                host: "b",
                                fingerprint: "none",
                                label: "E-mail address",
                                name: None,
                                kind: "email",
                                placeholder: None,
                                autocomplete: None,
                            },
                            0.3,
                        )
                        .await;
                }
            }));
        }
        for t in tasks {
            t.await?;
        }
        let qps = (per * 16) as f64 / t.elapsed().as_secs_f64();

        let rss = srv.pid.and_then(ps_rss_kb).map(|k| k / 1024);
        let disk = srv
            .dir
            .as_ref()
            .and_then(|d| du_kb(d.path()))
            .map(|k| k * 1024);
        let (s50, s95, s99) = (pct(&mut sem, 0.5), pct(&mut sem, 0.95), pct(&mut sem, 0.99));
        let (k50, k95) = (pct(&mut key, 0.5), pct(&mut key, 0.95));
        println!(
            "{:<14} {:>9} {:>9.1} | {:>6.1}ms {:>6.1}ms {:>6.1}ms | {:>6.1}ms {:>6.1}ms | {:>9.0} {:>9} {:>10}",
            format!("{tenants}x{facts}"), points, load_s, s50, s95, s99, k50, k95, qps,
            rss.map(|v| v.to_string()).unwrap_or("-".into()),
            disk.map(|d| (d / points.max(1)).to_string()).unwrap_or("-".into())
        );
        results.push(json!({"tenants": tenants, "facts_per_tenant": facts, "points": points, "load_s": load_s,
            "semantic_ms": {"p50": s50, "p95": s95, "p99": s99}, "key_lookup_ms": {"p50": k50, "p95": k95},
            "qps_16_clients": qps, "rss_mib": rss, "disk_bytes": disk}));
    }
    println!("\n'sem' = Memory::suggest semantic path (playbook lookup + filtered vector search = 2 requests). 'key' = exact fact lookup by key (1 filtered scroll).");
    Ok(json!({"memory_scale": results}))
}
