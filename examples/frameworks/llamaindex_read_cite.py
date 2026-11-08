"""LlamaIndex: pass your own MCP session to McpToolSpec so read and cite share one Ostler process."""

import asyncio

from llama_index.tools.mcp import McpToolSpec
from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

URL = "http://127.0.0.1:8787/packs/research/kestrel-lock.html"
SERVER = StdioServerParameters(command="ostler", args=["serve", "--tools", "research"])


async def main() -> None:
    async with stdio_client(SERVER) as (read, write), ClientSession(read, write) as session:
        await session.initialize()
        tools = {tool.metadata.name: tool for tool in await McpToolSpec(session).to_tool_list_async()}
        await tools["read"].acall(url=URL)
        for quote in [
            "It opened in 1887 and was designed by Marta Ilves",
            "Tomas Reyes designed the Kestrel Lock in 1887",
        ]:
            out = await tools["cite"].acall(url=URL, quote=quote)
            print(out.raw_output.content[0].text[:90].replace("\n", " "))


asyncio.run(main())
