"""Model backends. Each one turns (system, neutral messages, tools) into one assistant turn.

Neutral message format (the same for every backend, and what trajectories record):
    {"role": "user", "content": str}
    {"role": "assistant", "text": str, "tool_calls": [{"id", "name", "args": dict}]}
    {"role": "tool", "id": str, "name": str, "content": str}

A backend returns Turn(text, tool_calls, usage) where usage = {"in": int, "out": int, "estimated": bool}.

Backends:
    OpenAICompat   POST {base}/chat/completions with tools; Ollama, vLLM, llama.cpp server, hosted APIs.
    AnthropicMessages  POST {base}/v1/messages; request/response shape checked against the official docs
                       (see README) but NOT exercised against the live API by the authors.
    ScriptedRandom / ScriptedOracle  deterministic, no network; for testing the harness only.

The API key is read from the environment (or a flag) and sent only in the request header. It is
never written to trajectories, results or logs; `redact()` scrubs it from error text.
"""
import json
import os
import random
import time
import urllib.error
import urllib.request

import scoring

FINISH_TOOL = {
    "name": "finish",
    "description": ("Call this exactly once when you are done, or when you cannot make progress. "
                    "Report what you did and whether the task succeeded. Do not claim success unless you "
                    "verified the result on the page."),
    "input_schema": {
        "type": "object",
        "properties": {
            "final_answer": {"type": "string", "description": "Your answer / a short report of what you did and saw."},
            "cited_urls": {"type": "array", "items": {"type": "string"},
                           "description": "URLs of the pages your answer relies on (empty if none)."},
            "claim": {"type": "string", "enum": ["success", "failure", "unsure"],
                      "description": "success = the task is done and you verified it; failure = it cannot be done "
                                     "or did not work; unsure = you could not verify."},
        },
        "required": ["final_answer", "cited_urls", "claim"],
    },
}


class Turn:
    def __init__(self, text="", tool_calls=None, usage=None, raw_stop=None):
        self.text = text or ""
        self.tool_calls = tool_calls or []
        self.usage = usage or {"in": 0, "out": 0, "estimated": True}
        self.raw_stop = raw_stop


class ModelError(RuntimeError):
    pass


def estimate_tokens(obj):
    """Rough token estimate: serialised characters / 4. Always labelled 'estimated' by callers."""
    s = obj if isinstance(obj, str) else json.dumps(obj, separators=(",", ":"), default=str)
    return max(1, len(s) // 4) if s else 0


def redact(text, secret):
    if secret and len(secret) >= 6:
        text = text.replace(secret, "[REDACTED]")
    return text


def _post_json(url, headers, body, timeout, secret, retries=2):
    data = json.dumps(body).encode()
    last = None
    for attempt in range(retries + 1):
        req = urllib.request.Request(url, data=data, headers=headers, method="POST")
        try:
            with urllib.request.urlopen(req, timeout=timeout) as r:
                return json.loads(r.read())
        except urllib.error.HTTPError as e:
            msg = redact(e.read().decode(errors="replace")[:500], secret)
            last = ModelError(f"HTTP {e.code} from {url}: {msg}")
            if e.code not in (408, 429, 500, 502, 503, 504):
                raise last
        except (urllib.error.URLError, TimeoutError, ConnectionError) as e:
            last = ModelError(f"request to {url} failed: {redact(str(e), secret)}")
        except ValueError as e:
            last = ModelError(f"non-JSON response from {url}: {e}")
        if attempt < retries:
            time.sleep(min(2 ** attempt, 8))
    raise last


# ---------- OpenAI-compatible ----------

class OpenAICompat:
    name = "openai"
    scripted = False

    def __init__(self, model, base_url, api_key=None, temperature=None, max_tokens=None, timeout=300):
        self.model, self.base, self.key = model, base_url.rstrip("/"), api_key
        self.temperature, self.max_tokens, self.timeout = temperature, max_tokens, timeout

    def begin_task(self, task, ctx):
        pass

    @staticmethod
    def to_messages(system, messages):
        out = [{"role": "system", "content": system}]
        for m in messages:
            if m["role"] == "user":
                out.append({"role": "user", "content": m["content"]})
            elif m["role"] == "assistant":
                a = {"role": "assistant", "content": m.get("text") or None}
                if m.get("tool_calls"):
                    a["tool_calls"] = [{"id": c["id"], "type": "function",
                                        "function": {"name": c["name"], "arguments": json.dumps(c["args"])}}
                                       for c in m["tool_calls"]]
                out.append(a)
            elif m["role"] == "tool":
                out.append({"role": "tool", "tool_call_id": m["id"], "content": m["content"]})
        return out

    @staticmethod
    def to_tools(tools):
        return [{"type": "function", "function": {"name": t["name"], "description": t.get("description", ""),
                                                  "parameters": t.get("input_schema") or {"type": "object", "properties": {}}}}
                for t in tools]

    def chat(self, system, messages, tools):
        body = {"model": self.model, "messages": self.to_messages(system, messages), "tools": self.to_tools(tools)}
        if self.temperature is not None:
            body["temperature"] = self.temperature
        if self.max_tokens:
            body["max_tokens"] = self.max_tokens
        headers = {"Content-Type": "application/json"}
        if self.key:
            headers["Authorization"] = "Bearer " + self.key
        r = _post_json(self.base + "/chat/completions", headers, body, self.timeout, self.key)
        try:
            choice = r["choices"][0]
            msg = choice["message"]
        except (KeyError, IndexError, TypeError):
            raise ModelError("unexpected response shape (no choices[0].message)")
        calls = []
        for i, c in enumerate(msg.get("tool_calls") or []):
            fn = c.get("function") or {}
            raw = fn.get("arguments")
            try:
                args = json.loads(raw) if isinstance(raw, str) and raw.strip() else (raw or {})
            except ValueError:
                args = {"__unparsed_arguments__": raw}
            calls.append({"id": c.get("id") or f"call_{i}", "name": fn.get("name", ""), "args": args if isinstance(args, dict) else {"value": args}})
        u = r.get("usage") or {}
        if "prompt_tokens" in u and "completion_tokens" in u:
            usage = {"in": int(u["prompt_tokens"]), "out": int(u["completion_tokens"]), "estimated": False}
        else:
            usage = {"in": estimate_tokens([body["messages"], body["tools"]]),
                     "out": estimate_tokens(msg), "estimated": True}
        return Turn(msg.get("content") or "", calls, usage, choice.get("finish_reason"))


# ---------- Anthropic Messages ----------

class AnthropicMessages:
    name = "anthropic"
    scripted = False

    def __init__(self, model, base_url="https://api.anthropic.com", api_key=None, temperature=None,
                 max_tokens=4096, version="2026-10-21", timeout=300):
        self.model, self.base, self.key = model, base_url.rstrip("/"), api_key
        self.temperature, self.max_tokens, self.version, self.timeout = temperature, max_tokens or 4096, version, timeout

    def begin_task(self, task, ctx):
        pass

    @staticmethod
    def to_messages(messages):
        out = []

        def push(role, block):
            # Consecutive tool results must share one user message (parallel tool calls).
            if out and out[-1]["role"] == role and isinstance(out[-1]["content"], list):
                out[-1]["content"].append(block)
            else:
                out.append({"role": role, "content": [block]})

        for m in messages:
            if m["role"] == "user":
                push("user", {"type": "text", "text": m["content"]})
            elif m["role"] == "assistant":
                blocks = []
                if m.get("text"):
                    blocks.append({"type": "text", "text": m["text"]})
                for c in m.get("tool_calls") or []:
                    blocks.append({"type": "tool_use", "id": c["id"], "name": c["name"], "input": c["args"]})
                if blocks:
                    out.append({"role": "assistant", "content": blocks})
            elif m["role"] == "tool":
                push("user", {"type": "tool_result", "tool_use_id": m["id"], "content": m["content"]})
        return out

    def chat(self, system, messages, tools):
        body = {"model": self.model, "max_tokens": self.max_tokens, "system": system,
                "messages": self.to_messages(messages),
                "tools": [{"name": t["name"], "description": t.get("description", ""),
                           "input_schema": t.get("input_schema") or {"type": "object", "properties": {}}} for t in tools]}
        if self.temperature is not None:
            body["temperature"] = self.temperature
        headers = {"Content-Type": "application/json", "anthropic-version": self.version}
        if self.key:
            headers["x-api-key"] = self.key
        r = _post_json(self.base + "/v1/messages", headers, body, self.timeout, self.key)
        if not isinstance(r.get("content"), list):
            raise ModelError("unexpected response shape (no content list)")
        text, calls = [], []
        for b in r["content"]:
            if b.get("type") == "text":
                text.append(b.get("text", ""))
            elif b.get("type") == "tool_use":
                calls.append({"id": b["id"], "name": b["name"], "args": b.get("input") or {}})
        u = r.get("usage") or {}
        if "input_tokens" in u and "output_tokens" in u:
            usage = {"in": int(u["input_tokens"]) + int(u.get("cache_read_input_tokens") or 0)
                     + int(u.get("cache_creation_input_tokens") or 0),
                     "out": int(u["output_tokens"]), "estimated": False}
        else:
            usage = {"in": estimate_tokens([body["messages"], body["tools"], system]),
                     "out": estimate_tokens(r["content"]), "estimated": True}
        return Turn("\n".join(text), calls, usage, r.get("stop_reason"))


# ---------- scripted backends (harness self-test only) ----------

class _Scripted:
    scripted = True
    name = "scripted"

    def __init__(self, seed=0):
        self.seed = seed
        self.rng = random.Random(seed)
        self.task = None
        self.ctx = None

    def begin_task(self, task, ctx):
        self.task, self.ctx = task, ctx
        self.rng = random.Random(f"{self.seed}|{task['pack']}|{task['id']}|{ctx.get('rep', 0)}")
        self.step = 0

    def _usage(self, system, messages, tools, out):
        return {"in": estimate_tokens([system, messages, tools]), "out": estimate_tokens(out), "estimated": True}


class ScriptedRandom(_Scripted):
    """Calls 0 to 3 random tools with random schema-shaped arguments, then finishes with a random claim.
    It has no knowledge of the task. Over many tasks it must score near zero success, and any
    run where it claims success while the check fails is a false success: a sanity check on scoring."""
    name = "scripted-random"

    def _args(self, schema):
        props = (schema or {}).get("properties", {}) or {}
        out = {}
        for k, p in props.items():
            t = p.get("type")
            if "enum" in p:
                out[k] = self.rng.choice(p["enum"])
            elif t == "string":
                out[k] = self.rng.choice(["", "x", "http://127.0.0.1:1/", "Ada Lovelace", "42"])
            elif t in ("integer", "number"):
                out[k] = self.rng.randint(0, 5)
            elif t == "boolean":
                out[k] = self.rng.random() < 0.5
            elif t == "array":
                out[k] = []
            elif t == "object":
                out[k] = {}
        return out

    def chat(self, system, messages, tools):
        self.step += 1
        real = [t for t in tools if t["name"] != "finish"]
        n_calls = sum(1 for m in messages if m["role"] == "assistant" and m.get("tool_calls"))
        budget = self.rng.randint(0, 3)
        if real and n_calls < budget:
            t = self.rng.choice(real)
            calls = [{"id": f"rnd_{self.step}", "name": t["name"], "args": self._args(t.get("input_schema"))}]
            return Turn("", calls, self._usage(system, messages, tools, calls))
        args = {"final_answer": "scripted-random: no idea.", "cited_urls": [],
                "claim": self.rng.choice(["success", "failure", "unsure"])}
        calls = [{"id": f"rnd_fin_{self.step}", "name": "finish", "args": args}]
        return Turn("", calls, self._usage(system, messages, tools, calls))


class ScriptedOracle(_Scripted):
    """Emits a run that satisfies the task's expected outcome. FOR TESTING THE HARNESS ONLY.

    It reads the task's own outcome spec (which a real agent never sees), performs the required
    requests straight against the fixture server (bypassing every MCP server and browser), then
    calls finish(). Modes: `scripted-oracle` (does the work, claims success),
    `scripted-oracle:liar` (does nothing, claims success: exercises false_success),
    `scripted-oracle:honest-fail` (does nothing, claims failure: exercises true_failure),
    `scripted-oracle:unsure`. Its results say nothing about any browser server or model.
    """
    name = "scripted-oracle"

    def __init__(self, seed=0, mode="work"):
        super().__init__(seed)
        self.mode = mode

    def begin_task(self, task, ctx):
        super().begin_task(task, ctx)
        self.done = False

    def chat(self, system, messages, tools):
        self.step += 1
        o = self.task["outcome"]
        base = self.ctx["base"]
        if self.mode == "work" and not self.done:
            self.done = True
            spec = o.get("records", o) if o.get("kind") in ("records", "both") else {}
            for m in spec.get("must", []) or []:
                self.ctx["fixture"].send(m.get("method") or "POST", m["path"].rstrip("*"), m.get("body_contains") or {})
        ans = o.get("answer", o) if o.get("kind") in ("answer", "both") else {}
        answer = "scripted-oracle: " + "; ".join(str(s) for s in ans.get("must_contain", []) or []) if self.mode == "work" else "scripted-oracle: did nothing"
        cites = [str(u).replace("{BASE}", base.rstrip("/")) for u in ans.get("must_cite", []) or []]
        cites = [c if c.startswith("http") else base.rstrip("/") + "/" + c.lstrip("/") for c in cites] if self.mode == "work" else []
        claim = {"work": "success", "liar": "success", "honest-fail": "failure", "unsure": "unsure"}[self.mode]
        args = {"final_answer": answer, "cited_urls": cites, "claim": claim}
        calls = [{"id": f"orc_{self.step}", "name": "finish", "args": args}]
        return Turn("", calls, self._usage(system, messages, tools, calls))


# ---------- factory ----------

def build(spec, models_file=None, base_url=None, api_key_env=None, temperature=None, max_tokens=None,
          anthropic_version=None, seed=0):
    """spec: a key in models_file, or 'openai:<model>', 'anthropic:<model>', 'scripted-random',
    'scripted-oracle[:liar|:honest-fail|:unsure]'. Returns (backend, label, public_config)."""
    cfg = None
    if models_file:
        with open(models_file) as f:
            allm = json.load(f).get("models", {})
        if spec in allm:
            cfg = dict(allm[spec])
    if cfg is None:
        if spec == "scripted-random":
            return ScriptedRandom(seed), spec, {"backend": "scripted-random"}
        if spec.startswith("scripted-oracle"):
            mode = spec.split(":", 1)[1] if ":" in spec else "work"
            if mode not in ("work", "liar", "honest-fail", "unsure"):
                raise ValueError(f"unknown oracle mode {mode!r}")
            return ScriptedOracle(seed, mode), spec, {"backend": "scripted-oracle", "mode": mode}
        if ":" not in spec:
            raise ValueError(f"unknown model spec {spec!r}; use openai:<model>, anthropic:<model>, scripted-random, "
                             "scripted-oracle, or a name from --models-file")
        kind, model = spec.split(":", 1)
        cfg = {"backend": kind, "model": model}
    kind = cfg["backend"]
    env_name = cfg.get("api_key_env") or api_key_env or {
        "openai": "OSTLER_EVAL_API_KEY", "anthropic": "ANTHROPIC_API_KEY"}.get(kind)
    key = os.environ.get(env_name) if env_name else None
    t = cfg.get("temperature", temperature)
    mt = cfg.get("max_tokens", max_tokens)
    public = {k: v for k, v in cfg.items() if k not in ("api_key", "api_key_env")}
    public["api_key_env"] = env_name  # the variable NAME is recorded; the value never is
    public["api_key_present"] = bool(key)
    if kind == "openai":
        base = cfg.get("base_url") or base_url or os.environ.get("OSTLER_EVAL_BASE_URL")
        if not base:
            raise ValueError("openai backend needs --base-url, OSTLER_EVAL_BASE_URL, or base_url in the models file "
                             "(e.g. http://127.0.0.1:11434/v1 for Ollama)")
        public["base_url"] = base
        return OpenAICompat(cfg["model"], base, key, t, mt), spec, public
    if kind == "anthropic":
        base = cfg.get("base_url") or base_url or "https://api.anthropic.com"
        if not key and base.startswith("https://api.anthropic.com"):
            raise ValueError(f"anthropic backend needs the API key in ${env_name}")
        public["base_url"] = base
        return AnthropicMessages(cfg["model"], base, key, t, mt or 4096, anthropic_version or cfg.get("anthropic_version") or "2026-10-21"), spec, public
    raise ValueError(f"unknown backend {kind!r}")
