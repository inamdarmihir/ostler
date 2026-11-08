"""Recipe 13: research where every quote is checked and every gap is reported."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "common"))
from ostler_session import BASE, expect, ostler, run  # noqa: E402

LOCK = f"{BASE}/packs/research/kestrel-lock.html"


async def main() -> None:
    async with ostler() as s:
        s.require("open_many", "cite", "evidence")

        # Read several sources at once; the paywall and the bot wall become gaps, not guesses.
        batch = await s.call(
            "open_many",
            {
                "urls": [LOCK, f"{BASE}/packs/research/paywall.html", f"{BASE}/packs/research/protected.html"],
                "query": "who designed the lock and when did it open",
            },
        )
        expect("paywalled" in batch.text, "the paywalled page is reported as a gap")

        good = await s.call("cite", {"url": LOCK, "quote": "It opened in 1887 and was designed by Marta Ilves"})
        expect(good.text.startswith("VERIFIED"), "the real quote is VERIFIED")

        # A plausible but wrong claim: the page says Reyes restored the lock, not that he designed it.
        decoy = await s.call("cite", {"url": LOCK, "quote": "Tomas Reyes designed the Kestrel Lock in 1887"})
        expect(decoy.is_error and decoy.text.startswith("NOT VERIFIED"), "the decoy quote is refused")

        # Always finish with evidence: state only what it lists as verified, and name the gaps.
        report = await s.call("evidence")
        expect("1 verified citation" in report.text, "evidence lists exactly one verified citation")
        expect("Gaps" in report.text, "evidence names the pages that could not be read")


run(main)
