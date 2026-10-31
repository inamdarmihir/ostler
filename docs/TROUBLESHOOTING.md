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

If it still fails, open an issue with the output of `ostler doctor` and your OS and Chrome version.
