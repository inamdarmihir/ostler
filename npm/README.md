# ostler

A browser for AI agents (MCP). Fills a whole form in one call, reads every value back to check it, and learns from each fill. Optional [Qdrant](https://qdrant.tech) memory.

This npm package is a thin launcher: on first run it downloads the prebuilt `ostler` binary for your platform (macOS and Linux, x64 and arm64), verifies its sha256, and caches it in `~/.cache/ostler`. No install scripts run.

```sh
npx -y ostler doctor                      # checks that Chrome launches
claude mcp add ostler -- npx -y ostler serve
```

Needs Chrome or Chromium installed. Full docs, tools and safety notes: https://github.com/inamdarmihir/ostler

Environment: `OSTLER_BINARY` (use this binary instead), `OSTLER_CACHE_DIR`, `OSTLER_RELEASE_BASE`.
