# ADR-0001: Pluggable CDP backend, Chrome by default

Status: accepted (2026-10-30)

## Context
We want speed and reliability. Building a new engine (Servo/V8) is a multi-year effort unrelated to our goals (forms, memory). Lightpanda is fast but has no rendering and is AGPL-3.0.

## Decision
Waymarker talks CDP through a `Backend` trait. Chrome is the default. Lightpanda is an optional, externally launched fast path with automatic fallback to Chrome when a page needs capabilities it lacks. We do not link or embed Lightpanda.

## Consequences
- Reliability comes from Chrome; speed claims apply only where a benchmark shows them.
- Fallback logic must be tested against the fixture suite.
- Alternatives rejected: Chrome-only (gives up the fast path), own engine (scope).
