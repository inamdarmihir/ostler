# Submit only with a person's approval

**Use it for:** Applications, orders, messages: anything with consequences.  
**Tools:** `form_fill` `form_submit` with `confirm`, server flag `--require-confirm-submit`  
**Documented in:** [Cookbook recipe 10](https://github.com/inamdarmihir/ostler/blob/main/docs/COOKBOOK.md#10-submit-only-with-your-approval)

## Ask your agent

> Fill the contact form at http://127.0.0.1:8787/basic.html for Ada Lovelace, ada@example.com, show me what you filled, and submit only after I say yes.

## Best practices shown here

- Run consequential work with `--require-confirm-submit` and an `--allow-domain` list.
- Show the user what was filled, then call `form_submit` with `confirm: true` only after they agree.
- Never pass `confirm: true` on the first attempt to skip the gate.

## Run it

```sh
ostler fixtures &
python3 run.py
```

The script drives the same tools an agent would, with no model in the loop, and stops with `FAIL:` if any check does not hold.

## Real output

Recorded against Ostler built from `main` on 2026-11-08, with the bundled fixture pages; timestamps normalised.

```text

> navigate {'url': 'http://127.0.0.1:8787/basic.html'}
Navigated. Now at: <page_content untrusted="true">Contact us</page_content> — <page_content untrusted="true">http://127.0.0.1:8787/basic.html</page_content>
<page_content untrusted="true">
heading "Contact us" [e1] level=1
form [e2]:
  textbox "Full name" [e3] required
  textbox "Email" [e4] required type=email
  textbox "Message" [e5]
  checkbox "Subscribe to newsletter" [e6]
  button "Send message" [e7]
</page_content>
Only controls are listed; page text (article body, search snippets) is omitted. Call snapshot to read it.

> form_fill {'values': {'Full name': 'Ada Lovelace', 'Email': 'ada@example.com'}}
Filled 2/2.
<page_content untrusted="true">
verified: "Full name", "Email"
</page_content>
Ready: form_submit.

> form_submit 
ERROR Submission blocked: this server requires explicit confirmation. Show the user what will be submitted, and call form_submit again with confirm=true only after they agree.
CHECK OK: the submit is held for approval

(The user reviewed the form and approved it.)

> form_submit {'confirm': True}
Submitted: the form is gone and the page changed.
<page_content untrusted="true">
Now at: Thanks — http://127.0.0.1:8787/submit
Page says:
  heading "Thank you" [e1] level=1
  text "Your message was received."
</page_content>
CHECK OK: submitted only after confirm=true
```
