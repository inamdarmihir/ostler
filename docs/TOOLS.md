# Tools

Every MCP tool Ostler exposes, and example prompts. Back to the [README](../README.md).

## Tools

| group | tools |
|---|---|
| browse | `navigate` `snapshot` `click` `type` `press` `wait_for` `wait_for_human` `look` `screenshot` `back` |
| tabs | `tabs` (list, new, switch, close; popups arrive as tabs) · `context` (notes shared by every tab) |
| forms | `form_inspect` `form_fill` `form_submit` `fill_field` |
| memory | `memory_remember` `memory_recall` `memory_list` `memory_forget` |
| other | `stats` |

`--tools auto|core|forms|all` controls how many the agent sees. The default, `auto`, lists the 14 core tools (~4.1 KB) and adds forms and memory (21 in total) once a page shows form fields: the server sends `tools/list_changed`, the page result names the new tools, and every tool stays callable even if a client never refreshes its list. The full set (21 tools) is about 5.9 KB of schema, roughly 1.5k tokens on every turn; a test keeps it under 7 KB. Tool output is terse by default (verified fields are named once, no timings or fingerprints); `--verbose` brings the debug detail back.

**Tabs share one browser context**, so cookies and logins carry across them, and the `context` tool is a small key/value notepad every tab (and every later call) can read, for things like "applicant = Ada, order id = 4471". It survives a browser restart.

## Things to ask your agent


- "Fill in the contact form on this page with my details." (after `memory_remember` of your name and email, or with `--learn`)
- "Sign up for the newsletter on example.com, but ask me before you submit."  (`--require-confirm-submit`)
- "This form has a date picker and a country dropdown: fill it and tell me anything you couldn't verify."
- "What's blocking this page?"  (`look` shows numbered click targets; a CAPTCHA is reported, not bypassed)

## What the agent sees

Real output from the bundled fixtures (`ostler serve --tools forms`), lightly reformatted. *State* is disabled until *Country* is chosen.

```text
> form_inspect
form [e2] fingerprint=07efd5b3061187a1
  e3 text    "Recipient"      [required]
  e4 select  "Country"        [required] options: Choose… | United States | Germany | India
  e5 select  "State / region" [required, disabled] options: (Choose a country first)
  e6 text    "City"
  e7 submit  "Ship it"

> form_fill {"Country": "Germany", "State": "Bavaria", "Email": "ada@example.com"}
Filled 2/2 fields in 386ms (form e2).
  OK e4 "Country" = "de"
  OK e5 "State / region" = "by"        <- options loaded after Country changed
  UNMATCHED key "Email": no field on the page matches this key
Still empty and required:
  e3 "Recipient"

> form_submit
NOT ACCEPTED: the page reported problems.
  e3 "Recipient": "Please fill in this field."
```

A key that matches nothing is reported, not guessed. A rejected submit says why, in the page's words.

Verification is strict by design: a combobox must show the option you asked for (not a longer one that merely contains it), a `datetime-local` must read back as the requested instant, and `form_fill` reports `unsettled` when the page was still busy at the end of the fill so the verdicts can be treated with care.

## At a glance

| | |
|---|---|
| **One call per form** | `form_inspect`, `form_fill`, `form_submit`: typed fields in, intent out, instead of a click and a snapshot per field |
| **Verified, not assumed** | every value is read back from the live page and compared by field type; a page that rewrites a date or picks a different combobox option is reported as a failure |
| **Honest about gaps** | keys that match nothing are `UNMATCHED`, ties are `ambiguous`, a rejected submit returns the page's own messages |
| **Sees like a user** | `look` returns a screenshot with numbered click targets; red borders and toasts count as errors |
| **Remembers** | profile facts, per-site playbooks and episodes in Qdrant, scoped per tenant |
| **Two front doors** | MCP over stdio for coding agents, [A2A](A2A.md) over HTTP for other agents |
