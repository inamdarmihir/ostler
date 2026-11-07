# ADR-0019: Keep bulk data out of the context: result filters, `search_tools`, fact references and a sandboxed `run`

Status: accepted (2026-11-07)

## Context
Two write-ups describe the same cost from the MCP side: Anthropic's "Code execution with MCP" and Cloudflare's "Code Mode". Tool definitions loaded up front and every intermediate result passing through the model (sometimes twice) dominate the token bill of an agent doing real work. Their remedies: load tool schemas on demand, filter and aggregate in code, run loops and conditionals in code, keep sensitive values out of the model's context, and keep the code inside a sandbox with no ambient access.

Ostler already did part of this (ADR-0007 and the `auto` profile: 15 core tools, delta snapshots, `form_fill` taking intent once, playbooks that store structure and never values). What it lacked: a way to trim a large list before it is returned, a way to look up a tool's schema without listing every tool, a way to fill a field from a saved fact without the value passing through the agent, and a way to run a multi-page job without one model turn per step.

## Decision
1. **`where` / `select` on `extract` and `links`** (`filter.rs`). `where: "a|b"` keeps rows containing any term; `select: ["price", 0]` keeps table columns by header name or index. Filtering happens before the result is returned, only touches rows inside the untrusted fence, and states how many rows were kept.
2. **`search_tools(query, detail)`** returns names, one-line descriptions or full input schemas for any tool, listed or not. It lets a client that never refreshes its tool list still learn a schema on demand. Cost: one more core tool (about 350 bytes of schema).
3. **`{{fact:KEY}}` references** in `form_fill` values and `fill_field`. The agent names which saved fact goes in a field; Ostler reads the value and types it. The agent does not need to see, copy or retype it. An unknown key fails the whole call. This is input-side only: values already echoed back in a fill report are masked or shown exactly as before.
4. **`run(script)`**: a short Rhai script whose only way out is `tool(name, args)`. Rhai is embedded, with no file system, network, modules or `eval`; bounded operations, call depth, sizes (see `script.rs`), 60 tool calls and about 110 s of wall time. Each `tool()` call goes through the same entry point as a direct call, so profiles, the domain allowlist, the submit gate, robots.txt and per-tool deadlines still apply; `run` cannot call `run`. The script's printed lines and final value come back inside the untrusted fence, cut to `max_chars`. `run` is listed with the research tools (it is for multi-page jobs) and is off under `--tools core`/`forms`.

## Why Rhai
It is a pure-Rust interpreter that is sandboxed by construction (nothing is reachable unless registered), needs no C toolchain (important for the Windows and wheel builds), and keeps the binary small. The alternatives we weighed: a JavaScript engine (much larger, more attack surface), Lua through C bindings (a C build in every release target), and our own mini-language (more code to get wrong).

## Costs we accept, stated plainly
- **A new dependency with one policy exception.** Rhai depends on `smartstring`, which is MPL-2.0 and flagged unmaintained (RUSTSEC-2026-0249; no vulnerability). `deny.toml` and `.cargo/audit.toml` carry a crate-scoped exception, each with a reason and a removal condition (remove when rhai drops smartstring, or the interpreter is replaced).
- **A scripting surface.** A hostile page cannot write the script (the agent does), and anything a script reads from a page is data inside variables; only the final output is returned, fenced. But a script can make many calls quickly, hence the cap of 60 calls and the wall limit.
- **Default tool list grows by about 350 bytes** (`search_tools`), the full list by about 1 KB.

## Evidence and what is not yet measured
- On the small fixture page used in the end-to-end test, a scripted navigate + snapshot + grep returns 100 bytes against 274 bytes for the direct snapshot. The saving scales with page size and with the number of pages in the job; we have **not** yet benchmarked a real multi-page job against the tool-per-step baseline, and the cited 98.7% (Anthropic) and 99.9% (Cloudflare) figures are about their own much larger tool catalogues, not ours. Ostler's full schema is 32 tools (about 2.7k tokens), so the schema saving is modest; the gain here is fewer model turns and less intermediate data.
- Follow-up under ADR-0007: add a multi-page task to `ostler-bench` and compare turns, tokens and time with and without `run`.

## Not done
- PII tokenization with placeholders in tool *output* (`[EMAIL_1]`), as Anthropic describes. Masking of sensitive values in reports already exists; reversible placeholders would need a session-wide token table and are left for a later ADR.
- Saving scripts as reusable skills. Playbooks already do this for forms.
