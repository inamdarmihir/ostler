# Fill personal data the agent never sees

**Use it for:** Email, address, phone and other profile facts reused across forms.  
**Tools:** `memory_remember` `form_fill` with `{{fact:KEY}}`  
**Documented in:** [Cookbook recipe 9](https://github.com/inamdarmihir/ostler/blob/main/docs/COOKBOOK.md#9-never-fill-the-same-form-twice)

## Ask your agent

> Fill the contact form at http://127.0.0.1:8787/basic.html for Ada Lovelace, using my saved email.

## Best practices shown here

- Save facts once, outside the task; in the task, reference them as `{{fact:KEY}}`.
- The value is typed by Ostler and not echoed back, so it never enters the model's context.
- An unknown fact fails the whole call; nothing else is filled in its place.

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

> form_fill {'values': "{'Full name': 'Ada Lovelace', 'Email': '{{fact:email}}', 'Message': 'Please c..."}
Filled 3/3.
<page_content untrusted="true">
verified: "Full name", "Email", "Message"
</page_content>
Ready: form_submit.
CHECK OK: all three fields were filled and verified
CHECK OK: the saved value is not echoed back to the agent

> form_fill {'values': {'Email': '{{fact:no_such_fact}}'}}
ERROR no saved fact "no_such_fact" for {{fact:no_such_fact}}; save it with memory_remember or pass the value
CHECK OK: an unknown fact fails the call instead of filling something else
```
