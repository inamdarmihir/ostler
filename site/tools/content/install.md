# Install

Ostler is a single Rust binary, `ostler`, that launches and drives Chrome. You need **Chrome or Chromium** installed. Pick one way to get the binary. v0.2.0 is released; the table says what is live today.

| how | command | needs |
|---|---|---|
| **Python** ([PyPI: ostler-mcp](https://pypi.org/project/ostler-mcp/)), **live** | `pip install ostler-mcp`, or `uvx --from ostler-mcp ostler serve` | Python 3.8+ or uv; the wheel carries the binary |
| Prebuilt binary, **live** | `curl -fsSL https://inamdarmihir.github.io/ostler/install.sh \| sh`, then `ostler start` | nothing else; macOS and Linux |
| Claude Desktop (Mac), **live** | download the `.mcpb` from the [release](https://github.com/inamdarmihir/ostler/releases/latest) and double-click it | nothing else |
| **VS Code, Cursor, Windsurf**, **live on Open VSX** ([Open VSX](https://open-vsx.org/extension/inamdarmihir/ostler)) | install the **Ostler** extension. The VS Code Marketplace listing is not published yet. | the editor; no terminal |
| **From source**, works anywhere | `cargo install --git https://github.com/inamdarmihir/ostler ostler` | Rust 1.89+, a minute or two to compile |
| Docker, next release (v0.2.1) | `docker run -i --rm ghcr.io/inamdarmihir/ostler` | Docker; bundles Chromium, runs as a non-root user |
| Windows (x86_64), next release (v0.2.1), experimental | download `ostler-v<version>-x86_64-pc-windows-msvc.tar.gz` from the release, or `pip install ostler-mcp` | Chrome or Edge installed |
| Homebrew, not working yet | `brew install inamdarmihir/ostler/ostler` | the tap is not being updated yet |
| npm / `npx`, not published yet | `npx -y ostler doctor` | Node 18+ |
| crates.io, not published yet | `cargo install ostler` or `cargo binstall ostler` | Rust |

> [!NOTE]
> **Availability (v0.2.0):** the rows marked live work today. The others are not published yet, so their pages show "not found"; PyPI and the install script always work. Release process: [RELEASING.md](repo:docs/RELEASING.md). Not a developer? Start with [Getting started, no programming needed](get-started.html).

## Where Ostler is listed

- Source and releases: [GitHub](https://github.com/inamdarmihir/ostler) and [releases](https://github.com/inamdarmihir/ostler/releases)
- Python: [PyPI, `ostler-mcp`](https://pypi.org/project/ostler-mcp/) (the name `ostler` on PyPI is a different project)
- Editors: [Open VSX](https://open-vsx.org/extension/inamdarmihir/ostler) for Cursor, Windsurf and other forks (the VS Code Marketplace listing is not published yet)
- Not published yet: npm (`ostler`), crates.io (`ostler`), the Homebrew tap and the Docker image (`ghcr.io/inamdarmihir/ostler`, next release)

## Verify

```sh
ostler doctor        # Chrome launches? Memory report; Qdrant reachable? Every failing check prints the command that fixes it
ostler doctor --fix  # apply only the safe fixes: create the config directory, chmod 600 an open key file
```

`ostler doctor` explains launch failures (sandbox or root problems, a profile already in use, missing libraries, Chrome not installed) with a concrete command for each. `--fix` never creates keys, installs software, edits client configs or touches the network.

## Connect your agent

Easiest: `ostler start` finds the AI apps on your computer and connects Ostler to each. Or pick one:

```sh
ostler install --list                  # clients and their config files
ostler install --client claude-code    # or claude-desktop | cursor | vscode | windsurf | codex
```

This edits the client's config file for you: it refuses a file that is not valid JSON/TOML, makes a backup first, keeps your other servers and prints exactly what changed. Re-running it changes nothing; `--dry-run` previews and `--uninstall` removes the entry. Nothing else needs configuring: with no config at all, memory is in-process for the session and no Qdrant or key is involved.

## Use your own Chrome (optional)

Start Chrome yourself with a debugging port on loopback and a dedicated profile, then point Ostler at it:

```sh
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --remote-debugging-port=9222 --remote-debugging-address=127.0.0.1 --user-data-dir=$HOME/.ostler/chrome-profile
ostler doctor --cdp-url http://127.0.0.1:9222
ostler install --client claude-code -- --cdp-url http://127.0.0.1:9222
```

Only loopback is accepted. Ostler never opens the port for you, works in its own tab and leaves your browser running. While the port is open any local program can drive that Chrome, so use a dedicated profile.

## Platforms

- **macOS:** tested locally.
- **Linux:** tested in CI. In containers or as root you may need `OSTLER_NO_SANDBOX=1`, plus a larger `/dev/shm` (`--shm-size=1g`) for Chrome. See [Troubleshooting](troubleshooting.html).
- **Windows (x86_64):** builds and runs its test suite in CI; ships from the next release (v0.2.1). Experimental, with no day-to-day use yet.
- **Docker (linux/amd64, linux/arm64):** ships from the next release (v0.2.1).
- **Browser:** Chrome or Chromium only. If Chrome is in a non-standard place, set `OSTLER_CHROME=/path/to/chrome` or pass `--chrome-path`.

## Update

Rerun the same install command. `install.sh` replaces the binary in place. Then reconnect your MCP client (Claude Code: `/mcp`; others: restart it) so it starts the new binary.

## Uninstall

Run `ostler install --client <name> --uninstall` for each client you registered (it keeps your other servers and backs the file up). Delete the `ostler` binary (`~/.local/bin/ostler`, or `~/.cargo/bin/ostler`), and `~/.config/ostler` and `~/.cache/ostler` if you want the settings and the `npx` download gone.

Memory lives in your Qdrant, not on disk here. `ostler memory wipe --yes` erases it, so run that first if you want it gone.

## What gets written to disk

By default, nothing apart from the binary and, if you run `ostler init`, the config file `~/.config/ostler/config.toml` (owner-only permissions). With sealed memory, a key file `memory.key` next to the config. Chrome uses a throwaway profile unless you pass `--profile-dir`. Ostler sends no telemetry.

## The prebuilt installer

`install.sh` is served from this site and verifies a checksum before installing; on a mismatch it installs nothing and runs nothing. It installs to `~/.local/bin` by default (`OSTLER_INSTALL_DIR` changes that, `OSTLER_VERSION` pins a version) and never edits your shell profile or your agent's config. You can read the script first: [install.sh](../install.sh).
