# ADR-0006: Memory is structure plus opt-in facts, retrieved by the most reliable signal first

Status: accepted (2026-10-30)

## Decision
Three kinds of memory in separate Qdrant collections, all tenant-scoped (`tenant` keyword payload index with `is_tenant`): **profile** (user facts), **playbooks** (form structure that worked: labels and which fact fed each field, never values), **episodes** (deduplicated validation errors and failures). Retrieval order for a field: playbook mapping, then the field's `autocomplete` token matched to a fact via payload filter (no vectors), then semantic search. Writes happen in the background. Secrets are refused at the API boundary; `--learn` (off by default) is required to save values the agent typed.

## Why
- Exact signals are cheaper and more trustworthy than similarity; vectors are the fallback, not the first resort.
- Playbooks hold no user data, so the "it learns" behaviour is safe to enable by default.
- Measured: filtered retrieval stays ~0.4 ms p50 from 20k to 200k points; memory per point (~7-8 KB loaded and queried, untuned) is the open cost question (`docs/ECONOMICS.md`).

## Rejected
One shared collection without tenancy (isolation by convention only); embedding everything and relying on similarity alone; storing typed values by default.
