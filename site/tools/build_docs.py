#!/usr/bin/env python3
"""Build the Ostler documentation site (site/docs/*.html + search index).

Standard library only. Run from anywhere:

    python3 site/tools/build_docs.py

Inputs
  * site/tools/content/*.md   hand-written pages (Quickstart, Install, ...)
  * README-adjacent repo docs (SECURITY.md, CHANGELOG.md, docs/*.md) converted as-is
Outputs
  * site/docs/<slug>.html     one page each, plus site/docs/index.html
  * site/docs/search-index.json   small prebuilt index for client-side search

Everything is static and uses relative URLs, so the site works under any
subpath (for example https://user.github.io/ostler/).

Link rules inside Markdown
  * links in converted repo docs are resolved against the source file; known
    docs become local pages, anything else becomes a GitHub URL;
  * in hand-written content, `repo:path/to/file` links to that file on GitHub,
    other relative links are used as written (site-relative).
Extra syntax in hand-written content
  * `> [!NOTE]`, `> [!WARNING]`, `> [!TIP]` start a callout block;
  * `:unreleased:` renders a small "Unreleased" badge.
"""
from __future__ import annotations

import html
import json
import os
import posixpath
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
SITE = HERE.parent
REPO = SITE.parent
OUT = SITE / "docs"
CONTENT = HERE / "content"
GITHUB = "https://github.com/inamdarmihir/ostler"
SITE_URL = "https://inamdarmihir.github.io/ostler/"
# One definition, used verbatim by the landing page, README, llms.txt and the
# structured data, so search engines and AI answer engines see one entity.
DEFINITION = ("Ostler is an open-source MCP browser for AI coding agents such as Claude Code, Codex and Cursor. "
              "It checks its own work: research answers cite only pages it actually read, with every quote verified, "
              "forms are filled in one call with every value read back, and Qdrant remembers what worked.")

# --------------------------------------------------------------------------
# Page catalogue. Order here is the sidebar order and the prev/next order.
# src: ("content", file) for hand-written pages, ("repo", path) for converted.
# --------------------------------------------------------------------------
GROUPS = [
    ("Get started", [
        ("quickstart", "Quickstart", "Install, check, connect, and fill a demo form in five minutes.", ("content", "quickstart.md")),
        ("install", "Install", "Every way to get the binary, what works today, update and uninstall.", ("content", "install.md")),
        ("agents", "Use with your agent", "Add Ostler as an MCP browser to Claude Code, Codex, Cursor, VS Code or any MCP client, with a forms and a research workflow.", ("content", "agents.md")),
    ]),
    ("Guides", [
        ("cookbook", "Cookbook", "Recipes: deep research with checked citations, reading, QA, logins, forms, approvals and A2A, with what is not built yet.", ("repo", "docs/COOKBOOK.md")),
        ("forms", "Forms and verification", "How keys become fields, how values are verified by read-back, and what submit results mean.", ("content", "forms.md")),
        ("memory", "Memory and privacy", "What is stored, what sealing hides and does not, local embeddings, per-tenant tokens.", ("content", "memory.md")),
        ("configuration", "Configuration and CLI", "Config file, environment variables, every serve and a2a flag.", ("content", "configuration.md")),
    ]),
    ("Reference", [
        ("tools", "Tools reference", "All 30 MCP tools: browse, read and cite, search, extract, forms and memory, with parameters and profiles.", ("content", "tools.md")),
        ("security", "Security model", "The threat model: what Ostler does, and what it does not do.", ("repo", "SECURITY.md")),
        ("a2a", "A2A", "Let other agents hand Ostler forms over Agent2Agent v1.0.", ("repo", "docs/A2A.md")),
        ("architecture", "Architecture", "Crates, the injected runtime, the form model, and the decisions behind them.", ("repo", "docs/ARCHITECTURE.md")),
    ]),
    ("Evidence", [
        ("benchmarks", "Benchmarks", "Measured on local fixtures with a scripted agent. Read the caveats.", ("repo", "docs/BENCHMARKS.md")),
        ("economics", "Economics", "Unit costs, scaling limits, the moat question and what could go wrong.", ("repo", "docs/ECONOMICS.md")),
        ("review", "Design review", "The October 2026 review: five design flaws and their status.", ("repo", "docs/REVIEW-2026-10.md")),
    ]),
    ("Help", [
        ("troubleshooting", "Troubleshooting", "Symptoms, causes and fixes. Start with ostler doctor.", ("repo", "docs/TROUBLESHOOTING.md")),
        ("changelog", "Changelog", "What changed, including the Unreleased section.", ("repo", "CHANGELOG.md")),
    ]),
]

PAGES = []  # flat list of dicts
for gname, items in GROUPS:
    for slug, title, desc, src in items:
        PAGES.append(dict(slug=slug, title=title, desc=desc, src=src, group=gname))
BY_SLUG = {p["slug"]: p for p in PAGES}

# repo file -> local page slug (for link rewriting in converted docs)
REPO_TO_SLUG = {
    "SECURITY.md": "security",
    "CHANGELOG.md": "changelog",
    "docs/A2A.md": "a2a",
    "docs/ARCHITECTURE.md": "architecture",
    "docs/BENCHMARKS.md": "benchmarks",
    "docs/CLIENTS.md": "agents",
    "docs/COOKBOOK.md": "cookbook",
    "docs/ECONOMICS.md": "economics",
    "docs/REVIEW-2026-10.md": "review",
    "docs/TROUBLESHOOTING.md": "troubleshooting",
}
# README sections -> local pages
README_ANCHORS = {
    "install": "install.html",
    "quick-start": "quickstart.html",
    "configure": "configuration.html",
    "use-with-your-agent": "agents.html",
    "memory": "memory.html",
    "benchmarks": "benchmarks.html",
    "cli-reference": "configuration.html",
    "a2a-let-other-agents-hand-it-forms": "a2a.html",
    "safety": "security.html",
    "how-it-compares": "../index.html#compare",
    "roadmap": "../index.html#limits",
    "searching-the-web": "troubleshooting.html#searching-the-web",
}
# Intro paragraphs inserted (as Markdown) under the title of converted pages.
INTROS = {
    "changelog": "Entries under **Unreleased** are on the `main` branch and are not in a tagged release yet. This includes sealed memory, local embeddings and per-tenant tokens; see [Memory and privacy](memory.html).",
    "security": "This page is the repository's `SECURITY.md`. For how the memory-related rows work in practice, see [Memory and privacy](memory.html).",
}


# --------------------------------------------------------------------------
# Markdown -> HTML (small, purpose-built; covers what these docs use)
# --------------------------------------------------------------------------
def github_slug(text: str) -> str:
    text = re.sub(r"<[^>]+>", "", text)
    text = text.strip().lower()
    text = re.sub(r"[^\w\- ]", "", text, flags=re.UNICODE)
    return text.replace(" ", "-")


def esc(s: str) -> str:
    return html.escape(s, quote=False)


def attr(s: str) -> str:
    return html.escape(s, quote=True)


class Converter:
    def __init__(self, src_kind: str, src_path: str):
        self.kind = src_kind          # "content" or "repo"
        self.src_path = src_path      # repo-relative path for repo docs
        self.toc: list[tuple[int, str, str]] = []   # (level, id, text)
        self.ids: set[str] = set()
        self.title = ""

    # ---- links ---------------------------------------------------------
    def resolve(self, href: str) -> tuple[str, bool]:
        """Return (url, external)."""
        if re.match(r"^(https?:|mailto:)", href):
            return href, True
        if href.startswith("#"):
            return href, False
        if href.startswith("repo:"):
            path = href[5:]
            frag = ""
            if "#" in path:
                path, frag = path.split("#", 1)
                frag = "#" + frag
            return self.github(path) + frag, True
        if self.kind == "content":
            return href, False
        # converted repo doc: resolve against the source file's directory
        path, frag = (href.split("#", 1) + [""])[:2] if "#" in href else (href, "")
        base = posixpath.dirname(self.src_path)
        norm = posixpath.normpath(posixpath.join(base, path)) if path else self.src_path
        if norm in REPO_TO_SLUG:
            return REPO_TO_SLUG[norm] + ".html" + ("#" + frag if frag else ""), False
        if norm == "README.md" and frag in README_ANCHORS:
            return README_ANCHORS[frag], False
        return self.github(norm) + ("#" + frag if frag else ""), True

    @staticmethod
    def github(path: str) -> str:
        path = path.strip("/")
        full = REPO / path
        if full.is_dir():
            return f"{GITHUB}/tree/main/{path}"
        return f"{GITHUB}/blob/main/{path}"

    # ---- inline --------------------------------------------------------
    def inline(self, text: str) -> str:
        stash: list[str] = []

        def keep(markup: str) -> str:
            stash.append(markup)
            return f"\x00{len(stash) - 1}\x00"

        # code spans first
        def code(m):
            return keep("<code>" + esc(m.group(2).strip()) + "</code>")
        text = re.sub(r"(`+)(.+?)\1", code, text)
        # backslash escapes
        text = re.sub(r"\\([\\`*_{}\[\]()#+\-.!|<>])", lambda m: keep(esc(m.group(1))), text)
        text = esc(text)
        # autolinks <https://...>
        text = re.sub(r"&lt;(https?://[^\s&]+)&gt;",
                      lambda m: keep(f'<a href="{attr(m.group(1))}" rel="noopener">{esc(m.group(1))}</a>'), text)
        # badge
        text = text.replace(":unreleased:", keep('<span class="badge badge-unreleased">Unreleased</span>'))

        # links
        def link(m):
            label, href = m.group(1), html.unescape(m.group(2))
            url, external = self.resolve(href)
            extra = ' rel="noopener"' if external else ""
            return keep(f'<a href="{attr(url)}"{extra}>') + label + keep("</a>")
        text = re.sub(r"\[([^\]]+)\]\(([^)\s]+)(?:\s+&quot;[^)]*&quot;)?\)", link, text)
        # bold / italic
        text = re.sub(r"\*\*(?=\S)(.+?)(?<=\S)\*\*", r"<strong>\1</strong>", text)
        text = re.sub(r"(?<![\w*])\*(?=[^\s*])(.+?)(?<=[^\s*])\*(?![\w*])", r"<em>\1</em>", text)

        # restore (repeat: stash entries can nest)
        for _ in range(3):
            text = re.sub(r"\x00(\d+)\x00", lambda m: stash[int(m.group(1))], text)
        return text

    # ---- code ----------------------------------------------------------
    @staticmethod
    def highlight(code: str, lang: str) -> str:
        out = esc(code)
        if lang in ("sh", "bash", "shell", "toml"):
            def line(l: str) -> str:
                m = re.match(r"^(\s*)(#.*)$", l)
                if m:
                    return f'{m.group(1)}<span class="tok-c">{m.group(2)}</span>'
                m = re.match(r"^(.*?\S)(\s+)(#\s.*)$", l)
                if m and l.count('"') % 2 == 0 and l.count("'") % 2 == 0:
                    return f'{m.group(1)}{m.group(2)}<span class="tok-c">{m.group(3)}</span>'
                return l
            out = "\n".join(line(l) for l in out.split("\n"))
        elif lang == "json":
            out = re.sub(r'(&quot;|")((?:[^"\\\n]|\\.)*?)\1(\s*:)?',
                         lambda m: (f'<span class="tok-k">"{m.group(2)}"</span>{m.group(3)}' if m.group(3)
                                    else f'<span class="tok-s">"{m.group(2)}"</span>'), out)
        return out

    # ---- blocks --------------------------------------------------------
    def convert(self, md: str) -> str:
        lines = md.replace("\r\n", "\n").split("\n")
        return self.blocks(lines, top=True)

    def heading_id(self, text: str) -> str:
        base = github_slug(text) or "section"
        hid, n = base, 1
        while hid in self.ids:
            n += 1
            hid = f"{base}-{n}"
        self.ids.add(hid)
        return hid

    @staticmethod
    def indent_of(line: str) -> int:
        return len(line) - len(line.lstrip(" "))

    LIST_RE = re.compile(r"^(\s*)([-*+]|\d+[.)])\s+(.*)$")
    TABLE_SEP = re.compile(r"^\s*\|?\s*:?-{3,}:?\s*(\|\s*:?-{3,}:?\s*)*\|?\s*$")

    def split_row(self, row: str) -> list[str]:
        row = row.strip()
        if row.startswith("|"):
            row = row[1:]
        if row.endswith("|") and not row.endswith("\\|"):
            row = row[:-1]
        cells, cur, in_code = [], "", False
        i = 0
        while i < len(row):
            ch = row[i]
            if ch == "`":
                in_code = not in_code
                cur += ch
            elif ch == "\\" and i + 1 < len(row) and row[i + 1] == "|":
                cur += "|" if in_code else "\\|"
                i += 1
            elif ch == "|" and not in_code:
                cells.append(cur.strip())
                cur = ""
            else:
                cur += ch
            i += 1
        cells.append(cur.strip())
        return cells

    def blocks(self, lines: list[str], top: bool = False) -> str:
        out: list[str] = []
        i, n = 0, len(lines)
        while i < n:
            line = lines[i]
            if not line.strip():
                i += 1
                continue
            # fenced code
            m = re.match(r"^(\s*)(`{3,}|~{3,})\s*([\w+-]*)", line)
            if m:
                fence, lang, ind = m.group(2), m.group(3).lower(), len(m.group(1))
                i += 1
                buf = []
                while i < n and not lines[i].strip().startswith(fence[:3]):
                    buf.append(lines[i][ind:] if lines[i][:ind].strip() == "" else lines[i])
                    i += 1
                i += 1
                code = "\n".join(buf)
                label = {"sh": "shell", "bash": "shell", "text": "output", "": ""}.get(lang, lang)
                out.append(
                    f'<div class="code" data-lang="{attr(label)}"><pre tabindex="0"><code>'
                    f'{self.highlight(code, lang)}</code></pre></div>')
                continue
            # heading
            m = re.match(r"^(#{1,6})\s+(.*?)\s*#*\s*$", line)
            if m:
                level, raw = len(m.group(1)), m.group(2)
                if level == 1 and not self.title:
                    self.title = re.sub(r"[`*]", "", raw)
                    i += 1
                    continue
                hid = self.heading_id(re.sub(r"[`*]", "", raw))
                if level in (2, 3):
                    self.toc.append((level, hid, re.sub(r"[`*]", "", raw)))
                out.append(
                    f'<h{level} id="{hid}">{self.inline(raw)}'
                    f'<a class="anchor" href="#{hid}" aria-label="Link to this section">#</a></h{level}>')
                i += 1
                continue
            # hr
            if re.match(r"^\s*([-*_])(\s*\1){2,}\s*$", line) and not self.TABLE_SEP.match(line):
                out.append("<hr>")
                i += 1
                continue
            # table
            if "|" in line and i + 1 < n and self.TABLE_SEP.match(lines[i + 1]) and "|" in lines[i + 1]:
                head = self.split_row(line)
                aligns = []
                for c in self.split_row(lines[i + 1]):
                    c = c.strip()
                    aligns.append("right" if c.endswith(":") and not c.startswith(":") else
                                  "center" if c.startswith(":") and c.endswith(":") else "")
                i += 2
                rows = []
                while i < n and lines[i].strip() and "|" in lines[i]:
                    rows.append(self.split_row(lines[i]))
                    i += 1

                def cell(tag, txt, k):
                    a = aligns[k] if k < len(aligns) else ""
                    st = f' class="al-{a}"' if a else ""
                    return f"<{tag}{st}>{self.inline(txt)}</{tag}>"
                t = ['<div class="table-wrap"><table><thead><tr>']
                t += [cell("th", c, k) for k, c in enumerate(head)]
                t.append("</tr></thead><tbody>")
                for r in rows:
                    t.append("<tr>" + "".join(cell("td", c, k) for k, c in enumerate(r)) + "</tr>")
                t.append("</tbody></table></div>")
                out.append("".join(t))
                continue
            # blockquote / callout
            if line.lstrip().startswith(">"):
                buf = []
                while i < n and lines[i].lstrip().startswith(">"):
                    buf.append(re.sub(r"^\s*> ?", "", lines[i]))
                    i += 1
                kind = None
                m = re.match(r"^\[!(\w+)\]\s*$", buf[0].strip()) if buf else None
                if m:
                    kind = m.group(1).lower()
                    buf = buf[1:]
                inner = self.blocks(buf)
                if kind:
                    title = {"note": "Note", "warning": "Warning", "tip": "Tip"}.get(kind, kind.title())
                    out.append(f'<aside class="callout callout-{kind}" role="note"><p class="callout-title">{title}</p>{inner}</aside>')
                else:
                    out.append(f"<blockquote>{inner}</blockquote>")
                continue
            # list
            m = self.LIST_RE.match(line)
            if m:
                i, html_list = self.list_block(lines, i)
                out.append(html_list)
                continue
            # paragraph
            buf = [line.strip()]
            i += 1
            while i < n and lines[i].strip():
                l = lines[i]
                if (re.match(r"^\s*(`{3,}|~{3,})", l) or re.match(r"^#{1,6}\s", l) or l.lstrip().startswith(">")
                        or re.match(r"^\s*([-*+]|1[.)])\s+", l)
                        or ("|" in l and i + 1 < n and self.TABLE_SEP.match(lines[i + 1]))):
                    break
                buf.append(l.strip())
                i += 1
            out.append(f"<p>{self.inline(' '.join(buf))}</p>")
        return "\n".join(out)

    def list_block(self, lines: list[str], i: int) -> tuple[int, str]:
        n = len(lines)
        first = self.LIST_RE.match(lines[i])
        base_indent = len(first.group(1))
        ordered = first.group(2)[0].isdigit()
        items: list[list[str]] = []
        while i < n:
            m = self.LIST_RE.match(lines[i])
            if not m or len(m.group(1)) != base_indent or (m.group(2)[0].isdigit() != ordered):
                break
            content_indent = base_indent + len(m.group(2)) + 1
            item = [m.group(3)]
            i += 1
            while i < n:
                l = lines[i]
                if not l.strip():
                    # blank: does the item continue?
                    j = i
                    while j < n and not lines[j].strip():
                        j += 1
                    if j < n and self.indent_of(lines[j]) >= content_indent:
                        item.extend([""] * (j - i))
                        i = j
                        continue
                    break
                ind = self.indent_of(l)
                if ind >= content_indent:
                    item.append(l[content_indent:])
                    i += 1
                elif self.LIST_RE.match(l) and ind <= base_indent:
                    break
                elif item[-1].strip() and not re.match(r"^\s*(`{3,}|#{1,6}\s)", l) and ind > base_indent:
                    item.append(l.strip())  # lazy continuation
                    i += 1
                elif item[-1].strip() and ind == base_indent and not self.LIST_RE.match(l):
                    item.append(l.strip())
                    i += 1
                else:
                    break
            items.append(item)
            # blank line(s) between sibling items
            j = i
            while j < n and not lines[j].strip():
                j += 1
            if j < n and j != i:
                mm = self.LIST_RE.match(lines[j])
                if mm and len(mm.group(1)) == base_indent and mm.group(2)[0].isdigit() == ordered:
                    i = j
        tag = "ol" if ordered else "ul"
        parts = []
        for item in items:
            task = ""
            m = re.match(r"^\[( |x|X)\]\s+(.*)$", item[0])
            cls = ""
            if m:
                done = m.group(1).lower() == "x"
                item[0] = m.group(2)
                task = f'<input type="checkbox" disabled{" checked" if done else ""} aria-label="{"done" if done else "not done"}"> '
                cls = ' class="task"'
            inner = self.blocks(item)
            # unwrap a lone leading paragraph for compact lists
            inner = re.sub(r"^<p>(.*?)</p>", r"\1", inner, count=1, flags=re.S) if not re.search(r"</p>\s*<p>", inner) else inner
            parts.append(f"<li{cls}>{task}{inner}</li>")
        return i, f"<{tag}>" + "".join(parts) + f"</{tag}>"


# --------------------------------------------------------------------------
# Templates
# --------------------------------------------------------------------------
THEME_BOOT = ("<script>(function(){try{var t=localStorage.getItem('ostler-theme');"
              "if(t!=='light'&&t!=='dark'){t=matchMedia('(prefers-color-scheme: dark)').matches?'dark':'light'}"
              "document.documentElement.dataset.theme=t}catch(e){}})()</script>")

ICON_SEARCH = '<svg width="16" height="16" viewBox="0 0 16 16" fill="none" aria-hidden="true"><circle cx="7" cy="7" r="4.75" stroke="currentColor" stroke-width="1.5"/><path d="M10.5 10.5 14 14" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg>'
ICON_THEME = ('<svg class="i-sun" width="16" height="16" viewBox="0 0 16 16" fill="none" aria-hidden="true"><circle cx="8" cy="8" r="3" stroke="currentColor" stroke-width="1.5"/>'
              '<path d="M8 1.5v1.8M8 12.7v1.8M1.5 8h1.8M12.7 8h1.8M3.4 3.4l1.3 1.3M11.3 11.3l1.3 1.3M3.4 12.6l1.3-1.3M11.3 4.7l1.3-1.3" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg>'
              '<svg class="i-moon" width="16" height="16" viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="M13.5 9.5A5.5 5.5 0 0 1 6.5 2.5a5.5 5.5 0 1 0 7 7Z" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"/></svg>')
ICON_MENU = '<svg width="18" height="18" viewBox="0 0 18 18" fill="none" aria-hidden="true"><path d="M2 4.5h14M2 9h14M2 13.5h14" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/></svg>'
ICON_GH = '<svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true" fill="currentColor"><path d="M8 0a8 8 0 0 0-2.53 15.59c.4.07.55-.17.55-.38v-1.33c-2.23.48-2.7-1.07-2.7-1.07-.36-.92-.89-1.17-.89-1.17-.73-.5.05-.49.05-.49.8.06 1.23.83 1.23.83.72 1.22 1.88.87 2.34.66.07-.52.28-.87.5-1.07-1.78-.2-3.65-.89-3.65-3.95 0-.87.31-1.59.83-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82a7.6 7.6 0 0 1 4 0c1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.52.56.83 1.28.83 2.15 0 3.07-1.87 3.75-3.66 3.95.29.25.54.74.54 1.48v2.2c0 .21.15.46.55.38A8 8 0 0 0 8 0Z"/></svg>'


def header(prefix: str, section: str) -> str:
    """Shared top bar. prefix is the relative path to the site root ('' or '../')."""
    def cur(name):
        return ' aria-current="page"' if section == name else ""
    return f'''<header class="topbar">
<div class="topbar-inner">
<button class="icon-btn nav-toggle" type="button" aria-label="Open navigation" aria-controls="sidebar" aria-expanded="false">{ICON_MENU}</button>
<a class="brand" href="{prefix}index.html"><img src="{prefix}assets/logo.svg" alt="" width="26" height="26"><span>Ostler</span></a>
<nav class="topnav" aria-label="Main">
<a href="{prefix}docs/index.html"{cur("docs")}>Docs</a>
<a href="{prefix}docs/tools.html">Tools</a>
<a href="{prefix}docs/benchmarks.html">Benchmarks</a>
<a href="{prefix}docs/changelog.html">Changelog</a>
</nav>
<div class="topbar-actions">
<button class="search-btn" type="button" data-search-open aria-label="Search the docs" aria-keyshortcuts="Control+K Meta+K">{ICON_SEARCH}<span class="search-label">Search docs</span><kbd>/</kbd></button>
<button class="icon-btn theme-toggle" type="button" aria-label="Switch color theme" aria-pressed="false">{ICON_THEME}</button>
<a class="gh-link" href="{GITHUB}" rel="noopener">{ICON_GH}<span>GitHub</span></a>
</div>
</div>
</header>'''


SEARCH_DIALOG = '''<dialog class="search-dialog" id="search-dialog" aria-label="Search the documentation">
<form method="dialog" class="search-form" role="search">
<label class="visually-hidden" for="search-input">Search the documentation</label>
<input id="search-input" type="search" placeholder="Search the docs" autocomplete="off" spellcheck="false" role="combobox" aria-expanded="false" aria-controls="search-results" aria-autocomplete="list">
<button class="search-close" value="close" aria-label="Close search">Esc</button>
</form>
<div id="search-results" class="search-results" role="listbox" aria-label="Search results"></div>
<p class="search-status visually-hidden" id="search-status" aria-live="polite"></p>
<p class="search-foot"><span><kbd>&uarr;</kbd><kbd>&darr;</kbd> to move</span><span><kbd>Enter</kbd> to open</span><span><kbd>Esc</kbd> to close</span></p>
</dialog>'''


def sidebar(current: str) -> str:
    parts = ['<nav class="sidebar" id="sidebar" aria-label="Documentation">']
    parts.append('<div class="sidebar-head"><span>Documentation</span>'
                 '<button class="icon-btn sidebar-close" type="button" aria-label="Close navigation">&times;</button></div>')
    cur = ' aria-current="page"' if current == "index" else ""
    parts.append(f'<a class="side-overview" href="index.html"{cur}>Overview</a>')
    for gname, items in GROUPS:
        parts.append(f'<div class="side-group"><p class="side-title">{esc(gname)}</p><ul>')
        for slug, title, _d, _s in items:
            cur = ' aria-current="page"' if slug == current else ""
            parts.append(f'<li><a href="{slug}.html"{cur}>{esc(title)}</a></li>')
        parts.append("</ul></div>")
    parts.append(f'<div class="side-group"><p class="side-title">More</p><ul>'
                 f'<li><a href="{GITHUB}/tree/main/docs/adr" rel="noopener">Decision records</a></li>'
                 f'<li><a href="{GITHUB}" rel="noopener">GitHub repository</a></li></ul></div>')
    parts.append("</nav>")
    return "\n".join(parts)


def json_ld(data: dict) -> str:
    # `</` must not appear inside a script element.
    return ('<script type="application/ld+json">'
            + json.dumps(data, ensure_ascii=False, separators=(",", ":")).replace("</", "<\\/")
            + "</script>")


def head(title: str, desc: str, canonical: str, prefix: str, ld: dict | None = None, og_type: str = "website") -> str:
    return f'''<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>{esc(title)}</title>
<meta name="description" content="{attr(desc)}">
<link rel="icon" href="{prefix}favicon.svg" type="image/svg+xml">
<link rel="apple-touch-icon" href="{prefix}assets/icon-180.png">
<link rel="canonical" href="{attr(SITE_URL + canonical)}">
<link rel="alternate" type="text/plain" title="llms.txt" href="{prefix}llms.txt">
<meta name="color-scheme" content="light dark">
<meta property="og:type" content="{og_type}"><meta property="og:site_name" content="Ostler">
<meta property="og:title" content="{attr(title)}"><meta property="og:description" content="{attr(desc)}">
<meta property="og:url" content="{attr(SITE_URL + canonical)}"><meta property="og:image" content="{SITE_URL}assets/og.png">
<meta property="og:image:alt" content="Ostler">
<meta name="twitter:card" content="summary_large_image"><meta name="twitter:title" content="{attr(title)}"><meta name="twitter:description" content="{attr(desc)}"><meta name="twitter:image" content="{SITE_URL}assets/og.png">
{json_ld(ld) if ld else ""}
{THEME_BOOT}
<link rel="stylesheet" href="{prefix}assets/site.css">
<script defer src="{prefix}assets/site.js"></script>
</head>'''


def page_ld(page: dict) -> dict:
    url = f"{SITE_URL}docs/{page['slug']}.html"
    return {
        "@context": "https://schema.org",
        "@graph": [
            {
                "@type": "TechArticle",
                "headline": page["title"],
                "description": page["desc"],
                "url": url,
                "inLanguage": "en",
                "isPartOf": {"@type": "WebSite", "name": "Ostler", "url": SITE_URL},
                "about": {"@type": "SoftwareApplication", "name": "Ostler", "url": SITE_URL},
                "author": {"@type": "Person", "name": "Mihir Inamdar", "url": "https://github.com/inamdarmihir"},
                "license": "https://www.apache.org/licenses/LICENSE-2.0",
            },
            {
                "@type": "BreadcrumbList",
                "itemListElement": [
                    {"@type": "ListItem", "position": 1, "name": "Ostler", "item": SITE_URL},
                    {"@type": "ListItem", "position": 2, "name": "Docs", "item": f"{SITE_URL}docs/index.html"},
                    {"@type": "ListItem", "position": 3, "name": page["title"], "item": url},
                ],
            },
        ],
    }


def footer(prefix: str) -> str:
    return f'''<footer class="site-footer"><div class="footer-inner">
<p>Ostler is Apache-2.0 open source. v0.2, early: tested on local fixtures, not yet on the open web. Maintained by <a href="https://github.com/inamdarmihir" rel="author">Mihir Inamdar</a>.</p>
<nav aria-label="Footer"><a href="{prefix}docs/index.html">Docs</a><a href="{prefix}docs/security.html">Security</a><a href="{prefix}docs/changelog.html">Changelog</a><a href="{prefix}llms.txt">llms.txt</a><a href="{GITHUB}" rel="noopener">GitHub</a></nav>
</div></footer>'''


def page_html(page: dict, body: str, toc: list, idx: int) -> str:
    slug, title = page["slug"], page["title"]
    toc_items = "".join(
        f'<li class="toc-l{lv}"><a href="#{hid}">{esc(txt)}</a></li>' for lv, hid, txt in toc)
    toc_aside = (f'<aside class="toc" aria-label="On this page"><p class="toc-title">On this page</p><ul>{toc_items}</ul></aside>'
                 if toc else '<aside class="toc" aria-hidden="true"></aside>')
    toc_inline = (f'<details class="toc-inline"><summary>On this page</summary><ul>{toc_items}</ul></details>'
                  if toc else "")
    prev_p = PAGES[idx - 1] if idx > 0 else None
    next_p = PAGES[idx + 1] if idx + 1 < len(PAGES) else None
    pn = ['<nav class="pager" aria-label="Previous and next pages">']
    pn.append(f'<a class="pager-prev" rel="prev" href="{prev_p["slug"]}.html"><span>Previous</span><strong>{esc(prev_p["title"])}</strong></a>'
              if prev_p else '<a class="pager-prev" href="index.html"><span>Docs</span><strong>Overview</strong></a>')
    pn.append(f'<a class="pager-next" rel="next" href="{next_p["slug"]}.html"><span>Next</span><strong>{esc(next_p["title"])}</strong></a>'
              if next_p else "<span></span>")
    pn.append("</nav>")
    kind, src = page["src"]
    src_path = f"site/tools/content/{src}" if kind == "content" else src
    edit = f'{GITHUB}/blob/main/{src_path}'
    return "\n".join([
        head(f"{title} - Ostler MCP browser docs", page["desc"], f"docs/{slug}.html", "../", page_ld(page), "article"),
        '<body data-search="search-index.json">',
        '<a class="skip-link" href="#content">Skip to content</a>',
        header("../", "docs"),
        '<div class="doc-layout">',
        sidebar(slug),
        '<div class="scrim" data-scrim hidden></div>',
        '<main id="content" class="doc-main" tabindex="-1">',
        f'<p class="crumbs"><a href="index.html">Docs</a> <span aria-hidden="true">/</span> {esc(page["group"])}</p>',
        f'<article class="prose"><h1>{esc(title)}</h1>',
        f'<p class="lede">{esc(page["desc"])}</p>',
        toc_inline,
        body,
        f'<p class="edit-link"><a href="{edit}" rel="noopener">View this page&rsquo;s source on GitHub</a></p>',
        "</article>",
        "\n".join(pn),
        "</main>",
        toc_aside,
        "</div>",
        footer("../"),
        SEARCH_DIALOG,
        "</body></html>",
    ])


def overview_html() -> str:
    cards = []
    for gname, items in GROUPS:
        cards.append(f'<h2 id="{github_slug(gname)}">{esc(gname)}</h2><div class="card-grid">')
        for slug, title, desc, _ in items:
            cards.append(f'<a class="doc-card" href="{slug}.html"><strong>{esc(title)}</strong><span>{esc(desc)}</span></a>')
        cards.append("</div>")
    desc = "Documentation for Ostler, an open-source MCP browser for AI coding agents: research with checked citations, verified form filling, Qdrant memory."
    return "\n".join([
        head("Ostler documentation: MCP browser for AI agents", desc, "docs/index.html", "../"),
        '<body data-search="search-index.json">',
        '<a class="skip-link" href="#content">Skip to content</a>',
        header("../", "docs"),
        '<div class="doc-layout no-toc">',
        sidebar("index"),
        '<div class="scrim" data-scrim hidden></div>',
        '<main id="content" class="doc-main" tabindex="-1">',
        '<article class="prose"><h1>Ostler documentation</h1>',
        f'<p class="lede">{esc(DEFINITION)} It speaks MCP to coding agents and A2A to other agents.</p>',
        '<aside class="callout callout-note" role="note"><p class="callout-title">Status: v0.2, early</p><p>Tested end to end against real Chrome and a real Qdrant server, but evaluated on local fixtures, not the open web. The docs say what was measured and what was not. Features marked <span class="badge badge-unreleased">Unreleased</span> are on the main branch and not in a tagged release.</p></aside>',
        '<div class="start-row"><a class="btn primary" href="quickstart.html">Quickstart</a><a class="btn" href="agents.html">Use with your agent</a><a class="btn" href="tools.html">Tools reference</a></div>',
        "\n".join(cards),
        "</article></main></div>",
        footer("../"),
        SEARCH_DIALOG,
        "</body></html>",
    ])


# --------------------------------------------------------------------------
def plain_text(markup: str) -> str:
    t = re.sub(r"<a class=\"anchor\".*?</a>", "", markup)
    t = re.sub(r"<[^>]+>", " ", t)
    return re.sub(r"\s+", " ", html.unescape(t)).strip()


def chunks(text: str, size: int) -> list[str]:
    out, cur = [], ""
    for word in text.split(" "):
        if cur and len(cur) + len(word) + 1 > size:
            out.append(cur)
            cur = word
        else:
            cur = f"{cur} {word}".strip()
    if cur:
        out.append(cur)
    return out


def sitemap() -> str:
    urls = ["", "docs/index.html"] + [f"docs/{p['slug']}.html" for p in PAGES]
    rows = "".join(f"<url><loc>{SITE_URL}{u}</loc></url>\n" for u in urls)
    return ('<?xml version="1.0" encoding="UTF-8"?>\n'
            '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n' + rows + "</urlset>\n")


def llms_txt() -> str:
    """The llms.txt index (https://llmstxt.org): what Ostler is, then every page."""
    out = [
        "# Ostler",
        "",
        f"> {DEFINITION}",
        "",
        "Ostler is one local Rust binary (Apache-2.0) that runs Chrome and speaks MCP over stdio to coding agents "
        "and A2A over HTTP to other agents. It has no LLM inside, no cloud browser and no anti-bot evasion. "
        "Status: v0.2, early; tested against real Chrome and Qdrant on local fixtures, not yet benchmarked on the open web.",
        "",
        "Key facts:",
        "",
        "- Install: `cargo install --git https://github.com/inamdarmihir/ostler ostler`, then `ostler setup claude --apply` (Claude Code), or see the agents page for Codex, Cursor and VS Code.",
        "- Requirements: Rust 1.89+ and Chrome or Chromium. Prebuilt binaries arrive with the first tagged release.",
        "- Evidence: local fixtures; a five-task scripted comparison with Playwright MCP and Chrome DevTools MCP; seven live public practice pages. Not yet measured on the open web.",
        "- Research: `read` returns the best passages for a query; `cite` marks a quote VERIFIED only if its words are on a page read this session; `evidence` lists citations and the pages that could not be read (paywall, bot wall, robots.txt).",
        "- Forms: `form_fill` fills a whole form in one call and reads every value back; 3 tool calls instead of 22 on a 20-field form (same engine, one call per field; scripted, model-free, local fixtures).",
        "- Memory: optional, in Qdrant or an encrypted local file; stores facts you save and per-site playbooks (structure, not values). Secret-looking values are refused; detection is heuristic.",
        "- Safety: page text is fenced as untrusted, a domain allowlist holds every request, CAPTCHAs are handed to a person.",
        "",
        "## Docs",
        "",
    ]
    for gname, items in GROUPS:
        for slug, title, desc, _ in items:
            out.append(f"- [{title}]({SITE_URL}docs/{slug}.html): {desc}")
    out += [
        "",
        "## Optional",
        "",
        f"- [Full documentation as one Markdown file]({SITE_URL}llms-full.txt)",
        f"- [Source code on GitHub]({GITHUB})",
        f"- [README]({GITHUB}/blob/main/README.md)",
        "",
    ]
    return "\n".join(out)


def bundle_md(md: str, conv: "Converter") -> str:
    """Make one chapter readable out of context: absolute links, no repo: or :unreleased: markers."""
    from urllib.parse import urljoin

    md = md.replace(":unreleased:", "**Unreleased: main branch, not a tagged release.**")

    def link(m: re.Match) -> str:
        url, external = conv.resolve(m.group(2))
        if not external and not url.startswith("#"):
            url = urljoin(SITE_URL + "docs/", url)
        return f"[{m.group(1)}]({url}{m.group(3)})"

    return re.sub(r'\[([^\]]+)\]\(([^)\s]+)((?:\s+"[^"]*")?)\)', link, md)


def build() -> int:
    OUT.mkdir(parents=True, exist_ok=True)
    index = []
    full = [llms_txt().split("## Docs")[0].rstrip(), ""]
    for idx, page in enumerate(PAGES):
        kind, src = page["src"]
        path = (CONTENT / src) if kind == "content" else (REPO / src)
        if not path.exists():
            print(f"missing source: {path}", file=sys.stderr)
            return 1
        md = path.read_text(encoding="utf-8")
        intro = INTROS.get(page["slug"])
        conv = Converter(kind, src if kind == "repo" else "")
        body = conv.convert(md)
        if intro:
            body = f"<p>{Converter('content', '').inline(intro)}</p>\n" + body
        (OUT / f"{page['slug']}.html").write_text(page_html(page, body, conv.toc, idx), encoding="utf-8")
        if page["slug"] != "changelog":  # history, not documentation
            full += ["", "---", "", f"Source: {SITE_URL}docs/{page['slug']}.html", "", bundle_md(md, conv).strip(), ""]

        # search index: split the body at h2/h3
        pieces = re.split(r'(?=<h[23] id=")', body)
        for piece in pieces:
            m = re.match(r'<h[23] id="([^"]+)">(.*?)<a class="anchor"', piece, re.S)
            text = plain_text(piece)
            if m:
                sec = plain_text(m.group(2))
                url = f"{page['slug']}.html#{m.group(1)}"
                text = text[len(sec):].strip() if text.startswith(sec) else text
            else:
                sec, url = "", f"{page['slug']}.html"
                text = page["desc"] + " " + text
            # long sections are split into ~700-character chunks so nothing is
            # lost to truncation; the client collapses chunks of one section.
            for chunk in chunks(text, 700) or [""]:
                if chunk or sec:
                    index.append({"p": page["title"], "s": sec, "u": url, "t": chunk})
    (OUT / "index.html").write_text(overview_html(), encoding="utf-8")
    (OUT / "search-index.json").write_text(json.dumps(index, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    (SITE / "sitemap.xml").write_text(sitemap(), encoding="utf-8")
    (SITE / "llms.txt").write_text(llms_txt(), encoding="utf-8")
    (SITE / "llms-full.txt").write_text("\n".join(full), encoding="utf-8")
    print(f"built {len(PAGES) + 1} pages, {len(index)} search entries, sitemap and llms.txt -> {SITE}")
    return 0


if __name__ == "__main__":
    sys.exit(build())
