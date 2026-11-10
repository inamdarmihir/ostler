# Getting started (no programming needed)

Ostler lets an AI assistant (Claude, Cursor, Codex) use a real web browser on your computer: read pages, research a topic with sources it actually read, and fill in web forms. You do **not** need Rust, Node or any code. Back to the [README](../README.md).

**Status:** early (v0.2). v0.2.0 is released, so the download steps below work (macOS and Linux; Windows and Docker arrive in the next release, experimental). The VS Code Marketplace, crates.io and npm are not published yet ([INSTALL](INSTALL.md)).

## What you need

- A Mac or Linux computer. Windows and Docker are not released yet (planned v0.2.1, experimental, with known gaps listed in [COMPATIBILITY](COMPATIBILITY.md#known-windows-and-docker-gaps-v020-source)).
- **Google Chrome** ([download](https://www.google.com/chrome/)).
- An AI app that supports MCP: Claude Desktop, Claude Code, Cursor, VS Code, Windsurf or Codex.

## Path A: Claude Desktop on a Mac (one double-click)

1. Open the [latest release](https://github.com/inamdarmihir/ostler/releases/latest) and download the `.mcpb` file for your Mac (`aarch64` = Apple silicon M1 or newer, `x86_64` = Intel).
2. Double-click it. Claude Desktop shows an install prompt; click **Install**.
3. Start a chat and ask: *"Open https://example.com and tell me what the page says."*

## Path B: any supported app, one command

Open **Terminal** (Mac: press Cmd+Space, type Terminal) and paste:

```sh
curl -fsSL https://inamdarmihir.github.io/ostler/install.sh | sh
ostler start
```

`ostler start` finds the AI apps on your computer, connects Ostler to each one, checks that Chrome is there, and tells you what to restart. It backs up any file it changes, keeps your other settings, and is safe to run twice. To undo: `ostler install --client <name> --uninstall` (`ostler install --list` shows the names).

Then **quit and reopen your AI app**.

## Path C: VS Code, Cursor or Windsurf extension

Search the extensions panel for **Ostler** (Open VSX for Cursor and Windsurf; the VS Code Marketplace listing is not published yet), install it, and Ostler appears as an MCP server. No terminal. It uses an `ostler` already on your PATH, or downloads the matching release once and checks its checksum. Commands: *Ostler: Check setup*, *Ostler: Sign in to a website*. Source: [editors/vscode](../editors/vscode).

## Path D: Python (`pip` or `uvx`)

```sh
pip install ostler-mcp        # adds the `ostler` command (use pipx or `uv tool install` to keep it isolated)
ostler start
```

Or with no install step, in any MCP client's config: command `uvx`, arguments `["--from", "ostler-mcp", "ostler", "serve"]`. The package is the same Rust program inside a Python wheel (no Python code, nothing downloaded at run time). The name is `ostler-mcp` because `ostler` on PyPI is a different project.

## Path E: one-click links for Cursor and VS Code

After installing the `ostler` command (Path B, first line), these links add it to the app:

- [Add to Cursor](https://cursor.com/install-mcp?name=ostler&config=eyJjb21tYW5kIjoib3N0bGVyIiwiYXJncyI6WyJzZXJ2ZSJdfQ==)
- [Add to VS Code](https://insiders.vscode.dev/redirect/mcp/install?name=ostler&config=%7B%22name%22%3A%22ostler%22%2C%22command%22%3A%22ostler%22%2C%22args%22%3A%5B%22serve%22%5D%7D)

They assume `ostler` is on your `PATH`; `ostler start` does not need that. The link formats follow the apps' published install-link schemes but have not been tested here.

## Ready-made prompts

Apps that show MCP prompts (Claude Desktop lists them under the **+** menu) offer five from Ostler: summarize a page, research with checked sources, fill in a form (review before sending), check my web page, compare pages. Pick one, fill in the blank, send.

## Signing in to a website

For tasks behind a login, sign in once yourself:

```sh
ostler login https://example.com/login
```

A Chrome window opens. Sign in, then press Enter in Terminal. Then connect Ostler with that saved session: `ostler install --client claude-desktop -- --profile-dir ~/.ostler/profile`. Anyone who can read that folder can use your sign-in, so use it on your own computer only.

## Things to ask

- "Open this page and summarize it: <link>"
- "Research <topic>. Only use pages you actually read, and check every quote against the page."
- "Fill in the contact form on <link> with my name and email, then show me what you entered before submitting."
- "Check that the login page on my site still works after my change."

For a safe practice page, run `ostler fixtures` and ask the assistant to fill in the form at http://127.0.0.1:8787/dependent.html. Nothing real is submitted.

## Extra caution

`ostler start --cautious` connects Ostler so page requests that change things (form posts and similar) are blocked unless the assistant uses its confirmed submit step. It is stricter than normal and can also stop some sign-ins or buttons. Run `ostler start` again without the flag to go back.

## What the messages mean

| you see | meaning |
|---|---|
| `VERIFIED` / `NOT VERIFIED` | the quoted words were (or were not) found on a page the assistant actually read. It proves the words are there, not that they are true |
| `NOT READ` | the assistant cited a page it never opened |
| gaps in `evidence` | pages it could not read (paywall, bot check, blocked by the site's robots.txt, error) |
| `CHALLENGE:` | the site showed a CAPTCHA or "are you human" check. Ostler does not solve these |
| `UNMATCHED key` | a detail you gave matches no field on the form, so nothing was guessed |
| `NOT ACCEPTED` | the site rejected the form and says why |

## Good to know

- Ostler uses its own Chrome window, hidden by default. To sign in to a site once and reuse it, or to solve a CAPTCHA yourself, see [Troubleshooting](TROUBLESHOOTING.md).
- Ostler does not solve CAPTCHAs and does not submit real forms unless you ask the assistant to.
- Your AI app may send what it reads to its own model provider. Ostler itself sends pages nowhere.

## If something goes wrong

Run `ostler doctor`. Every failing check prints the exact fix. More in [Troubleshooting](TROUBLESHOOTING.md).
