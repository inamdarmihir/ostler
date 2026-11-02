# Install

Ostler is a single Rust binary, `ostler`, that launches and drives Chrome. You need **Chrome or Chromium** installed. Pick one way to get the binary.

| how | command | needs |
|---|---|---|
| **From source** (works today) | `cargo install --git https://github.com/inamdarmihir/ostler ostler` | Rust 1.89+, a minute or two to compile |
| Prebuilt binary | `curl -fsSL https://inamdarmihir.github.io/ostler/install.sh \| sh` | nothing else; macOS and Linux |
| npm / `npx` | `npx -y ostler doctor` | Node 18+; downloads the binary on first run |
| crates.io | `cargo install ostler` or `cargo binstall ostler` | Rust (or cargo-binstall for the prebuilt) |

> [!WARNING]
> **Availability:** nothing is published to crates.io or npm yet, and the prebuilt binaries are created by the first tagged release (the release process is in [RELEASING.md](repo:docs/RELEASING.md); every install path is tested against a fake release in CI). Until then use the first row. Windows is not built or tested.

## Verify

```sh
ostler doctor        # Chrome launches? Qdrant reachable (if you set one)? prints the fix when not
```

`ostler doctor` explains launch failures: sandbox or root problems, a profile already in use, missing libraries, and otherwise how to see Chrome's own error.

## Platforms

- **macOS:** tested locally.
- **Linux:** tested in CI. In containers or as root you may need `OSTLER_NO_SANDBOX=1`, plus a larger `/dev/shm` (`--shm-size=1g`) for Chrome. See [Troubleshooting](troubleshooting.html).
- **Windows:** not built or tested.
- **Browser:** Chrome or Chromium only. If Chrome is in a non-standard place, set `OSTLER_CHROME=/path/to/chrome` or pass `--chrome-path`.

## Update

Rerun the same install command. `install.sh` replaces the binary in place. Then reconnect your MCP client (Claude Code: `/mcp`; others: restart it) so it starts the new binary.

## Uninstall

Delete the `ostler` binary (`~/.local/bin/ostler`, or `~/.cargo/bin/ostler`), and `~/.config/ostler` and `~/.cache/ostler` if you want the settings and the `npx` download gone.

Memory lives in your Qdrant, not on disk here. `ostler memory wipe --yes` erases it, so run that first if you want it gone.

## What gets written to disk

By default, nothing apart from the binary and, if you run `ostler init`, the config file `~/.config/ostler/config.toml` (owner-only permissions). With sealed memory, a key file `memory.key` next to the config. Chrome uses a throwaway profile unless you pass `--profile-dir`. Ostler sends no telemetry.

## The prebuilt installer

`install.sh` is served from this site and verifies a checksum before installing; on a mismatch it installs nothing and runs nothing. It installs to `~/.local/bin` by default (`OSTLER_INSTALL_DIR` changes that, `OSTLER_VERSION` pins a version) and never edits your shell profile or your agent's config. You can read the script first: [install.sh](../install.sh).
