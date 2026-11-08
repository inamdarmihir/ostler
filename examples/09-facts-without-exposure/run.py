"""Recipe 9 and ADR-0019: fill personal data from saved facts without the agent ever handling the value."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "common"))
from ostler_session import ADR_0019, BASE, expect, ostler, run  # noqa: E402


async def main() -> None:
    async with ostler() as s:
        s.require("memory_remember", "form_fill", "search_tools", feature=ADR_0019)
        # Saved once (by the user or a setup step), outside the agent's task.
        await s.call(
            "memory_remember", {"key": "email", "value": "ada@example.com", "autocomplete": "email"}, show=False
        )

        await s.call("navigate", {"url": f"{BASE}/basic.html"})
        filled = await s.call(
            "form_fill",
            {
                "values": {
                    "Full name": "Ada Lovelace",
                    "Email": "{{fact:email}}",
                    "Message": "Please call me back.",
                }
            },
        )
        expect("Filled 3/3" in filled.text, "all three fields were filled and verified")
        expect("ada@example.com" not in filled.text, "the saved value is not echoed back to the agent")

        unknown = await s.call("form_fill", {"values": {"Email": "{{fact:no_such_fact}}"}})
        expect(unknown.is_error, "an unknown fact fails the call instead of filling something else")


run(main)
