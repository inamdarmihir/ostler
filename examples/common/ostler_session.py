"""One Ostler session over MCP stdio, shared by every example.

Best practices this helper encodes:

* One session per task. ``cite`` only accepts quotes from pages ``read`` in the
  same Ostler process, so every example keeps a single session open.
* Least privilege by default: ``--allow-domain 127.0.0.1`` so the browser can
  only reach the bundled fixture pages.
* Refs come from the latest observation. ``ref()`` finds an element by role and
  name in the text Ostler just returned; examples never hard-code ``e6``.
* Errors are data. ``call()`` returns the text and the error flag; ``expect()``
  stops the example on an unexpected result instead of carrying on.
* Feature detection: ``require()`` skips an example cleanly when the installed
  Ostler does not list a tool it needs.
"""

from __future__ import annotations

import asyncio
import os
import re
import shlex
import sys
from contextlib import asynccontextmanager
from dataclasses import dataclass
from typing import AsyncIterator, Awaitable, Callable

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

BASE = os.environ.get("OSTLER_FIXTURES", "http://127.0.0.1:8787")
OSTLER = os.environ.get("OSTLER_BIN", "ostler")
SKIP_EXIT_CODE = 3
# Result filters, {{fact:KEY}} values and run landed together (ADR-0019); search_tools marks them.
ADR_0019 = "the ADR-0019 features (result filters, {{fact:KEY}}, run)"


@dataclass
class Result:
    text: str
    is_error: bool

    def has(self, needle: str) -> bool:
        return needle in self.text


class Session:
    def __init__(self, session: ClientSession, tools: set[str]) -> None:
        self._session = session
        self.tools = tools

    async def call(self, name: str, args: dict | None = None, *, show: bool = True) -> Result:
        response = await self._session.call_tool(name, args or {})
        text = "\n".join(block.text for block in response.content if getattr(block, "type", "") == "text")
        # mcp 1.x names the flag isError, mcp 2.x is_error.
        is_error = bool(getattr(response, "is_error", getattr(response, "isError", False)))
        if show:
            marker = "ERROR " if is_error else ""
            print(f"\n> {name} {format_args(args)}\n{marker}{text}")
        return Result(text, is_error)

    def require(self, *names: str, feature: str = "") -> None:
        """Skip the example unless every named tool is listed. ``feature`` names what the tools stand for."""
        missing = [name for name in names if name not in self.tools]
        if missing:
            needed = feature or ", ".join(missing)
            print(f"SKIP: this Ostler lacks {needed}; build Ostler from main or upgrade to run this example.")
            sys.exit(SKIP_EXIT_CODE)


def format_args(args: dict | None) -> str:
    if not args:
        return ""
    shown = {key: (value if len(str(value)) < 80 else str(value)[:77] + "...") for key, value in args.items()}
    return str(shown)


def ref(text: str, role: str, name: str) -> str:
    """Return the ref of the element with this role and accessible name, e.g. ``button "Save" [e6]``."""
    pattern = rf'{re.escape(role)} "{re.escape(name)}" \[(e\d+)\]'
    found = re.findall(pattern, text)
    if len(found) != 1:
        raise SystemExit(f"FAIL: expected exactly one {role} named {name!r}, found {len(found)}")
    return found[0]


def target(text: str, role: str, name: str) -> str:
    """Return the ref of a numbered click target from ``look``, e.g. ``e5 button "Accept all" -> button``."""
    found = re.findall(rf'(e\d+) {re.escape(role)} "{re.escape(name)}"', text)
    if len(found) != 1:
        raise SystemExit(f"FAIL: expected exactly one {role} target named {name!r}, found {len(found)}")
    return found[0]


def expect(condition: bool, message: str) -> None:
    if not condition:
        raise SystemExit(f"FAIL: {message}")
    print(f"CHECK OK: {message}")


def run(main: Callable[[], Awaitable[None]]) -> None:
    """Run an example; a FAIL or SKIP raised inside the MCP session exits with its own code and message."""
    try:
        asyncio.run(main())
    except BaseExceptionGroup as group:
        exits = group.subgroup(SystemExit)
        if exits is None:
            raise
        while isinstance(exits, BaseExceptionGroup):
            exits = exits.exceptions[0]
        raise exits from None


@asynccontextmanager
async def ostler(*extra_args: str) -> AsyncIterator[Session]:
    args = ["serve", "--tools", "all", "--allow-domain", "127.0.0.1", *extra_args]
    args += shlex.split(os.environ.get("OSTLER_EXTRA_ARGS", ""))
    params = StdioServerParameters(command=OSTLER, args=args)
    async with stdio_client(params) as (read, write), ClientSession(read, write) as session:
        await session.initialize()
        listed = await session.list_tools()
        yield Session(session, {tool.name for tool in listed.tools})
