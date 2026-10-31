# ADR-0007: Measure cost, latency and footprint before claiming them

Status: accepted (2026-10-30)

## Context
"Fast" and "cheap" are the claims most likely to be asserted without evidence. The first benchmarks changed our own priorities.

## What measuring changed
- Found a 3 s-per-action latency bug (settle polling counted as activity). Fixed: 3,031 ms to 160 ms.
- Found snapshots dropped checkboxes/radios wrapped in `<label>`. Fixed with a regression test.
- Showed end-to-end time is dominated by LLM turns, not the browser, so we optimise call count and output size before engine speed.
- Showed summed RSS overstates Chrome's memory ~3.3x on macOS; we now report physical footprint (macOS) / PSS (Linux) and label the method.
- Showed the lean profile saves 37% memory at no latency cost.

## Decision
Benchmarks (`ostler-bench`) are part of the product: flags expose every assumption, results are committed under `bench-results/`, the docs state what is measured versus assumed, and a CI job reproduces them on Linux.
