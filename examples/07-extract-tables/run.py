"""Structured extraction: tables as rows, filtered before they reach the model (ADR-0019)."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "common"))
from ostler_session import ADR_0019, BASE, expect, ostler, run  # noqa: E402


async def main() -> None:
    async with ostler() as s:
        s.require("extract", "search_tools", feature=ADR_0019)
        await s.call("navigate", {"url": f"{BASE}/packs/research/table.html"})

        table = await s.call("extract", {"kind": "tables"})
        expect("| South | Delta | 58 | 61 | 63 | 71 |" in table.text, "the table comes back as rows")

        # Keep only the rows and columns the question needs; the rest never enters the context.
        south = await s.call("extract", {"kind": "tables", "where": "South", "select": ["Sub-region", "2024 / H2"]})
        expect("2 of 4 row(s) kept" in south.text, "the filter states how many rows it kept")
        rows = south.text.split("</page_content>")[0]
        expect("North" not in rows and "| Delta | 71 |" in rows, "only the South rows and two columns are left")

        # A paginated list says when it is incomplete instead of looking finished.
        await s.call("navigate", {"url": f"{BASE}/packs/research/catalog-1.html"})
        listing = await s.call("extract", {"kind": "lists"})
        expect(listing.text.startswith("PARTIAL: 5 of 13"), "page 1 of the register is reported as partial")


run(main)
