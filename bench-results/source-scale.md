# Source memory cost by store (measured)

Same synthetic corpus (10 chunks per page, 60 words per chunk, skewed 4,000-word vocabulary, one planted
identifier per page) through three stores. One Linux container, release build, Qdrant 1.20.0 started locally,
256-dimension hash embedder, payloads not sealed on the Qdrant side, the local file sealed (AES-256-GCM).

```sh
OSTLER_QDRANT_BIN=/path/to/qdrant ostler-bench --json bench-results/source-scale.json sources --sizes 500,2000,10000
```

| store | chunks | save one 10-chunk page (p50 / p95) | recall (p50) | open a new process | store file | chunks lost, 2 writers |
|---|---:|---:|---:|---:|---:|---:|
| in-process (no persistence, not shared) | 500 | 0.3 / 0.4 ms | 0.49 ms | n/a | n/a | n/a |
| local encrypted file | 500 | **117 / 244 ms** | 0.46 ms | 20 ms | 2.6 MiB | 0 |
| Qdrant 1.20 | 500 | 11 / 15 ms | 1.78 ms | n/a | n/a | 0 |
| in-process | 2,000 | 0.4 / 0.5 ms | 1.32 ms | n/a | n/a | n/a |
| local encrypted file | 2,000 | **1,043 / 1,274 ms** | 1.44 ms | 94 ms | 10.3 MiB | 0 |
| Qdrant 1.20 | 2,000 | 11 / 13 ms | 2.56 ms | n/a | n/a | 0 |
| in-process | 10,000 | 1.4 / 1.6 ms | 6.82 ms | n/a | n/a | n/a |
| local encrypted file | 10,000 | not reached: 2,340 chunks in the 150 s budget; **1,522 / 1,785 ms** there | 2.05 ms | 141 ms | 12.4 MiB | 0 |
| Qdrant 1.20 | 10,000 | 9 / 12 ms | 2.54 ms | n/a | n/a | 0 |

What this says:

- **Writes are where the local file stops working.** It rewrites its whole file on every save, so one page costs
  about 0.1 s at 500 chunks and about 1 s at 2,000, and ingesting 2,000 chunks took 120 s. Qdrant stays near 10 ms
  from 500 to 10,000 chunks. Reading is not the problem: recall on the loaded local file is as fast as Qdrant.
- **Opening costs every process.** A new process loads the file: 20 ms at 500 chunks, 141 ms at 2,340. In a
  swarm where each agent starts its own Ostler, each one pays it.
- **Neither lost an update** with two writers on one store (20 pages each, 400 chunks expected, 0 missing). The
  local file's lock and atomic writes hold; this does not test two *processes*, only two handles.
- **A person reading docs** saves a page at a time. At 500 chunks (about 50 pages) the local file adds 0.1 s per
  read, at 2,000 (about 200 pages) about 1 s. Ostler now saves in the background so `read` does not wait for it,
  but the file is still rewritten each time.

What this does **not** say:

- **Nothing about retrieval quality.** The embedder is a hash embedder (shared words, not meaning). The
  `id only` column (the bare identifier, 100% on every store) shows exact-term search works; `id+words` (the
  identifier plus common words, 12 to 26%) and the `sparse x4` column are an artefact of this corpus: its
  vocabulary is tiny and heavily skewed, so "common" words are not distinctive and additive BM25 lets a chunk
  matching several of them beat a chunk with one rare identifier. Giving the exact-word ranking four times the
  weight did not change it. A quality benchmark needs real text and labelled questions.
- **Nothing about a remote Qdrant.** Latencies are localhost; add a network round trip.
- **One machine, one run.** Sizes beyond 10,000 chunks and Qdrant's memory use were not measured here.
