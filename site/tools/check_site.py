#!/usr/bin/env python3
"""Publish-time checks for the static site. Run after build_docs.py; exits 1 on any failure.

  python3 site/tools/check_site.py          check
  python3 site/tools/check_site.py --fix    rewrite the homepage FAQPage JSON-LD from the visible FAQ

What it guards (each one has failed on this site before, or is a rule search engines document):
  * every page: exactly one <h1>, one canonical URL, no noindex, unique <title> and meta description
  * every JSON-LD block parses as JSON
  * homepage release status and tool count agree with build_docs.py, Cargo.toml and tools.md
  * the FAQPage JSON-LD is the visible FAQ, word for word (structured data must match visible content)
  * the homepage defines #app, #creator and #website once; documentation pages point at them
  * the sitemap lists exactly the pages that exist
  * llms.txt and llms-full.txt use absolute links and contain no repo: or :unreleased: markers
"""
from __future__ import annotations

import html
import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import build_docs as bd  # noqa: E402

SITE = bd.SITE
REPO = bd.REPO
LD_RE = re.compile(r'<script type="application/ld\+json">(.*?)</script>', re.S)
FAQ_RE = re.compile(r'<details id="[^"]+"[^>]*><summary><b>(.*?)</b></summary>\s*<p>(.*?)</p></details>', re.S)
# Navigation links after the last sentence ("Quickstart", "Research · recipe") are not part of the answer.
TRAILING_LINKS = re.compile(r'(?:\s*<a [^>]*>[^<]*</a>)(?:\s*&middot;\s*<a [^>]*>[^<]*</a>)*\s*$')


def plain(markup: str) -> str:
    return re.sub(r"\s+", " ", html.unescape(re.sub(r"<[^>]+>", "", markup))).strip()


def visible_faq(page: str) -> list[tuple[str, str]]:
    out = []
    for q, a in FAQ_RE.findall(page):
        out.append((plain(q), plain(TRAILING_LINKS.sub("", a.strip()))))
    return out


def faq_node(pairs: list[tuple[str, str]]) -> dict:
    return {
        "@type": "FAQPage", "@id": bd.SITE_URL + "#faq", "url": bd.SITE_URL + "#faq",
        "about": {"@id": bd.SITE_URL + "#app"},
        "mainEntity": [{"@type": "Question", "name": q, "acceptedAnswer": {"@type": "Answer", "text": a}} for q, a in pairs],
    }


def dump_graph(graph: list[dict]) -> str:
    rows = ",\n".join(json.dumps(n, ensure_ascii=False, separators=(",", ":")).replace("</", "<\\/") for n in graph)
    return '<script type="application/ld+json">{"@context":"https://schema.org","@graph":[\n' + rows + "\n]}</script>"


def workspace_version() -> str:
    m = re.search(r'^\[workspace\.package\][^\[]*?^version\s*=\s*"([^"]+)"', (REPO / "Cargo.toml").read_text(), re.S | re.M)
    return m.group(1) if m else ""


def main(argv: list[str]) -> int:
    errors: list[str] = []
    err = errors.append

    home_path = SITE / "index.html"
    home = home_path.read_text(encoding="utf-8")

    if "--fix" in argv:
        blocks = LD_RE.findall(home)
        graph = json.loads(blocks[0])["@graph"]
        graph = [n for n in graph if n.get("@type") != "FAQPage"] + [faq_node(visible_faq(home))]
        home = home.replace(LD_RE.search(home).group(0), dump_graph(graph))
        home_path.write_text(home, encoding="utf-8")
        print(f"synced FAQPage JSON-LD from {len(graph[-1]['mainEntity'])} visible questions")

    pages = {"index.html": home}
    for p in sorted((SITE / "docs").glob("*.html")):
        pages[f"docs/{p.name}"] = p.read_text(encoding="utf-8")

    titles: dict[str, str] = {}
    descs: dict[str, str] = {}
    for name, text in pages.items():
        if len(re.findall(r"<h1[ >]", text)) != 1:
            err(f"{name}: expected exactly one <h1>, found {len(re.findall(r'<h1[ >]', text))}")
        if len(re.findall(r'<link rel="canonical"', text)) != 1:
            err(f"{name}: expected exactly one canonical link")
        if re.search(r'<meta name="robots" content="[^"]*noindex', text):
            err(f"{name}: has noindex")
        t = re.search(r"<title>(.*?)</title>", text, re.S)
        d = re.search(r'<meta name="description" content="([^"]*)"', text)
        if not t or not d:
            err(f"{name}: missing <title> or meta description")
            continue
        if t.group(1) in titles:
            err(f"{name}: title duplicates {titles[t.group(1)]}")
        if d.group(1) in descs:
            err(f"{name}: meta description duplicates {descs[d.group(1)]}")
        titles[t.group(1)], descs[d.group(1)] = name, name
        for block in LD_RE.findall(text):
            try:
                json.loads(block)
            except json.JSONDecodeError as e:
                err(f"{name}: JSON-LD does not parse: {e}")

    # --- homepage entity and release status -------------------------------------------------------
    graph = json.loads(LD_RE.findall(home)[0])["@graph"]
    by_id = {}
    for n in graph:
        if "@id" in n:
            if n["@id"] in by_id:
                err(f"homepage: duplicate @id {n['@id']}")
            by_id[n["@id"]] = n
    for frag in ("#app", "#creator", "#website"):
        if bd.SITE_URL + frag not in by_id:
            err(f"homepage JSON-LD does not define {frag}")
    app = by_id.get(bd.SITE_URL + "#app", {})
    want = f"{bd.VERSION} ({bd.STATUS})"
    if app.get("softwareVersion") != want:
        err(f"homepage softwareVersion is {app.get('softwareVersion')!r}, build_docs.py says {want!r}")
    if bd.VERSION != workspace_version():
        err(f"build_docs.py VERSION {bd.VERSION} != Cargo.toml workspace version {workspace_version()}")
    if "npmjs.com" in json.dumps(app.get("sameAs", [])):
        err("homepage sameAs links an npm package that is not published")

    tools_md = (bd.CONTENT / "tools.md").read_text(encoding="utf-8")
    m = re.search(r"exposes (\d+) MCP tools", tools_md)
    headings = len(re.findall(r"^### `", tools_md, re.M))
    if not m or int(m.group(1)) != headings:
        err(f"tools.md says {m.group(1) if m else '?'} tools but documents {headings}")
    else:
        for n in set(re.findall(r"\b(\d+) (?:MCP )?tools\b", plain(home))):
            if int(n) != headings:
                err(f"homepage says {n} tools; tools.md documents {headings}")

    # --- FAQ structured data == visible FAQ ---------------------------------------------------------
    faq = next((n for n in graph if n.get("@type") == "FAQPage"), None)
    visible = visible_faq(home)
    if not faq:
        err("homepage has no FAQPage JSON-LD")
    else:
        ld = [(q["name"], q["acceptedAnswer"]["text"]) for q in faq["mainEntity"]]
        if ld != visible:
            err("FAQPage JSON-LD differs from the visible FAQ; run: python3 site/tools/check_site.py --fix")

    # --- documentation pages reuse the homepage identifiers ----------------------------------------
    for name, text in pages.items():
        if not name.startswith("docs/") or name == "docs/index.html":
            continue
        g = json.loads(LD_RE.findall(text)[0])["@graph"]
        art = next((n for n in g if n.get("@type") == "TechArticle"), {})
        if art.get("about") != {"@id": bd.SITE_URL + "#app"} or art.get("isPartOf") != {"@id": bd.SITE_URL + "#website"}:
            err(f"{name}: TechArticle must reference #app and #website by @id")

    # --- sitemap ------------------------------------------------------------------------------------
    locs = re.findall(r"<loc>(.*?)</loc>", (SITE / "sitemap.xml").read_text(encoding="utf-8"))
    for loc in locs:
        rel = loc[len(bd.SITE_URL):] or "index.html"
        if not loc.startswith(bd.SITE_URL) or not (SITE / rel).exists():
            err(f"sitemap lists {loc}, which has no file")
    if len(locs) != len(pages):
        err(f"sitemap has {len(locs)} URLs but the site has {len(pages)} pages")

    # --- LLM files ----------------------------------------------------------------------------------
    for fname in ("llms.txt", "llms-full.txt"):
        text = (SITE / fname).read_text(encoding="utf-8")
        for marker in (":unreleased:", "](repo:"):
            if marker in text:
                err(f"{fname}: contains {marker}")
        bad = [u for u in re.findall(r"\]\(([^)\s]+)", text) if not u.startswith(("http://", "https://", "#", "mailto:"))]
        if bad:
            err(f"{fname}: {len(bad)} non-absolute link target(s), first: {bad[0]}")

    for e in errors:
        print("FAIL:", e)
    if errors:
        return 1
    print(f"OK: {len(pages)} pages, {len(locs)} sitemap URLs, {len(visible)} FAQ answers match their JSON-LD")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
