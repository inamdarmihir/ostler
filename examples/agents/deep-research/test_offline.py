"""Test the agent's harness against real Ostler and the fixture pages, with a scripted model.

The model is replaced by a script that plays a plausible research session, including the mistakes a
real model makes: a claim whose number is not in its quote, a quote that is not on the page, and a
report that cites a finding that does not exist. The test checks that the harness catches each one.
No API key is needed; everything else (MCP, Ostler, Chrome, the fixture pages) is real.
"""

from __future__ import annotations

import asyncio
import os
import sys
from types import SimpleNamespace
from typing import Any

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
try:
    import research_agent as agent
except ModuleNotFoundError as missing:  # the anthropic SDK is an extra for this example
    print(f"SKIP: {missing.name} is not installed; pip install -r requirements.txt")
    sys.exit(3)

BASE = os.environ.get("OSTLER_FIXTURES", "http://127.0.0.1:8787")
LOCK = f"{BASE}/packs/research/kestrel-lock.html"
PAYWALL = f"{BASE}/packs/research/paywall.html"
SKIP_EXIT_CODE = 3


def usage() -> SimpleNamespace:
    return SimpleNamespace(input_tokens=100, output_tokens=50)


def tool_call(call_id: str, name: str, **arguments: Any) -> SimpleNamespace:
    return SimpleNamespace(type="tool_use", id=call_id, name=name, input=arguments)


def reply(*content: SimpleNamespace, stop: str = "tool_use") -> SimpleNamespace:
    return SimpleNamespace(content=list(content), stop_reason=stop, usage=usage())


def text(value: str) -> SimpleNamespace:
    return SimpleNamespace(type="text", text=value)


class ScriptedModel:
    """Stands in for `client.beta.messages`; records every tool result the harness sends back."""

    def __init__(self) -> None:
        self.tool_results: list[str] = []
        self.report_attempts = 0

    async def parse(self, **kwargs: Any) -> SimpleNamespace:
        assert kwargs["model"] == agent.MODEL and kwargs["fallbacks"] == "default"
        plan = agent.Plan(
            sub_questions=[
                agent.SubQuestion(question="Who designed the Kestrel Lock and when did it open?", start_urls=[LOCK]),
                agent.SubQuestion(question="What did the restoration cost?", start_urls=[PAYWALL]),
            ]
        )
        return SimpleNamespace(parsed_output=plan, stop_reason="end_turn", usage=usage())

    async def create(self, **kwargs: Any) -> SimpleNamespace:
        messages = kwargs["messages"]
        for message in messages:
            if message["role"] == "user" and isinstance(message["content"], list):
                self.tool_results += [r["content"] for r in message["content"] if r.get("type") == "tool_result"]
        if "tools" not in kwargs:
            return self.report()
        turn = sum(1 for m in messages if m["role"] == "assistant")
        if "designed" in messages[0]["content"]:
            return self.designer(turn)
        return self.cost(turn)

    def designer(self, turn: int) -> SimpleNamespace:
        if turn == 0:
            return reply(tool_call("d1", "read_page", url=LOCK, query="who designed the lock and when it opened"))
        if turn == 1:
            return reply(
                tool_call(
                    "d2",
                    "submit_findings",
                    open_questions=[],
                    findings=[
                        {
                            "claim": "Marta Ilves designed the lock, which opened in 1887.",
                            "url": LOCK,
                            "quote": "It opened in 1887 and was designed by Marta Ilves",
                        },
                        {"claim": "The lock opened in 1889.", "url": LOCK, "quote": "It opened in 1887"},
                        {
                            "claim": "Tomas Reyes designed the lock.",
                            "url": LOCK,
                            "quote": "Tomas Reyes designed the Kestrel Lock",
                        },
                    ],
                )
            )
        return reply(
            tool_call(
                "d3",
                "submit_findings",
                open_questions=[],
                findings=[
                    {
                        "claim": "Marta Ilves designed the lock, which opened in 1887.",
                        "url": LOCK,
                        "quote": "It opened in 1887 and was designed by Marta Ilves",
                    },
                    {
                        "claim": "Tomas Reyes supervised a restoration in 1962.",
                        "url": LOCK,
                        "quote": "A major restoration in 1962 was supervised by the engineer Tomas Reyes",
                    },
                ],
            )
        )

    def cost(self, turn: int) -> SimpleNamespace:
        if turn == 0:
            return reply(tool_call("c1", "read_page", url=PAYWALL, query="cost of the restoration"))
        return reply(
            tool_call(
                "c2", "submit_findings", findings=[], open_questions=["The Courier article on the cost is paywalled."]
            )
        )

    def report(self) -> SimpleNamespace:
        self.report_attempts += 1
        if self.report_attempts == 1:
            return reply(
                text("Marta Ilves designed it and it opened in 1887 [1]. It cost a fortune [3]."), stop="end_turn"
            )
        return reply(
            text(
                "The Kestrel Lock opened in 1887 and was designed by Marta Ilves [1]; Tomas Reyes "
                "supervised its 1962 restoration [2]. Its cost could not be established: the article "
                "on it is paywalled."
            ),
            stop="end_turn",
        )


def expect(condition: bool, message: str) -> None:
    if not condition:
        raise SystemExit(f"FAIL: {message}")
    print(f"CHECK OK: {message}")


async def main() -> None:
    model = ScriptedModel()
    async with agent.open_browser(["127.0.0.1"]) as browser:
        report = await agent.deep_research(
            model,
            browser,
            "Who designed the Kestrel Lock, when did it open, and what did its restoration cost?",
            [LOCK, PAYWALL],
        )
    print(report)
    verdicts = "\n".join(model.tool_results)
    expect("1. ACCEPTED: verified" in verdicts, "the real quote is accepted")
    expect("the claim states 1889, which the quote does not contain" in verdicts, "the number mismatch is rejected")
    expect("NOT VERIFIED" in verdicts, "the quote that is not on the page is rejected")
    expect(model.report_attempts == 2, "the report citing a finding that does not exist was sent back")
    expect("[3]" not in report and "[2]" in report, "the final report cites only verified findings")
    expect("#:~:text=" in report, "each source carries a link to check the quote")
    expect("paywalled" in report, "the paywalled page is reported as a gap")


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except BaseExceptionGroup as group:
        exits = group.subgroup(SystemExit)
        if exits is None:
            raise
        while isinstance(exits, BaseExceptionGroup):
            exits = exits.exceptions[0]
        raise exits from None
