# Install

Full install options, update and uninstall. Back to the [README](../README.md).

## Install options

You need **Chrome or Chromium** (Ostler drives a real browser). Pick one way to get the binary:

| how | command | needs |
|---|---|---|
| **From source** (works today) | `cargo install --git https://github.com/inamdarmihir/ostler ostler` | Rust 1.89+, a minute or two to compile |
| Prebuilt binary | `curl -fsSL https://inamdarmihir.github.io/ostler/install.sh \| sh` | nothing else; macOS and Linux |
| Homebrew (macOS, Linux), **not working yet** | `brew install inamdarmihir/ostler/ostler` | the tap is not being updated yet |
| Python | `pip install ostler-mcp` or `uvx --from ostler-mcp ostler serve` | Python 3.8+ (or uv); the wheel carries the binary; live since v0.2.0 |
| VS Code / Cursor / Windsurf | install the **Ostler** extension from Open VSX (Cursor, Windsurf, VSCodium); the VS Code Marketplace listing is not published yet | the editor (source in [`editors/vscode`](../editors/vscode)) |
| Claude Desktop (Mac) | download the `.mcpb` from the release and double-click | nothing else; live since v0.2.0 |
| npm / `npx` | `npx -y ostler doctor` | Node 18+; downloads the binary on first run; not published to npm yet |
| crates.io | `cargo install ostler` or `cargo binstall ostler` | Rust (or cargo-binstall for the prebuilt); not published to crates.io yet |
| Docker (linux/amd64, linux/arm64), **next release (v0.2.1)** | `docker run -i --rm ghcr.io/inamdarmihir/ostler` | Docker; the image bundles Chromium and runs as a non-root user |
| Windows, **next release (v0.2.1)** | download `ostler-v<version>-x86_64-pc-windows-msvc.tar.gz` from the release, or `pip install ostler-mcp` | Google Chrome or Edge-compatible Chromium installed; experimental, see [COMPATIBILITY](COMPATIBILITY.md) |

> **Status (v0.2.0 is released):** live today are the GitHub release binaries, the install script, the `.mcpb` bundles (macOS), PyPI (`ostler-mcp`) and Open VSX. **Not yet:** crates.io, npm, the VS Code Marketplace, and Homebrew (the tap token needs fixing). Windows and the Docker image arrive in the next release (v0.2.1) and are experimental. Release process: [docs/RELEASING.md](RELEASING.md).

Then run `ostler start` to connect every AI app it finds (or `ostler install --client <name>` for one), and check it:

```sh
ostler doctor        # Chrome launches? Memory report (sealing, key, embeddings, isolation, transport); Qdrant reachable? Prints the fix when not
```

**Update:** rerun the same install command (`install.sh` replaces the binary in place). **Uninstall:** delete the `ostler` binary (`~/.local/bin/ostler`, or `~/.cargo/bin/ostler`), and `~/.config/ostler` and `~/.cache/ostler` if you want the settings and the npx download gone. Memory lives in your Qdrant; `ostler memory wipe --yes` erases it first.
