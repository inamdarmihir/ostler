# ostler

An open-source MCP browser for AI coding agents (Claude Code, Codex, Cursor) that checks its own work. Research answers cite only pages it read, with every quote verified; forms are filled in one call and every value is read back. Optional [Qdrant](https://qdrant.tech) memory.

This npm package is a thin launcher: on first run it downloads the prebuilt `ostler` binary for your platform (macOS and Linux, x64 and arm64), verifies its sha256, and caches it in `~/.cache/ostler`. No install scripts run.

```sh
npx -y ostler doctor                      # checks that Chrome launches
claude mcp add ostler -- npx -y ostler serve
```

Needs Chrome or Chromium installed. Full docs, tools and safety notes: https://github.com/inamdarmihir/ostler

Environment: `OSTLER_BINARY` (use this binary instead), `OSTLER_CACHE_DIR`, `OSTLER_RELEASE_BASE`.
