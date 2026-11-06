# Tools reference

Ostler exposes 30 MCP tools in five profiles. `--tools auto|core|forms|research|all` (or `OSTLER_TOOLS`) controls how many the agent sees. The default, `auto`, starts with the 15 core tools (4.9 KB of schema), adds the research tools after the first `read` or `search`, and adds the form and memory tools once a page shows form fields. The server sends `tools/list_changed`, the result names the new tools, and every tool stays callable even if a client never refreshes its list. The full set is about 9.7 KB of schema, roughly 2.4k tokens on every turn, and a test keeps it under 10 KB. Tool output is terse by default (verified fields are named once, no timings or fingerprints); `--verbose` brings the debug detail back.

| profile | tools | count |
|---|---|---:|
| `auto` (default) | core, growing to research, forms and memory as the session needs them | 15 to 30 |
| `core` | browse, `read`, tabs and context tools | 15 |
| `forms` | core plus the form tools | 19 |
| `research` | core plus the research tools | 21 |
| `all` | everything, including memory and `stats` | 30 |

Page-derived text in results (page content, titles, URLs, dialog text, the `look` legend) is wrapped in `<page_content untrusted="true">`. The server instructions tell the model not to follow it. See [Security model](security.html).

## Browse (profile: core)

### `navigate`

Open a URL. Returns page state with interactive elements, so no separate snapshot call is needed.

| parameter | type | notes |
|---|---|---|
| `url` | string, **required** | |
| `observe` | `interactive` \| `full` \| `none` | default `interactive` |

It reports the HTTP status (`Navigated (HTTP 404)`) with a plain-language reason, labels PDFs, downloads and redirects to a sign-in page, and explains navigation errors. A CAPTCHA or "verify you are human" page produces a `CHALLENGE:` line at the top. On a search-results URL whose visible text matches fewer than about half of the query's words, it adds a `Warning: results do not seem to match the query`.

### `snapshot`

Read the page as text with element refs (`e1`, `e2`, …). Use delta to see only changes. Pass the ref of a heading to read just that section.

| parameter | type | notes |
|---|---|---|
| `mode` | `full` \| `delta` \| `interactive` | |
| `ref` | string | limit to a subtree |
| `max_chars` | integer | |

Long text is split into several lines with an explicit `… [N more chars cut]` marker. Text a person cannot see is left out and counted.

### `read`

Read a page for research (opens `url` if given). With `query`: only the best passages, ranked locally with BM25 and MMR, so a page costs a few hundred tokens instead of the whole text. Without: the start of the page. Only pages read this session can be cited.

| parameter | type | notes |
|---|---|---|
| `url` | string | opens it first; omit to read the current tab |
| `query` | string | what you are looking for |
| `k` | integer | passages, default 5, max 12 |
| `max_chars` | integer | default 8000 |

The header names the page, its size, fetch time and content hash, and flags what a reader must know first: `PAYWALL`, `CHANGED` (the page differs from an earlier read; earlier citations from it are re-checked), the page's own paging text (`showing 1-5 of 13`), and `ATTENTION` when the page contains text aimed at an AI. A bot wall, robots.txt refusal, HTTP error or empty page is `NOT READ` and is recorded as a gap, never as a source.

### `click`

Click an element by ref. The result ends with a `Result:` line: navigated, popup opened as tab `tN`, page updated, or no visible change.

| parameter | type | notes |
|---|---|---|
| `ref` | string, **required** | |
| `observe` | `delta` \| `full` \| `none` | default `delta` |

### `hover`

Move the pointer over an element by ref (opens menus and tooltips).

| parameter | type | notes |
|---|---|---|
| `ref` | string, **required** | |
| `observe` | `delta` \| `full` \| `none` | default `delta` |

### `scroll`

Scroll an element into view (`ref`) or the page (`direction`). Reports the position as scrolled/max, so the agent knows when it has reached the end of an infinite list.

| parameter | type | notes |
|---|---|---|
| `ref` | string | element to bring into view |
| `direction` | `up` \| `down` \| `top` \| `bottom` | |
| `amount` | integer | pixels |
| `observe` | `delta` \| `full` \| `none` | default `delta` |

### `type`

Type into an element by ref, replacing its content.

| parameter | type | notes |
|---|---|---|
| `ref` | string, **required** | |
| `text` | string, **required** | |
| `submit` | boolean | press Enter after |
| `observe` | `delta` \| `full` \| `none` | default `delta` |

### `press`

Press a key, for example `Enter`, `Tab`, `Escape`, `Control+a`.

| parameter | type | notes |
|---|---|---|
| `key` | string, **required** | |
| `observe` | `delta` \| `full` \| `none` | default `delta` |

### `wait_for`

Wait until text appears or disappears, or a CSS selector shows, or just pause for `ms` milliseconds (max 10000).

| parameter | type | notes |
|---|---|---|
| `text` | string | wait for it to appear |
| `text_gone` | string | wait for it to disappear |
| `selector` | string | CSS selector |
| `timeout_ms` | integer | |
| `ms` | integer | plain pause |

### `wait_for_human`

A CAPTCHA or verification wall is blocking the page: wait for the user to complete it in the browser window. Needs `--headed`. Ostler never solves these itself.

| parameter | type | notes |
|---|---|---|
| `timeout_s` | integer | default 120, max 600 |

### `screenshot`

Capture a PNG of the page or one element.

| parameter | type | notes |
|---|---|---|
| `ref` | string | one element |
| `full_page` | boolean | |

### `look`

See where to click: a screenshot with every clickable element boxed and numbered by its ref, plus a legend saying what each does (type here, submits the form, error, …). Use the refs with `click`, `type` and `fill_field`.

| parameter | type | notes |
|---|---|---|
| `max` | integer | |

Boxes come from DOM geometry and are coloured by role: green submits, blue inputs, purple links, red has an error. Elements hidden under a cookie banner, modal or sticky bar are left out and counted, so the agent closes the overlay first. It covers only the visible viewport. The overlay is removed afterwards. With `--lean`, screenshots omit images.

### `back`

Go back in history. Takes no parameters. At the start of a tab's history it says so instead of landing on `about:blank`.

## Tabs and shared notes (profile: core)

### `tabs`

Manage tabs. They share cookies and the `context` tool. Popups open as new tabs.

| parameter | type | notes |
|---|---|---|
| `action` | `list` \| `new` \| `switch` \| `close`, **required** | |
| `tab` | string | tab id, for example `t2` |
| `url` | string | for `new` |

### `context`

Notes shared by every tab and later calls: `get`, `set`, `delete`, `list`. Keep short facts, not page dumps. Survives a browser restart.

| parameter | type | notes |
|---|---|---|
| `action` | `get` \| `set` \| `delete` \| `list`, **required** | |
| `key` | string | |
| `value` | string | |

## Research (profile: research)

Built around one contract: **an answer can only contain what was read and verified.** See the [Cookbook](cookbook.html#13-research-with-citations-that-are-checked) for the workflow.

### `cite`

Check a quote against a page read this session. `VERIFIED` returns the page's exact words and a `#:~:text=` link a person can open to check them. `NOT READ` (the page was never read), `NOT VERIFIED` (with the closest text on the page) and `TOO SHORT` are errors: do not use that claim. Markdown on either side is ignored, so `**1887**` and `[Marta Ilves](/m)` still verify.

| parameter | type | notes |
|---|---|---|
| `url` | string, **required** | a page read with `read` or `open_many` |
| `quote` | string, **required** | words from the page, 12+ characters; `...` marks an omission |

### `evidence`

What the agent has read and may state: verified citations, citations made stale by a changed page, pages read but not cited, and the gaps (paywall, bot wall, robots.txt, HTTP error, empty page). Call it before answering. Takes no parameters.

### `search`

Web search through SearXNG (`OSTLER_SEARXNG_URL`) or Brave (`OSTLER_SEARCH_PROVIDER=brave` and `OSTLER_BRAVE_API_KEY`). Returns titles, URLs and snippets; a snippet is not evidence until the page is read.

| parameter | type | notes |
|---|---|---|
| `query` | string, **required** | |
| `limit` | integer | default 8, max 20 |
| `recency` | `day` \| `week` \| `month` \| `year` | |
| `site` | string | host name, for example `example.com` |

### `open_many`

Read up to 10 URLs in a scratch tab; the agent's current tab is untouched. The domain policy is checked before robots.txt is fetched, robots.txt (RFC 9309) is obeyed, requests to one host are spaced, and the call has a time budget. Each page's outcome is reported.

| parameter | type | notes |
|---|---|---|
| `urls` | string array, **required** | |
| `query` | string | best passages from each page |
| `k` | integer | passages per page, default 2, max 5 |
| `force` | boolean | re-read pages already read |

### `extract`

Structured data from the current page: `tables` as rows, or `lists` of repeated cards with pagination hints and a completeness verdict (`PARTIAL: 5 of 13` when the page states a total it does not show).

| parameter | type | notes |
|---|---|---|
| `kind` | `tables` \| `lists`, **required** | |
| `max_rows` | integer | per table or list, default 30 |

### `links`

Links on the current page with anchor text and context, optionally filtered.

| parameter | type | notes |
|---|---|---|
| `same_origin` | boolean | |
| `text_contains` | string | |
| `href_contains` | string | |
| `limit` | integer | default 30 |

## Forms (profile: forms)

### `form_inspect`

List the page's forms as typed fields (label, kind, required, options, errors) plus submit controls and what memory knows about this form.

| parameter | type | notes |
|---|---|---|
| `form` | string | form ref |

### `form_fill`

Fill many fields at once from `{label | name | ref: value}`. Handles selects, radios, checkbox groups, dates, files, comboboxes and dependent dropdowns, and verifies each field by reading it back. `auto: "required"` or `"all"` fills the rest from memory.

| parameter | type | notes |
|---|---|---|
| `values` | object | `{label \| name \| ref: value}` |
| `form` | string | form ref |
| `auto` | `off` \| `required` \| `all` | fill the rest from memory |
| `min_confidence` | number | |

The report names each field `OK`, or says `UNMATCHED` (no field matches the key), `ambiguous` (several match equally well, so use the ref), or a read-back mismatch. It lists fields still empty and required, and says `unsettled` (a count) when the page was still busy at the end of the fill. File fields are refused unless `--upload-dir` allows the directory.

### `form_submit`

Click the form's submit or next control and report whether it was accepted, advanced to another step, or failed validation.

| parameter | type | notes |
|---|---|---|
| `form` | string | |
| `control` | string | submit button ref |
| `confirm` | boolean | required when the server runs with `--require-confirm-submit` |

Result: `Submitted`, `Advanced` (next step), `NOT ACCEPTED` with the page's own messages, or `Nothing changed` when the form stays put and says nothing. Just before clicking it re-reads the fields it verified and reports any the page cleared or rewrote (`CHANGED BEFORE SUBMIT`).

### `fill_field`

Set one field by ref (any kind, including custom widgets).

| parameter | type | notes |
|---|---|---|
| `ref` | string, **required** | |
| `value` | any, **required** | |

## Memory (profile: all)

These need memory enabled (`--memory local`, a Qdrant URL, or `--memory ephemeral`). See [Memory and privacy](memory.html).

### `memory_remember`

Save a fact about the user for future forms (for example email, company). Secrets are refused.

| parameter | type | notes |
|---|---|---|
| `key` | string, **required** | |
| `value` | any, **required** | |
| `description` | string | |
| `autocomplete` | string | HTML autocomplete token, for example `given-name` |

### `memory_recall`

Semantic search over saved facts.

| parameter | type | notes |
|---|---|---|
| `query` | string, **required** | |
| `limit` | integer | |

### `memory_list`

List all saved facts. Takes no parameters.

### `memory_forget`

Delete a saved fact by key.

| parameter | type | notes |
|---|---|---|
| `key` | string, **required** | |

### `stats`

Session cost and latency counters: calls, milliseconds, estimated tokens, browser memory. Takes no parameters.

## Design notes

- **Observation is part of the action.** `navigate` returns the interactive elements; clicks and typing return only what changed. `observe: none` opts out but still records a baseline.
- **Actions are never replayed after a navigation.** Calls that change the page report that the page changed instead of re-running.
- **A crashed Chrome is relaunched** and the agent is told. Open tabs and page state are lost.
