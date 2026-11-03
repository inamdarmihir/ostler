# Troubleshooting

Start with `ostler doctor`. It launches Chrome, round-trips a page and, if memory is on, prints a memory report and checks Qdrant. The report says in plain words what is stored where and what leaves the machine:

```text
Memory: on, Qdrant at https://… (tenant "default")
  sealing:    ON … (or OFF, with a warning)
  key:        <path to memory.key> (owner-only, good)   | from OSTLER_MEMORY_KEY | not created yet
  embeddings: cloud | local (ollama at …) | local dev-hash
  isolation:  shared collections | one set of collections per tenant
  transport:  https (encrypted) | http to this machine | http to a remote host: REFUSED / ALLOWED
  qdrant:     reachable, collections ready. OK
```

With memory off it prints `Memory: off` and the rest still works. It never creates the sealing key.

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
| `NOT ACCEPTED` with the page's message | the page rejected the values | fix the named fields and call `form_submit` again |
| `sealing: OFF. warning: anyone who can read your Qdrant ...` | `--seal off` (or `seal = off` in the config) | `ostler config set seal on`, then `ostler memory seal` to protect facts saved earlier (learned skills and episodes from before are dropped and relearned) |
| `warning: other users on this machine can read the key` | `memory.key` is not owner-only | `chmod 600` on the path doctor prints |
| the sealing key is unreadable or malformed, or sealed memory returns nothing | `memory.key` / `OSTLER_MEMORY_KEY` is missing, damaged or a different key than the one that sealed it | restore your backup to the path doctor names. **There is no recovery without the key**: delete the key file and run `ostler memory wipe --yes` to start over. Points that fail to open are skipped, never returned |
| `refusing plain http` / `transport: http to a remote host: REFUSED` | the Qdrant URL is plain `http://` to a non-loopback host, so the API key and any unsealed data would cross the network in clear text | use an `https://` URL; if a VPN or SSH tunnel protects the link, pass `--allow-insecure-qdrant` (`ostler config set allow_insecure_qdrant true`) |
| `warning: Cloud Inference only exists on Qdrant Cloud, and this URL is not one` | `--embeddings cloud` (the default) with a self-hosted URL | `--embeddings ollama` (semantic, local) or `dev-hash` (local, not semantic) |
| `cannot reach the local embedder` / embedding model not found | `--embeddings ollama` but Ollama is not running, or the model is not pulled | `ollama serve`, `ollama pull all-minilm` (the Ollama default), or `ostler config set embedding_model <name>` and `embedding_dim` to match |
| `warning: model "…/…" looks like a Qdrant Cloud model name` | an Ollama setup still carrying the Cloud default model | `ostler config set embedding_model all-minilm` |
| `Qdrant rejected the API key` (401/403) with `--isolation collection` | a scoped token only covers its own tenant's collections | use the token from `ostler memory token` for the same `--tenant`; Qdrant must run with `service.jwt_rbac: true` and an admin must have created the collections first (`ostler memory list` as admin does it) |
| `this collection was created with a different embedding model` | the collection's vector size does not match `--embedding-model` / `--embedding-dim` | use the same model as before, or a different tenant or cluster |
| `SERVER ERROR: the site answered with an HTTP error` from `form_submit` | the submit got a 4xx/5xx and the page showed no validation messages | the data may or may not have been stored: do not retry blindly; check with `snapshot` or ask the user |
| `2 forms on this page and none was filled: pass form (...)` | `form_submit` with several forms and no `form` or `control`, and no earlier `form_fill` on this page | pass the form ref from `form_inspect`, or a submit `control` ref |
| `qdrant:     FAILED` in doctor | wrong `QDRANT_URL`/key, or Cloud Inference not enabled on the cluster | doctor prints a specific fix (unreachable, rejected key, or embedding-size mismatch). Check the URL includes the port (`:6333`), enable Inference in the Qdrant Cloud console, or use `--embeddings ollama` (local, semantic) or `--embeddings dev-hash` (not semantic) against open-source Qdrant. Memory is optional: unset `QDRANT_URL` to run without it |
| A tool result starts with `Note: memory is unavailable (...)` | Qdrant failed twice in a row (down, slow past the 4 s request limit, or returning 5xx); lookups are skipped for 30 s, then one request retries | the form tools keep working: pass values in the request. Fix Qdrant (`ostler doctor`), no restart needed |
| `... is a cloud metadata address (always blocked ...)` | the agent tried 169.254.169.254, `fe80::/10` or `metadata.google.internal` | intended. If you really need it, add it to `--allow-domain` |
| A submit comes back as `Nothing changed` on a page that stays put | the site posts in the background and says nothing visible afterwards, so Ostler cannot tell it worked | check with `snapshot`; do not resubmit blindly (it may have gone through) |
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
