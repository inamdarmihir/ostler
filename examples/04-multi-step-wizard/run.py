"""Recipe 8: a three-step wizard, one fill and one submit per step."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "common"))
from ostler_session import BASE, expect, ostler, run  # noqa: E402


async def main() -> None:
    async with ostler() as s:
        s.require("form_inspect", "form_fill", "form_submit")
        await s.call("navigate", {"url": f"{BASE}/wizard.html"})

        await s.call("form_fill", {"values": {"First name": "Ada", "Last name": "Lovelace"}})
        step = await s.call("form_submit")
        expect(step.text.startswith("Advanced"), "step 1 advanced")

        await s.call("form_fill", {"values": {"Email": "ada@example.com", "Phone": "+44 20 7946 0000"}})
        step = await s.call("form_submit")
        expect(step.text.startswith("Advanced"), "step 2 advanced")

        # Each new step has new fields: inspect again rather than assume.
        review = await s.call("form_inspect")
        expect("I confirm the details are correct" in review.text, "the review step asks for confirmation")
        await s.call("form_fill", {"values": {"I confirm the details are correct": True}})
        done = await s.call("form_submit")
        expect(done.text.startswith("Submitted"), "the application was submitted")


run(main)
