# Compatibility and versioning policy

Ostler is pre-1.0 (current workspace version in `Cargo.toml`). This page says what callers may rely on and how changes are announced. Where a rule is a commitment going forward rather than something enforced by tooling, it says so.

## What is public surface

| surface | examples | covered by this policy |
|---|---|---|
| MCP tool names | `navigate`, `form_fill`, `form_submit`, ... | yes |
| Tool input schemas | argument names, types, required set, enum values | yes |
| Tool output format | the text of outcomes and notices that agents act on: field statuses (`Verified`, `Reformatted`, `Mismatch`, ...), submit outcomes (`Submitted`, `Advanced`, `ValidationFailed`, `Unchanged`, `ServerError`), `CHALLENGE:` notice, `<page_content untrusted="true">` wrapper | the vocabulary yes; exact prose and whitespace no |
| A2A | agent card, task states, data-part request shape (`url`, `values`, `auto`, `submit`, `confirm`) | yes |
| CLI | subcommands, flags, environment variables (`OSTLER_*`), config keys | yes |
| On-disk and on-wire formats | sealed payload format (`v2`), `memory.key`, `memory.local`, Qdrant collection names | yes, with migration |
| Internals | Rust crate APIs, `runtime.js` internals, snapshot layout and ref numbering | no; crates are published for the CLI, not as libraries |

## Semver policy

- Before 1.0: a **minor** bump (0.x to 0.y) may break the public surface, but only with a CHANGELOG entry that says "Breaking" and gives the replacement. A **patch** bump never breaks it.
- From 1.0: breaking changes only in a major release.
- Always allowed in any release: adding a tool, adding an optional argument, adding a new status/notice value, adding a new profile entry. Agents must ignore unknown fields and treat unknown statuses as "not verified".
- Never allowed silently: removing or renaming a tool, making an optional argument required, changing a status word's meaning, weakening a safety default (for example opening an allowlist or turning a gate off). Safety-default changes are called out in the CHANGELOG and in SECURITY.md.
- Tightening safety (refusing more) is allowed in a minor release even if it breaks a workflow, with a CHANGELOG note and an opt-out flag where one is reasonable (precedent: `--accept-dialogs`, `--block-private-networks`, `--allow-insecure-qdrant`).

## Deprecation

1. Announce in the CHANGELOG and, for a tool or flag, keep the old name working and mark it in its description.
2. Keep it for at least **one minor release** (and no less than 30 days) before removal.
3. The tool-count budget ([SYSTEM-DESIGN.md](SYSTEM-DESIGN.md) s5) means a removal may be paired with an addition in the same release.

Status: this is a commitment from this release on; there are no deprecated items yet.

## Data format migrations

The sealed format has already moved (`v1` to `v2`). Rule: a build refuses to start rather than guess when stored data needs migration, and names the command (`ostler memory seal`). Migrations never silently drop user facts; learned skills and episodes may be relearned (stated in the CHANGELOG when so).

## MSRV

Rust **1.89** (`rust-version` in the workspace `Cargo.toml`; the `msrv` CI job runs `cargo check --workspace --locked` on 1.89). Policy: raising the MSRV is allowed in a minor release with a CHANGELOG note; it is never raised in a patch release. CI builds and tests with `stable`.

## Platform and browser matrix (only what CI or tests cover)

| platform | build | tests | note |
|---|---|---|---|
| Linux x86_64 (Ubuntu runner) | CI | CI runs `cargo test --workspace` with Chrome and a pinned Qdrant; `weekly.yml` reruns the Chrome suites against stable Chrome | the supported test platform |
| macOS (arm64) | release build (`macos-14`) | developed and benchmarked locally; not run in CI tests | numbers in BENCHMARKS are from macOS |
| macOS (x86_64) | release build (`macos-13`) | none | build only |
| Linux arm64 | release build (`ubuntu-22.04-arm`) | none | build only |
| Windows (x86_64) | built in CI | CI build and test suite | experimental |

| dependency | tested | policy |
|---|---|---|
| Chrome / Chromium | the stable channel installed by `browser-actions/setup-chrome` in CI and weekly; a Chrome with `--remote-debugging-pipe` support | newer stable should work; the weekly job is the early warning. No older-version matrix is tested |
| Other CDP engines | none | planned backend only (ADR-0001) |
| Qdrant | the version pinned in `scripts/ci-install-qdrant.sh` (sha256 verified), plus a JWT-RBAC run for tenant tokens (Qdrant 1.20) | other versions untested |
| Ollama | manual | `--embeddings ollama` only needs the embeddings endpoint |
| MCP clients | Claude Code, Claude Desktop, Cursor, VS Code, Windsurf, Codex config writers are tested by `ostler install` tests; behaviour inside each client is not | see [CLIENTS.md](CLIENTS.md) |
| A2A | protocol v1.0 (JSON-RPC over HTTP, SSE); official Python SDK checked manually once | v0.3 wire format, gRPC, REST bindings not supported |
