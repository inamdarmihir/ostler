"""AutoGen: McpWorkbench as a context manager keeps one Ostler session for read and cite."""

import asyncio

from autogen_ext.tools.mcp import McpWorkbench, StdioServerParams

URL = "http://127.0.0.1:8787/packs/research/kestrel-lock.html"
PARAMS = StdioServerParams(command="ostler", args=["serve", "--tools", "research"], read_timeout_seconds=60)


async def main() -> None:
    async with McpWorkbench(PARAMS) as workbench:  # one session = one Ostler process
        await workbench.call_tool("read", {"url": URL})
        for quote in [
            "It opened in 1887 and was designed by Marta Ilves",
            "Tomas Reyes designed the Kestrel Lock in 1887",
        ]:
            result = await workbench.call_tool("cite", {"url": URL, "quote": quote})
            print(result.to_text()[:90].replace("\n", " "))


asyncio.run(main())
