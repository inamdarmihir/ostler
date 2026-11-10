# Memory

Qdrant memory, the learning loop, and the privacy model. Back to the [README](../README.md).

## Memory

Three Qdrant collections, multi-tenant through a `tenant` payload index:

| collection | holds | never holds |
|---|---|---|
| `ostler_profile` | facts you saved (`email`, `company`, …) | passwords, card numbers, anything `looks_sensitive` |
| `ostler_playbooks` | per site and form: which fact fills which field, with wins, losses and last verified time | the values themselves |
| `ostler_episodes` | what went wrong on a form, so it is not repeated | secrets |

Retrieval order for a field: the form's own playbook, then an exact `autocomplete`-token match, then a **playbook from a similar form on the same site** (a redesign or a sibling form), then semantic search. `form_fill` with `auto: "required"` fills what memory can supply and reports the rest.

### It learns from every fill

Playbooks work like skills that improve with use. Ostler doesn't need a model to judge whether a fill worked: the browser reads every value back and the page accepts or rejects the submit, so every outcome is ground truth.

| step | what happens | Qdrant feature |
|---|---|---|
| recall | exact form first; after a redesign, mappings transfer from a similar form on the same site with the same label | keyword-array filter on `labels_norm` + `host`, tenant index |
| rank | proven, recently verified skills beat stale or failing ones | Query API prefetch + **score-boosting formula** (similarity + smoothed win rate + `exp_decay` on `verified_ts`) |
| reinforce | verified fields on accepted submits add wins per mapping | integer payload indexes |
| demote | a mapping that loses more than it wins is dropped | |
| forget | `ostler memory prune --days 90` removes stale, unproven skills | range filter on `verified_ts` |

`ostler memory skills` shows what has been learned: wins, losses, trusted mappings, and which form a skill was learned from. On a scripted site with redesigned forms, correct suggestions went from 1 to 8 of 10 fillable fields, with zero wrong fills in both conditions. That's a synthetic fixture, not real sites ([ADR-0008](adr/0008-learning-loop.md)).

### Privacy and security model of memory

**Qdrant version:** CI tests against v1.20.0. Ostler ranks suggestions itself over the candidates Qdrant returns, so no server-side formula is needed; older servers are not tested.

Memory is optional and off until you give Ostler a Qdrant URL. With memory in Qdrant, payloads are **sealed on your machine by default** (`--seal auto`). This summary matches [SECURITY.md](../SECURITY.md), which has the full threat model.

- **What someone who can read your Qdrant (operator, leaked key, backup) cannot read:** fact values, descriptions, playbooks and episodes are encrypted on your machine (AES-256-GCM); site names, field labels, keys and point ids are keyed hashes; episodes are redacted of emails and numbers; plain `http://` to a remote Qdrant is refused (`--allow-insecure-qdrant` overrides it for a VPN or tunnel).
- **What they can still see:** the **vector**. With `--embeddings cloud` (the default) the field text is still sent to Qdrant Cloud Inference to be embedded, and vectors of labels reveal roughly what they say. `--embeddings ollama` keeps text on your machine so only vectors leave. The **tenant name**, **win/loss counters**, timing and point counts are also visible.
- **The key:** by default a file (`memory.key` beside the config file, owner-only), or the `OSTLER_MEMORY_KEY` variable, or optionally the OS credential store (`key_store = keychain`). **If you lose it, sealed memory is unrecoverable**; the only way forward is `ostler memory wipe --yes`. Back it up (`ostler memory key export --yes-print`).
- **Cross-user exposure:** every tenant shares the same collections, separated by a `tenant` filter that Ostler sends with every query. Anyone holding the Qdrant API key can read every tenant's records (they are sealed, but the tenant name is visible). Give each machine or user its own key.
- **Secrets:** the memory API refuses password/token/card-like keys and values; playbooks store structure, never values. Detection is heuristic, so a secret stored under an innocuous key will not be caught.
- **Before sealing:** memory saved earlier stays readable until you run `ostler memory seal` (facts are re-saved sealed; learned skills and episodes from before are dropped and relearned).
- **Retention:** on start `serve` forgets learned skills and error episodes not verified for `--retain-days` (default 180, 0 keeps them). Facts you saved never expire. No telemetry.

`ostler doctor` prints this state for your setup (sealing, key location, embeddings, transport). The sealing, local-embedding, retention and transport features are on `main`, not in a tagged release yet. Nothing here has been audited by a third party.

Details: [ARCHITECTURE](ARCHITECTURE.md), [ADRs](adr/).
