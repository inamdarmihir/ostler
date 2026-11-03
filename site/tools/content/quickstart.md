# Quickstart

Install Ostler, check that it can drive Chrome, connect your agent, and fill a demo form. About five minutes, most of it compiling.

> [!NOTE]
> **Status: v0.2, early.** Ostler is tested against real Chrome and a real Qdrant server, but only on local fixtures, not the open web. A [design review](review.html) found real flaws and lists them openly with a fix order. Read [what that means](economics.html#6-what-can-go-wrong) before relying on it.

## 1. Install

You need **Chrome or Chromium** (Ostler drives a real browser) and Rust 1.89 or newer to build from source.

```sh
cargo install --git https://github.com/inamdarmihir/ostler ostler
```

Prebuilt binaries, `npx` and crates.io are described on the [Install](install.html) page. Nothing is published to crates.io or npm yet, so building from source is the path that works today.

## 2. Check it works

```sh
ostler doctor
```

`ostler doctor` launches Chrome, round-trips a page and, if you configured Qdrant, prints a memory report (sealing, key location, where embeddings are computed, isolation, transport) and checks that Qdrant is reachable. When something is wrong it prints the fix. See [Troubleshooting](troubleshooting.html) if it does not pass.

## 3. Connect your agent

```sh
ostler setup claude --apply
```

That runs `claude mcp add` with the binary's absolute path. For the others, replace `claude --apply` with `codex`, `cursor` or `vscode`: each prints the config for you to place. Details for every client are in [Use with your agent](agents.html).

After installing or rebuilding `ostler`, reconnect the MCP client (Claude Code: `/mcp`; others: restart it) so it starts the new binary.

## 4. Try the demo

```sh
ostler fixtures        # serves synthetic test forms at http://127.0.0.1:8787
```

Then ask your agent:

> *Open http://127.0.0.1:8787/dependent.html and fill in the shipping form for Ada Lovelace in Germany, Bavaria.*

Ostler fills Country, watches State load, reads every value back, and reports what the page said. Nothing real is submitted.

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

A key that matches nothing is reported, not guessed. A rejected submit says why, in the page's words. More on how this works in [Forms and verification](forms.html).

## Things to ask your agent

- "Fill in the contact form on this page with my details." (after `memory_remember` of your name and email, or with `--learn`)
- "Sign up for the newsletter on example.com, but ask me before you submit." (`--require-confirm-submit`)
- "This form has a date picker and a country dropdown: fill it and tell me anything you couldn't verify."
- "What's blocking this page?" (`look` shows numbered click targets; a CAPTCHA is reported, not bypassed)

## Optional: memory

Browsing and form filling need no account and no key. Memory is separate and optional:

```sh
ostler init                                   # saves your Qdrant URL, key and privacy choices (sealing, embeddings, retention); tests the connection
ostler setup claude --apply -- --learn --lean # re-register with memory flags
```

See [Memory and privacy](memory.html) for exactly what is stored and what is hidden.

## Next steps

- [Use with your agent](agents.html): Claude Code, Codex, Cursor, VS Code, anything else.
- [Tools reference](tools.html): all 21 tools and their parameters.
- [Security model](security.html): the allowlist, uploads, dialogs, CAPTCHAs.
