# Troubleshooting

Start with `ostler doctor`. It launches Chrome, round-trips a page and (if `QDRANT_URL` is set) checks Qdrant.

| symptom | likely cause | fix |
|---|---|---|
| Client says it can't start `ostler` / command not found | the client has a different `PATH` than your shell | use `ostler setup <client>`, which writes the binary's **absolute path** |
| `Chrome: could not find Chrome or Chromium` | none on the usual paths | install one (the message has the command for your OS), or set `OSTLER_CHROME=/path/to/chrome` |
| `the Chrome path "…" does not exist` | `OSTLER_CHROME` or `--chrome-path` points at a missing file | fix the path, or unset it to auto-detect (a wrong explicit path is an error on purpose, never silently replaced) |
| Launch fails with "exited before DevTools was ready" on Linux/Docker | running as root, or no sandbox support | set `OSTLER_NO_SANDBOX=1`; in containers also give Chrome a larger `/dev/shm` (`--shm-size=1g`) |
| The first call after Chrome crashed reports a restart | the server relaunches Chrome and tells the agent; open tabs and page state are lost | re-navigate and continue; if it keeps failing, restart the MCP server (in Claude Code: `/mcp`, reconnect) |
| Server prints nothing and the agent sees no tools | stdout must carry only MCP messages; something else wrote to it, or the client expects another transport | run `ostler serve` in a terminal: it should print one `ready` line on stderr and wait |
| `form_fill` says `UNMATCHED key` | no field label/name matches that key | call `form_inspect`, then pass the exact label or the `eNN` ref |
| `ambiguous` in a fill report | several fields match equally well | use the ref (`e12`) from `form_inspect` instead of a label |
| `NOT SUBMITTED` with a validation message | the page rejected the values | fix the named fields and call `form_submit` again |
| Memory: `FAILED` in doctor | wrong `QDRANT_URL`/key, or Cloud Inference not enabled on the cluster | doctor prints a specific fix (unreachable, rejected key, or embedding-size mismatch). Check the URL includes the port (`:6333`), enable Inference in the Qdrant Cloud console, or use `--embeddings dev-hash` against open-source Qdrant (not semantic). Memory is optional: unset `QDRANT_URL` to run without it |
| `npx -y ostler …` is slow the first time, or fails offline | the launcher downloads the binary once (cached in `~/.cache/ostler`) | run `npx -y ostler doctor` once while online; or install the binary with `install.sh` / `cargo install ostler` and point your client at it |
| `checksum mismatch … Not installing` | the download was corrupted or altered | nothing was installed or run. Retry; if it persists, install from source and open an issue |
| Agent context fills up | too many tools or large snapshots | `--tools forms`, `--lean`, and ask for `form_inspect` instead of full `snapshot` |
| A tool result starts with `CHALLENGE:` | a CAPTCHA or "verify you are human" page is blocking | Ostler never solves or evades these. Run with `--headed`, complete it in the window, then the agent calls `wait_for_human`. A persistent `--profile-dir` with a logged-in session triggers fewer challenges |
| `browser exited before DevTools was ready (is another Ostler or Chrome already using profile …)` | Chrome allows one process per profile and two servers (two agent windows, subagents) share one `--profile-dir` | give each server its own `--profile-dir`, or omit it (throwaway profile per server; any number can run at once) |
| Google, DuckDuckGo or Bing return a wall or no results | search engines challenge headless browsers; Ostler is built for forms and does not evade these | use your agent's own search tool for search and Ostler for the target site. If you must search through Ostler, run one `--headed` server with its own `--profile-dir` and solve the check yourself |
| A site blocks the browser or shows a CAPTCHA | automation detection | Ostler does not evade it; see [Searching the web](#searching-the-web) |
| Rebuilt `ostler` but the agent still behaves as before | the client keeps the old server process | reconnect the MCP client (Claude Code: `/mcp`; others: restart it) |

## Searching the web

Ostler drives a real Chrome, but from a fresh, headless session many sites treat it as a bot. What to expect (it varies by network and over time):

- **Google, DuckDuckGo, Brave, Startpage, Reddit, news sites such as the NYT:** often a bot wall or CAPTCHA. Ostler reports it and hands off to a person; it never tries to solve or get around it.
- **Bing:** usually loads (but it may ignore `first=` / "Page 2" and serve page 1 again to a headless session), but may return results unrelated to your query ("Some results have been removed"). When a search-results URL's visible text matches fewer than about half of the query's words, `navigate` adds `Warning: results do not seem to match the query "..."`. Treat the results as unreliable and try another engine or `--headed`.

What helps, honestly:

1. Run `--headed` with a persistent `--profile-dir`, one profile per server (Chrome locks a profile while it is in use). A person clears a CAPTCHA or signs in once in the visible window; cookies persist for later runs.
2. When the agent hits a wall it calls `wait_for_human`: it pauses, you solve the page in the window, and it resumes from the same page.
3. For pure discovery (finding which pages to read), use a search API; it is more reliable than scraping a results page. Ostler is best at reading the pages you already have URLs for and at filling forms.

Do not spoof the user agent or try to hide automation; it is unreliable and not something Ostler supports.

If it still fails, open an issue with the output of `ostler doctor` and your OS and Chrome version.
