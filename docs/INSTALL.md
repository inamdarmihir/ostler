# Install

Full install options, update and uninstall. Back to the [README](../README.md).

## Install options

You need **Chrome or Chromium** (Ostler drives a real browser). Pick one way to get the binary:

| how | command | needs |
|---|---|---|
| **From source** (works today) | `cargo install --git https://github.com/inamdarmihir/ostler ostler` | Rust 1.89+, a minute or two to compile |
| Prebuilt binary | `curl -fsSL https://inamdarmihir.github.io/ostler/install.sh \| sh` | nothing else; macOS and Linux |
| Homebrew (macOS, Linux) | `brew install inamdarmihir/ostler/ostler` | Homebrew; installs the prebuilt binary |
| npm / `npx` | `npx -y ostler doctor` | Node 18+; downloads the binary on first run |
| crates.io | `cargo install ostler` or `cargo binstall ostler` | Rust (or cargo-binstall for the prebuilt) |

> **Status:** nothing is published to crates.io or npm yet, and the prebuilt binaries are created by the first tagged release (the release process is in [docs/RELEASING.md](RELEASING.md); every install path is tested against a fake release in CI). Until then use the first row. Windows is not built or tested.

Then check it:

```sh
ostler doctor        # Chrome launches? Memory report (sealing, key, embeddings, isolation, transport); Qdrant reachable? Prints the fix when not
```

**Update:** rerun the same install command (`install.sh` replaces the binary in place). **Uninstall:** delete the `ostler` binary (`~/.local/bin/ostler`, or `~/.cargo/bin/ostler`), and `~/.config/ostler` and `~/.cache/ostler` if you want the settings and the npx download gone. Memory lives in your Qdrant; `ostler memory wipe --yes` erases it first.
