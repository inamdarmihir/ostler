# ADR-0008: A verified learning loop, ranked inside Qdrant

Status: accepted (2026-10-31)

## Context
Playbooks were looked up by exact form fingerprint only. One renamed field, a reorder or a new optional field changed the fingerprint and Ostler forgot everything it had learned about that form. Nothing ever expired, a mapping that started failing kept its 0.95 confidence, and nothing was learned from rejected submits except free-text episodes (external review, 2026-10-31, section 5).

Self-improving agents such as Nous Research's Hermes Agent use a loop of *do the task, verify it, save the durable lesson, patch the skill*. Ostler has an advantage there: it never has to ask a model whether a fill worked, because the browser reads every value back and the page accepts or rejects the submit. Every outcome is ground truth.

## Decision
Playbooks are skills, and every fill outcome patches them:

1. **Recall.** Exact fingerprint first. Otherwise **transfer**: a playbook from another form on the *same host* that has a field with the *same normalized label and kind* and a trusted mapping (`via: similar_form`, confidence 0.85, below the exact playbook at 0.95 and autocomplete at 0.9).
2. **Rank in Qdrant.** Candidates are filtered by tenant, host and label (`labels_norm`, a keyword array index) and re-scored in one Query API call with a score-boosting formula: `similarity + 0.3 * (wins+1)/(wins+losses+2) + 0.15 * exp_decay(verified_ts, half-life 30 days)`. The win rate is Laplace-smoothed, so an untested skill starts at 0.5. `Rank::score` evaluates the same formula client-side for the in-memory store, and a test pins the maths.
3. **Reinforce or demote.** Wins and losses are kept per playbook and per field mapping. Only verified fields on accepted submits count as wins. Failed read-backs and fields the page rejected count as losses. A mapping that has lost more often than it won is dropped, and the field falls back to the autocomplete or semantic signals, or to the agent.
4. **Patch, don't rewrite.** An outcome updates only the fields it mentions. Headings and other fields keep their history.
5. **Forget.** `Memory::prune` / `ostler memory prune --days N` deletes playbooks not verified within N days whose losses at least equal their wins, and episodes not seen within N days. It uses an integer range filter on `verified_ts`.
6. **Provenance.** A playbook first learned through transfer records `learned_from`. `ostler memory skills` lists every skill with its record, its trusted mappings and where it came from.

## Measured
`crates/ostler-memory/tests/learning_eval.rs` covers a scripted site: three forms learned, then two redesigns, a sibling form and a foreign site with the same labels. 13 fields in total: 10 with ground truth, 3 that must stay empty. Hash embeddings, in-memory store and real Qdrant 1.20.0 gave identical results:

| | correct | wrong | empty |
|---|---|---|---|
| baseline (facts only: what a new fingerprint got before) | 1 | 0 | 12 |
| learned | 8 | 0 | 5 |

The 5 empties are the 3 fields that must stay empty plus the 2 on the foreign site, which never borrows another site's skills. This is a synthetic fixture, not real sites. It shows the mechanism works and does not add wrong fills on this scenario. It is not evidence of a real-world hit rate.

## Rejected
- **Vector-only fuzzy playbook match with a similarity threshold** (borrow a whole playbook when the form "looks similar"). It's hard to calibrate across embedders: the dev hash vectors and MiniLM have different score scales. It would also map fields whose labels differ. Label-and-kind identity within a host is stricter and explainable. Structural similarity is used only for ranking and for the `form_inspect` hint.
- **Cross-site transfer.** "Org" on one site and "Org" on another need not mean the same thing, and sharing skills across sites raises privacy questions. A future opt-in, structure-only shared corpus is a separate decision.
- **LLM-written skills.** The calling agent is the planner. Memory learns only from verified outcomes, which keeps it deterministic, cheap and auditable.

## Risks
- A same-host sibling form with the same label but a different meaning (an "Email" field on a refer-a-friend form) can receive the user's fact. This is no worse than the existing semantic path, which would also match it, and the other-person guard still applies. Read-back cannot catch a wrong but valid value.
- The score-boosting formula needs Qdrant 1.14 or later.
