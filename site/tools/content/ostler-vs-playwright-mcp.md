# Ostler vs Playwright MCP

Playwright MCP and Ostler are both MCP servers that give an agent a browser, and both have a batched form-fill tool. On five scripted form tasks on local fixtures, both finished every task correctly; Ostler used fewer tool calls (19 against 27) and Playwright MCP's tool list is larger. No language model was in the loop, no real website was used, and the numbers say nothing about which tool a real agent will complete tasks with more often. Choose on the differences below, not on the step counts alone.

## Where they differ

| | Ostler | Playwright MCP |
|---|---|---|
| What it is | Local Rust binary that runs Chrome and serves MCP and A2A. Early: v0.2, development source, no tagged release. | Microsoft's MCP server for Playwright, installed from npm. Mature and broad. |
| Form filling | `form_fill` takes a label-to-value map, then reads every value back from the live page and reports mismatches. `form_submit` is a separate step with its own outcome. | `browser_fill_form` fills several fields in one call from a page snapshot. |
| Research | `read`, `cite` and `evidence` check quotations against pages read in the session and report pages that could not be read. See [research with checked citations](research-with-citations.html). | Reads pages as accessibility snapshots. This project did not evaluate it for quotation checking, so no claim is made either way. |
| Memory | Optional, in your own Qdrant: saved facts and per-site playbooks. See [memory and privacy](memory.html). | Browser profile only. |
| Tool list size | 21 tools in the tested version, about 1,513 estimated tokens of definitions. Today's full list is 30 tools in five profiles; the default `auto` profile starts with 15. | 25 tools, about 5,265 estimated tokens of definitions. |
| Platforms | Chrome or Chromium; tested on macOS locally and Linux in CI. Windows and Docker are experimental and arrive in v0.2.1. | See Playwright MCP's own documentation. |

The Ostler column is from this project's own documentation. For Playwright MCP, the only facts used here are the ones this project measured on the version below; check its documentation for anything else.

## What was measured

Scripted, model-free, five tasks on local fixtures. A deterministic script plays the shortest sensible tool sequence for each server, using each server's own batch-fill tool, and every task is judged by what the fixture server received, not by what the tool said.

| | Ostler | Playwright MCP |
|---|---|---|
| Version tested | 0.2.0 (the revision was not recorded; see below) | `@playwright/mcp` 0.0.84 (reports Playwright 1.65.0-alpha-2026-10-30), `--headless --isolated --browser chrome` |
| Tasks completed correctly | 5 of 5 | 5 of 5 |
| Tool calls, all five tasks | 19 | 27 |
| Output tokens, all five tasks (bytes / 4, an estimate) | 1,550 | 2,960 |
| Tool time, all five tasks | 4.1 s | 8.8 s |

Per task, tool calls (Ostler against Playwright MCP): basic form 3 against 4, validation 3 against 4, dependent dropdowns 3 against 5, three-step wizard 7 against 10, 20-field form 3 against 4.

Conditions: 2026-11-02, Apple M2, macOS 26.6, one machine over localhost, headless, Google Chrome 156.0.8078.74, Node v26.10.0, median of 3 runs per task. The Chrome DevTools MCP server was measured in the same run and is in the [benchmarks](benchmarks.html#5-ostler-vs-playwright-mcp-vs-chrome-devtools-mcp-same-fixtures-scripted-agent).

## What this does not show

- **No language model.** A real model may take more or fewer steps than the scripted ideal. The script gives every server its best case.
- **Fixtures only.** Five tasks on local pages. None of the hostile behaviours in the other fixtures (CAPTCHA, shadow DOM, comboboxes, rich inputs) were run against Playwright MCP, so how it handles them is not measured here.
- **Correctness is a tie.** All three servers completed every task correctly in every run. The read-back that Ostler adds is what lets an agent detect a silent failure, but these fixtures never triggered one, so its value is not measured here.
- **Ostler is not always cheaper.** On small forms Chrome DevTools MCP returned fewer output tokens than Ostler, because Ostler's replies carry the read-back text. The step gap against batch-fill tools is about 1.4 times, not the larger gap seen against one call per field.
- **Wall time is not end-to-end latency.** Differences of a few hundred milliseconds are well under one model turn.
- **Versions move.** Playwright MCP has changed how navigation returns snapshots between releases. The sequences are this project's best attempt for each server and were not reviewed by its authors. A shorter sequence would change the numbers.
- **The Ostler revision was not recorded.** The raw results record the version string `ostler 0.2.0` and the date, not a commit. `git log` of the repository gives the history of the documents, not the revision that was measured.

The larger headline gaps elsewhere on this site (3 calls against 22 on a 20-field form, 13 times fewer input tokens) compare Ostler's form engine with the same engine driven one field per call. That is not a measurement of Playwright MCP.

## When to choose which

- **Choose Playwright MCP** if you need a mature, widely used tool, you want the documentation and ecosystem around Playwright, or you need a platform Ostler does not support. For simple, small forms the two tools are close.
- **Choose Ostler** if the agent must report what actually ended up in each field, if your workflow needs checked quotations and an explicit list of pages it could not read, or if you want per-site form memory in your own Qdrant. Accept that it is early software without a tagged release and without real-site success rates.
- **Choose neither on this page's evidence alone.** Run your own task. The harness is in the repository: `scripts/bench-compare.sh` reproduces the table above, and the [benchmark methods](benchmarks.html#8-reproduce) list every flag.

## Reproduce it

```sh
scripts/bench-compare.sh    # builds, installs the pinned competitors in a temp dir, writes bench-results/compare.{json,md}
```

Raw data: [`bench-results/compare.json`](repo:bench-results/compare.json) and [`compare.md`](repo:bench-results/compare.md). If you find a shorter sequence for either server, please [open an issue](https://github.com/inamdarmihir/ostler/issues).
