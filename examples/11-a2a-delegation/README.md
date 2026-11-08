# Hand a form to Ostler from another agent (A2A)

**Use it for:** An orchestrator or another agent that should delegate form work as a task.  
**Tools:** A2A v1.0 `SendMessage` with a `data` part  
**Documented in:** [A2A guide](https://github.com/inamdarmihir/ostler/blob/main/docs/A2A.md)

## Best practices shown here

- Always start the server with `--allow-domain`; without it, A2A blocks loopback and private networks.
- Discover the JSON-RPC endpoint from the agent card instead of hard-coding it.
- Back off on HTTP 429 and 503 for as long as `Retry-After` says: each conversation holds a Chrome, and the server caps them (`--max-contexts`, default 4).
- `TASK_STATE_INPUT_REQUIRED` is where a human-in-the-loop client stops; reply on the same task with `{"confirm": true}`.
- Artifacts are marked untrusted: they contain website text.

## Run it

```sh
ostler fixtures &
ostler a2a --allow-domain 127.0.0.1 &
python3 run.py
```

The script drives the same tools an agent would, with no model in the loop, and stops with `FAIL:` if any check does not hold.

## Real output

Recorded against Ostler built from `main` on 2026-11-08, with the bundled fixture pages; timestamps normalised.

```text
Agent card: Ostler 0.2.0, JSON-RPC at http://127.0.0.1:8765/a2a

> SendMessage {"url": "http://127.0.0.1:8787/basic.html", "values": {"Full name": "Ada Lovelace", "Email": "ada@example.com"}, "submit": true}
TASK_STATE_INPUT_REQUIRED
Filled and verified. Reply with {"confirm": true} to submit; this sends data to the site.
CHECK OK: the task waits for confirmation

> SendMessage {"confirm": true}
TASK_STATE_COMPLETED
Submitted: the form is gone and the page changed.
Now at: Thanks — http://127.0.0.1:8787/submit
Page says:
  heading "Thank you" [e1] level=1
  text "Your message was received."

CHECK OK: submitted after confirm
```
