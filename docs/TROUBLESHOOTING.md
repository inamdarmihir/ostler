# Troubleshooting

Start with `ostler doctor`. It launches Chrome, round-trips a page and (if `QDRANT_URL` is set) checks Qdrant.

| symptom | likely cause | fix |
|---|---|---|
| Client says it can't start `ostler` / command not found | the client has a different `PATH` than your shell | use `ostler setup <client>`, which writes the binary's **absolute path** |
| `Chrome: could not find…` | no Chrome/Chromium on the usual paths | install Chrome, or set `OSTLER_CHROME=/path/to/chrome` |
| Launch fails with "exited before DevTools was ready" on Linux/Docker | running as root, or no sandbox support | set `OSTLER_NO_SANDBOX=1`; in containers also give Chrome a larger `/dev/shm` (`--shm-size=1g`) |
| Every tool call fails after Chrome crashed | the server does not relaunch Chrome yet (known, see [review](REVIEW-2026-10.md)) | restart the MCP server (in Claude Code: `/mcp`, reconnect) |
| Server prints nothing and the agent sees no tools | stdout must carry only MCP messages; something else wrote to it, or the client expects another transport | run `ostler serve` in a terminal: it should print one `ready` line on stderr and wait |
| `form_fill` says `UNMATCHED key` | no field label/name matches that key | call `form_inspect`, then pass the exact label or the `eNN` ref |
| `ambiguous` in a fill report | several fields match equally well | use the ref (`e12`) from `form_inspect` instead of a label |
| `NOT SUBMITTED` with a validation message | the page rejected the values | fix the named fields and call `form_submit` again |
| Memory: `FAILED` in doctor | wrong `QDRANT_URL`/key, or Cloud Inference not enabled on the cluster | check the URL includes the port (`:6333`), enable Inference in the Qdrant Cloud console, or use `--embeddings dev-hash` against open-source Qdrant (not semantic) |
| Agent context fills up | too many tools or large snapshots | `--tools forms`, `--lean`, and ask for `form_inspect` instead of full `snapshot` |
| A site blocks the browser or shows a CAPTCHA | automation detection | not handled; Ostler does not try to evade it |

If it still fails, open an issue with the output of `ostler doctor` and your OS and Chrome version.
