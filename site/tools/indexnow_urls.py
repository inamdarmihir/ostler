#!/usr/bin/env python3
"""Print, as a JSON list, the sitemap URLs whose pages changed, for the IndexNow ping.

  python3 site/tools/indexnow_urls.py [BEFORE_SHA]

A page counts as changed if its file differs between BEFORE_SHA and HEAD, or if the deploy's own
rebuild changed it (uncommitted difference in the working tree). With no usable BEFORE_SHA (manual
run, first push, unknown commit) every sitemap URL is returned. IndexNow is a notification, not a
confirmation of indexing, so an empty list is a valid answer: nothing to announce.
"""
from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path

SITE = Path(__file__).resolve().parent.parent
REPO = SITE.parent
SITE_URL = "https://inamdarmihir.github.io/ostler/"


def git(*args: str) -> str | None:
    r = subprocess.run(["git", *args], cwd=REPO, capture_output=True, text=True)
    return r.stdout if r.returncode == 0 else None


def url_for(path: str) -> str | None:
    """site/index.html -> homepage; site/docs/x.html -> docs/x.html; anything else is not a page."""
    if path == "site/index.html":
        return SITE_URL
    m = re.fullmatch(r"site/(docs/[^/]+\.html)", path)
    return SITE_URL + m.group(1) if m else None


def main(argv: list[str]) -> int:
    urls = re.findall(r"<loc>(.*?)</loc>", (SITE / "sitemap.xml").read_text(encoding="utf-8"))
    before = argv[0] if argv else ""
    committed = git("diff", "--name-only", before, "HEAD", "--", "site") if re.fullmatch(r"[0-9a-f]{7,40}", before) and not set(before) == {"0"} else None
    if committed is None:
        print(json.dumps(urls))
        return 0
    rebuilt = git("status", "--porcelain", "--", "site") or ""
    changed = set(committed.split()) | {line[3:] for line in rebuilt.splitlines()}
    out = [u for u in (url_for(p) for p in sorted(changed)) if u in urls]
    print(json.dumps(out))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
