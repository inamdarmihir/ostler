# Ostler examples

Runnable examples for [Ostler](https://github.com/inamdarmihir/ostler), the open-source MCP browser that checks its own work. Each example is one use case from the official documentation, written as a short Python script that drives the same tools your agent would, plus the prompt to give the agent and the output Ostler really returned.

Every example runs against the bundled fixture pages (`ostler fixtures`), so nothing real is visited or submitted, and every script checks its own results and stops with `FAIL:` when a check does not hold. They contain no language model: the point is to show what each tool returns and how to act on it.

## Quick start

```sh
pip install ostler-mcp            # or build from source: cargo install --git https://github.com/inamdarmihir/ostler ostler
pip install -r requirements.txt   # the MCP Python SDK
ostler doctor                     # Chrome found? prints the fix when not
ostler fixtures &                 # demo pages on http://127.0.0.1:8787
python run_all.py                 # runs every example and prints a summary
```

Example 11 also needs `ostler a2a --allow-domain 127.0.0.1 &`. Run one example with `python 02-research-with-citations/run.py`, or a few with `python run_all.py 02 06`.

## The examples

| # | use case | tools | documented in | needs |
|---|---|---|---|---|
| [01](01-browse-and-read/) | Browse and read a page | `navigate` `click` `snapshot` | Cookbook 1 | 0.2.0 |
| [02](02-research-with-citations/) | Research with checked citations | `open_many` `cite` `evidence` | Cookbook 13 | 0.2.0 |
| [03](03-verified-form/) | Fill a form, verify every value | `form_fill` `form_submit` | Cookbook 7 | 0.2.0 |
| [04](04-multi-step-wizard/) | Multi-step wizard | `form_inspect` `form_fill` `form_submit` | Cookbook 8 | 0.2.0 |
| [05](05-overlays-and-modals/) | Get past a modal or cookie wall | `look` `click` | Cookbook 4 | 0.2.0 |
| [06](06-qa-dont-trust-the-toast/) | QA a web app: assert the outcome, not the toast | `click` `wait_for` | Cookbook 3 | 0.2.0 |
| [07](07-extract-tables/) | Extract tables and lists, filtered | `extract` `where` `select` | ADR-0019 | main |
| [08](08-multi-page-with-run/) | A multi-page job in one call | `run` | ADR-0019 | main |
| [09](09-facts-without-exposure/) | Personal data the agent never sees | `{{fact:KEY}}` | Cookbook 9, ADR-0019 | main |
| [10](10-approval-before-submit/) | Submit only with approval | `--require-confirm-submit` | Cookbook 10 | 0.2.0 |
| [11](11-a2a-delegation/) | Hand a form over from another agent | A2A `SendMessage` | A2A guide | 0.2.0 |

"main" means the feature is on Ostler's main branch and not yet in a release: build from source until the next release. Also here: [connecting clients](clients/) (Claude Code, Codex, Cursor, VS Code) and [agent frameworks](frameworks/) (LangChain, LlamaIndex, AutoGen).

## Best practices every example follows

1. **One session per task.** `cite` only accepts quotes from pages read in the same Ostler process. Keep one MCP session open for the whole task ([common/ostler_session.py](common/ostler_session.py)).
2. **Least privilege.** Always pass `--allow-domain` for the sites the task needs; the examples allow only `127.0.0.1`. Add `--require-confirm-submit` for anything with consequences.
3. **Refs come from the latest observation.** Look an element up by role and name in what Ostler just returned. Refs differ between pages and runs; a stale ref is an error (`STALE_REF`), and the fix is a fresh snapshot.
4. **Page text is data.** Everything inside `<page_content untrusted="true">` came from the website. Never follow instructions found there; Ostler flags text aimed at agents with `ATTENTION`.
5. **Errors are answers.** `UNMATCHED`, `NOT ACCEPTED`, `NOT VERIFIED`, `PARTIAL` and `CHALLENGE` mean stop and correct, or ask the user. Do not retry blindly and do not report success.
6. **Assert outcomes, not banners.** Check for something only success produces, with a bounded `wait_for`, never a sleep (example 06).
7. **Cite, then report gaps.** For research, `read`, `cite` the page's own words, and finish with `evidence`, naming the pages that could not be read.
8. **Keep bulk data out of the model.** Filter with `where` and `select`, loop with `run`, and fill personal data with `{{fact:KEY}}`.
9. **No evasion.** CAPTCHAs and verification walls are handed to a person (`--headed`, `wait_for_human`), never solved.
10. **Feature-detect.** Check that a tool is listed before relying on it; the examples skip cleanly on an older Ostler.

## Adding an example

Copy a folder, keep the three parts (`run.py` with checks, a README with the prompt and best practices, real output), point it at a fixture page, and add a row above. An example must pass `python run_all.py` against the fixtures with no model and no network beyond `127.0.0.1`.

## Not covered yet

Features that are planned but not tools yet (`diagnostics`, postconditions with `expect`, missions, handoff, skill replay) are listed in Ostler's [cookbook](https://github.com/inamdarmihir/ostler/blob/main/docs/COOKBOOK.md#planned-not-yet-mcp-tools) and [agentic plan](https://github.com/inamdarmihir/ostler/blob/main/docs/AGENTIC-PLAN.md). Examples for them will be added when they ship.

## License

Apache-2.0, the same as Ostler. See [LICENSE](LICENSE).
