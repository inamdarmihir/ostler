# Agent frameworks

Ostler's tools load into LangChain, LlamaIndex and AutoGen through their MCP adapters. The code here is the code from the [frameworks guide](https://inamdarmihir.github.io/ostler/docs/frameworks.html), where it was run against Ostler 0.2.0 with no language model in the loop. It was not re-run for this repository.

**The one rule:** keep one Ostler session open for the whole task, or `cite` answers `NOT READ` for pages that `read` fetched in a different process.

| file | install | keeps one session by |
|---|---|---|
| [langchain_read_cite.py](langchain_read_cite.py) | `pip install langchain-mcp-adapters` | `client.session("ostler")` and `load_mcp_tools(session)` |
| [llamaindex_read_cite.py](llamaindex_read_cite.py) | `pip install llama-index-tools-mcp` | your own `mcp.ClientSession` passed to `McpToolSpec` |
| [autogen_read_cite.py](autogen_read_cite.py) | `pip install "autogen-ext[mcp]" autogen-agentchat "mcp<2"` | `async with McpWorkbench(params)` |

Run any of them with `ostler fixtures` serving. To hand the tools to an agent, create the agent inside the same `async with` block.
