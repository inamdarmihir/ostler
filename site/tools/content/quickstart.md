# Quickstart

Install Ostler, check that it can drive Chrome, connect your agent, and try it on a demo page: read and cite a source, or fill a form. About five minutes, most of it compiling.

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

`ostler doctor` launches Chrome, round-trips a page and, if you configured Qdrant, prints a memory report (sealing, key location, where embeddings are computed, isolation, transport) and checks that Qdrant is reachable. When something is wrong it prints the command that fixes it; `ostler doctor --fix` applies the safe ones (creating the config directory). With no config at all, memory is in-process for the session and nothing needs Qdrant or a key. See [Troubleshooting](troubleshooting.html) if it does not pass.

## 3. Connect your agent

```sh
ostler install --client claude-code      # or claude-desktop | cursor | vscode | windsurf | codex
```

That writes the entry into the client's config file with the binary's absolute path: it backs the file up first, keeps your other servers, refuses a malformed file and prints what changed (`--dry-run` previews, `--uninstall` removes). `ostler install --list` shows every client. To only print a snippet instead, use `ostler setup <client>`. Prefer your own Chrome window? See "Use your own Chrome" on the [Install](install.html) page. Details for every client are in [Use with your agent](agents.html).

After installing or rebuilding `ostler`, reconnect the MCP client (Claude Code: `/mcp`; others: restart it) so it starts the new binary.

## 4. Try the demo

```sh
ostler fixtures        # serves synthetic test pages at http://127.0.0.1:8787
```

Pick one. Nothing real is read or submitted; these are bundled test pages.

**Research with a checked quote.** Ask your agent:

> *Read http://127.0.0.1:8787/packs/research/kestrel-lock.html. Who designed the Kestrel Lock and when did it open? Cite every claim with `cite` and call `evidence` before answering.*

Ostler reads the page, accepts a quote that is on it, refuses one that is not, and lists any page it could not read. The full walk-through, with unedited output, is in [Research with checked citations](research-with-citations.html).

**Fill a form.** Ask your agent:

> *Open http://127.0.0.1:8787/dependent.html and fill in the shipping form for Ada Lovelace in Germany, Bavaria.*

Ostler fills Country, watches State load, reads every value back, and reports what the page said.

## What the agent sees

### Research

Unedited output from the bundled research pages (`ostler serve --tools research`), captured 2026-11-06. The `<page_content untrusted="true">` tags mark text that came from the page.

```text
> cite {"url": "http://127.0.0.1:8787/packs/research/kestrel-lock.html", "quote": "opened in 1887 and was designed by Marta Ilves"}
VERIFIED [1] <page_content untrusted="true">http://127.0.0.1:8787/packs/research/kestrel-lock.html</page_content> (read 2026-11-06T16:01:32Z, sha256:9f602117)
<page_content untrusted="true">opened in 1887 and was designed by Marta Ilves</page_content>
Check it yourself (best effort): http://127.0.0.1:8787/packs/research/kestrel-lock.html#:~:text=opened%20in%201887%20and%20was%20designed%20by%20Marta%20Ilves

> cite {"url": "http://127.0.0.1:8787/packs/research/kestrel-lock.html", "quote": "Tomas Reyes designed the Kestrel Lock in 1887"}
NOT VERIFIED: these words are not on <page_content untrusted="true">The Kestrel Lock</page_content>. Do not state this claim as sourced; quote the page's own words, not a summary.
Closest text on the page (similarity 0.48): <page_content untrusted="true">The Kestrel Lock The Kestrel Lock is a double</page_content>
```

`cite` proves the words are on the page, not that the claim is true.

### A form

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

- "Find out what this page says about the rate limit and quote the sentence. Say so if you could not read it." (`read`, `cite`, `evidence`)
- "Compare the pricing on these three pages and tell me what you could not check." (`open_many`, `evidence`)
- "Open this web app, click through the signup flow and tell me what breaks." (`navigate`, `click`, `look`)
- "Fill in the contact form on this page with my details." (after `memory_remember` of your name and email, or with `--learn`)
- "Sign up for the newsletter on example.com, but ask me before you submit." (`--require-confirm-submit`)
- "This form has a date picker and a country dropdown: fill it and tell me anything you couldn't verify."
- "What's blocking this page?" (`look` shows numbered click targets; a CAPTCHA is reported, not bypassed)

## Optional: memory

Browsing, reading, citing and form filling need no account and no key (only `search` needs a provider). Memory is separate and optional:

```sh
ostler init                                   # saves your Qdrant URL, key and privacy choices (sealing, embeddings, retention); tests the connection
ostler install --client claude-code -- --learn --lean # re-register with memory flags
```

See [Memory and privacy](memory.html) for exactly what is stored and what is hidden.

## Next steps

- [Use with your agent](agents.html): Claude Code, Codex, Cursor, VS Code, anything else.
- [Tools reference](tools.html): all 30 tools and their parameters.
- [Security model](security.html): the allowlist, uploads, dialogs, CAPTCHAs.
