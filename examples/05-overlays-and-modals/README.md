# Get past a cookie wall or modal

**Use it for:** Consent banners, newsletter modals, anything covering the page.  
**Tools:** `look` `click`  
**Documented in:** [Cookbook recipe 4](https://github.com/inamdarmihir/ostler/blob/main/docs/COOKBOOK.md#4-get-past-a-cookie-wall-or-modal)

## Ask your agent

> Open http://127.0.0.1:8787/overlay.html, dismiss anything blocking the page, then tell me what the form asks for.

## Best practices shown here

- `look` counts the controls hidden under an overlay: close the overlay first instead of clicking through it.
- Use the ref from `look`'s legend; the click result shows the dialog leaving in its delta.
- Never accept a CAPTCHA or verification wall this way: Ostler reports those (`CHALLENGE:`) and hands them to a person.

## Run it

```sh
ostler fixtures &
python3 run.py
```

The script drives the same tools an agent would, with no model in the loop, and stops with `FAIL:` if any check does not hold.

## Real output

Recorded against Ostler built from `main` on 2026-11-08, with the bundled fixture pages; timestamps normalised.

```text

> navigate {'url': 'http://127.0.0.1:8787/overlay.html'}
Navigated. Now at: <page_content untrusted="true">Newsletter (modal)</page_content> — <page_content untrusted="true">http://127.0.0.1:8787/overlay.html</page_content>
<page_content untrusted="true">
heading "Newsletter" [e1] level=1
form [e2]:
  textbox "Email" [e3]
  button "Subscribe" [e4]
dialog:
  button "Accept all" [e5]
</page_content>
Only controls are listed; page text (article body, search snippets) is omitted. Call snapshot to read it.

> look 
1 clickable in view (1280x757); boxes on the image carry these refs. Green = submits, blue = input, purple = link, orange = other, red = has an error.
<page_content untrusted="true">
e5 button "Accept all" -> button
2 more are hidden under an overlay (dialog, banner, sticky bar); close it first.
</page_content>
CHECK OK: look reports controls hidden by the modal

> click {'ref': 'e5'}
Clicked e5. Now at: <page_content untrusted="true">Newsletter (modal)</page_content> — <page_content untrusted="true">http://127.0.0.1:8787/overlay.html</page_content>
Result: page updated in place
<page_content untrusted="true">
(delta: 4 unchanged lines omitted)
- dialog:
- text "We use cookies."
- button "Accept all" [e5]
</page_content>
CHECK OK: the click removed the dialog (shown in the delta)

> look 
2 clickable in view (1280x757); boxes on the image carry these refs. Green = submits, blue = input, purple = link, orange = other, red = has an error.
<page_content untrusted="true">
e3 textbox "Email" -> type here
e4 button "Subscribe" -> submits the form
</page_content>
CHECK OK: nothing is hidden any more
CHECK OK: the form behind the modal is reachable
```
