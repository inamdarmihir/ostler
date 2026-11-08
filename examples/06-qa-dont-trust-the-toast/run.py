"""Recipe 3: check a web app after a change, asserting on the outcome rather than on a toast.

The fixture app shows "Settings saved" before the server answers. Saving the dark theme
succeeds; saving sepia fails on the server while the toast still says it was saved.
"""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "common"))
from ostler_session import BASE, expect, ostler, ref, run  # noqa: E402

APP = f"{BASE}/packs/dev-qa/app.html"
SUCCESS_TEXT = "Your theme preference is now active"


async def save_theme(s, theme: str) -> bool:
    page = await s.call("navigate", {"url": f"{APP}#/settings"})
    await s.call("form_fill", {"values": {"Theme": theme}})
    await s.call("click", {"ref": ref(page.text, "button", "Save settings")})
    # Assert on what only a real save produces (the #/saved view), with a bounded wait.
    outcome = await s.call("wait_for", {"text": SUCCESS_TEXT, "timeout_ms": 3000})
    return not outcome.is_error


async def main() -> None:
    async with ostler() as s:
        home = await s.call("navigate", {"url": APP})
        expect("Ledgerly" in home.text, "the app loaded")
        expect(await save_theme(s, "dark"), "saving the dark theme really saved")
        expect(not await save_theme(s, "sepia"), "the sepia save is caught as a failure despite the toast")


run(main)
