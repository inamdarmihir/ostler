# ADR-0004: Drive pages with an injected runtime, not the CDP accessibility tree

Status: accepted (2026-10-30)

## Context
Agents need (a) a compact view of the page and (b) reliable ways to set field values. CDP offers `Accessibility.getFullAXTree` and per-node DOM commands; each fact costs a round-trip, and neither sees into shadow DOM or iframes without extra plumbing.

## Decision
Inject one script (`runtime.js`) into every document before page scripts run. It walks the composed tree (open shadow roots, slots, same-origin iframes), assigns stable element refs, produces snapshots and the form model, applies values with framework-safe native setters, and tracks in-flight `fetch`/XHR/short timers for settle detection. Rust calls it with `Runtime.evaluate` and keeps typing/clicking on real CDP `Input.*` events.

## Consequences
- One round-trip answers questions that would take dozens of CDP calls; the 20-field form needs ~143 CDP calls for fill + verify + submit.
- Shadow DOM and same-origin iframes work through one code path.
- Cost: our script shares a JS world with the page. It avoids `eval`, but a hostile page can tamper with it. Closed shadow roots and cross-origin frames are invisible.
- Isolated world, investigated (2026-11): `Page.addScriptToEvaluateOnNewDocument` with a `worldName` (or `Page.createIsolatedWorld`) plus `Runtime.evaluate` with that world's `contextId` would stop a page from replacing `__wm` or patching builtins the runtime uses. It was **not** adopted because the runtime does three things that only work in the page's own world: (1) settle detection wraps `window.fetch`, `XMLHttpRequest.prototype.send/open` and `setTimeout`/`clearTimeout` to count in-flight work, and wrappers installed in an isolated world are never called by page scripts; (2) `pendingDeep` and the iframe walk call `frame.contentWindow.__wm`, which another frame's isolated world cannot see; (3) Rust evaluates `__wm.el(ref)` (upload, `fill.rs`) and `__wm.<fn>(...)` from 50-odd `Runtime.evaluate` call sites without a `contextId`, and a navigation or popup creates a new context each time. A workable design is two scripts: a minimal main-world tracker exposing only `pending`/times (still tamperable, with only timing at stake), and the rest of the runtime in an isolated world, with the context id tracked per frame from `Runtime.executionContextCreated`, and settle moved next to the tracker (its `scope` argument would pass the element through a temporary attribute, since refs would live in the other world). That is a redesign of the settle path, which is the most heavily tuned part of the code, so it is left for its own change with the whole browser e2e suite as the gate.
- A bug in settle tracking (the runtime's own timers counted as page activity) cost 3 s per action until a benchmark exposed it; settle timing is now guarded by the `actions_settle_quickly` test.
- Rejected: AX-tree-only (more round-trips, no shadow DOM), Playwright-style locators (needs a Node runtime and a selector layer the model must write).
