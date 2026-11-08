"""Recipe 4: see what is blocked by a modal, close it, then continue."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "common"))
from ostler_session import BASE, expect, ostler, run, target  # noqa: E402


async def main() -> None:
    async with ostler() as s:
        await s.call("navigate", {"url": f"{BASE}/overlay.html"})

        # look returns numbered click targets and counts the ones hidden under the overlay.
        blocked = await s.call("look")
        expect("hidden under an overlay" in blocked.text, "look reports controls hidden by the modal")
        accept = target(blocked.text, "button", "Accept all")

        closed = await s.call("click", {"ref": accept})
        expect("dialog" in closed.text, "the click removed the dialog (shown in the delta)")

        clear = await s.call("look")
        expect("hidden under an overlay" not in clear.text, "nothing is hidden any more")
        expect('textbox "Email"' in clear.text, "the form behind the modal is reachable")


run(main)
