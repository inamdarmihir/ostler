"""Test helpers: a stub fixture server, a fake LLM endpoint, and paths."""
import json
import os
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HERE = os.path.dirname(os.path.abspath(__file__))
EVAL = os.path.dirname(HERE)
sys.path.insert(0, EVAL)
STUB_SERVER = os.path.join(HERE, "stub_mcp_server.py")


class _Quiet(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass


def make_fixture_handler(state):
    class H(_Quiet):
        def _body(self):
            n = int(self.headers.get("Content-Length") or 0)
            raw = self.rfile.read(n).decode() if n else ""
            try:
                return json.loads(raw) if raw else None
            except ValueError:
                return raw

        def _send(self, code, obj, ctype="application/json"):
            data = obj.encode() if isinstance(obj, str) else json.dumps(obj).encode()
            self.send_response(code)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self):
            path = self.path.split("?")[0]
            if path == "/__records":
                return self._send(200, state["records"])
            state["records"].append({"ts": time.time(), "method": "GET", "path": path,
                                     "query": self.path.partition("?")[2], "body": None, "pack": "stub"})
            self._send(200, "Welcome to the stub site. Order form at /order.", "text/plain")

        def do_POST(self):
            path = self.path.split("?")[0]
            body = self._body()
            if path == "/__reset":
                state["records"].clear()
                return self._send(200, {"ok": True})
            state["records"].append({"ts": time.time(), "method": "POST", "path": path,
                                     "query": self.path.partition("?")[2], "body": body, "pack": "stub"})
            self._send(200, {"ok": True})
    return H


class StubFixture:
    def __init__(self):
        self.state = {"records": []}
        self.httpd = ThreadingHTTPServer(("127.0.0.1", 0), make_fixture_handler(self.state))
        self.base = f"http://127.0.0.1:{self.httpd.server_address[1]}"
        self.t = threading.Thread(target=self.httpd.serve_forever, daemon=True)

    def __enter__(self):
        self.t.start()
        return self

    def __exit__(self, *a):
        self.httpd.shutdown()
        self.httpd.server_close()


class FakeLLM:
    """Serves canned responses for /chat/completions (OpenAI shape) and /v1/messages (Anthropic shape)
    and records the last request (headers + body) for assertions."""

    def __init__(self, responses):
        self.responses = list(responses)
        self.requests = []
        outer = self

        class H(_Quiet):
            def do_POST(self):
                n = int(self.headers.get("Content-Length") or 0)
                body = json.loads(self.rfile.read(n))
                outer.requests.append({"path": self.path, "headers": {k.lower(): v for k, v in self.headers.items()}, "body": body})
                r = outer.responses.pop(0) if outer.responses else {}
                code = 200
                if isinstance(r, tuple):
                    code, r = r
                data = json.dumps(r).encode()
                self.send_response(code)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

        self.httpd = ThreadingHTTPServer(("127.0.0.1", 0), H)
        self.base = f"http://127.0.0.1:{self.httpd.server_address[1]}"
        self.t = threading.Thread(target=self.httpd.serve_forever, daemon=True)

    def __enter__(self):
        self.t.start()
        return self

    def __exit__(self, *a):
        self.httpd.shutdown()
        self.httpd.server_close()


def read(path):
    with open(path) as f:
        return f.read()


def jsonl(path):
    return [json.loads(line) for line in read(path).splitlines() if line.strip()]


def sample_tasks():
    return [
        {"id": "order", "pack": "demo", "title": "Place the order", "difficulty": "easy",
         "instruction": "Open {BASE}/order and order 2 widgets for Ada.", "start": "/order", "tags": ["forms"],
         "outcome": {"kind": "records",
                     "must": [{"method": "POST", "path": "/order", "body_contains": {"item": "widget", "qty": 2}}],
                     "must_not": [{"path": "/delete"}]},
         "expect_gaps": [], "risk": "reversible"},
        {"id": "read", "pack": "demo", "title": "Read the price", "difficulty": "easy",
         "instruction": "What is on {BASE}/order?", "start": "/order", "tags": ["research"],
         "outcome": {"kind": "answer", "must_contain": ["order form"], "must_cite": ["/order"],
                     "must_not_contain": ["secret"]},
         "expect_gaps": [], "risk": "read"},
    ]


def write_packs(root, tasks=None):
    tasks = tasks or sample_tasks()
    by_pack = {}
    for t in tasks:
        by_pack.setdefault(t["pack"], []).append(t)
    for pack, ts in by_pack.items():
        os.makedirs(os.path.join(root, pack), exist_ok=True)
        with open(os.path.join(root, pack, "tasks.json"), "w") as f:
            json.dump({"tasks": ts}, f)
    return root
