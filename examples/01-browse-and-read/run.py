"""Recipe 1: read a page and answer a question, quoting the page."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "common"))
from ostler_session import BASE, expect, ostler, ref, run  # noqa: E402


async def main() -> None:
    async with ostler() as s:
        page = await s.call("navigate", {"url": f"{BASE}/packs/research/index.html"})
        expect(not page.is_error, "the archive index loaded")
        link = ref(page.text, "link", "The Kestrel Lock")

        # Click by the ref Ostler just returned, never by a hard-coded id.
        clicked = await s.call("click", {"ref": link})
        expect("kestrel-lock.html" in clicked.text, "the click navigated to the article")

        # snapshot returns the page as text; page text is fenced as untrusted data.
        article = await s.call("snapshot", {"mode": "full"})
        expect('untrusted="true"' in article.text, "page text arrives fenced as untrusted")
        expect("opened in 1887" in article.text, "the answer is on the page")


run(main)
