# Fill a form and verify every value

**Use it for:** Sign-ups, shipping forms, any form with dependent dropdowns or validation.  
**Tools:** `form_fill` `form_submit`  
**Documented in:** [Cookbook recipe 7](https://github.com/inamdarmihir/ostler/blob/main/docs/COOKBOOK.md#7-fill-a-form-and-verify-it)

## Ask your agent

> Open http://127.0.0.1:8787/dependent.html and fill the shipping form for Ada Lovelace in Germany, Bavaria.

## Best practices shown here

- Send the whole form in one `form_fill` call; Ostler handles dependent options and reads every value back.
- A key that matches nothing is reported, not guessed. Fix the key or ask the user.
- `NOT ACCEPTED` returns the page's own messages: correct what they name instead of resubmitting blindly.

## Run it

```sh
ostler fixtures &
python3 run.py
```

The script drives the same tools an agent would, with no model in the loop, and stops with `FAIL:` if any check does not hold.

## Real output

Recorded against Ostler built from `main` on 2026-11-08, with the bundled fixture pages; timestamps normalised.

```text

> navigate {'url': 'http://127.0.0.1:8787/dependent.html'}
Navigated. Now at: <page_content untrusted="true">Shipping</page_content> — <page_content untrusted="true">http://127.0.0.1:8787/dependent.html</page_content>
<page_content untrusted="true">
heading "Shipping address" [e1] level=1
form [e2]:
  textbox "Recipient" [e3] required
  combobox "Country" [e4] required value="Choose…" options=["Choose…","United States","Germany","India"]
  combobox "State / region" [e5] disabled required value="Choose a country first" options=["Choose a country first"]
  textbox "City" [e6]
  button "Ship it" [e7]
</page_content>
Only controls are listed; page text (article body, search snippets) is omitted. Call snapshot to read it.

> form_fill {'values': {'Country': 'Germany', 'State': 'Bavaria', 'Email': 'ada@example.com'}}
Filled 2/3.
<page_content untrusted="true">
verified: "Country", "State / region"
  UNMATCHED key "Email": no field on the page matches this key
Still empty and required:
  e3 "Recipient"
</page_content>
Screenshot: call screenshot with ref=e3 to see the field and its message.
CHECK OK: two fields verified, the unmatched key reported
CHECK OK: State was filled after its options loaded

> form_submit 
NOT ACCEPTED: the page reported problems.
<page_content untrusted="true">
  e3 "Recipient": "Please fill out this field."
</page_content>
Screenshot: call screenshot with ref=e3 to see the form and what the page says.
CHECK OK: the page's own validation stops the submit

> form_fill {'values': {'Recipient': 'Ada Lovelace'}}
Filled 1/1.
<page_content untrusted="true">
verified: "Recipient"
</page_content>
Ready: form_submit.

> form_submit 
Submitted: the form is gone and the page changed.
<page_content untrusted="true">
Page says:
  heading "Shipped" [e8] level=1
</page_content>
CHECK OK: the corrected form is accepted
```
