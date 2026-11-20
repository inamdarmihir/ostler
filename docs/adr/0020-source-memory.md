# ADR-0020: Source memory is opt-in, and a stored copy never passes for a live read

Status: accepted, 2026-11-20

## Context

`ostler-memory` has had a `source` kind since the research work: page chunks in their own collection with a
dense and a sparse (BM25) vector, hybrid recall by weighted RRF, MMR, a per-URL cap and retention. Nothing
called it, so Qdrant held profile facts and form playbooks only, and a page read in one session was gone in the
next. The README promised exactly that ("read pages are not kept across sessions").

Wiring it in creates one real risk. `cite` returns `VERIFIED` only for words on a page *read in this session*.
A citation backed by a copy from last week must not look the same, or the verification promise (the thing that
separates Ostler from a page-to-markdown API) gets a false-success path of its own: the page may have changed,
been taken down, or never said that in the first place.

## Decision

1. **Opt-in.** `ostler serve --keep-sources` (config `keep_sources`, env `OSTLER_KEEP_SOURCES`). Without it
   nothing is written and the README promise holds. It needs a memory (Qdrant or `--memory local`); with none,
   the flag is reported and ignored. Chunks expire after `source_retain_days` (default 30) and `forget` removes
   them, as for the other kinds.
2. **`read` stores, `read {stored: true, query}` recalls.** No new tool: a parameter on `read` (the schema budget
   in `tools::schemas_are_small` is a hard cap). It does not navigate. Every hit shows its URL, fetch time,
   content hash and score under a header that says these are stored copies from earlier sessions.
3. **`cite` has a second verdict.** For a page *not* read in this session, `cite` checks the quote against the
   stored chunks of that URL and answers `VERIFIED AGAINST A STORED COPY (not read in this session) <url>
   (fetched <time>, sha256:<hash>)`. That is never `VERIFIED [n]`, it is not recorded as a citation, and
   `evidence` does not list it. A quote that is on no stored chunk stays `NOT READ`. Re-reading the page turns
   it into a live citation.
4. **Stored text stays untrusted.** Recall output is fenced and quarantined like live page text, and the
   injection heuristics are re-run on recalled chunks, so an instruction planted in a page is flagged again
   when it comes back weeks later. Sealing and card-number refusal are unchanged (`ostler-memory`).

## Consequences

- A quote that spans two stored chunks does not verify as stored (live `cite` checks the whole page). It
  returns `NOT READ`: refusing is the safe failure, and the cost is a re-read.
- Persistence is a memory-poisoning surface: a hostile page that was read once can resurface. Mitigations are
  the opt-in, the 30-day expiry, the stored-copy wording, and re-flagging. Listed in THREAT-MODEL.md.
- This is the first place Qdrant sits inside the research loop rather than beside it, which makes it
  measurable: `crates/ostler-mcp/tests/source_memory_e2e.rs` runs two sessions (second one in a fresh `App`
  sharing only the memory) with keeping on and off. It measures the mechanism only (an in-process store and a
  hash embedder, one small page). It says nothing yet about retrieval quality at scale, or whether agents
  answer better with it; that needs the model-in-the-loop run (`scripts/eval/`) with two-session tasks.

## What would change this decision

If a model-in-the-loop run shows no gain in cost or correctness for session 2, source memory goes back behind
a build feature and the README keeps saying pages are not kept. If it shows citation errors from stale copies,
the `cite` fallback is removed and recall stays advisory.
