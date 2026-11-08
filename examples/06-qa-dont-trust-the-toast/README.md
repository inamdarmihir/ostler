# QA a web app: assert the outcome, not the toast

**Use it for:** Checking your own app on localhost or staging after a change.  
**Tools:** `navigate` `form_fill` `click` `wait_for`  
**Documented in:** [Cookbook recipe 3](https://github.com/inamdarmihir/ostler/blob/main/docs/COOKBOOK.md#3-check-a-web-app-after-a-change-qa)

## Ask your agent

> Open http://127.0.0.1:8787/packs/dev-qa/app.html, go to Settings, choose sepia and click Save settings. Tell me whether the settings were really saved: judge by the URL and the page that follows, not by the toast.

## Best practices shown here

- Assert on something only success produces (here the `#/saved` view), with a bounded `wait_for`, never a sleep.
- The app shows "Settings saved" before the server answers; this example catches the sepia save failing anyway. That is a false success, the thing Ostler is built to expose.
- Refs differ between runs of the same page (`e6` in one, `e7` in another), which is why the example looks them up.
- Allow only your own origin: `ostler serve --allow-domain localhost`.

## Run it

```sh
ostler fixtures &
python3 run.py
```

The script drives the same tools an agent would, with no model in the loop, and stops with `FAIL:` if any check does not hold.

## Real output

Recorded against Ostler built from `main` on 2026-11-08, with the bundled fixture pages; timestamps normalised.

```text

> navigate {'url': 'http://127.0.0.1:8787/packs/dev-qa/app.html'}
Navigated. Now at: <page_content untrusted="true">Ledgerly</page_content> — <page_content untrusted="true">http://127.0.0.1:8787/packs/dev-qa/app.html</page_content>
<page_content untrusted="true">
heading "Ledgerly" [e1] level=1
navigation:
  link "Home" [e2] href="#/home"
  link "Settings" [e3] href="#/settings"
main:
  heading "Home" [e4] level=2
</page_content>
Only controls are listed; page text (article body, search snippets) is omitted. Call snapshot to read it.
CHECK OK: the app loaded

> navigate {'url': 'http://127.0.0.1:8787/packs/dev-qa/app.html#/settings'}
Navigated. Now at: <page_content untrusted="true">Ledgerly</page_content> — <page_content untrusted="true">http://127.0.0.1:8787/packs/dev-qa/app.html#/settings</page_content>
<page_content untrusted="true">
heading "Ledgerly" [e1] level=1
navigation:
  link "Home" [e2] href="#/home"
  link "Settings" [e3] href="#/settings"
main:
  heading "Settings" [e5] level=2
  combobox "Theme" [e6] value="light" options=["light","dark","sepia"]
  button "Save settings" [e7]
</page_content>
Only controls are listed; page text (article body, search snippets) is omitted. Call snapshot to read it.

> form_fill {'values': {'Theme': 'dark'}}
Filled 1/1.
<page_content untrusted="true">
verified: "Theme"
</page_content>
Ready: form_submit.

> click {'ref': 'e7'}
Clicked e7. Now at: <page_content untrusted="true">Ledgerly</page_content> — <page_content untrusted="true">http://127.0.0.1:8787/packs/dev-qa/app.html#/saved</page_content>
Result: navigated to http://127.0.0.1:8787/packs/dev-qa/app.html#/saved
<page_content untrusted="true">
(delta: 5 unchanged lines omitted)
+ heading "Saved" [e8] level=2
+ text "Settings saved. Your theme preference is now active."
- heading "Settings" [e5] level=2
- combobox "Theme" [e6] value="light" options=["light","dark","sepia"]
- button "Save settings" [e7]
</page_content>

> wait_for {'text': 'Your theme preference is now active', 'timeout_ms': 3000}
Condition met.
CHECK OK: saving the dark theme really saved

> navigate {'url': 'http://127.0.0.1:8787/packs/dev-qa/app.html#/settings'}
Navigated. Now at: <page_content untrusted="true">Ledgerly</page_content> — <page_content untrusted="true">http://127.0.0.1:8787/packs/dev-qa/app.html#/settings</page_content>
<page_content untrusted="true">
heading "Ledgerly" [e1] level=1
navigation:
  link "Home" [e2] href="#/home"
  link "Settings" [e3] href="#/settings"
main:
  heading "Settings" [e9] level=2
  combobox "Theme" [e10] value="light" options=["light","dark","sepia"]
  button "Save settings" [e11]
</page_content>
Only controls are listed; page text (article body, search snippets) is omitted. Call snapshot to read it.

> form_fill {'values': {'Theme': 'sepia'}}
Filled 1/1.
<page_content untrusted="true">
verified: "Theme"
</page_content>
Ready: form_submit.

> click {'ref': 'e11'}
Clicked e11. Now at: <page_content untrusted="true">Ledgerly</page_content> — <page_content untrusted="true">http://127.0.0.1:8787/packs/dev-qa/app.html#/settings</page_content>
Result: page updated in place
<page_content untrusted="true">
(delta: 7 unchanged lines omitted)
+ combobox "Theme" [e10] value="sepia" options=["light","dark","sepia"]
+ status:
+ text "Settings saved"
- combobox "Theme" [e10] value="light" options=["light","dark","sepia"]
</page_content>

> wait_for {'text': 'Your theme preference is now active', 'timeout_ms': 3000}
ERROR Timed out after 3000ms.
CHECK OK: the sepia save is caught as a failure despite the toast
```
