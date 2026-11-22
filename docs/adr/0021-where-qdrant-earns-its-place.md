# ADR-0021: Qdrant holds what needs a vector server, and nothing else

Status: accepted for the option; the **default is unchanged** (see "Not decided here"), 2026-11-22

## Context

Ostler began with Qdrant holding everything it remembers: saved facts, form playbooks, error episodes. Source
memory (ADR-0020) added page chunks. As agents share Ostler (subagents, swarms, several machines), the
question is which of these genuinely need a vector database, and which are there only because Qdrant was
there. Using Qdrant where a file would do costs a server to run, a network round trip, a cloud embedding of
your field labels when `--embeddings cloud` is on (the default), and a story that overstates what the database
does.

The test applied to each kind of data is about the data, not its name: is it **large and growing**, is it
**searched fuzzily over unstructured text** (meaning plus exact terms), and does it have to be **shared across
processes or machines**?

## The audit

| data | size | how it is queried | shared by | needs a vector server? |
|---|---|---|---|---|
| Saved facts (`profile`) | tens per person | exact key; a fuzzy label-to-key fallback, margin-gated (the design review found its top-1 behaviour caused wrong fills) | one person | **No.** A local file does it. |
| Form playbooks | a few per site, hundreds per person | exact form fingerprint; keyword filter on host and labels; ranking by win rate and recency | one person; **a team** may share them | No for one person. A team sharing playbooks across machines is a real server use, and the argument for `--qdrant-scope all`. |
| Error episodes | small, deduplicated | by hash | one person | **No.** |
| **Source chunks** (pages read) | thousands to millions, grows with every read | hybrid: meaning plus exact terms, diversity across pages, expiry | **every agent, process and machine** | **Yes.** Measured below. |
| Session evidence, citations, ledger, `context` notes | hundreds, per session | exact | agents in **one** Ostler process | **No, and not stored at all**: in-process. |
| Swarm coordination (who is reading what) | ephemeral | exact | agents | **Never a vector database.** Not built; if it is, a lock file. |
| Action traces and skills (future) | grows | similarity over trajectories | a team | **Candidate**, only after the evaluation shows replay helps. |

## Evidence

- **Source chunks need it.** [bench-results/source-scale.md](../../bench-results/source-scale.md): the local file
  store rewrites its whole file per save, so saving one page takes about 0.12 s at 500 chunks and 1.0 s at 2,000;
  Qdrant stays near 10 ms from 500 to 10,000. Reads are not the difference (recall is 1 to 3 ms on both).
- **Facts and playbooks do not.** The learning-loop evaluation ([ADR-0008](0008-learning-loop.md)) gives the same
  result on the in-process store and on a real Qdrant 1.20: correct fills 1 to 8 of 13, wrong fills 0 in both
  (`cargo test -p ostler-memory --test learning_eval`). Both runs use the hash embedder, so this says nothing
  about a real semantic embedder, which affects only the fuzzy fallback.
- **Routing works on a real server.** `crates/ostler-memory/tests/routed_real.rs`: with the router, a saved fact
  is in no Qdrant collection for the tenant and the page chunk is.

## Shared context for subagents and swarms

- **Claude Code subagents share the parent's MCP connection** by default (its subagent documentation: "string
  references share the parent session's connection", and subagents inherit the parent's tools). Subagents of one
  session therefore already share one Ostler process, and with it `context`, the evidence ledger and `cite`.
  That is shared context with no store behind it. `--swarm` ([cookbook recipe 15](../COOKBOOK.md#15-subagents-and-agent-swarms))
  makes the one shared thing that is hazardous, the active tab, explicit.
- **Not verified:** how Codex, Cursor or other clients give subagents their MCP connections.
- **Across processes**, what is shared is the pages: `--keep-sources` with a memory every process reaches
  (Qdrant for several processes or machines). Evidence is not shared across processes, and does not need to be:
  the stored-copy verdict ([ADR-0020](0020-source-memory.md)) is the handover.
- **Not built:** file-backed `context` notes across processes. Only worth building for clients that start one
  Ostler per subagent, and it would need a lock held across the read-modify-write, sealing or refusal of
  sensitive-looking values, and a `0600` file that refuses symlinks. No Qdrant.

## Decision

1. Add `--qdrant-scope all|sources` (`OSTLER_QDRANT_SCOPE`, config key `qdrant_scope`). With `sources`, a
   `RoutedStore` sends source chunks to Qdrant and everything else to the local encrypted file. It adds no
   logic: each call goes to exactly one store. `ostler doctor` and the startup block say which scope is on.
2. `read` saves pages in the background, so a slow store does not slow the tool.
3. State the rule in the docs: Qdrant for what is large, fuzzy and shared; a file for the rest.

## Consequences and risks

- **The local half uses the local embedder**, the hash embedder unless `--embeddings ollama`. With
  `--embeddings cloud` the fuzzy fallback for facts becomes word matching in this scope. The learning-loop result
  above is the same with hash embeddings; a real semantic embedder was not compared.
- **Existing facts in Qdrant become invisible** if someone switches an existing setup to `sources`: they are
  still in Qdrant, but reads go to the local file. There is no migration. This is why the default is not changed.
- **Privacy:** with `--embeddings cloud` and `sources`, the text of *pages agents read* is sent to Qdrant Cloud
  Inference to be embedded, even when sealed; field labels are not. The CLI prints this when it applies.
- **The local file is not a fallback for pages**: it was measured to be unusable beyond a few hundred chunks.

## Not decided here

Whether `sources` should become the default for new setups, or for everyone. That changes where saved data is
read from for existing users. It is the maintainer's decision; the recommendation is `sources` for new setups
(`ostler init`) and an explicit migration before changing it for existing ones.

## What would change this

If a semantic embedder makes the facts fallback measurably better and the wrong-fill rate stays at zero, the case
for Qdrant holding facts strengthens, for teams at least. If a model-in-the-loop run shows agents do not use
source memory, even the one place Qdrant is justified here is only justified for scale, not for value.
