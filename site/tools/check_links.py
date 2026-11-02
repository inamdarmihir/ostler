#!/usr/bin/env python3
"""Check the static site: internal links, fragments, and third-party loads.

    python3 site/tools/check_links.py

Fails (exit 1) when
  * a relative href/src points at a file that does not exist,
  * a #fragment (same page or other page) has no matching id,
  * any page loads a resource (script, stylesheet, image, font, iframe, ...)
    from another origin. Plain <a href="https://..."> links are allowed: they
    cause no request until clicked.
Standard library only.
"""
import re
import sys
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import unquote, urldefrag, urlparse

SITE = Path(__file__).resolve().parent.parent


class P(HTMLParser):
    def __init__(self):
        super().__init__()
        self.ids, self.links, self.loads = set(), [], []

    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        if "id" in a:
            self.ids.add(a["id"])
        if tag == "a" and a.get("name"):
            self.ids.add(a["name"])
        if tag == "a" and "href" in a:
            self.links.append(a["href"])
        elif tag == "link" and "href" in a:
            (self.loads if a.get("rel") in ("stylesheet", "preload", "prefetch", "modulepreload", "icon", "apple-touch-icon") else self.links).append(a["href"])
        for k in ("src", "srcset", "poster", "data"):
            if tag in ("script", "img", "iframe", "source", "video", "audio", "embed", "object", "track") and k in a:
                self.loads.append(a[k])


def main() -> int:
    pages = {}
    for f in sorted(SITE.rglob("*.html")):
        if "tools" in f.relative_to(SITE).parts:
            continue
        p = P()
        text = f.read_text(encoding="utf-8")
        p.feed(text)
        # CSS url(...) and inline @import
        css = re.findall(r"url\(([^)]+)\)|@import\s+['\"]?([^'\";)]+)", text)
        pages[f] = (p, [c for pair in css for c in pair if c])
    for f in SITE.rglob("*.css"):
        css = re.findall(r"url\(([^)]+)\)|@import\s+['\"]?([^'\";)]+)", f.read_text())
        pages[f] = (P(), [c for pair in css for c in pair if c])

    errors, ext, checked = [], set(), 0
    for f, (p, cssrefs) in pages.items():
        rel = f.relative_to(SITE)
        for ref in p.loads + [c.strip("'\" ") for c in cssrefs]:
            if re.match(r"^(https?:)?//", ref):
                errors.append(f"{rel}: THIRD-PARTY LOAD {ref}")
        for href in p.links + [r for r in p.loads]:
            href = href.strip()
            if not href or href.startswith(("mailto:", "data:", "javascript:")):
                continue
            if re.match(r"^(https?:)?//", href):
                ext.add(href)
                continue
            checked += 1
            path, frag = urldefrag(href)
            if not path:
                target = f
            else:
                u = urlparse(path)
                target = (f.parent / unquote(u.path)).resolve()
                if target.is_dir():
                    target = target / "index.html"
                try:
                    target.relative_to(SITE)
                except ValueError:
                    errors.append(f"{rel}: link escapes site: {href}")
                    continue
                if not target.exists():
                    errors.append(f"{rel}: missing file {href}")
                    continue
            if frag and target.suffix == ".html":
                ids = pages.get(target, (None,))[0]
                if ids is not None and frag not in ids.ids:
                    errors.append(f"{rel}: missing anchor {href}")
    print(f"pages: {len([f for f in pages if f.suffix == '.html'])}, internal references checked: {checked}, external links (not fetched): {len(ext)}")
    for e in errors:
        print("ERROR", e)
    print("OK: no broken internal links, no third-party loads" if not errors else f"{len(errors)} problem(s)")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
