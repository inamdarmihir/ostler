# Forms and verification

Ostler's form engine is built so the model states intent once and the engine does the mechanics: it resolves each key to a field, fills in order, lets dependent controls appear, reads every value back, and reports what actually happened.

## The three-call flow

1. **`form_inspect`** returns typed fields: kind, label, required, options, pattern, current value, and any error. It also lists submit controls and says what memory knows about this form.
2. **`form_fill`** takes `{label | name | ref: value}` for the whole form.
3. **`form_submit`** clicks the submit or next control and reports the outcome.

Dependent dropdowns, multi-step wizards, input masks, comboboxes, shadow DOM and same-origin iframes are handled inside these calls. See the [tools reference](tools.html#forms-profile-forms) for parameters.

## How a key becomes a field

Keys are resolved in this order: exact ref, then name or id, then label, then fuzzy label. A key can match by ref, name, label or fuzzy label, and keys are resolved against every visible field once, so an ambiguous key cannot win by elimination.

- **Ambiguity is reported, never guessed.** If two fields match equally well, the report says `ambiguous` and asks for the ref. `email` finds "E-mail address", because a key equal to a run of the label's words written together matches, but two equally plausible fields are still refused.
- **A key that matches nothing is `UNMATCHED`.** It is not dropped silently, and the fill report only counts as OK if no key was unmatched.
- **Forms re-mounted by a framework are followed.**

## How a value is normalised

The engine converts the value for the field before writing it:

| you send | field | written |
|---|---|---|
| `"Germany"` | select | the option index for that option text |
| `"March 5, 2001"` | date | `2001-03-05` |
| `"3:30 PM"` | time | `15:30` |
| `03/04/2024` | date | **refused as ambiguous** (day-first or month-first), so the agent must say which |

Fields are filled in document order and the form is **re-inspected after every field**, so a dependent control (State after Country) appears before it is needed.

## Verification: read it back

After writing a field, the engine reads the value back from the live page and compares it by field type.

- A value the page rewrote or cleared fails. A `datetime-local` must read back as the requested instant.
- A combobox must show the option you asked for. A longer option that merely contains it is a mismatch ("United States" versus "United States Minor Outlying Islands"), and the in-page picker only picks an exact, normalised option label, listing the options otherwise.
- Values the page legitimately reformats, such as `(555) 123-4567`, are reported as **reformatted**, not as errors. Only exact matches and genuine reformatting pass.
- `form_fill` reports `unsettled` (a count) when the page was still busy at the end of the fill, so the verdicts can be treated with care.

Typed input is tried first, because it fires the same events a user would. If the page rejects it (masked or framework-controlled inputs), the engine retries with the native value setter plus events. Fills also send one harmless `keyup`, because some date pickers learn the text only from that event.

### Verified, then submitted empty

Just before clicking submit, `form_submit` re-reads the fields it verified and reports any the page cleared or rewrote in between as `CHANGED BEFORE SUBMIT`. This was added after a date picker was reported verified and then submitted empty.

## Submit results

| result | meaning |
|---|---|
| `Submitted` | the page accepted it: a new URL, or new confirmation text appeared after the click (for example a form that posts in the background and says "Thanks!" in place) |
| `Advanced` | the page moved to the next step of a wizard; repeat inspect, fill, submit |
| `NOT ACCEPTED` | the page reported problems, and the page's own messages follow |
| `SERVER ERROR` | the site answered the submit with an HTTP error (4xx/5xx) and showed no validation messages. The data may or may not have been stored: do not report it as submitted and do not retry blindly; check with `snapshot` or ask the user |
| `Nothing changed` | the form stayed put and said nothing visible. Check with `snapshot`; do not resubmit blindly, because it may have gone through |

With several forms on a page, `form_submit` with no `form` or `control` submits the form `form_fill` last filled; if none was filled it refuses and lists the forms rather than guessing the first.

A thank-you page that still has an optional input (a newsletter box, a search) counts as `Submitted`, not `Advanced`, unless something on it is required, it has a step counter, or it has a "next" button.

## Errors you can only see

Fields marked invalid only by colour (a red border, outline or glow, or red message text with no ARIA or class) and floating toasts or banners are reported as field and page errors. An error that was already on a field before the fill is flagged as possibly stale. An error message belongs to the field it follows, not to every field sharing a wrapper, and Bootstrap 3 `.help-block` is help text unless the group is `.has-error`.

## Pages without a `<form>`

Only real action buttons are submit candidates. The header's "Search" button, anything inside `header`, `nav` or `role=search`, and type-less buttons whose label says nothing about submitting ("Show tips") do not count.

## Dates and pickers

Calendar date pickers on read-only inputs are handled heuristically: when a text field is read-only and the value is a date, the engine opens the popup, navigates months and clicks the day. It fails with a reason, not silently, when no popup or navigation is found. The heuristic looks for a month header, previous and next controls and day cells. Dropdown-only and year-only pickers are not handled.

## Seeing the page

Two tools help when a form is not obvious:

- **`look`** returns a screenshot with every clickable element boxed and numbered, and a legend such as `e6 button "Send" -> submits the form`.
- **`snapshot`** with a heading's ref reads just that section.

A CAPTCHA, hCaptcha, Turnstile or "verify you are human" wall is reported with a `CHALLENGE:` line at the top of the result. The agent should stop retrying and ask you; with `--headed`, `wait_for_human` waits while you complete it. Ostler never solves or bypasses these ([ADR-0009](repo:docs/adr/0009-challenges-are-handed-off.md)).

## What it does not handle

- Cross-origin iframes and closed shadow roots are invisible to the injected runtime.
- 2FA, CAPTCHA solving and anti-bot evasion are out of scope.
- Fill on very large forms (thousands of fields) is slow, because every key re-reads the form.
- It has been evaluated on local fixtures, not on the open web. See [Benchmarks](benchmarks.html).

## Design records

The reasoning, with the alternatives that were rejected, is in [ADR-0004](repo:docs/adr/0004-injected-runtime.md) (injected runtime) and [ADR-0005](repo:docs/adr/0005-form-ir-and-verification.md) (form model and verification). A summary is on the [Architecture](architecture.html) page.
