# Extract tables and lists, filtered

**Use it for:** Pulling rows out of a page without reading the whole page into the model.  
**Tools:** `extract` with `where` and `select`  
**Documented in:** [ADR-0019](https://github.com/inamdarmihir/ostler/blob/main/docs/adr/0019-scripted-runs-and-result-filtering.md)

## Ask your agent

> From http://127.0.0.1:8787/packs/research/table.html, give me the South region's 2024 H2 output by sub-region.

## Best practices shown here

- Filter with `where` and `select` before the result is returned; the filter states how many rows it kept.
- A list that says `PARTIAL: 5 of 13` is not the whole set: follow the next link (example 08).
- Extracted cells are not citations; to quote a value as evidence, `read` and `cite` it.

## Run it

```sh
ostler fixtures &
python3 run.py
```

The script drives the same tools an agent would, with no model in the loop, and stops with `FAIL:` if any check does not hold.

## Real output

Recorded against Ostler built from `main` on 2026-11-08, with the bundled fixture pages; timestamps normalised.

```text

> navigate {'url': 'http://127.0.0.1:8787/packs/research/table.html'}
Navigated. Now at: <page_content untrusted="true">Regional grain output</page_content> — <page_content untrusted="true">http://127.0.0.1:8787/packs/research/table.html</page_content>
<page_content untrusted="true">
heading "Regional grain output" [e1] level=1
table:
</page_content>
Only controls are listed; page text (article body, search snippets) is omitted. Call snapshot to read it.

> extract {'kind': 'tables'}
1 table(s). Cells are as written; quoting them requires read + cite.
<page_content untrusted="true">
Table 1 "Grain output, kt" (4 row(s))
| Region | Sub-region | 2023 / H1 | 2023 / H2 | 2024 / H1 | 2024 / H2 |
| North | Coastal | 41 | 44 | 47 | 52 |
| North | Upland | 30 | 33 | 36 | 38 |
| South | Delta | 58 | 61 | 63 | 71 |
| South | Plateau | 25 | 27 | 29 | 31 |
</page_content>
CHECK OK: the table comes back as rows

> extract {'kind': 'tables', 'where': 'South', 'select': ['Sub-region', '2024 / H2']}
1 table(s). Cells are as written; quoting them requires read + cite.
<page_content untrusted="true">
Table 1 "Grain output, kt" (4 row(s))
| Sub-region | 2024 / H2 |
| Delta | 71 |
| Plateau | 31 |
</page_content>
Filtered: 2 of 4 row(s) kept (where south).
CHECK OK: the filter states how many rows it kept
CHECK OK: only the South rows and two columns are left

> navigate {'url': 'http://127.0.0.1:8787/packs/research/catalog-1.html'}
Navigated. Now at: <page_content untrusted="true">Heritage Locks Register, page 1</page_content> — <page_content untrusted="true">http://127.0.0.1:8787/packs/research/catalog-1.html</page_content>
<page_content untrusted="true">
heading "Heritage Locks Register" [e1] level=1
link "Next page" [e2] href="/packs/research/catalog-2.html"
</page_content>
Only controls are listed; page text (article body, search snippets) is omitted. Call snapshot to read it.

> extract {'kind': 'lists'}
PARTIAL: 5 of 13 stated. More pages exist; follow the next link before concluding anything about the whole set.
<page_content untrusted="true">
No card-like list found (plain list items are in read); the page's own paging text follows.
</page_content>
<page_content untrusted="true">
next: link → http://127.0.0.1:8787/packs/research/catalog-2.html
</page_content>
CHECK OK: page 1 of the register is reported as partial
```
