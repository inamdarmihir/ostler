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
- A bug in settle tracking (the runtime's own timers counted as page activity) cost 3 s per action until a benchmark exposed it; settle timing is now guarded by the `actions_settle_quickly` test.
- Rejected: AX-tree-only (more round-trips, no shadow DOM), Playwright-style locators (needs a Node runtime and a selector layer the model must write).
