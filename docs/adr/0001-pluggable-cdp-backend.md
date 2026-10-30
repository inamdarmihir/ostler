# ADR-0001: Pluggable CDP backend, Chrome by default

Status: direction accepted; **not implemented in v0.1** (Chrome only, no `Backend` trait yet). Updated 2026-10-30.

## Context
We want speed and reliability. Building a new engine (Servo/V8) is a multi-year effort unrelated to our goals (forms, memory). Lightpanda is fast but has no rendering and is AGPL-3.0.

## Decision
Waymarker talks CDP through a `Backend` trait. Chrome is the default. Lightpanda is an optional, externally launched fast path with automatic fallback to Chrome when a page needs capabilities it lacks. We do not link or embed Lightpanda.

## Consequences
- Reliability comes from Chrome; speed claims apply only where a benchmark shows them.
- Fallback logic must be tested against the fixture suite.
- Alternatives rejected: Chrome-only (gives up the fast path), own engine (scope).

## Update after the first benchmarks
Measured end-to-end time is dominated by LLM turns (tool time is 0.4-1.1 s of a 12-25 s task), so a faster engine barely moves task latency. Lightpanda would still help density (memory per session) and cold start, but it is lower priority than real-site reliability. The backend boundary should be introduced when there is a second backend worth maintaining.
