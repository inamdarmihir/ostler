"""Recipe 7: fill a whole form in one call, read every value back, and stop on what the page rejects."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "common"))
from ostler_session import BASE, expect, ostler, run  # noqa: E402


async def main() -> None:
    async with ostler() as s:
        s.require("form_fill", "form_submit")
        await s.call("navigate", {"url": f"{BASE}/dependent.html"})

        # Email is not on this form: Ostler reports the key instead of guessing a field for it.
        first = await s.call(
            "form_fill", {"values": {"Country": "Germany", "State": "Bavaria", "Email": "ada@example.com"}}
        )
        expect("Filled 2/3" in first.text, "two fields verified, the unmatched key reported")
        expect('"State / region"' in first.text, "State was filled after its options loaded")

        rejected = await s.call("form_submit")
        expect("NOT ACCEPTED" in rejected.text, "the page's own validation stops the submit")

        # Fix what the page asked for, then submit again.
        await s.call("form_fill", {"values": {"Recipient": "Ada Lovelace"}})
        done = await s.call("form_submit")
        expect(done.text.startswith("Submitted"), "the corrected form is accepted")


run(main)
