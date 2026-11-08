# Fill a multi-step wizard

**Use it for:** Job applications, government forms, checkouts split over steps.  
**Tools:** `form_fill` `form_submit` `form_inspect`  
**Documented in:** [Cookbook recipe 8](https://github.com/inamdarmihir/ostler/blob/main/docs/COOKBOOK.md#8-fill-a-multi-step-wizard)

## Ask your agent

> Apply on http://127.0.0.1:8787/wizard.html as Ada Lovelace, ada@example.com, +44 20 7946 0000.

## Best practices shown here

- One `form_fill` and one `form_submit` per step; `Advanced` means a new step is showing.
- Call `form_inspect` on each new step: fields appear that were not there before.
- A required confirmation checkbox is the user's statement; in a real task, ask before ticking it.

## Run it

```sh
ostler fixtures &
python3 run.py
```

The script drives the same tools an agent would, with no model in the loop, and stops with `FAIL:` if any check does not hold.

## Real output

Recorded against Ostler built from `main` on 2026-11-08, with the bundled fixture pages; timestamps normalised.

```text

> navigate {'url': 'http://127.0.0.1:8787/wizard.html'}
Navigated. Now at: <page_content untrusted="true">Application</page_content> — <page_content untrusted="true">http://127.0.0.1:8787/wizard.html</page_content>
<page_content untrusted="true">
heading "Job application" [e1] level=1
form [e2]:
  heading "About you" [e3] level=2
  textbox "First name" [e4] required
  textbox "Last name" [e5] required
  button "Continue" [e6]
</page_content>
Only controls are listed; page text (article body, search snippets) is omitted. Call snapshot to read it.

> form_fill {'values': {'First name': 'Ada', 'Last name': 'Lovelace'}}
Filled 2/2.
<page_content untrusted="true">
verified: "First name", "Last name"
</page_content>
Step 1/3.
Ready: form_submit.

> form_submit 
Advanced: a new form or step is showing.
<page_content untrusted="true">
Page says:
  heading "Job application" [e1] level=1
  text "Step 2 of 3"
  heading "Contact" [e10] level=2
  textbox "Email" [e7] required type=email
  textbox "Phone" [e8] type=tel
Next: form_inspect (form e2).
</page_content>
CHECK OK: step 1 advanced

> form_fill {'values': {'Email': 'ada@example.com', 'Phone': '+44 20 7946 0000'}}
Filled 2/2.
<page_content untrusted="true">
verified: "Email", "Phone"
</page_content>
Step 2/3.
Ready: form_submit.

> form_submit 
Advanced: a new form or step is showing.
<page_content untrusted="true">
Page says:
  heading "Job application" [e1] level=1
  text "Step 3 of 3"
  heading "Review" [e14] level=2
Next: form_inspect (form e2).
</page_content>
CHECK OK: step 2 advanced

> form_inspect 
<page_content untrusted="true">
form [e2] "f" step 3/3
  e12 checkbox "I confirm the details are correct" [required]
  e13 submit "Submit application"
</page_content>
CHECK OK: the review step asks for confirmation

> form_fill {'values': {'I confirm the details are correct': True}}
Filled 1/1.
<page_content untrusted="true">
verified: "I confirm the details are correct"
</page_content>
Step 3/3.
Ready: form_submit.

> form_submit 
Submitted: the form is gone and the page changed.
<page_content untrusted="true">
Page says:
  heading "Application received" [e16] level=1
  text "We will be in touch."
</page_content>
CHECK OK: the application was submitted
```
