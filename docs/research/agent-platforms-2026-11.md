# Agent platforms and browser agents (snapshot 2026-11-08)

Background for [AGENTIC-PLAN.md](../AGENTIC-PLAN.md). Facts come from the vendor's own page where one is linked; rows marked **secondary** come from third-party write-ups and need a primary source before they are quoted anywhere public. Products in this space change monthly: re-check before relying on any row.

## 1. Anthropic

### 1.1 The browser use tool in the Claude API (`browser_toolset_20260801`)

Source: [Browser use tool](https://platform.claude.com/docs/en/agents-and-tools/tool-use/browser-use-tool) (Claude Platform docs).

- A **client toolset**: one `tools` entry gives Claude 27 member tools, plus four optional ones (`javascript_exec`, `file_upload`, `read_console`, `read_network`) that are off by default. **The application runs every call against its own browser automation; nothing runs on Anthropic's side.** The docs call that part of the application the *executor*.
- Members: `navigate`, `screenshot`, `zoom`; pointer actions (`left_click`, `double_click`, `left_click_drag`, `scroll`, `scroll_to`, ...); `type`, `key`, `hold_key`, `wait`; `read_page` (accessibility tree with `[ref_N]` tags, `filter: interactive|all`, `depth`, `ref` subtree, capped at 50,000 characters), `find` (natural-language element search, up to 20 matches), `get_page_text`; `form_input`, `file_upload`; tab management that returns `browser_state` blocks.
- Targets are either viewport coordinates or element references. The executor assigns references, keeps the mapping, and must return an error for a stale reference rather than renumber.
- **Batch actions**: several member calls in one turn run in order; at the first failure every later call gets the exact text `Not executed: an earlier action in this turn failed.`
- Classifiers scan what the browser returns (page text, screenshots) for prompt injection. The tool is not yet available in Claude Managed Agents.

What it means for Ostler: Anthropic defined the agent-facing surface and left the executor to everyone else. An executor that verifies what it does is an open slot.

### 1.2 Claude Cowork and its built-in browser

Sources: [Get started with Claude Cowork](https://support.claude.com/en/articles/13345190-get-started-with-claude-cowork), [Let Claude use your computer in Cowork](https://support.claude.com/en/articles/14128542-let-claude-use-your-computer-in-cowork), [Use the built-in browser in Claude Cowork](https://support.claude.com/en/articles/16607400-use-the-built-in-browser-in-claude-cowork), [Use plugins in Claude](https://support.claude.com/en/articles/13837440-use-plugins-in-claude), [Claude Cowork product page](https://claude.com/product/cowork).

- Cowork uses the same agentic architecture as Claude Code. Sessions run in the cloud and reach local files, the browser and apps through the desktop app.
- **Tool precedence**: connectors first (fastest, most reliable), then the browser, then computer use (screen control) only when nothing more precise exists.
- The built-in browser is **separate from the user's browser**: its own profile, opened in a side panel the user can watch. Logins can be imported **site by site**; banking, email and single sign-on sites stay unchecked by default. Claude asks permission before acting on a site for the first time. High-risk sites are blocked.
- **Plugins** bundle skills, connectors (MCP) and sub-agents into one installable package per role.

### 1.3 Engineering principles

| source | the idea that matters here |
|---|---|
| [Scaling Managed Agents with stable interfaces](https://www.anthropic.com/research/managed-agents) | Session (append-only event log), harness (the loop) and sandbox (the hands) are decoupled behind small interfaces: `execute(name, input) -> string`, `provision`, `wake(sessionId)`, `getSession`, `emitEvent`, `getEvents`. "The session is not Claude's context window." A crashed harness resumes from the log; a failed container is cattle, surfaced as a tool error. Credentials never enter the sandbox: they are bundled at provisioning or fetched by a proxy from a vault. |
| [Effective harnesses for long-running agents](https://www.anthropic.com/research/effective-harnesses-for-long-running-agents), [Harness design for long-running apps](https://www.anthropic.com/research/harness-design-long-running-apps) | Make incremental progress per session and leave clear artifacts for the next one; a separate evaluator makes quality gradable; start simple and add complexity only when needed. |
| [Effective context engineering](https://www.anthropic.com/research/effective-context-engineering-for-ai-agents) | Context is a budget; keep bulk data out of it. Already adopted in ADR-0019 together with "Code execution with MCP". |
| [Agent Skills](https://www.anthropic.com/research/equipping-agents-for-the-real-world-with-agent-skills) | Folders of instructions, scripts and resources loaded on demand. |
| [Trustworthy agents in practice](https://www.anthropic.com/research/trustworthy-agents) | Five principles: humans in control, alignment with values, secure interactions, transparency, privacy. Mechanisms: per-action permissions (always allow, needs approval, block), plan mode before execution, agents trained to pause rather than assume. |
| [Mitigating prompt injection in browser use](https://www.anthropic.com/research/prompt-injection-defenses), Claude Opus 5.5 system card section 5.2 | Browser use amplifies injection risk because pages are untrusted and actions are many. Defence in two independent checkpoints: probes on **data coming in** and a classifier on **actions going out**, so an attack must defeat both. Anthropic red-teamed browser-specific vectors: hidden DOM form fields, URL text and tab titles only an agent sees. |

## 2. Cloudflare

| source | facts |
|---|---|
| [Browser Run: give your agents a browser](https://blog.cloudflare.com/browser-run-for-ai-agents/) (2026-04-15) | Browser Rendering renamed Browser Run. Structured as five needs: browsers on demand, control (Puppeteer, Playwright, raw **CDP endpoint**, MCP clients via `chrome-devtools-mcp --wsEndpoint`, **WebMCP**), observability (**Live View** with DOM, console and network; **Session Recordings** as rrweb JSON), **Human in the Loop** (take over a live session; an agent-initiated handoff signal is "next"), and scale (120 concurrent browsers). The `/crawl` endpoint is a **signed agent** (Web Bot Auth), has a fixed User-Agent, follows robots.txt and AI Crawl Control, and does not bypass bot protection or CAPTCHAs. |
| [Introducing Kitesurf](https://blog.cloudflare.com/kitesurf/) (2026-08-06) | A browser built for agents on Workers, speaking CDP so existing clients work. Design rules: every page load is untrusted input and every session starts fresh; **one component (SandboxOutbound) owns all network access** and enforces policy; "any failure degrades to a blank frame or a missing element, never a dead session"; stateless wherever possible. Against warm Chromium on 14 URLs: 3-7x less CPU and memory, about 1.7x slower wall time. Not for persistent authenticated sessions, video, WebGL or bot-challenge handshakes. To be open-sourced. |
| [See the agents, set the terms](https://blog.cloudflare.com/personal-agents-visibility-control/) (2026-11-04), [Agents aren't humans (but they kinda are)](https://blog.cloudflare.com/trusted-agent-delegation/) (2026-11-05) | Site owners now get per-agent analytics and allow/block policies for **personal agents**. The open problem: sites lose sight of the user behind an agent (account sharing, compliance, fraud, risk-based auth). Proposed direction: Web Bot Auth identity plus explicit site-agent protocols; a Trusted Agent Delegation coalition. |
| [Agents docs](https://developers.cloudflare.com/agents/) | The harness runs in a Durable Object with state, WebSockets and scheduling. |
| Code Mode | Covered in ADR-0019. |

## 3. WebMCP

Sources: [WebMCP, Chrome for Developers](https://developer.chrome.com/docs/ai/webmcp); [What is WebMCP](https://nohacks.co/blog/what-is-webmcp) (**secondary**).

- Sites register tools with `document.modelContext.registerTool({name, description, inputSchema, execute})`, or annotate a `<form>` with `toolname`, `tooldescription` and per-field `toolparamdescription`.
- Status (**secondary**): Chrome origin trial from 149; `navigator.modelContext` deprecated in favour of `document.modelContext` in Chromium 150; local testing behind `chrome://flags/#enable-webmcp-testing`.
- Tools are discovered on the page, not preloaded, which suits the long tail of sites. Tool names and descriptions are written by the site and are therefore untrusted text.

## 4. Consumer agent browsers

All **secondary** ([tech-insider](https://tech-insider.org/comet-vs-gemini-agent-vs-chatgpt-atlas-2026/), [citeme](https://www.citeme.io/ressources/best-ai-browsers-2026-compared), [HUMAN Security](https://www.humansecurity.com/learn/blog/chatgpt-atlas-vs-perplexity-comet-agentic-browsers/)).

| product | shape | note |
|---|---|---|
| ChatGPT Atlas | standalone Chromium browser | Retired 2026-08-09; browsing folded into the ChatGPT desktop app. |
| Perplexity Comet | standalone browser, all platforms | Research-first, answers with sources. |
| Gemini in Chrome, auto browse | agent inside the existing browser | Project Mariner folded into it (2026-05). Paid plans, US first. |
| Claude in Chrome | extension, now a Cowork session in the side panel | Works with the user's own logins. |
| Edge Copilot Mode | built into Edge | Strongest admin controls for companies. |

Pattern: the category is consolidating on **an agent layer inside a browser people already use**, not a new browser. HUMAN found Atlas and Comet indistinguishable from Chrome by User-Agent and Client Hints: these agents do not identify themselves.

## 5. Developer frameworks and infrastructure

| project | what it does that matters here |
|---|---|
| Browserbase + Stagehand | [Four autonomy levels](https://www.browserbase.com/blog/browser-agent-autonomy-levels): AI as helper inside code, up to a fully autonomous agent. Stagehand caches actions and "self-heals" steps when the LLM adapts. Live view, replays, logs. |
| Browser Use | Open-source Python agent plus a cloud; [CAPTCHA solving on by default in the cloud](https://browser-use.com/posts/best-browser-infrastructure-for-ai-agents). |
| Skyvern | Planner, actor and validator; workflows with audit trails and approval gates; AGPL. |
| Notte and others (**secondary**, [r/LocalLLaMA](https://www.reddit.com/r/LocalLLaMA/comments/1r3ovso/whats_the_workflow_for_browser_automation_in_2026/)) | "Let an agent figure out the flow once, then convert it to a script." |
| Playwright MCP, chrome-devtools-mcp, agent-browser | See [landscape.md](landscape.md). chrome-devtools-mcp is now also Cloudflare's recommended MCP front end for remote browsers. |

## 6. What nobody in this list does

1. Return a **verdict** on the outcome (did the order go through, does the page say what was quoted) rather than a new snapshot for the model to interpret.
2. Offer an **open, local, verified executor** for Anthropic's browser toolset.
3. Keep a **replayable record with verdicts** on a user's own machine, not in a vendor dashboard.
4. Combine **WebMCP site tools with outcome checks**: a site-declared tool is called, and the result is then checked against the page.
5. Identify itself honestly as a user-delegated agent from a local machine (signed), while refusing stealth.
