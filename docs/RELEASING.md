# Releasing Ostler

What it takes to publish, in the order to do it. Nothing here is automatic on purpose: crates.io
versions can never be deleted or overwritten (only *yanked*), so a human pushes the button.

## Accounts and one-time setup

| what | where | notes |
|---|---|---|
| crates.io account | https://crates.io (sign in with GitHub) | then **Account Settings → API Tokens → New token** with scopes `publish-new` and `publish-update`. Run `cargo login` and paste it. Never commit it. |
| npm account | https://www.npmjs.com/signup | `npm login`. Enable 2FA (npm requires it for publishing). |
| GitHub | already set up | the release workflow uses the built-in `GITHUB_TOKEN`; no secrets needed. |

Names checked free on 2026-10-31: crates.io `ostler`, `ostler-cdp`, `ostler-forms`, `ostler-browser`, `ostler-memory`, `ostler-mcp`, `ostler-a2a`, `ostler-fixtures`; npm `ostler`. Check again right before publishing, because first come, first served.

## Why eight crates

`cargo install ostler` only works if every crate it depends on is on crates.io too, so the internal crates are published (with "internal crate" descriptions and no stability promise). The user-facing package is `ostler` (the `crates/ostler-cli` directory). `ostler-bench` is `publish = false`.

## Each release

1. **Version.** Bump `version` in the root `Cargo.toml` (`[workspace.package]`), the `ostler-*` dependency versions in each crate's `Cargo.toml`, and `npm/package.json`. They must all match; the release workflow checks the tag against `Cargo.toml`. Move the changelog's `Unreleased` entries under the new version.
2. **Verify locally.**
   ```sh
   cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   cargo publish --workspace --dry-run          # packages and builds every crate from its tarball
   scripts/test-installers.sh                   # install.sh + npx against a fake release
   ```
3. **Tag and push** (this builds the binaries and creates the GitHub release):
   ```sh
   git tag v0.2.0 && git push origin v0.2.0
   ```
   Wait for the **Release** workflow to go green. The assets must exist before step 5, because the npm launcher and `install.sh` download them.
4. **Publish the crates** (cargo orders them by dependency):
   ```sh
   cargo publish --workspace
   ```
   If it stops partway, crates.io already has the ones that went through. Fix the cause, then publish the remaining crates one at a time with `cargo publish -p <name>`, dependencies first (`ostler-cdp`, `ostler-forms`, `ostler-memory`, `ostler-fixtures`, `ostler-browser`, `ostler-mcp`, `ostler-a2a`, `ostler`).
5. **Publish npm** (only after the release assets exist):
   ```sh
   cd npm && npm publish --access public
   ```
6. **Smoke test the real thing** on a machine without the repo:
   ```sh
   curl -fsSL https://inamdarmihir.github.io/ostler/install.sh | sh && ostler doctor
   npx -y ostler doctor
   cargo install ostler
   ```
7. **MCP registry.** After npm is live, list the server (`mcpName` in `npm/package.json` is already `io.github.inamdarmihir/ostler`). Use the official `mcp-publisher` and follow the registry's current `server.json` schema; check its docs rather than copying an old example.

## Mistakes

- A bad crate version: `cargo yank --version 0.2.0 ostler` stops new projects choosing it, and you publish 0.2.1. Never try to reuse a version number.
- A bad npm version: `npm deprecate ostler@0.2.0 "use 0.2.1"` (unpublish is restricted).
- A bad GitHub release asset: delete the release and tag, fix, retag. The installers verify sha256 so a half-uploaded release fails loudly instead of installing something broken.

## Platforms

Prebuilt binaries: macOS (Apple silicon, Intel) and Linux (x86_64, aarch64, built on an older glibc). **Windows is not built or tested.** Chrome detection has Windows paths, but nothing has run there; use `cargo install ostler` at your own risk, and treat a Windows report as a new feature.
