# Economics and scale: the hard questions

Written as the questions a skeptical investor or senior engineer would ask, answered with what we measured ([BENCHMARKS.md](BENCHMARKS.md)) and clearly separated from what we assume. Where we do not know, we say so.

## 1. What does one task cost, and who pays for what?

For a typical form task (measured on fixtures; LLM price is an illustrative $3 / $15 per Mtok):

| cost line | per task | share | basis |
|---|---:|---:|---|
| LLM tokens (3 calls, prompt-cached) | ~$0.017-0.024 | **>99%** | measured token counts x illustrative price |
| Browser compute | ~$0.000006 | ~0.03% | measured 0.5 CPU-s + 0.5 GiB for ~1 s, illustrative cloud price |
| Qdrant (memory ops) | ~2 requests, sub-ms | negligible | measured; Cloud Inference model cost not measured |

**Implication:** the browser is not the cost center, the model is. A browser that is 2x faster or cheaper to run changes the bill by nothing. A browser that makes the agent take fewer turns and read fewer tokens changes it a lot: on a 20-field form we measure **5.0x lower LLM cost and 5.4x lower latency** than a per-field interaction pattern. Ostler's economic claim is therefore "fewer turns and fewer tokens per completed form", not "faster browser".

## 2. Is the saving real, or an artefact of the baseline?

Partly baseline. The "per-field" comparison models the worst-case pattern (one call per field, full snapshot after each). Tools with batched fills close part of the gap. What stays differentiated even against a batched filler: verification by read-back, dependent dropdown/wizard handling in one call, shadow DOM/iframes, validation-error reporting, and cross-session memory. We do not yet have a head-to-head against other products on the same tasks; that is on the roadmap and we will publish it either way.

## 3. How does it scale?

**Per node (measured, 8-core laptop):** 8 parallel sessions gave 354-393 tasks/min, p95 under 1 s, 100% success, flat latency, 450-550 MiB per session. Derived planning numbers, assuming CPU-bound at 0.5 CPU-s per task: ~7,000 tasks per core-hour; memory-bound at ~0.5 GiB per session: ~14 concurrent sessions per 8 GB, ~120 per 64 GB. Assumptions: Linux numbers will differ; real sites are heavier than fixtures (more CPU, more RAM per page); idle sessions hold memory. Treat as a first-order estimate until the Linux benchmark job reports.

**Memory layer (measured on real Qdrant):** retrieval latency stays ~0.4 ms p50 from 20k to 200k points because queries are tenant-scoped via a payload index. Throughput ~2.4-2.9k QPS with 16 clients on one laptop-hosted node.

**What does not scale yet (be skeptical here):**
- Qdrant RAM per user is the open number. We saw ~6.8-8.4 KB per point at 200k points untuned, i.e. ~130-160 KB per user at 20 facts. At 1M users that is ~130-160 GB untuned. It is a loaded-and-queried figure, not isolated steady state, so it is an upper-ish bound, but it needs a proper sizing study with quantization and on-disk vectors before any capacity claim.
- Hosted multi-tenant browsers (isolation, anti-abuse, residential proxies, CAPTCHAs) are not built and are where hosted-browser companies spend most of their cost.

## 4. What are the unit economics of a business around this?

This repository is an open-source tool (Apache-2.0), not a business. The plausible shapes, with their honest weaknesses:

1. **Pull-through for Qdrant Cloud.** Each active user holds a small memory footprint and issues a few queries per form. Value to Qdrant is adoption and a reference workload, not direct revenue per user (the volume per user is small). *Weakness:* low revenue per seat.
2. **Hosted sessions** (managed browsers with memory). Compute is cheap (section 1), so gross margin is high before proxies, CAPTCHAs and abuse handling, which is where margin disappears. *Weakness:* crowded, and the hard costs are the ones we have not built.
3. **Enterprise** (audit log, policy, SSO, approval workflows, on-prem). *Weakness:* needs real-site reliability evidence first.

We do not put revenue numbers here because we have no basis for them.

## 5. What is the moat?

Honest answer: **not the language, not the browser.** agent-browser (Vercel) is already a native Rust CLI with MCP and ~45k stars; Rust is table stakes. What can compound:
- **Per-user memory** (playbooks, profile, known issues) is switching cost and quality that improves with use.
- **A form corpus**: fingerprints of forms and which strategies worked. Opt-in, anonymised cross-user sharing is a possible network effect, and a privacy risk that must be designed in from the start (not built).
- **Published, reproducible benchmarks** that others have to match.

What can erode it: competitors adding memory (it is not hard in principle), and model vendors shipping first-party browser agents that need no external tool.

## 6. What can go wrong?

| risk | status |
|---|---|
| Real-site success rate unknown (we test fixtures only) | **Biggest gap.** Next milestone: a real-site form eval with published results, including failures. |
| Computer-use models make DOM tooling less necessary | Real. Forms with consent/legal gates still want structured, verifiable filling; unproven. |
| Anti-bot, CAPTCHA, SSO/2FA, cross-origin iframes | Not handled (documented limitations). |
| Memory leaks PII or is poisoned by a malicious page | Mitigated: secrets refused at write, sensitive fields never auto-filled or stored, values not stored in playbooks, tenant isolation tested, page content marked untrusted. Not a security audit. |
| Qdrant Cloud Inference path untested live | The request shape follows Qdrant's docs and is pinned by tests, but no live cluster was available. Everything else is tested against a real Qdrant server. |
| Benchmark credibility | One machine, fixtures, illustrative prices. Raw data, harness, flags and caveats are published; Linux CI job adds a second platform. |

## 7. Test matrix (what is covered and how)

| aspect | what we test | where | status |
|---|---|---|---|
| **Correctness** | Browser e2e tests (`forms_e2e.rs`, `security_e2e.rs`; 56 and 10 `#[tokio::test]` functions when checked) assert on what the *server received*: dependent dropdowns, wizard, controlled inputs, masks, comboboxes, shadow DOM, iframes, dates/times/files, 20-field form, validation errors | `crates/ostler-browser/tests` | passing |
| **Form logic** | key->field matching (ambiguity reported, never guessed), option/date/time normalisation, sensitive-field detection, fingerprints | `ostler-forms` unit tests (14) | passing |
| **Protocol** | real binary over MCP stdio: initialize, tools, memory learning loop, confirm gate, domain policy, profiles | `crates/ostler-cli/tests` | passing |
| **Memory** | store semantics on a real Qdrant: roundtrip, isolation, upsert, dimension mismatch, 20 concurrent tenants; sealed payloads and tenant tokens on a real Qdrant (`sealed_real.rs`, `tenant_token_real.rs`); Cloud Inference request shape via mock | `ostler-memory` tests (11 + 3 real) | passing |
| **Cost** | tool calls and tokens per task, LLM cost model (unit-tested) | `ostler-bench forms`, `cost.rs` tests | measured |
| **Latency** | per-task tool time, cold start; regression found and fixed (3 s to 160 ms) | bench + e2e | measured |
| **Scale** | parallel sessions, memory per session, CPU per task, Qdrant at 200k points | `ostler-bench concurrency/memory/footprint` | measured (one machine) |
| **Context cost** | tool schema size capped by a unit test (under 7 KB) | `ostler-mcp` tests | enforced |
| **Security** | domain allowlist (suffix-attack cases), scheme blocking, confirm gate, secret refusal, untrusted-content marking | policy + MCP tests | passing; no external audit |
| **Not yet covered** | real-site success rate; Linux/Windows; cross-origin iframes; long-running leak soak; Cloud Inference live; fuzzing the JSON-RPC layer | roadmap | open |

## 8. What would change our minds

If the real-site evaluation shows success rates no better than existing tools, the form engine is not a differentiator and the project's value reduces to the memory integration. If Qdrant memory per user cannot be brought under a few tens of KB with quantization and on-disk storage, the "memory for everyone" story needs a smaller unit (store less, or share more).
