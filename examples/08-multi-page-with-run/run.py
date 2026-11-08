"""A multi-page job in one call: a sandboxed run script reads three pages and returns only the matches."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "common"))
from ostler_session import ADR_0019, BASE, expect, ostler, run  # noqa: E402

SCRIPT = """
let found = [];
for n in 1..4 {
    let page = tool("read", #{ url: `BASE/packs/research/catalog-${n}.html` });
    for line in lines(grep(page, "first operated")) { found.push(line); }
}
print(`${found.len()} locks found across 3 pages`);
found
"""


async def main() -> None:
    async with ostler() as s:
        s.require("run", feature=ADR_0019)
        # The script can only call Ostler tools: no files, no network, 60 calls at most.
        result = await s.call("run", {"script": SCRIPT.replace("BASE", BASE)})
        expect("Script finished: 3 tool call(s)" in result.text, "three reads, one model turn")
        expect("13 locks found across 3 pages" in result.text, "the whole register was collected")


run(main)
