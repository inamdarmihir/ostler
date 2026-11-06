# ADR-0017: cargo-dist for releases: defer to v0.4, after a dry run and an installer-naming migration

Status: proposed (M0 spike, 2026-11-06). Answers the `cargo-dist` row of BUILD-PLAN section 4; relates to [docs/RELEASING.md](../RELEASING.md).

## Context
Releases today: pushing a `v*` tag runs `.github/workflows/release.yml` (60 lines: tag-matches-`Cargo.toml` check, four-target build matrix, `scripts/package.sh`, `gh release create`), then a human runs `cargo publish --workspace` and `npm publish`. The packaging contract is `ostler-v<version>-<target>.tar.gz` with a top-level directory and a `.sha256` per archive, and three things depend on those names: `site/install.sh`, the `[package.metadata.binstall]` block in `crates/ostler-cli/Cargo.toml`, and `npm/bin/ostler.js` (plus `scripts/test-installers.sh`). The plan proposes cargo-dist (binaries, installers, Homebrew) for v0.4. M0 also tags a v0.2.x release.

## Decision
1. **Do not switch for the M0 v0.2.x tag.** Use the current workflow; it works and is documented.
2. **Adopt cargo-dist for v0.4**, but only after a dry run in a scratch repository and these changes, in one PR with the installer tests updated:
   - decide the archive name: either configure cargo-dist to keep `ostler-v<version>-<target>` names (the spike did not find or test such a setting: UNVERIFIED), or move `install.sh`, binstall metadata and `ostler.js` to cargo-dist's names and keep serving the old names for two releases;
   - keep the tag-versus-`Cargo.toml` check (cargo-dist's `plan` job is not the same check; confirm it fails a mismatched tag);
   - decide on Windows: cargo-dist added `x86_64-pc-windows-msvc` by default, and RELEASING.md says Windows is not built or tested; remove the target unless we mean to support it;
   - leave `cargo publish` and the npm publish as the documented manual steps; cargo-dist does not do them.
3. Run `dist plan` in CI on pull requests so release-config drift is caught before a tag.
4. cargo-dist's own installers, Homebrew tap and npm package are opt-in config; turn each on only when its replacement of an existing path is tested (`scripts/test-installers.sh`).

## Rejected alternatives
- *Keep hand-rolled scripts indefinitely.* Fine at four targets; the plan adds installers, a Homebrew formula and signing/attestation work that cargo-dist already covers.
- *Adopt now, during M0.* A rename of release assets breaks `curl | sh`, `cargo binstall` and `npx` for the first release made with it unless all three are updated together; M0 should not risk the install path.
- *`cargo-release` or `release-plz`.* Version and changelog automation, not binaries and installers; not evaluated.

## Consequences
- cargo-dist **regenerates and owns** `release.yml`: hand edits are overwritten by `dist generate`, so our extra steps must be expressed in its config or in separate workflows.
- A new workflow file triggers on pull requests as well as tags; pull requests get a plan-only run.
- `Cargo.toml` gains a `[profile.dist]` (inherits `release`, thin LTO) and the repo a `dist-workspace.toml`.
- RELEASING.md step 3 ("wait for the Release workflow") stays true; the asset list in the release changes.

## Verification
Run 2026-11-06 on a **copy** of the repo outside the working tree (nothing was changed in the repository).

| claim | how verified | result |
|---|---|---|
| version, licence | crates.io API; `cargo install cargo-dist --locked` | 0.34.0 (2026-11-02), `MIT OR Apache-2.0`, MSRV 1.74; installs the binary `dist`; 4m35s to build here |
| what `init` generates | `dist init --yes` in the copy | `dist-workspace.toml` (`ci = "github"`, `installers = []`, five targets: aarch64 and x86_64 macOS, aarch64 and x86_64 Linux gnu, **x86_64 Windows msvc**), `[profile.dist]` added to `Cargo.toml`, and `.github/workflows/release.yml` (296 lines) **replacing ours** |
| triggers | read the generated workflow | `on: pull_request` and `push: tags: '**[0-9]+.[0-9]+.[0-9]+*'`; jobs `plan`, `build-local-artifacts`, `build-global-artifacts`, `host`, `announce` |
| artifacts | `dist plan` | `ostler-<target>.tar.xz` (binary plus `CHANGELOG.md`, `LICENSE`, `README.md`) with a `.sha256` each, `ostler-x86_64-pc-windows-msvc.zip`, `source.tar.gz`, `sha256.sum`. **Not** `ostler-v<version>-<target>.tar.gz` |
| conflict with our consumers | grep of `site/install.sh`, `crates/ostler-cli/Cargo.toml`, `npm/bin/ostler.js`, `scripts/package.sh` | all four assume the `ostler-v<version>-<target>.tar.gz` layout |
| cargo-dist and publishing | read the generated plan and workflow | no `cargo publish` or `npm publish` step |

**Not verified (UNVERIFIED):** a real run of the generated workflow (needs GitHub Actions on a fork or scratch repo); whether archive naming, archive format and inner directory can be customised; the shell, Homebrew and npm installers it can generate; whether the workflow fits the macOS runner images in the current matrix (`macos-13` is in ours); signing or attestations; how it handles the eight-crate workspace where only `ostler` ships a binary (the plan found one app, `ostler 0.2.0`, which is the expectation).

**Dry-run checklist before v0.4:** scratch repo, push a `v0.0.1-rc.1` tag, check the generated assets, run `scripts/test-installers.sh` against them, point `install.sh`, binstall and `ostler.js` at the new names, and record the time from tag to release.
