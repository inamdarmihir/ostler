# Use Ostler from LangChain, LlamaIndex and AutoGen

Ostler is an MCP server, so any agent framework with an MCP adapter can use its browser, research and form tools. This page shows the three popular Python frameworks, with the code that was run and the output it produced.

**What was tested, and what was not.** Each snippet below was run against Ostler 0.2.0 and the bundled demo page (`ostler fixtures`): the adapter connects over stdio, loads the tools, and calls `read` then `cite` directly. **No language model was in the loop**, so the step that hands the tools to an agent is shown as documented usage and marked as not run.

## The one rule: keep one session open

`cite` only accepts a quote from a page that `read` fetched **in the same Ostler session**. A session is one running Ostler process. Adapters differ in whether they keep it:

| framework | default behaviour | what to do |
|---|---|---|
| LangChain (`langchain-mcp-adapters`) | `client.get_tools()` starts a new Ostler process per tool call, so `cite` returns `NOT READ` | open `client.session("ostler")` and use `load_mcp_tools(session)` |
| LlamaIndex (`llama-index-tools-mcp`) | `BasicMCPClient` starts a new process per call, so `cite` returns `NOT READ` | open an `mcp.ClientSession` yourself and pass it to `McpToolSpec` |
| AutoGen (`autogen-ext[mcp]`) | `McpWorkbench` as a context manager keeps one session | use `async with McpWorkbench(params)` |

Both failures were observed, not assumed: the `NOT READ` reply is `NOT READ: <url> was not read in this session ... call read first`. It is Ostler refusing to certify a quote from a page it did not read in that session, which is the point of `cite`. Run the agent inside the `async with` block; when the block exits, the Ostler process and its Chrome end and the read history is gone.

Every example assumes `ostler` is on your `PATH` (see [install](install.html)) and the demo pages are being served with `ostler fixtures`. `--tools research` pins the 21 research-profile tools; use `--tools all` for all 30 ([profiles](tools.html)).

## LangChain

```sh
pip install langchain-mcp-adapters      # tested: 0.3.2, with mcp 1.30.0
```

```python
import asyncio
from langchain_mcp_adapters.client import MultiServerMCPClient
from langchain_mcp_adapters.tools import load_mcp_tools

URL = "http://127.0.0.1:8787/packs/research/kestrel-lock.html"
client = MultiServerMCPClient({"ostler": {"transport": "stdio", "command": "ostler",
                                          "args": ["serve", "--tools", "research"]}})

async def main():
    async with client.session("ostler") as session:  # one session = one Ostler process
        tools = {t.name: t for t in await load_mcp_tools(session)}
        print("loaded:", len(tools), list(tools)[:8])
        await tools["read"].ainvoke({"url": URL})
        for quote in ["It opened in 1887 and was designed by Marta Ilves",
                      "Tomas Reyes designed the Kestrel Lock in 1887"]:
            r = await tools["cite"].ainvoke({"url": URL, "quote": quote})
            print(r[0]["text"][:90].replace("\n", " "))

asyncio.run(main())
```

Output (the second quote is a decoy that is not on the page):

```
loaded: 21 ['navigate', 'snapshot', 'read', 'click', 'type', 'press', 'hover', 'scroll']
VERIFIED [1] <page_content untrusted="true">http://127.0.0.1:8787/packs/research/kestrel-l
NOT VERIFIED: these words are not on <page_content untrusted="true">The Kestrel Lock</page
```

Tool results come back as a list of content blocks, so the text is `r[0]["text"]`. To hand the tools to an agent, keep the agent call inside the same `async with` block. **Not run** (no model available here; `langchain` and a model package are extra installs):

```python
from langchain.agents import create_agent
async with client.session("ostler") as session:
    agent = create_agent("anthropic:claude-sonnet-4-5", await load_mcp_tools(session))
    await agent.ainvoke({"messages": "Who designed the Kestrel Lock? Cite the exact quote."})
```

## LlamaIndex

```sh
pip install llama-index-tools-mcp       # tested: 0.6.0, with mcp 2.3.0
```

```python
import asyncio
from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client
from llama_index.tools.mcp import McpToolSpec

URL = "http://127.0.0.1:8787/packs/research/kestrel-lock.html"
SERVER = StdioServerParameters(command="ostler", args=["serve", "--tools", "research"])

async def main():
    async with stdio_client(SERVER) as (r, w), ClientSession(r, w) as session:  # one Ostler process
        await session.initialize()
        tools = {t.metadata.name: t for t in await McpToolSpec(session).to_tool_list_async()}
        print("loaded:", len(tools), list(tools)[:8])
        await tools["read"].acall(url=URL)
        for quote in ["It opened in 1887 and was designed by Marta Ilves",
                      "Tomas Reyes designed the Kestrel Lock in 1887"]:
            out = await tools["cite"].acall(url=URL, quote=quote)
            print(out.raw_output.content[0].text[:90].replace("\n", " "))

asyncio.run(main())
```

Output:

```
loaded: 21 ['navigate', 'snapshot', 'read', 'click', 'type', 'press', 'hover', 'scroll']
VERIFIED [1] <page_content untrusted="true">http://127.0.0.1:8787/packs/research/kestrel-l
NOT VERIFIED: these words are not on <page_content untrusted="true">The Kestrel Lock</page
```

The clean text of a result is `out.raw_output.content[0].text`; `out.content` is the noisy `str()` of the whole result. Pass `url` to `cite` explicitly, since it is required. To use the tools in an agent, build a `FunctionAgent(tools=list(tools.values()), llm=...)` and run it inside the same session block. **Not run** (no model).

## AutoGen

```sh
pip install "autogen-ext[mcp]" autogen-agentchat "mcp<2"   # tested: autogen-ext 0.7.5, mcp 1.30.0
```

The `mcp<2` pin is needed: with `mcp` 2.3.0 installed, `from autogen_ext.tools.mcp import ...` failed in testing with `ImportError: cannot import name 'RequestContext' from 'mcp.shared.context'`.

```python
import asyncio
from autogen_ext.tools.mcp import McpWorkbench, StdioServerParams

URL = "http://127.0.0.1:8787/packs/research/kestrel-lock.html"
params = StdioServerParams(command="ostler", args=["serve", "--tools", "research"],
                           read_timeout_seconds=60)

async def main():
    async with McpWorkbench(params) as wb:  # one session = one Ostler process
        tools = await wb.list_tools()
        print("loaded:", len(tools), [t["name"] for t in tools[:8]])
        await wb.call_tool("read", {"url": URL})
        for quote in ["It opened in 1887 and was designed by Marta Ilves",
                      "Tomas Reyes designed the Kestrel Lock in 1887"]:
            r = await wb.call_tool("cite", {"url": URL, "quote": quote})
            print(r.to_text()[:90].replace("\n", " "))

asyncio.run(main())
```

Output:

```
loaded: 21 ['navigate', 'snapshot', 'read', 'click', 'type', 'press', 'hover', 'scroll']
VERIFIED [1] <page_content untrusted="true">http://127.0.0.1:8787/packs/research/kestrel-l
NOT VERIFIED: these words are not on <page_content untrusted="true">The Kestrel Lock</page
```

A `NOT VERIFIED` result comes back with `is_error = True` on the `ToolResult` rather than as an exception. `AssistantAgent` takes the workbench through its `workbench` parameter (the parameter exists; the model call was **not run**):

```python
async with McpWorkbench(params) as wb:
    agent = AssistantAgent("researcher", model_client=model_client, workbench=wb)
    await agent.run(task="Read the Kestrel Lock page and cite who designed it.")
```

## Other frameworks

Anything that speaks MCP over stdio works the same way: launch `ostler serve --tools research` (or `forms`, or `all`), keep the process for the whole task, and give the model the tools. For Claude Code, Codex, Cursor and VS Code, see [use with your agent](agents.html). Not tested here: CrewAI, the OpenAI Agents SDK, Semantic Kernel, and non-Python frameworks.
