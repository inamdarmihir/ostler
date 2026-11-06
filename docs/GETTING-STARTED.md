# Getting started (no programming needed)

Ostler lets an AI assistant (Claude, Cursor, Codex) use a real web browser on your computer: read pages, research a topic with sources it actually read, and fill in web forms. You do **not** need Rust, Node or any code. Back to the [README](../README.md).

**Status:** early (v0.2). No release is published yet, so the download steps below start working with the first release; until then a developer can build it for you ([INSTALL](INSTALL.md)). Windows is not supported yet.

## What you need

- A Mac or Linux computer (Windows: not yet).
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

## Things to ask

- "Open this page and summarize it: <link>"
- "Research <topic>. Only use pages you actually read, and check every quote against the page."
- "Fill in the contact form on <link> with my name and email, then show me what you entered before submitting."
- "Check that the login page on my site still works after my change."

For a safe practice page, run `ostler fixtures` and ask the assistant to fill in the form at http://127.0.0.1:8787/dependent.html. Nothing real is submitted.

## Good to know

- Ostler uses its own Chrome window, hidden by default. To sign in to a site once and reuse it, or to solve a CAPTCHA yourself, see [Troubleshooting](TROUBLESHOOTING.md).
- Ostler does not solve CAPTCHAs and does not submit real forms unless you ask the assistant to.
- Your AI app may send what it reads to its own model provider. Ostler itself sends pages nowhere.

## If something goes wrong

Run `ostler doctor`. Every failing check prints the exact fix. More in [Troubleshooting](TROUBLESHOOTING.md).
