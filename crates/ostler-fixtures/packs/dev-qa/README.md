# Pack: dev-qa

A tiny SPA-like app ("Ledgerly") with deliberate defects, for tasks where an agent verifies a flow and reports errors. Served at `/packs/dev-qa/...`; tasks in `tasks.json`.

**These are synthetic replicas of widget shapes, not real sites.** The app is a toy; it is not a model of any real product.

## Covers

Planted on `app.html` at load:

- a `console.error` ("Failed to hydrate widget: cart-badge element missing");
- an uncaught `TypeError` (reading `amount` of undefined) thrown from a timer;
- a failing `fetch` (`GET /packs/dev-qa/api/orders` answers `500`);
- a `404` script (`/packs/dev-qa/vendor/analytics.js` does not exist).

Flows (hash routes `#/home`, `#/settings`, `#/saved`):

- Saving the `dark` or `light` theme answers `200` and moves the URL to `#/saved`.
- Saving the `sepia` theme answers `500`, but the page shows an optimistic "Settings saved" toast first and never changes the URL. An agent that trusts the toast reports a false success (task 005). A correct check needs the URL change **and** the text, ideally also the server result.

## Does NOT cover

- Real frameworks, source maps, service workers, authentication, CI dashboards, or cloud consoles.
- Reading the browser console or network log: Ostler has no `diagnostics` tool yet, so the console and network tasks are tagged `needs-tool:diagnostics` and the scripted baseline skips them rather than guessing.

## Harness notes

`POST /packs/dev-qa/api/settings` is recorded with the chosen theme, so tests can see whether and how the agent saved.
