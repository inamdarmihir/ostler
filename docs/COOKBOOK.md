# Cookbook

Recipes for what you can do with Ostler **today**. Each one names the tools involved, a prompt to give your agent, the flags that make it safe, and what Ostler will and will not tell you. Back to the [README](../README.md); tool details are in [TOOLS.md](TOOLS.md).

Forms are the flagship because that is where Ostler verifies the most. They are not the only thing it does: the 14 core tools are a general browser (navigate, read, click, type, tabs, screenshots), and recipes 1 to 6 use no form tools at all.

Everything marked **planned** at the end is built as a library but is not an MCP tool yet. It is listed so you know what not to expect.

## Contents

| # | recipe | tools |
|---|---|---|
| 1 | [Read a page and answer a question](#1-read-a-page-and-answer-a-question) | `navigate` `snapshot` |
| 2 | [Compare several pages side by side](#2-compare-several-pages-side-by-side) | `tabs` `snapshot` `context` |
| 3 | [Check a web app after a change (QA)](#3-check-a-web-app-after-a-change-qa) | `navigate` `click` `wait_for` `look` `screenshot` |
| 4 | [Get past a cookie wall or modal](#4-get-past-a-cookie-wall-or-modal) | `look` `click` |
| 5 | [Log in once, reuse the session](#5-log-in-once-reuse-the-session) | `--profile-dir` `wait_for_human` |
| 6 | [Handle a CAPTCHA honestly](#6-handle-a-captcha-honestly) | `wait_for_human` `--headed` |
| 7 | [Fill a form and verify it](#7-fill-a-form-and-verify-it) | `form_inspect` `form_fill` `form_submit` |
| 8 | [Fill a multi-step wizard](#8-fill-a-multi-step-wizard) | `form_fill` `form_submit` |
| 9 | [Never fill the same form twice](#9-never-fill-the-same-form-twice) | `memory_remember` `--learn` |
| 10 | [Submit only with your approval](#10-submit-only-with-your-approval) | `--require-confirm-submit` |
| 11 | [Hand a form to another agent over A2A](#11-hand-a-form-to-another-agent-over-a2a) | `ostler a2a` |
| 12 | [Keep a run cheap](#12-keep-a-run-cheap) | `--tools` `stats` `snapshot` delta |

## 1. Read a page and answer a question

**Use it for:** documentation lookups, reading an article, checking what a page says.

**Prompt:** "Open https://example.com/docs/limits and tell me the rate limit for the free plan. Quote the sentence."

`navigate` returns the page state with interactive elements. `snapshot` returns the page as text with element refs. On a long page, pass the ref of a heading to read only that section, and use `delta` on later calls to see only what changed. Page text arrives wrapped as untrusted content, so instructions inside a page are data, not commands.

**Be aware:** Ostler gives your agent the text. It does not check that the agent's quote is on the page; that check (`cite`) is [planned](#planned-not-yet-mcp-tools).

## 2. Compare several pages side by side

**Use it for:** comparing plans, specs or release notes from three sources.

**Prompt:** "Open these three pricing pages in separate tabs. Write down each plan's price in the shared notes, then give me a table."

`tabs` with `new <url>` opens each page; all tabs share cookies and the `context` notepad, so the agent can store `plan_a=$12/mo` and read it back after switching tabs. Keep notes short: `context` is for facts, not page dumps. Popups open as new tabs under the same policy.

## 3. Check a web app after a change (QA)

**Use it for:** a quick end-to-end check of your own app on localhost or staging.

**Prompt:** "Start at http://localhost:3000, sign in with the test account, add an item to the cart, and tell me whether the cart badge says 1. Take a screenshot at the end."

`click`, `type`, `press` and `wait_for` drive the flow. `wait_for` waits for text to appear or disappear or for a selector, so the agent asserts on what the page shows instead of sleeping. `look` and `screenshot` give a visual record. Allow only your own origin: `ostler serve --allow-domain localhost`.

**Be aware:** there is no console-error or failed-request report over MCP yet (`diagnostics` is [planned](#planned-not-yet-mcp-tools)).

## 4. Get past a cookie wall or modal

**Prompt:** "Dismiss anything blocking the page, then continue."

`look` returns a screenshot with every clickable element boxed and numbered, plus a legend (`e6 button "Accept" → ...`). Elements hidden under a modal or banner are left out and counted, so the agent knows to close the overlay first rather than click something it cannot reach.

## 5. Log in once, reuse the session

**Use it for:** sites that need a login for every task.

```sh
ostler serve --headed --profile-dir ~/.ostler/profile
```

Log in yourself in the visible window (including any 2FA). Later runs with the same `--profile-dir` start already signed in. Ostler does not store your password and never types a credential you did not supply.

## 6. Handle a CAPTCHA honestly

When a CAPTCHA or verification wall blocks the page, Ostler reports it (a `CHALLENGE:` line) and does not try to solve or disguise itself ([ADR-0009](adr/0009-challenges-are-handed-off.md)). With `--headed`, `wait_for_human` pauses until you complete it in the browser window, then the agent continues. Over A2A the task moves to `input-required`.

## 7. Fill a form and verify it

**Prompt:** "Open http://127.0.0.1:8787/dependent.html (from `ostler fixtures`) and fill the shipping form for Ada Lovelace in Germany, Bavaria."

```text
> form_inspect                      typed fields: kind, label, required, options, errors
> form_fill {"Country": "Germany", "State": "Bavaria", "Recipient": "Ada Lovelace"}
Filled 3/3 fields (form e2).
  OK e4 "Country" = "de"
  OK e5 "State / region" = "by"     <- options loaded after Country changed
> form_submit                       Submitted | Advanced | NOT ACCEPTED + the page's messages
```

Every value is read back from the live page. A key that matches nothing is `UNMATCHED`, a tie is `ambiguous`, and a rejected submit returns the page's own words. Treat `UNMATCHED` and `NOT ACCEPTED` as the signal to stop and ask, not to retry blindly.

## 8. Fill a multi-step wizard

Call `form_fill` then `form_submit` per step. A result of `Advanced` means the page moved to the next step; run `form_inspect` again to get its fields. Conditional sections that appear after an answer are picked up by the next inspect.

## 9. Never fill the same form twice

```sh
ostler serve --learn
```

Save facts once (`memory_remember email ada@example.com`) or let `--learn` save non-sensitive values after an accepted submit. Memory defaults to `--memory auto` (Qdrant if reachable, else a local file). With `auto`, `form_fill` fills the remaining required fields from memory, and `form_inspect` tells the agent what worked on this site before. Memory lives in Qdrant or an encrypted local file, is scoped per `--tenant`, and secrets are refused. See [MEMORY.md](MEMORY.md).

## 10. Submit only with your approval

```sh
ostler serve --require-confirm-submit
```

The agent can fill and verify, but `form_submit` is held until you confirm. Use it for anything with consequences: applications, orders, messages. Pair it with `--allow-domain` so the agent can only reach the sites you name.

## 11. Hand a form to another agent over A2A

```sh
ostler a2a --allow-domain example.com
```

Another agent sends a task over A2A v1.0 and gets `input-required` back when a field is missing or a submit needs confirmation. See [A2A.md](A2A.md).

## 12. Keep a run cheap

- `--tools auto` (default) shows 14 core tools and adds the form and memory tools once a page has fields. `--tools core` keeps the schema smallest.
- Use `snapshot` with `delta` and heading refs rather than re-reading whole pages.
- Call `stats` to see calls, time and estimated tokens for the session.
- Form recipes beat per-field clicking: 3 calls versus 22 on the 20-field fixture, measured on a scripted agent ([BENCHMARKS.md](BENCHMARKS.md)).

## Planned, not yet MCP tools

These exist as libraries in the workspace (`ostler-extract`, `ostler-workflow`, `ostler-research`, the memory `source` kind) and are covered by tests and a fixture pack, but **no MCP tool exposes them yet**. Do not write prompts that depend on them.

| recipe | needs | status |
|---|---|---|
| Search the web, then read the top results | `search`, `open_many` | providers verified against mocks only |
| Pull a table into rows and know if it is complete | `extract` | library only |
| Cite a claim and verify the quote is on the page | `cite` | library only |
| Assert a postcondition after a click ("the page now shows step 2") | `expect` | library only |
| See console errors and failed requests | `diagnostics` | per-tab capture built, no tool |
| Remember what you read across sessions | source memory | library only |

Tracking: [BUILD-PLAN.md](BUILD-PLAN.md).
