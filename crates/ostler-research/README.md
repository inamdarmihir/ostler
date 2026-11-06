# ostler-research

Internal Ostler crate: the network-side building blocks for research tools. No browser dependency.

- `search`: `SearchProvider` trait, SearXNG and Brave Search API providers, URL normalisation and dedupe.
- `robots`: RFC 9309 parser/matcher, fetch policy, TTL cache, `RobotsClient`.
- `budget`: `Budget`, `BudgetTracker`, per-host `HostLimiter`.
- `ledger`: source ledger with optional JSONL persistence and gap reporting.
- `fetch_plan`: pure planner for multi-URL opens.

Providers were tested against local mock servers only; see the module docs for what is and is not verified against the real services.
