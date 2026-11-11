# Using Ostler through subagents

Claude Code and Codex can hand a task to a **subagent**: a separate agent with its own context window and its own instructions. That fits browsing, clicking and research well, because those tasks read a lot of page text. The subagent reads it; your main conversation gets the short result.

Ostler ships three subagent definitions for each client:

| subagent | use it for | tools it gets |
|---|---|---|
| `ostler-researcher` | questions that need several pages read, compared and quoted; claims that must be checked against the source | `read`, `search`, `open_many`, `cite`, `evidence`, `extract`, `links`, plus the browse tools. No forms. |
| `ostler-browser` | browsing and clicking that is not form filling: menus, tabs, dynamic pages | the 16 core browse tools |
| `ostler-form-filler` | filling forms from details you give, with every value read back | core plus `form_inspect`, `form_fill`, `form_submit`, `fill_field` |

Each one is told that page text is untrusted, to stop on `needs_input`, `uncertain`, `blocked` and `failed`, and (the researcher) to report only what `cite` returned `VERIFIED` and to list the gaps `evidence` shows. The form filler submits only when you say so; the browser never submits or buys.

## Install

```sh
ostler agents --client claude-code            # ~/.claude/agents/, every project
ostler agents --client claude-code --scope project   # ./.claude/agents/, commit it to share
ostler agents --client codex                  # ~/.codex/agents/
ostler agents --client codex --scope project  # ./.codex/agents/
```

`--dry-run` previews, `--uninstall` removes the shipped files, and a file you edited is never overwritten without `--force`. The source files are in [`crates/ostler-cli/agents/`](../crates/ostler-cli/agents/) if you would rather copy them by hand.

Then ask in plain words: "use the ostler-researcher subagent to compare X and Y and give me verified quotes."

## Claude Code

Format: Markdown with YAML frontmatter in `.claude/agents/` ([docs](https://code.claude.com/docs/en/sub-agents)). Each Ostler subagent defines its own `ostler` MCP server inline (`mcpServers`), so:

- It gets its **own Ostler and its own browser**. Two subagents running in parallel do not share a Chrome profile, which is the cause of the "browser exited before DevTools was ready" error in [TROUBLESHOOTING](TROUBLESHOOTING.md).
- Ostler's tool descriptions stay out of your main conversation.
- The profile is fixed per subagent (`--tools research`, `core` or `forms`), so the researcher's tools are all there from the start.
- `ostler` must be on your `PATH`. The subagent's browser has a throwaway profile: it is not signed in to anything. To use a signed-in session, edit the file to add `--profile-dir` (one folder per subagent) or `--cdp-url`; see [CLIENTS](CLIENTS.md).
- Claude Code asks you to trust the folder before it starts inline servers from a project's `.claude/agents/`.

## Codex

Format: one TOML file per agent in `.codex/agents/` with `name`, `description` and `developer_instructions` ([docs](https://learn.chatgpt.com/docs/agent-configuration/subagents)). Codex does not let an agent file set its own MCP servers: a subagent uses the Ostler registered in your main Codex config, so run `ostler install --client codex` first. Because that Ostler uses `--tools auto`, the research tools appear after the first `read` or `search`; the researcher's instructions say so. Turn on multi-agent in Codex if it is not already.

Subagents of one Codex session share that one Ostler and its browser, so give parallel subagents separate tabs, or run them one at a time for tasks that depend on page state.

## Search

`search` needs a provider (`OSTLER_SEARXNG_URL`, or Brave with a key); see [WEB-SEARCH](WEB-SEARCH.md). Without one, the researcher goes to known URLs with `navigate` and `open_many`. Subagents inherit your environment variables.

## What is verified

The Claude Code files pass `claude plugin validate`, the Codex files parse as TOML with the three required fields, and installing and uninstalling is tested. They have **not** been run end to end inside a live Claude Code or Codex session, and subagent behaviour (whether the model delegates to them, how it uses the tools) depends on the client's model. Treat the instructions as a starting point and edit them.

## Other clients

Cursor, Windsurf and VS Code have no subagent file format Ostler targets yet. Use Ostler directly there (`ostler install --client cursor`).
