# ADR-0016: Mozilla Readability.js injected in the page for extraction; `htmd` in Rust for HTML to markdown

Status: proposed (M0 spike, 2026-11-06). Answers BUILD-PLAN section 4 rows "Article extraction" and "HTML to markdown"; feeds M3 (`ostler-extract`, `read`).

## Context
`read` must return clean, chunked, cited text. The plan chooses Mozilla Readability, injected through the existing runtime ([ADR-0004](0004-injected-runtime.md)), and "an existing Rust crate, chosen in M0" for markdown. The open choice is where the HTML to markdown step runs: in the page (`runtime.js`) or in Rust.

## Decision
1. **Extraction: Readability.js (`@mozilla/readability` 0.6.0, Apache-2.0), vendored and injected on demand** by `read`, run against a **clone** of the document (`document.cloneNode(true)`; Readability mutates the DOM it is given). The browser already holds a live DOM, so no `jsdom` or second parser is needed. Vendor one file (`Readability.js`, 89,980 bytes) with its licence header and a NOTICE entry; record the version and sha256 in the repo and add a CI check that the vendored file matches.
2. **Markdown: do it in Rust with `htmd`** (0.5.6, Apache-2.0), on the cleaned HTML that Readability returns. Reasons: conversion becomes a pure function of a string that `ostler-extract` can unit-test and fuzz without Chrome (the plan's layout), and `runtime.js` stays small (it is injected and parsed on every page).
3. Configure `htmd` to skip `script`, `style` and `noscript` (its default output kept script text in the spike).
4. Keep a **pure-Rust fallback path open** for pages we fetch without a browser (`open_many`, search results): evaluate `dom_smoothie` (a Rust port of Readability) in M3 against the same fixture pages and compare output to Readability.js before using it. Not evaluated here beyond metadata.

## Rejected alternatives
- *`html-to-markdown-rs`* (MIT): releases from 3.20.0 declare `rust_version` 1.99, above this machine's rustc 1.97 and the workspace's 1.89; 3.19.1 declares 1.88. Heavier and faster-moving than we need.
- *`html2md`*: **GPL-3.0+**, not allowed by `deny.toml` and not acceptable for an Apache-2.0 project.
- *`turndown` in `runtime.js`* (MIT, npm, 7.2.5): works (below) but puts conversion in a path we cannot fuzz without a browser and adds code to every injection.
- *A custom boilerplate scorer.* Rejected by the plan, and Readability is the reference implementation other readers are compared to.

## Consequences
- An extra ~90 KB script injected on `read` calls only; parse cost on a tiny document was 21 to 35 ms in Node with jsdom (not representative of Chrome on a real page; measure in M3).
- Readability is heuristic: it removes navigation and asides, and can drop content on non-article pages (tables, listings, dashboards). `extract` (tables, lists) must not depend on it, and `read` must say when Readability returned nothing (`isProbablyReaderable` helper, 4,271 bytes, is available).
- Output is deterministic for a given input and versions, so fixtures can assert markdown exactly.
- Text fragments and locators for `cite` are derived from the final markdown/text, not from Readability's DOM.

## Verification
Run 2026-11-06 with Node v22.23.4 and cargo 1.97.0.

| claim | how verified | result |
|---|---|---|
| licence, version, size, deps | npm registry `@mozilla/readability`; package `LICENSE.md` | 0.6.0 (2025-03-03), `Apache-2.0`, no dependencies, 154,574 bytes unpacked, `Readability.js` 89,980 bytes. Licence header in the package: "Licensed under the Apache License, Version 2.0" |
| it runs and does the job | `npm install @mozilla/readability@0.6.0 jsdom@31.0.0 turndown@7.2.5`, ran a script on a small article page | title found; nav ("Pricing") and aside ("newsletter") removed; table cell `A-100` and the link kept |
| `turndown` | npm registry; same script | 7.2.5, MIT, depends on `@mixmark-io/domino`; converted the extracted HTML to ATX markdown. Without the GFM plugin a table comes out as plain text |
| Rust options metadata | crates.io API | `htmd` 0.5.6 (2026-10-15) Apache-2.0, ~7.6M recent downloads; `html-to-markdown-rs` 3.21.0 MIT, `rust_version` 1.99; `html2md` 0.2.17 GPL-3.0+; `dom_smoothie` 0.19.0 MIT, `rust_version` 1.85 |
| `htmd` works | spike: `htmd::convert` on a page with heading, link, table, list and `<script>` | headings, links, pipe table and list rendered correctly; the script body (`x()`) leaked into the output; 38 crate versions; 20 s cold release build |
| licences | script over `cargo metadata` vs `deny.toml` | `htmd` tree (32 packages) within the allow list; Readability.js is not a Cargo dependency |

**Not verified (UNVERIFIED):** Readability.js inside Chrome through the runtime (no Chrome run here); quality on real article, table-heavy, paginated and paywalled pages; `dom_smoothie`'s fidelity; `htmd` options (`skip_tags` was read about, not run); whether the 90 KB injection is acceptable on slow pages; the vendoring CI check.

Open point: a parallel branch in this repository appears to add its own readability-style scorer to `runtime.js` (inferred from a commit command visible on the build machine; the code was not read). If so, reconcile before M3: run it and Readability.js over the same fixtures and keep one. The plan's "do not rebuild" rule favours Readability.
