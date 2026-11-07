# Reaching non-technical users

Analysis of what stops a non-developer from using Ostler, what this change fixes, and what is still open. Back to the [README](../README.md).

## Who is blocked, and where

| step today | who it stops | fix |
|---|---|---|
| `cargo install` needs Rust 1.89+, minutes of compiling | everyone who is not a Rust developer | prebuilt binaries, the `.mcpb` bundle and `pip install ostler-mcp` (all live since v0.2.0) |
| Terminal, `PATH`, absolute binary paths | most people | `ostler start` needs no flags and no client name |
| Naming a client and editing its JSON/TOML | anyone unsure which app or file | `ostler start` detects installed apps; `ostler install` already edits safely with backups |
| No double-click path for Claude Desktop, the largest non-developer MCP client | the main target audience | `.mcpb` extension bundle built by the release workflow |
| Docs are written for developers (ADRs, benchmarks first) | newcomers | [GETTING-STARTED.md](GETTING-STARTED.md), linked at the top of the README |
| "What do I ask it?" | people without an MCP mental model | example prompts in the guide and printed by `ostler start` |
| Windows not built or tested | roughly half of desktop users | open, see below |

## Done in this change

1. `ostler start`: detect, connect, check Chrome, print next steps. Tested end to end (detected-only, idempotent, empty case).
2. `.mcpb` bundle for Claude Desktop on macOS: `scripts/package-mcpb.sh` and a release job step. Untested inside Claude Desktop itself (no macOS here); the zip and manifest were built and checked structurally. Verify on a Mac before announcing.
3. Plain-English guide and README entry point.
4. MCP prompt templates (`crates/ostler-mcp/src/prompts.rs`): five click-to-run tasks. Unit and JSON-RPC tested; not yet seen in a real client menu.
5. `ostler login <url>`: sign in once, reuse the session. Builds and parses; not run against a real display here.
6. Cursor and VS Code install links in the guide (formats not verified against the apps).
7. `ostler start --cautious` (registers `--require-confirm-submit`), a practice prompt printed by `start`, and a plain-English table of the tool messages in the guide.
8. [DISTRIBUTION-CHECKLIST.md](DISTRIBUTION-CHECKLIST.md): where to list Ostler, with honest status.
9. **Python distribution:** `pyproject.toml` (maturin, `bindings = "bin"`) builds `ostler-mcp` wheels carrying the binary. Built here, installed in a clean virtualenv, and run through `uvx --from <wheel> ostler serve` including an MCP `initialize` and `prompts/list`. Release CI builds per-platform wheels and uploads them to PyPI when `PYPI_TOKEN` exists. Not a rewrite: there is no Python code. The PyPI name `ostler` is taken (an unrelated project), so the project name is `ostler-mcp`.
10. **VS Code extension** (`editors/vscode`): registers an MCP server definition provider, resolves the binary (setting, PATH, then a checksum-verified one-time download), adds *Check setup* and *Sign in* commands. The download, checksum and settings logic has Node tests against a local fake release; the extension was packaged into a `.vsix`. It has **not** been run inside VS Code (no editor here), so the MCP provider API use needs a check in a real VS Code 1.101+ before publishing.

Deliberately not done: confirmation on by default (the flag blocks every non-GET request, which breaks logins), a visible-browser default (changes behaviour for existing agents), and an update notice (needs a network call).

## Still open, in suggested order

1. **Finish the channels v0.2.0 started.** Done: GitHub release, PyPI, Open VSX. Left: `VSCE_PAT` (VS Code Marketplace), a working `HOMEBREW_TAP_TOKEN`, and publish jobs for npm and crates.io ([RELEASING](RELEASING.md)).
2. **Windows build, `.mcpb` and wheel.** Claude Desktop is mostly used on Windows and macOS. Needs a Windows CI job, Chrome discovery and `ostler install` path checks on a real Windows machine; the install code already has Windows paths but nothing has run there.
3. **Sign and notarize the macOS binary.** An unsigned download triggers Gatekeeper warnings that will stop non-technical users. Needs an Apple Developer account.
4. **Submit to the Claude Desktop extension directory** and the MCP registry so it can be found without the GitHub releases page.
5. **Bundle or auto-fetch Chrome for Testing** so "install Chrome first" disappears (trade-off: download size, licensing review).
6. **A small desktop installer or web page** that wraps `ostler start` with buttons, if usage justifies maintaining a GUI.
7. **Task-shaped prompt library** (research report, price check, form filling) in the docs and the site.
