#!/usr/bin/env python3
"""A tiny stdio MCP server used by the tests, so the agent loop runs without Chrome.

Tools:
  navigate(url)          GET the URL (the stub fixture) and return its text; the first call reveals `submit`
                         and sends notifications/tools/list_changed, like a server with a dynamic tool list.
  submit(path, body)     POST body (an object) as JSON to STUB_FIXTURE + path; returns the status line.
  big_output()           returns 20000 'x' characters (truncation tests).
  image()                returns an image part plus a text part.
  fail()                 returns isError.
  slow(seconds)          sleeps (timeout tests).
Env: STUB_FIXTURE=http://127.0.0.1:PORT
"""
import json
import os
import sys
import time
import urllib.request

FIXTURE = os.environ.get("STUB_FIXTURE", "http://127.0.0.1:1")
revealed = False

OBJ = {"type": "object", "properties": {}}
TOOLS = {
    "navigate": {"description": "Open a URL and return the page text.",
                 "inputSchema": {"type": "object", "properties": {"url": {"type": "string"}}, "required": ["url"]}},
    "big_output": {"description": "Return a very large text.", "inputSchema": OBJ},
    "image": {"description": "Return an image.", "inputSchema": OBJ},
    "fail": {"description": "Always errors.", "inputSchema": OBJ},
    "slow": {"description": "Sleep for the `seconds` argument (undeclared on purpose, so random callers never sleep).", "inputSchema": OBJ},
}
SUBMIT = {"description": "POST a JSON body to a path on the site.",
          "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}, "body": {"type": "object"}},
                          "required": ["path", "body"]}}


def tools():
    t = [dict(name=k, **v) for k, v in TOOLS.items()]
    if revealed:
        t.append(dict(name="submit", **SUBMIT))
    return t


def send(m):
    sys.stdout.write(json.dumps(m) + "\n")
    sys.stdout.flush()


def text(s, err=False):
    return {"content": [{"type": "text", "text": s}], "isError": err}


def call(name, a):
    global revealed
    if name == "navigate":
        try:
            body = urllib.request.urlopen(a["url"], timeout=5).read().decode()
        except Exception as e:
            return text(f"navigation failed: {e}", True)
        if not revealed:
            revealed = True
            return text(body), True
        return text(body)
    if name == "submit" and revealed:
        req = urllib.request.Request(FIXTURE + a["path"], data=json.dumps(a["body"]).encode(), method="POST",
                                     headers={"Content-Type": "application/json"})
        try:
            r = urllib.request.urlopen(req, timeout=5)
            return text(f"submitted: HTTP {r.status}")
        except Exception as e:
            return text(f"submit failed: {e}", True)
    if name == "big_output":
        return text("x" * 20000)
    if name == "image":
        return {"content": [{"type": "image", "data": "A" * 400, "mimeType": "image/png"},
                            {"type": "text", "text": "a picture"}]}
    if name == "fail":
        return text("it broke", True)
    if name == "slow":
        time.sleep(float(a.get("seconds", 0)))
        return text("slept")
    return None


for line in sys.stdin:
    if not line.strip():
        continue
    m = json.loads(line)
    meth, mid = m.get("method"), m.get("id")
    if meth == "initialize":
        send({"jsonrpc": "2.0", "id": mid, "result": {
            "protocolVersion": "2025-06-18", "capabilities": {"tools": {"listChanged": True}},
            "serverInfo": {"name": "stub", "version": "0.0.1"}}})
    elif meth == "tools/list":
        send({"jsonrpc": "2.0", "id": mid, "result": {"tools": tools()}})
    elif meth == "tools/call":
        p = m["params"]
        r = call(p["name"], p.get("arguments") or {})
        if r is None:
            send({"jsonrpc": "2.0", "id": mid, "error": {"code": -32602, "message": "unknown tool " + p["name"]}})
        elif isinstance(r, tuple):
            send({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"})
            send({"jsonrpc": "2.0", "id": mid, "result": r[0]})
        else:
            send({"jsonrpc": "2.0", "id": mid, "result": r})
    elif mid is not None:
        send({"jsonrpc": "2.0", "id": mid, "error": {"code": -32601, "message": "method not found"}})
