"""Recipe 10: the agent fills and verifies, but nothing is submitted until a person agrees."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "common"))
from ostler_session import BASE, expect, ostler, run  # noqa: E402


async def main() -> None:
    async with ostler("--require-confirm-submit") as s:
        await s.call("navigate", {"url": f"{BASE}/basic.html"})
        await s.call("form_fill", {"values": {"Full name": "Ada Lovelace", "Email": "ada@example.com"}})

        held = await s.call("form_submit")
        expect(held.is_error and "requires explicit confirmation" in held.text, "the submit is held for approval")

        # In a real agent this is where you show the user the filled form and ask.
        print("\n(The user reviewed the form and approved it.)")
        done = await s.call("form_submit", {"confirm": True})
        expect(done.text.startswith("Submitted"), "submitted only after confirm=true")


run(main)
