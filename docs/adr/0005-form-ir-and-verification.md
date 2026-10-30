# ADR-0005: A typed form model with verified filling

Status: accepted (2026-10-30)

## Context
Public evaluations report agents failing on forms: omitted required fields, mis-clicks on similarly labelled elements, filter/sort errors (see `docs/research/landscape.md`; secondary sources, unverified). Per-field tool calls also multiply LLM turns.

## Decision
`form_inspect` returns typed fields; `form_fill` accepts `{label|name|ref: value}`, resolves keys to fields (ambiguity is reported, never guessed), normalises values, fills in document order re-inspecting after each field, and reads each value back with a fallback strategy. `form_submit` classifies the result as submitted / advanced / validation-failed / unchanged and returns the page's own validation messages.

## Consequences
- A 20-field form completes in 3 calls instead of 22 in our per-field baseline (13x fewer input tokens, 5x cheaper). Small forms gain little (~1.3-2x). Numbers: `docs/BENCHMARKS.md`.
- The agent must trust label matching. We bias to refusing: `03/04/2024` is ambiguous and rejected; two equally good field matches are returned as candidates.
- Real-site behaviour is unproven; fixtures cover the documented failure modes only.
