# Model-in-the-loop evaluation harness

Runs a language model as a browser agent against any stdio MCP server, on the fixture tasks in
`crates/ostler-fixtures/packs/<pack>/tasks.json`, and scores each run against **what the fixture server
actually received**, not against what a tool or the model said. The headline number is **false success**:
the agent claimed success but the outcome check failed.

Python 3, standard library only. Nothing here ships in the product. It exists so the benchmark in
`docs/BUILD-PLAN.md` section 8 is run by one loop that is not tuned for Ostler.

## Quick self-test (no model, no browser, no API key)

```sh
ostler fixtures &                       # fixture server, default http://127.0.0.1:8787
python3 scripts/eval/run.py --server none --model scripted-oracle,scripted-oracle:liar,scripted-random \
    --out /tmp/eval-selftest
cat /tmp/eval-selftest/results.md
```

Expected: `scripted-oracle` scores 100% success, `scripted-oracle:liar` scores 100% **false** success,
`scripted-random` scores about 0%. If not, the harness or the task files are wrong. `--server none` means
"no MCP server, no tools": it tests scoring and plumbing only.

Unit tests (stub MCP server and stub fixture server included, no Chrome needed):

```sh
cd scripts/eval && python3 -m unittest            # about 15 s
```

## A real run

```sh
ostler fixtures &

# Open-weight model served locally by Ollama (OpenAI-compatible endpoint), three servers:
python3 scripts/eval/run.py \
  --servers-file scripts/eval/examples/servers.example.json \
  --server ostler,playwright,chrome-devtools \
  --model openai:YOUR_MODEL --base-url http://127.0.0.1:11434/v1 \
  --pack gov-forms --reps 3 --temperature 0 --out scripts/eval/results/ollama-run1

# Hosted OpenAI-compatible API (key from the environment, never from a flag):
export OSTLER_EVAL_API_KEY=...            # the harness only ever reads the variable
python3 scripts/eval/run.py --server ostler --model openai:MODEL --base-url https://HOST/v1 ...

# Several models from a file (names are what --model takes):
python3 scripts/eval/run.py --models-file scripts/eval/examples/models.example.json \
  --server ostler --model ollama-local,vllm-local ...

# Anthropic Messages API:
export ANTHROPIC_API_KEY=...
python3 scripts/eval/run.py --server ostler --model anthropic:MODEL_ID ...
```

Useful flags: `--list-tasks`, `--pack`, `--task ID`, `--tag`, `--reps N`, `--max-steps` (default 30),
`--max-result-chars` (default 8000), `--mcp-timeout`, `--reuse-server`, `--seed`, `--fixture URL`,
`--packs-dir`. `python3 scripts/eval/run.py --help` lists all.

Servers are defined in a JSON file (`examples/servers.example.json`): an id and the argv that starts a
stdio MCP server. The example uses `npx ... @latest` for Playwright MCP and Chrome DevTools MCP and
`ostler serve`; **for a published run pin exact versions** and read the resolved versions from
`config.json` (`server_info`, filled from each server's `initialize` reply). The flags in the example were
copied from `scripts/bench-compare/run.py`; check each server's current `--help` before relying on them.

## Fairness rules built into the loop

One implementation (`agent.py`) serves every server and model:

- the same system prompt (hash recorded as `prompt_version`), nudge text and `finish` tool;
- the same step limit and the same tool-result truncation (head kept, marker appended);
- tool results are text only: images become `[image omitted by harness: N base64 chars]` for every server;
- the tool list is re-read from the server (`tools/list`) before every model turn, so servers that reveal
  tools as the page changes (Ostler does) and servers with a fixed list are treated alike;
- nothing in the loop names a server or knows its tools.

The agent ends by calling the built-in `finish(final_answer, cited_urls, claim)`; `claim` must be
`success`, `failure` or `unsure` (an invalid claim is rejected once and the run continues). A reply with no
tool call is nudged twice, then the run ends as `no_finish`.

## Backends

| spec | what | notes |
|---|---|---|
| `openai:<model>` | `POST {base}/chat/completions` with `tools` | Ollama (`/v1`), vLLM, llama.cpp server, hosted providers. Needs `--base-url` or `OSTLER_EVAL_BASE_URL`. Key: `OSTLER_EVAL_API_KEY` or `--api-key-env NAME`. |
| `anthropic:<model>` | `POST {base}/v1/messages` | Key: `ANTHROPIC_API_KEY`. Request and response shape checked against the official Messages API reference on 2026-11-06 (headers `x-api-key`, `anthropic-version`; body `model`, `max_tokens`, `system`, `messages`, `tools[{name, description, input_schema}]`; response `content` blocks `text` and `tool_use{id,name,input}`, `usage.input_tokens/output_tokens`; results returned as `tool_result{tool_use_id}` blocks in a user message). **The adapter has not been run against the live API** (no key was available); `anthropic-version` defaults to the value shown in the current docs (`2026-10-21`) and can be set with `--anthropic-version`. |
| `scripted-random` | seeded random tool calls, then a random claim | harness test only |
| `scripted-oracle[:liar\|:honest-fail\|:unsure]` | see below | harness test only |

Keys are read from the environment, sent only in the request header, never written to any output (a test
checks the whole results directory), and scrubbed from error text. `config.json` records the variable
**name** and whether a key was present.

### Scripted backends and the oracle

`scripted-oracle` reads the task's own expected outcome (which a real agent never sees) and, for records
checks, sends the required requests **directly to the fixture server**, bypassing every MCP server and
browser, then calls `finish` with `success`. `:liar` does nothing and claims success (exercises
`false_success`); `:honest-fail` does nothing and claims failure; `:unsure` claims unsure. The oracle exists
only to test the harness end to end. A results table that contains scripted rows says so at the top. Never
publish oracle or random numbers as results for a server or model.

## Task contract

Tasks come from `crates/ostler-fixtures/packs/*/tasks.json` (a JSON list, or `{"tasks": [...]}`), written by
the packs engineer. This harness is built against this contract; if the real files differ, `tasks.py` and
`scoring.py` are the two places to adapt.

```
task    = {id, pack, title, difficulty, instruction (may contain {BASE}), start, tags,
           outcome, expect_gaps, risk}
outcome = {kind: "records", must: [{method, path, body_contains: {}}], must_not: [{path}]}
        | {kind: "answer",  must_contain: [], must_cite: [], must_not_contain: []}
        | {kind: "both", ...both sets, either flat or under "records" and "answer"}
records = GET /__records -> [{ts, method, path, query, body, pack}]   POST /__reset clears them
```

Check semantics (as implemented, so the task authors can write to them):

- `must`: some record has the same method (case-insensitive; omitted means any), the same path (a trailing
  `*` is a prefix match), and every `body_contains` key present with an equal value. `body` may be an
  object, a JSON string or a urlencoded string. Keys may be dotted (`a.b`, `items.0.sku`). Values compare as
  trimmed strings, numbers numerically, booleans case-insensitively; a list value matches the same list in
  any order.
- `must_not`: no record matches (method and body optional) .
- `must_contain` / `must_not_contain`: case-insensitive substring of `final_answer`.
- `must_cite`: a full URL must equal a cited URL (trailing `/` ignored); an entry starting with `/` must
  equal the path of some cited URL. `{BASE}` is replaced in both.
- `expect_gaps`, `risk`, `difficulty`, `tags` are carried into the results but **not scored yet**.

## Scoring

Per run: `check_passed` (the outcome check), `claim`, and a class:

| claim | check | class |
|---|---|---|
| success | passed | `true_success` |
| success | failed | **`false_success`** (the headline) |
| failure | failed | `true_failure` |
| failure | passed | `false_failure` |
| unsure | any | `unsure` |
| none (step limit, no tool call, invalid finish) | any | `no_finish` |

`success` (task success) is `check_passed`, whatever the agent claimed. Reported per server x model x pack
and per server x model overall: n, success, false success (count, rate over counted runs, 95% Wilson
interval), false successes among claimed successes, true failures, unsure, no finish, mean tool calls
(excluding `finish`), mean tokens in and out, median wall time, and the estimated size of the tool
definitions the server sends every turn.

Tokens are the API's reported usage when present. When an endpoint omits `usage`, characters / 4 is used and
the row is marked `est` in `results.md` (`tokens_estimated` in the JSON). Wall time includes model latency.

Runs that end in an infrastructure error (server would not start, model endpoint failed after retries,
fixture server unreachable) are `status: error`: listed, counted, and **excluded from all rates**, so a flaky
endpoint is not scored as an agent failure.

## Output

```
<out>/config.json        prompt version and text, limits, server commands (env var NAMES only), model configs
                         (key variable name, never the key), server_info, harness commit, python/platform
<out>/tasks.json         the task definitions used
<out>/runs.jsonl         one line per run (written as the run finishes, so a crash keeps what was done)
<out>/trajectories/*.jsonl   one file per run: meta, tools offered, model turns (text, calls, usage),
                         tool calls with truncated results, the finish call, the final check and the records
<out>/results.json       config + summary + runs
<out>/results.md         the table
<out>/server-logs/*.stderr.log
```

`python3 scripts/eval/report.py <out>` regenerates `results.json` and `results.md` from `runs.jsonl`.
Runs are sequential on purpose: the fixture server's record list is global and is reset before every run.

## Hugging Face export

```sh
python3 scripts/eval/export_hf.py <results-dir> <out-dir> [--license apache-2.0] [--name NAME]
```

Writes `data/runs.jsonl`, `data/trajectories.jsonl` (events as a JSON string per run, which loads cleanly in
`datasets`), `data/tasks.jsonl`, parquet copies **only if `pyarrow` is already installed**, copies of
`config.json`/`results.*`, and a dataset card `README.md` (YAML header, method, fields, caveats, licence).
It never uploads anything and contains no network code. Review the card, check that the licence covers the
tasks and the model outputs, then publish yourself (for example with the Hugging Face CLI).

## What this does NOT prove

- **Not a sample of the web.** Tasks are local fixture pages built to replicate widget shapes. A good score
  here says nothing about banks, government portals, bot walls or sites that change.
- **The outcome check is the task author's opinion.** A check can be too strict (a correct alternative is
  marked wrong) or too lax. Read `check_details` and a few trajectories before trusting a number.
- **False success is only as good as the claim.** The model's `claim` comes from a prompt we wrote; a different
  prompt, tool-use scaffold or `finish` design could change it. Results hold for this loop, not for how a
  product wraps the same model.
- **Small n.** Dozens of tasks and a few repetitions give wide intervals (shown). Models are non-deterministic
  even at temperature 0; differences smaller than the intervals are noise. Task sets with unequal difficulty
  per pack make cross-pack comparisons weak.
- **Text-only tool results.** Servers that rely on screenshots are handicapped, and vision models are not
  used as such. Truncation at the same character limit can hurt servers with long outputs more than others.
- **Server defaults.** Each server runs with the flags in the servers file, not tuned per server. Another
  maintainer might configure theirs better; the servers file is the place to say so, and results should name it.
- **Versions drift.** `npx ...@latest` resolves to whatever is current. Pin and record versions.
- **Scripted rows prove only that the harness runs.** The oracle sidesteps the browser entirely.
- **Token counts** are API-reported only where the endpoint reports them; estimated rows are labelled.
- **No claim about speed or cost of a server** beyond what the table prints for these runs; wall time is
  dominated by model latency.
- The Anthropic adapter is verified against the documentation only, not run live.
- Nothing here measures safety (prompt-injection resistance, destructive actions) beyond the `must_not`
  records the task authors wrote.
