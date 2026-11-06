"""Stdio MCP client for the evaluation harness.

It extends scripts/bench-compare/mcpclient.py (same newline-delimited JSON-RPC framing, same
`MCP` class for process start-up) and adds what a model-in-the-loop agent needs:

* a reader thread, so a hung server cannot block the harness past the timeout;
* server notifications are recorded (`notifications`), not dropped, so dynamic tool lists
  (`notifications/tools/list_changed`) are visible in trajectories;
* `list_tools()` is cheap and called on every turn by the agent loop, so servers that reveal
  tools as the page changes (Ostler does) are handled the same way as servers with a fixed list;
* image and other non-text content parts are reported as placeholders (the harness is text-only
  for every server, so no server gets an advantage from returning screenshots).
"""
import importlib.util
import json
import os
import queue
import threading
import time

_HERE = os.path.dirname(os.path.abspath(__file__))


def _load_bench_compare_client():
    """Import scripts/bench-compare/mcpclient.py by path. It is NOT put on sys.path because that
    directory has modules named tasks, report, run and servers that would shadow this directory's."""
    path = os.path.join(_HERE, "..", "bench-compare", "mcpclient.py")
    spec = importlib.util.spec_from_file_location("bench_compare_mcpclient", path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod.MCP


MCP = _load_bench_compare_client()  # reused: the client class (framing, bookkeeping fields)

PROTOCOL_VERSION = "2025-06-18"


class McpError(RuntimeError):
    pass


class McpTimeout(McpError):
    pass


class StdioMcp(MCP):
    def __init__(self, argv, env=None, cwd=None, stderr_path=None, timeout=120.0):
        # Do not call MCP.__init__: it discards stderr, and we want a log file for post-mortems.
        import subprocess
        self._stderr_f = open(stderr_path, "ab") if stderr_path else subprocess.DEVNULL
        full_env = dict(os.environ)
        if env:
            full_env.update(env)
        self.p = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=self._stderr_f, text=True, env=full_env, cwd=cwd, bufsize=1)
        self.n = 0
        self.calls = []
        self.info = {}
        self.timeout = timeout
        self.notifications = []
        self._q = queue.Queue()
        self._t = threading.Thread(target=self._reader, daemon=True)
        self._t.start()

    def _reader(self):
        try:
            for line in self.p.stdout:
                try:
                    self._q.put(json.loads(line))
                except ValueError:
                    continue  # logging noise on stdout; ignore like the original client
        finally:
            self._q.put(None)

    def _rpc(self, method, params=None, notify=False, timeout=None):
        timeout = timeout or self.timeout
        msg = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            msg["params"] = params
        if not notify:
            self.n += 1
            msg["id"] = self.n
        try:
            self.p.stdin.write(json.dumps(msg) + "\n")
            self.p.stdin.flush()
        except (BrokenPipeError, OSError) as e:
            raise McpError(f"server closed stdin: {e}")
        if notify:
            return None
        want = self.n
        deadline = time.time() + timeout
        while True:
            left = deadline - time.time()
            if left <= 0:
                raise McpTimeout(f"{method} timed out after {timeout}s")
            try:
                r = self._q.get(timeout=left)
            except queue.Empty:
                raise McpTimeout(f"{method} timed out after {timeout}s")
            if r is None:
                raise McpError("server closed stdout")
            if "method" in r:
                if "id" in r:
                    # Server-to-client request (sampling, roots, ...): we advertise no capabilities.
                    self._reply_unsupported(r)
                else:
                    self.notifications.append(r["method"])
                continue
            if r.get("id") == want:
                return r

    def _reply_unsupported(self, req):
        resp = {"jsonrpc": "2.0", "id": req["id"], "error": {"code": -32601, "message": "not supported by client"}}
        try:
            self.p.stdin.write(json.dumps(resp) + "\n")
            self.p.stdin.flush()
        except OSError:
            pass

    def start(self):
        r = self._rpc("initialize", {"protocolVersion": PROTOCOL_VERSION, "capabilities": {},
                                      "clientInfo": {"name": "ostler-eval", "version": "1"}})
        if "result" not in r:
            raise McpError(f"initialize failed: {r.get('error')}")
        self.info = r["result"].get("serverInfo", {})
        self.protocol = r["result"].get("protocolVersion")
        self._rpc("notifications/initialized", notify=True)
        return self

    def list_tools(self):
        """All tools, following pagination. Returns the raw MCP tool dicts."""
        tools, cursor = [], None
        for _ in range(20):
            r = self._rpc("tools/list", {"cursor": cursor} if cursor else None)
            if "result" not in r:
                raise McpError(f"tools/list failed: {r.get('error')}")
            tools.extend(r["result"].get("tools", []))
            cursor = r["result"].get("nextCursor")
            if not cursor:
                break
        return tools

    def call_tool(self, name, arguments, timeout=None):
        """Returns (text, is_error, image_bytes). Never raises for a tool-level error."""
        t0 = time.time()
        try:
            r = self._rpc("tools/call", {"name": name, "arguments": arguments or {}}, timeout=timeout or self.timeout)
        except McpTimeout as e:
            return f"[harness] tool call timed out: {e}", True, 0
        ms = (time.time() - t0) * 1000
        res = r.get("result")
        if res is None:
            return "[mcp error] " + json.dumps(r.get("error")), True, 0
        text, img = [], 0
        for part in res.get("content", []) or []:
            if part.get("type") == "text":
                text.append(part.get("text", ""))
            elif part.get("type") == "image":
                n = len(part.get("data", ""))
                img += n
                text.append(f"[image omitted by harness: {n} base64 chars]")
            else:
                text.append(f"[{part.get('type', 'content')} part omitted by harness]")
        if res.get("structuredContent") is not None and not text:
            text.append(json.dumps(res["structuredContent"]))
        self.last_call_ms = ms
        return "\n".join(text), bool(res.get("isError")), img

    def close(self):
        try:
            self.p.stdin.close()
            self.p.wait(timeout=5)
        except Exception:
            self.p.kill()
        finally:
            for pipe in (self.p.stdin, self.p.stdout):
                try:
                    pipe.close()
                except Exception:
                    pass
            if hasattr(self._stderr_f, "close"):
                try:
                    self._stderr_f.close()
                except Exception:
                    pass


def load_servers(path):
    """servers file: {"servers": {"<id>": {"command": [argv...], "env": {...}, "cwd": "..."}}}"""
    with open(path) as f:
        d = json.load(f)
    servers = d.get("servers", d)
    for sid, s in servers.items():
        if not isinstance(s.get("command"), list) or not s["command"]:
            raise ValueError(f"server {sid!r}: 'command' must be a non-empty list of strings")
    return servers
