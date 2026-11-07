# Ostler for VS Code

Gives VS Code's AI chat (and Cursor, Windsurf and other VS Code forks) a real web browser through [Ostler](https://github.com/inamdarmihir/ostler): read pages, research with quotes checked against the page, and fill in forms with every value read back.

**No Rust, no terminal, no config files.** Install the extension and Ostler appears as an MCP server named *Ostler*. The first time it starts, the extension downloads the matching Ostler release once and verifies its checksum.

## Needs

- Google Chrome
- macOS or Linux (Windows support is experimental and arrives with Ostler v0.2.1)
- Ostler v0.2.0 or newer. The extension uses an `ostler` on your PATH if there is one (`pip install ostler-mcp`), and otherwise downloads the matching release once.

## Commands

- **Ostler: Check setup** runs `ostler doctor` and prints the fix for anything wrong.
- **Ostler: Sign in to a website** opens a Chrome window so you can sign in once; set `ostler.profileDir` to the saved folder (`~/.ostler/profile`) so the assistant can use the session.
- **Ostler: Open the getting-started guide**

## Settings

| setting | what it does |
|---|---|
| `ostler.path` | use this binary instead of the one on PATH or a download |
| `ostler.headed` | show the Chrome window while the assistant works |
| `ostler.cautious` | block page requests that change things unless the confirmed `form_submit` step is used (stricter; can stop some sign-ins) |
| `ostler.profileDir` | keep sign-ins in this folder |

Ostler is early (v0.2) and mostly tested on local fixtures; see the [project README](https://github.com/inamdarmihir/ostler#readme) for what it does not do. Apache-2.0.
