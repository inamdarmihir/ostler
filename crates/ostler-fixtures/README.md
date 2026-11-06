# ostler-fixtures

Local form fixtures and a recording HTTP server for Ostler tests and demos (internal crate).

Part of [Ostler](https://github.com/inamdarmihir/ostler), a browser for AI agents that fills forms, verifies every value, and learns from each fill. This crate is an implementation detail with no stability guarantee; install the `ostler` crate instead:

```sh
cargo install ostler
```

## Domain packs

`packs/<pack>/` holds synthetic page sets plus `tasks.json` for `research`, `gov-forms`, `jobs`, `dev-qa` and `commerce`, served at `/packs/<pack>/...`. The server records every request: `GET /__records` returns them in order (`ts`, `method`, `path`, `query`, `body`, `pack`), `POST /__reset` clears them, and `packs::check_outcome` judges a task from the records plus the agent's final answer. A second listener (`FixtureServer::port2`) provides a second origin for the commerce payment iframe. See each pack's README for what it covers and does not, and the schema in `src/packs.rs`.

Licensed under Apache-2.0.
