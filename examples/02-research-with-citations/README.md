# Research with checked citations

**Use it for:** Any answer someone will rely on: a fact, a comparison, what a source says.  
**Tools:** `open_many` `read` `cite` `evidence`  
**Documented in:** [Cookbook recipe 13](https://github.com/inamdarmihir/ostler/blob/main/docs/COOKBOOK.md#13-research-with-citations-that-are-checked)

## Ask your agent

> Who designed the Kestrel Lock and when did it open? Start at http://127.0.0.1:8787/packs/research/index.html. Use `read` or `open_many`, back every claim with `cite`, and call `evidence` before answering. If a page could not be read, say so instead of guessing.

## Best practices shown here

- Read first, then cite the page's own words: `cite` refuses quotes from pages not read in this session.
- Treat `NOT VERIFIED` as a stop sign, not a retry prompt: rephrase from the page's text or drop the claim.
- Always finish with `evidence` and report its gaps (paywall, bot wall, robots.txt) in the answer.
- `cite` proves the words are on the page, not that the claim is true; keep claims as narrow as the quote.

## Run it

```sh
ostler fixtures &
python3 run.py
```

The script drives the same tools an agent would, with no model in the loop, and stops with `FAIL:` if any check does not hold.

## Real output

Recorded against Ostler built from `main` on 2026-11-08, with the bundled fixture pages; timestamps normalised.

```text

> open_many {'urls': "['http://127.0.0.1:8787/packs/research/kestrel-lock.html', 'http://127.0.0.1:...", 'query': 'who designed the lock and when did it open'}
open_many: 2 read, 1 not read. 3 sources: 1 ok; 2 gaps: 1 blocked: 127.0.0.1, 1 paywalled

[1] <page_content untrusted="true">The Kestrel Lock</page_content> — <page_content untrusted="true">http://127.0.0.1:8787/packs/research/kestrel-lock.html</page_content> · 297 chars · sha256:9f602117
<page_content untrusted="true">
[p1 · The Kestrel Lock]
# The Kestrel Lock

The Kestrel Lock is a double-chamber canal lock on the Kestrel River. It opened in 1887 and was designed by [Marta Ilves](http://127.0.0.1:8787/packs/research/marta-ilves.html).

A major restoration in 1962 was supervised by the engineer Tomas Reyes, who is sometimes wrongly credited with the original design.

See also: [Archive index](http://127.0.0.1:8787/packs/research/index.html).
</page_content>

[2] <page_content untrusted="true">The Valley Courier: the cost of restoring the Kestrel Lock</page_content> — <page_content untrusted="true">http://127.0.0.1:8787/packs/research/paywall.html</page_content> · 293 chars · sha256:8952387c · PAYWALL: teaser only
<page_content untrusted="true">
[p1 · The cost of restoring the Kestrel Lock]
# The cost of restoring the Kestrel Lock

The Valley Courier (synthetic). When the council approved the Kestrel Lock restoration, nobody expected the bill to become the talk of the valley...
</page_content>
GAP blocked: 127.0.0.1 — http://127.0.0.1:8787/packs/research/protected.html — NOT READ: 127.0.0.1 verification wall instead of the page. Ostler does not solve or bypass these; get the information elsewhere.

To use any of this in an answer, call cite with the page URL and an exact quote.
CHECK OK: the paywalled page is reported as a gap

> cite {'url': 'http://127.0.0.1:8787/packs/research/kestrel-lock.html', 'quote': 'It opened in 1887 and was designed by Marta Ilves'}
VERIFIED [1] <page_content untrusted="true">http://127.0.0.1:8787/packs/research/kestrel-lock.html</page_content> (read 2026-11-08T02:00:00Z, sha256:9f602117)
<page_content untrusted="true">It opened in 1887 and was designed by Marta Ilves</page_content>
Check it yourself (best effort): http://127.0.0.1:8787/packs/research/kestrel-lock.html#:~:text=It%20opened%20in%201887%20and%20was%20designed%20by%20Marta%20Ilves
CHECK OK: the real quote is VERIFIED

> cite {'url': 'http://127.0.0.1:8787/packs/research/kestrel-lock.html', 'quote': 'Tomas Reyes designed the Kestrel Lock in 1887'}
ERROR NOT VERIFIED: these words are not on <page_content untrusted="true">The Kestrel Lock</page_content>. Do not state this claim as sourced; quote the page's own words, not a summary.
Closest text on the page (similarity 0.48): <page_content untrusted="true">The Kestrel Lock The Kestrel Lock is a double</page_content>
CHECK OK: the decoy quote is refused

> evidence 
EVIDENCE: 2 page(s) read, 1 verified citation(s), 2 gap kind(s). 3 sources: 1 ok; 2 gaps: 1 blocked: 127.0.0.1, 1 paywalled

Verified (state only what these support):
[1] <page_content untrusted="true">http://127.0.0.1:8787/packs/research/kestrel-lock.html</page_content> — <page_content untrusted="true">It opened in 1887 and was designed by Marta Ilves</page_content>
    check: http://127.0.0.1:8787/packs/research/kestrel-lock.html#:~:text=It%20opened%20in%201887%20and%20was%20designed%20by%20Marta%20Ilves

Gaps (not read; say so rather than guess what they contain):
- blocked: 127.0.0.1: http://127.0.0.1:8787/packs/research/protected.html
- paywalled: http://127.0.0.1:8787/packs/research/paywall.html

Read but not cited:
- <page_content untrusted="true">http://127.0.0.1:8787/packs/research/paywall.html</page_content>
CHECK OK: evidence lists exactly one verified citation
CHECK OK: evidence names the pages that could not be read
```
