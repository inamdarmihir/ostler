"""LangChain: load Ostler's tools in one session, read a page, verify a real and a decoy quote."""

import asyncio

from langchain_mcp_adapters.client import MultiServerMCPClient
from langchain_mcp_adapters.tools import load_mcp_tools

URL = "http://127.0.0.1:8787/packs/research/kestrel-lock.html"
client = MultiServerMCPClient(
    {"ostler": {"transport": "stdio", "command": "ostler", "args": ["serve", "--tools", "research"]}}
)


async def main() -> None:
    async with client.session("ostler") as session:  # one session = one Ostler process
        tools = {tool.name: tool for tool in await load_mcp_tools(session)}
        await tools["read"].ainvoke({"url": URL})
        for quote in [
            "It opened in 1887 and was designed by Marta Ilves",
            "Tomas Reyes designed the Kestrel Lock in 1887",
        ]:
            result = await tools["cite"].ainvoke({"url": URL, "quote": quote})
            print(result[0]["text"][:90].replace("\n", " "))


asyncio.run(main())
