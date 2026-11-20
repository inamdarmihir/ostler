# Source memory ablation (session 2, scripted, no model)

Task: answer "who designed the Kestrel Lock?" in a fresh process, citing the page.

| arm | tool calls to a cited answer | output bytes | browser needed | citation verdict |
|---|---:|---:|---|---|
| source memory off | 2 (read, cite) | 973 + cite | yes: navigates and reads the page again | live `VERIFIED [n]` |
| source memory on (stored) | 2 (read stored, cite) | 730 + cite | no | `VERIFIED AGAINST A STORED COPY`, with fetch time and hash |

Store: in-process, hash embedder. This measures the mechanism, not retrieval quality at scale; see docs/adr/0020-source-memory.md.
