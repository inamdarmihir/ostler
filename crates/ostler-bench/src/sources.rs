//! Where does Qdrant start to earn its place for source memory? The same synthetic corpus goes through
//! three stores: the in-process one, the local encrypted file, and a real Qdrant. For each corpus size:
//! how long a page takes to save, how long a recall takes, how long a new process takes to open the store,
//! and whether two writers on one store lose each other's updates.
//!
//! This measures cost and correctness of the storage, not retrieval quality: the embedder is a hash
//! embedder (words, not meaning), so recall hit-rates here say nothing about real embeddings.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use ostler_memory::{
    HashEmbedder, InMemoryStore, LocalStore, Memory, QdrantConfig, QdrantStore, SourceChunk,
    SourceRecallOptions, Store, VectorSource,
};
use serde_json::{json, Value};

use crate::memory::start_server;

const CHUNKS_PER_PAGE: usize = 10;
const WORDS_PER_CHUNK: usize = 60;
const VOCAB: usize = 4000;
const INGEST_BUDGET: Duration = Duration::from_secs(150);

fn lcg(seed: &mut u64) -> u64 {
    *seed = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *seed >> 33
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    if v.is_empty() {
        return 0.0;
    }
    v[(((v.len() - 1) as f64) * p).round() as usize]
}

/// A page of chunks drawn from a skewed vocabulary, with one planted identifier (`zx<page>q`) in chunk 3.
fn page(n: usize, seed: &mut u64) -> (String, Vec<SourceChunk>) {
    let url = format!("https://docs.example.test/p{n}");
    let hash = format!("{n:064x}");
    let chunks = (0..CHUNKS_PER_PAGE)
        .map(|i| {
            let mut words: Vec<String> = (0..WORDS_PER_CHUNK)
                .map(|_| {
                    // Skewed: low ranks are common, like real text.
                    let r = (lcg(seed) % 1000) as f64 / 1000.0;
                    format!("w{}", ((r * r * r) * VOCAB as f64) as usize)
                })
                .collect();
            if i == 3 {
                words.insert(10, format!("zx{n}q"));
            }
            SourceChunk {
                url: url.clone(),
                chunk_id: format!("c{i}"),
                ordinal: i as u32,
                heading_path: vec![format!("Section {i}")],
                text: words.join(" "),
                fetched_at: 1_700_000_000,
                content_hash: hash.clone(),
                title: Some(format!("Page {n}")),
            }
        })
        .collect();
    (url, chunks)
}

struct Row {
    store: &'static str,
    chunks: usize,
    ingested: usize,
    ingest_s: f64,
    save_ms_p50: f64,
    save_ms_p95: f64,
    recall_ms_p50: f64,
    recall_ms_p95: f64,
    id_hit: f64,
    id_only_hit: f64,
    id_heavy_hit: f64,
    open_ms: Option<f64>,
    file_mib: Option<f64>,
    lost_updates: Option<i64>,
}

async fn measure(
    label: &'static str,
    mem: &Memory,
    target: usize,
    queries: usize,
    reopen: Option<&dyn Fn() -> Result<Memory>>,
    file: Option<&std::path::Path>,
    second_writer: Option<Memory>,
) -> Result<Row> {
    let pages = target / CHUNKS_PER_PAGE;
    let mut seed = 7u64;
    let t0 = Instant::now();
    let mut saves = vec![];
    let mut done = 0;
    for n in 0..pages {
        let (url, chunks) = page(n, &mut seed);
        let s = Instant::now();
        mem.save_sources(&url, chunks).await?;
        // Steady-state cost: the last 50 pages saved, at (nearly) the final corpus size.
        if n + 50 >= pages {
            saves.push(s.elapsed().as_secs_f64() * 1e3);
        }
        done = n + 1;
        if t0.elapsed() > INGEST_BUDGET {
            break;
        }
    }
    let ingest_s = t0.elapsed().as_secs_f64();
    if saves.is_empty() {
        // Did not reach the end inside the budget: time the next few saves at the size reached.
        for n in done..done + 5 {
            let (url, chunks) = page(n, &mut seed);
            let s = Instant::now();
            mem.save_sources(&url, chunks).await?;
            saves.push(s.elapsed().as_secs_f64() * 1e3);
        }
    }

    let mut recall = vec![];
    let mut hit = 0usize;
    let mut hit_only = 0usize;
    let mut hit_heavy = 0usize;
    let mut qseed = 99u64;
    for _ in 0..queries {
        let n = (lcg(&mut qseed) as usize) % done.max(1);
        let q = format!("zx{n}q w{} w{}", lcg(&mut qseed) % 50, lcg(&mut qseed) % 50);
        let s = Instant::now();
        let hits = mem
            .recall_sources(&q, 5, &SourceRecallOptions::default())
            .await?;
        recall.push(s.elapsed().as_secs_f64() * 1e3);
        let on_page = |hs: &[ostler_memory::SourceHit]| {
            hs.iter().any(|h| h.chunk.url.ends_with(&format!("/p{n}")))
        };
        if on_page(&hits) {
            hit += 1;
        }
        // The same mixed query with the exact-word ranking weighted 4x.
        let heavy = SourceRecallOptions {
            sparse_weight: 4.0,
            ..Default::default()
        };
        if on_page(&mem.recall_sources(&q, 5, &heavy).await?) {
            hit_heavy += 1;
        }
        // The bare identifier, as a person pasting an error code or part number would search.
        let only = mem
            .recall_sources(&format!("zx{n}q"), 5, &SourceRecallOptions::default())
            .await?;
        if on_page(&only) {
            hit_only += 1;
        }
    }

    let open_ms = match reopen {
        Some(f) => {
            let s = Instant::now();
            let m = f()?;
            m.init().await?;
            Some(s.elapsed().as_secs_f64() * 1e3)
        }
        None => None,
    };
    let file_mib = file
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.len() as f64 / 1_048_576.0);

    // Two writers on one store, at the same time, each saving 20 new pages: how many chunks are missing?
    let lost_updates = if let Some(other) = second_writer {
        let before = mem.source_stats().await?.chunks as i64;
        let (a, b) = (mem.clone(), other);
        let base = done + 1000;
        let wa = tokio::spawn(async move {
            let mut s = 1u64;
            for n in base..base + 20 {
                let (u, c) = page(n, &mut s);
                a.save_sources(&u, c).await.unwrap();
            }
        });
        let wb = tokio::spawn(async move {
            let mut s = 2u64;
            for n in base + 100..base + 120 {
                let (u, c) = page(n, &mut s);
                b.save_sources(&u, c).await.unwrap();
            }
        });
        wa.await?;
        wb.await?;
        let after = mem.source_stats().await?.chunks as i64;
        Some((40 * CHUNKS_PER_PAGE as i64) - (after - before))
    } else {
        None
    };

    Ok(Row {
        store: label,
        chunks: target,
        ingested: done * CHUNKS_PER_PAGE,
        ingest_s,
        save_ms_p50: pct(&mut saves.clone(), 0.5),
        save_ms_p95: pct(&mut saves, 0.95),
        recall_ms_p50: pct(&mut recall.clone(), 0.5),
        recall_ms_p95: pct(&mut recall, 0.95),
        id_hit: hit as f64 / queries.max(1) as f64,
        id_only_hit: hit_only as f64 / queries.max(1) as f64,
        id_heavy_hit: hit_heavy as f64 / queries.max(1) as f64,
        open_ms,
        file_mib,
        lost_updates,
    })
}

pub async fn run(sizes: &str, queries: usize) -> Result<Value> {
    let embedder = || Arc::new(HashEmbedder::new(256));
    let key = [7u8; 32];
    let mut rows: Vec<Row> = vec![];
    for size in sizes.split(',').map(|s| s.trim().parse::<usize>()) {
        let size = size?;

        // In-process: what a swarm member would hold if it kept everything in RAM (nothing is shared).
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new(embedder()));
        let mem = Memory::new(store, "bench");
        mem.init().await?;
        rows.push(measure("in-process", &mem, size, queries, None, None, None).await?);

        // Local encrypted file: shared by every process on the machine, whole file rewritten per save.
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("memory.bin");
        let open = {
            let path = path.clone();
            move || -> Result<Memory> {
                let s: Arc<dyn Store> = Arc::new(LocalStore::open(&path, embedder(), Some(key))?);
                Ok(Memory::new(s, "bench"))
            }
        };
        let mem = open()?;
        mem.init().await?;
        let other = open()?;
        rows.push(
            measure(
                "local file (sealed)",
                &mem,
                size,
                queries,
                Some(&open),
                Some(&path),
                Some(other),
            )
            .await?,
        );

        // Real Qdrant.
        let srv = start_server().await?;
        let q = || -> Result<Arc<dyn Store>> {
            Ok(Arc::new(QdrantStore::new(QdrantConfig {
                url: srv.url.clone(),
                api_key: None,
                vectors: VectorSource::Client(embedder()),
                timeout: Duration::from_secs(60),
                on_disk: false,
                allow_insecure: false,
                collection_prefix: String::new(),
            })?))
        };
        let mem = Memory::new(q()?, "bench");
        mem.init().await?;
        let other = Memory::new(q()?, "bench");
        rows.push(measure("qdrant 1.20", &mem, size, queries, None, None, Some(other)).await?);
        drop(srv);
    }

    println!("\nSource memory cost by store (synthetic text, hash embedder: cost and correctness, not retrieval quality)\n");
    println!(
        "{:<20} {:>8} {:>9} | {:>9} {:>9} | {:>9} {:>9} | {:>7} {:>7} | {:>9} {:>8} {:>6}",
        "store",
        "chunks",
        "ingest s",
        "save p50",
        "save p95",
        "recall p50",
        "recall p95",
        "id+words",
        "id only",
        "open ms",
        "file MiB",
        "lost"
    );
    for r in &rows {
        println!(
            "{:<20} {:>8} {:>9.1} | {:>8.1}ms {:>8.1}ms | {:>8.2}ms {:>8.2}ms | {:>6.0}% {:>6.0}% {:>7.0}% | {:>9} {:>8} {:>6}",
            r.store,
            r.ingested,
            r.ingest_s,
            r.save_ms_p50,
            r.save_ms_p95,
            r.recall_ms_p50,
            r.recall_ms_p95,
            r.id_hit * 100.0,
            r.id_only_hit * 100.0,
            r.id_heavy_hit * 100.0,
            r.open_ms.map_or("-".into(), |v| format!("{v:.0}")),
            r.file_mib.map_or("-".into(), |v| format!("{v:.1}")),
            r.lost_updates.map_or("-".into(), |v| v.to_string()),
        );
    }
    Ok(json!({"source_scale": rows.iter().map(|r| json!({
        "store": r.store, "target_chunks": r.chunks, "ingested_chunks": r.ingested,
        "ingest_s": r.ingest_s, "save_ms": {"p50": r.save_ms_p50, "p95": r.save_ms_p95},
        "recall_ms": {"p50": r.recall_ms_p50, "p95": r.recall_ms_p95}, "id_with_words_hit_rate": r.id_hit, "id_only_hit_rate": r.id_only_hit, "id_with_words_sparse_x4_hit_rate": r.id_heavy_hit,
        "open_ms": r.open_ms, "file_mib": r.file_mib, "lost_updates_two_writers": r.lost_updates,
    })).collect::<Vec<_>>()}))
}
