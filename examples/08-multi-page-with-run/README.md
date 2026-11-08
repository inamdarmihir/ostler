# A multi-page job in one call with `run`

**Use it for:** Paginated registers, result pages, any loop that would otherwise cost one model turn per page.  
**Tools:** `run` (Rhai), calling `read`  
**Documented in:** [ADR-0019](https://github.com/inamdarmihir/ostler/blob/main/docs/adr/0019-scripted-runs-and-result-filtering.md)

## Ask your agent

> List every lock in the Heritage Locks Register, all pages, starting at http://127.0.0.1:8787/packs/research/catalog-1.html. Use a `run` script so only the matching lines come back.

## Best practices shown here

- Loops and filtering belong in the script; only the final value returns to the model.
- The script can only call Ostler tools: no files, no network, at most 60 calls, and the allowlist and robots.txt still apply.
- Check completeness against the page's own count (13 here) before answering about the whole set.

## Run it

```sh
ostler fixtures &
python3 run.py
```

The script drives the same tools an agent would, with no model in the loop, and stops with `FAIL:` if any check does not hold.

## Real output

Recorded against Ostler built from `main` on 2026-11-08, with the bundled fixture pages; timestamps normalised.

```text

> run {'script': '\nlet found = [];\nfor n in 1..4 {\n    let page = tool("read", #{ url: `http://...'}
Script finished: 3 tool call(s).
<page_content untrusted="true">
13 locks found across 3 pages
["1. Kestrel Lock, first operated 1887","2. Millrace Lock, first operated 1842","3. Fennick Lock, first operated 1856","4. Tarn Lock, first operated 1903","5. Harrow Staircase, first operated 1820","6. Weir End Lock, first operated 1869","7. Sedge Lock, first operated 1911","8. Pike Lock, first operated 1878","9. Lantern Lock, first operated 1834","10. Cobb Lock, first operated 1895","11. Ashby Staircase, first operated 1791","12. Reed Lock, first operated 1925","13. Dunmere Lock, first operated 1809"]
</page_content>
CHECK OK: three reads, one model turn
CHECK OK: the whole register was collected
```
