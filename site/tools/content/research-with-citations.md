# Research with checked citations

Ostler checks a quotation against the text of a page it actually read in this session, and lists the pages it could not read. An agent using `read`, `cite` and `evidence` can then answer with only the claims that were verified, and say plainly what it could not check. It runs on your machine, in your Chrome, from any MCP client such as Claude Code, Codex or Cursor.

**What this does not do.** `cite` proves that the words are on the page. It does not prove that the claim is true, that the page is a good source, or that the agent quoted in context. Quote matching is not fact-checking.

## The workflow

Three tools carry it. They are in the `research` profile; `--tools research` pins them, and the default `--tools auto` reveals them after the first `read`. All 30 tools are in the [tools reference](tools.html#research-profile-research).

| step | tool | what it does |
|---|---|---|
| 1 | `read` | Opens a page and returns the best passages for your `query`, with a fetch time and a content hash. The page text is stored for this session. |
| 2 | `cite` | Takes a `url` and a `quote` (12 or more characters). Returns `VERIFIED` with a link a person can open to check the words, or `NOT VERIFIED`, `NOT READ` or `TOO SHORT`. |
| 3 | `evidence` | Lists verified citations, pages read but not cited, and gaps: paywall, bot wall, robots.txt refusal, HTTP error, empty page. Call it before answering. |

`open_many` reads up to 10 URLs at once in a scratch tab and obeys robots.txt. `search` finds pages to read; it needs a provider (`OSTLER_SEARXNG_URL`, or `OSTLER_SEARCH_PROVIDER=brave` with `OSTLER_BRAVE_API_KEY`) and has been tested against mock servers only.

## Try it on the bundled pages

Nothing leaves your machine. Install Ostler from source ([install guide](install.html)), then start the local fixture server:

```sh
ostler fixtures        # serves http://127.0.0.1:8787, loopback only
```

Add `ostler serve --tools research` to your MCP client ([setup for each client](agents.html)), and give the agent this prompt:

> Find out who designed the Kestrel Lock and when it opened. Use `read`, then back every claim with `cite`, and finish with `evidence`. If a page could not be read, say so instead of guessing. Start at `http://127.0.0.1:8787/packs/research/index.html`.

## Expected output

This is unedited tool output from `ostler 0.2.0` built from the `main` branch (no tagged release), captured on 2026-11-06 against `ostler fixtures` with `ostler serve --tools research`. Fetch times and the shown similarity score can differ on your run; the structure and verdicts should not.

### A page is read

```text
> read {"url": "http://127.0.0.1:8787/packs/research/kestrel-lock.html", "query": "who designed the lock"}
Read: <page_content untrusted="true">The Kestrel Lock</page_content> — <page_content untrusted="true">http://127.0.0.1:8787/packs/research/kestrel-lock.html</page_content> · page · 297 chars · fetched 2026-11-06T16:01:32Z · sha256:9f602117
1 of 1 passages for <page_content untrusted="true">who designed the lock</page_content>:
<page_content untrusted="true">
[p1 · The Kestrel Lock]
# The Kestrel Lock

The Kestrel Lock is a double-chamber canal lock on the Kestrel River. It opened in 1887 and was designed by [Marta Ilves](http://127.0.0.1:8787/packs/research/marta-ilves.html).

A major restoration in 1962 was supervised by the engineer Tomas Reyes, who is sometimes wrongly credited with the original design.

See also: [Archive index](http://127.0.0.1:8787/packs/research/index.html).
</page_content>
To use any of this in an answer, call cite with the page URL and an exact quote.
```

`<page_content untrusted="true">` marks text that came from the page. The agent is meant to treat it as data, not as instructions.

### A quote that is on the page

```text
> cite {"url": "http://127.0.0.1:8787/packs/research/kestrel-lock.html", "quote": "opened in 1887 and was designed by Marta Ilves"}
VERIFIED [1] <page_content untrusted="true">http://127.0.0.1:8787/packs/research/kestrel-lock.html</page_content> (read 2026-11-06T16:01:32Z, sha256:9f602117)
<page_content untrusted="true">opened in 1887 and was designed by Marta Ilves</page_content>
Check it yourself (best effort): http://127.0.0.1:8787/packs/research/kestrel-lock.html#:~:text=opened%20in%201887%20and%20was%20designed%20by%20Marta%20Ilves
```

The `#:~:text=` link opens the page with the quoted words highlighted, so a person can check the source without trusting the agent.

### A quote that is not on the page

The page says the 1962 restoration was supervised by Tomas Reyes, who "is sometimes wrongly credited with the original design". An agent that skims could easily write that Reyes designed the lock. Ostler rejects the claim:

```text
> cite {"url": "http://127.0.0.1:8787/packs/research/kestrel-lock.html", "quote": "Tomas Reyes designed the Kestrel Lock in 1887"}
NOT VERIFIED: these words are not on <page_content untrusted="true">The Kestrel Lock</page_content>. Do not state this claim as sourced; quote the page's own words, not a summary.
Closest text on the page (similarity 0.48): <page_content untrusted="true">The Kestrel Lock The Kestrel Lock is a double</page_content>
```

The agent is told not to state the claim as sourced. In this run the "closest text" it offered was the page's title and the start of its first sentence, at similarity 0.48, which is not the sentence that matters. Treat that hint as a pointer to look again, not as the answer.

### A page that cannot be fully read

```text
> read {"url": "http://127.0.0.1:8787/packs/research/paywall.html"}
Read: <page_content untrusted="true">The Valley Courier: the cost of restoring the Kestrel Lock</page_content> — <page_content untrusted="true">http://127.0.0.1:8787/packs/research/paywall.html</page_content> · page · 293 chars · fetched 2026-11-06T16:01:32Z · sha256:8952387c
PAYWALL: only a teaser is readable here. Do not state what the rest of the article says; report it as a gap.
<page_content untrusted="true">
# The cost of restoring the Kestrel Lock

The Valley Courier (synthetic). When the council approved the Kestrel Lock restoration, nobody expected the bill to become the talk of the valley...

## Subscribe to keep reading

This article is for subscribers only. Sign in or subscribe to read the rest.
</page_content>
To use any of this in an answer, call cite with the page URL and an exact quote.
```

### What the agent may say

```text
> evidence {}
EVIDENCE: 2 page(s) read, 1 verified citation(s), 1 gap kind(s). 2 sources: 1 ok; 1 gaps: 1 paywalled

Verified (state only what these support):
[1] <page_content untrusted="true">http://127.0.0.1:8787/packs/research/kestrel-lock.html</page_content> — <page_content untrusted="true">opened in 1887 and was designed by Marta Ilves</page_content>
    check: http://127.0.0.1:8787/packs/research/kestrel-lock.html#:~:text=opened%20in%201887%20and%20was%20designed%20by%20Marta%20Ilves

Gaps (not read; say so rather than guess what they contain):
- paywalled: http://127.0.0.1:8787/packs/research/paywall.html

Read but not cited:
- <page_content untrusted="true">http://127.0.0.1:8787/packs/research/paywall.html</page_content>
```

The paywalled page is a gap: the agent must report that it could not read it, and must not guess what the article says. In this output the same page also appears under "Read but not cited"; that is how the report looks today, and it does not change the rule that a gap is not evidence.

## What it checks, and what it does not

| Checked | Not checked |
|---|---|
| The quoted words are on a page read in this session (Markdown around them is ignored, `...` marks an omission). | That the claim is true, current or well sourced. |
| The agent cannot cite a page it never read: `NOT READ` is an error. | Whether the quote is used in its original context. |
| A changed page invalidates earlier citations: `read` reports `CHANGED` and `evidence` lists them as stale. | Pages behind a bot wall or paywall: these are reported as gaps, not worked around. |
| Text aimed at the agent is flagged (`ATTENTION`), not obeyed. | PDFs: there is no PDF text layer. |

Other limits today: pages are read one at a time, sources are not kept across sessions, and `search` has not been run against a live provider in tests.

## Provenance and evidence

- Tool behaviour: [tools reference](tools.html#research-profile-research) and the [cookbook recipe](cookbook.html#13-research-with-citations-that-are-checked).
- Evidence for these tools is a transcript on bundled fixtures, plus the project's tests. There is no published accuracy evaluation of citation checking on real pages yet; see [benchmarks](benchmarks.html) for what has and has not been measured.
- Source code: [github.com/inamdarmihir/ostler](https://github.com/inamdarmihir/ostler).

## Next step

For forms rather than research, see [verified form filling](forms.html).
