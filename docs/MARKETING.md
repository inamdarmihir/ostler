# Marketing plan

Rule for everything below: say only what the benchmarks and tests show. The headline "3 calls vs 22" is a scripted comparison on local fixtures, not a real-model result on real sites, and every post must say so. The credibility of the project is the honest caveats, so lead with them.

## Positioning

**One line:** A browser for AI agents that fills forms, checks what it wrote, and remembers.

**Who it is for:** people running Claude Code, Codex or Cursor who watch the agent burn twenty turns on a signup form, and people building agents that need to hand a form to something reliable (A2A).

**Why it is different** (in the order to use them):
1. Whole form in one call, with every value read back. Wrong or missing fields are reported, never guessed.
2. `look`: numbered click targets drawn from the DOM, so the agent knows where to click, and anything hidden under an overlay is excluded.
3. Catches errors that only exist visually (red border, red text, toast).
4. Remembers with Qdrant: profile facts, per-site playbooks, past failures.
5. MCP and A2A in one Rust binary.

**Do not claim:** real-site success rates, "faster than X" for any named product, or anything about Lightpanda support.

## Assets (all in `site/`)

| asset | file |
|---|---|
| Icon (SVG, 128 grid) | `assets/logo.svg`, `site/favicon.svg` |
| Icon PNG 180 / 512 | `site/assets/icon-180.png`, `icon-512.png` |
| Social card 1200x630 | `site/assets/og.png` (also used for link previews) |
| Site | https://inamdarmihir.github.io/ostler/ |

The mark is a horseshoe (an ostler tends horses) with a verified check inside. Orange `#fb923c`, teal `#5eead4`, ink `#14110f`.

The README demo is `assets/demo.gif` (5 steps, 960 px, under 100 KB): real tool output from `overlay.html` and `validation.html` (`look` under a cookie wall, a `form_fill` the page rejects, the corrected fill, `form_submit`), with each caption copied from the tool's own reply. Re-record it if those replies change. Still to do by hand: GitHub **social preview** (Settings → General → Social preview: upload `site/assets/og.png`; there is no API for it). A longer screen recording is optional.

## Channels, in order

1. **Demo first.** One GIF in the README: agent sees the numbered page, closes the cookie banner, fills the form in one call, reads back an error. Everything else links to it.
2. **Show HN**: "Show HN: Ostler, a Rust headless browser for coding agents that verifies every form field". First comment: what is measured, what is not, how to reproduce (`ostler bench`). Post on a weekday morning US time. Reply to every comment for the first three hours.
3. **Qdrant**: the memory design is a real showcase (multi-tenant collection, payload filters, Cloud Inference). Pitch a guest post or a community-showcase entry, and post in their Discord. Offer the memory benchmark and its limits.
4. **MCP and Claude communities**: the MCP servers list, the Claude Code Discord, r/ClaudeAI, r/LocalLLaMA, r/rust. Tailor: for r/rust talk about the hand-written CDP client and the in-page runtime; for agents talk about `look` and error cues.
5. **Directories**: mcp.so, Smithery, the official MCP servers registry, awesome-mcp-servers, awesome-rust, A2A project's community list.
6. **Write-ups** (each stands alone, each links the repo):
   - "Why agents fail at forms: eleven failure modes and the fixtures that reproduce them"
   - "Verifying a form fill by reading values back"
   - "Set-of-marks from the DOM instead of vision"
   - "What our own design review found" (the review is already in `docs/REVIEW-2026-10.md`; honest post-mortems travel well)
7. **X / LinkedIn / Bluesky**: the GIF plus one sentence. Tag the Qdrant and MCP accounts only when the post is useful to their audience.

## Launch checklist

- [ ] Prebuilt binaries (macOS arm64/x64, Linux) so install is not `cargo install`
- [ ] `brew` tap or `cargo binstall` metadata
- [x] README GIFs (`assets/demo.gif`: browse and forms; `assets/demo-research.gif`: read, cite, evidence). Both are real tool replies and real page screenshots. [ ] Social preview uploaded (manual, in repo settings)
- [x] `glama.json` in the repo root (maintainer `inamdarmihir`). [ ] Add the server on glama.ai and press Claim ownership. Smithery: its publishing docs could not be fetched when this was checked (2026-11-06), so no `smithery.yaml` was written; read their current docs before submitting, and note Ostler needs a local Chrome, so a hosted listing is not a fit.
- [x] Python framework docs (LangChain, LlamaIndex, AutoGen) with snippets that were run: `site/tools/content/frameworks.md`
- [x] Real-site check published, including what it found (`ostler-bench real`, [BENCHMARKS §7](BENCHMARKS.md#7-real-site-evaluation-fill-only-no-llm)): 7 practice pages, 44/45 fields, the 2 false claims it caught fixed. **Still to do for a stronger claim:** a wider set of real (not practice) sites, with failures
- [ ] LLM-in-the-loop benchmark against Playwright MCP and chrome-devtools-mcp
- [ ] Close the open safety items listed in the design review status table (uploads, DevTools port, default allowlist, `confirm()`)
- [ ] GitHub Discussions on, `good first issue` labels, a roadmap issue

## Measuring

Stars are noise. Track: installs from release downloads, issues opened by people who are not us, and whether anyone reproduces the benchmark.
