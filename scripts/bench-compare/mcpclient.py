"""Minimal stdio MCP client (newline-delimited JSON-RPC) that records per-call cost."""
import json, subprocess, time, threading


class Call:
    __slots__ = ("tool", "args", "ms", "out_bytes", "args_bytes", "image_bytes", "is_error", "text")


class MCP:
    def __init__(self, argv, env=None, cwd=None):
        self.p = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=subprocess.DEVNULL, text=True, env=env, cwd=cwd, bufsize=1)
        self.n = 0
        self.calls = []
        self.info = {}

    def _rpc(self, method, params=None, notify=False, timeout=120):
        msg = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            msg["params"] = params
        if not notify:
            self.n += 1
            msg["id"] = self.n
        self.p.stdin.write(json.dumps(msg) + "\n")
        self.p.stdin.flush()
        if notify:
            return None
        want = self.n
        t0 = time.time()
        while True:
            line = self.p.stdout.readline()
            if not line:
                raise RuntimeError("server closed stdout")
            try:
                r = json.loads(line)
            except ValueError:
                continue
            if r.get("id") == want and "method" not in r:
                return r
            if time.time() - t0 > timeout:
                raise TimeoutError(method)

    def start(self):
        r = self._rpc("initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                                      "clientInfo": {"name": "ostler-bench-compare", "version": "1"}})
        self.info = r["result"].get("serverInfo", {})
        self._rpc("notifications/initialized", notify=True)
        tl = self._rpc("tools/list")["result"]["tools"]
        self.tools = {t["name"]: t for t in tl}
        # Size of the tool definitions a client sends to the model every turn.
        self.schema_bytes = len(json.dumps(tl, separators=(",", ":")).encode())
        return self

    def call(self, tool, **args):
        c = Call()
        c.tool, c.args = tool, args
        c.args_bytes = len(json.dumps(args, separators=(",", ":")).encode())
        t = time.time()
        r = self._rpc("tools/call", {"name": tool, "arguments": args}, timeout=180)
        c.ms = (time.time() - t) * 1000
        res = r.get("result")
        text, img = [], 0
        if res is None:
            c.is_error = True
            text.append(json.dumps(r.get("error")))
        else:
            c.is_error = bool(res.get("isError"))
            for part in res.get("content", []):
                if part.get("type") == "text":
                    text.append(part.get("text", ""))
                else:
                    img += len(part.get("data", ""))
        c.text = "\n".join(text)
        c.out_bytes = len(c.text.encode())
        c.image_bytes = img
        self.calls.append(c)
        return c

    def close(self):
        try:
            self.p.stdin.close()
            self.p.wait(timeout=5)
        except Exception:
            self.p.kill()
