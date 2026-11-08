"""Recipe 11: another agent hands Ostler a form over A2A v1.0 and approves the submit.

Start the server first, with an allowlist (A2A blocks loopback without one):

    ostler a2a --allow-domain 127.0.0.1

This client uses only the standard library, so the JSON-RPC shapes are visible.
"""

import json
import os
import sys
import time
import urllib.error
import urllib.request
import uuid

A2A = os.environ.get("OSTLER_A2A", "http://127.0.0.1:8765")
BASE = os.environ.get("OSTLER_FIXTURES", "http://127.0.0.1:8787")


MAX_ATTEMPTS = 4
BUSY_STATUSES = (429, 503)  # too many tasks in flight, or every conversation slot is taken


def post(request: urllib.request.Request) -> dict:
    """POST once per attempt, backing off as the server's Retry-After header asks when it is busy."""
    for attempt in range(1, MAX_ATTEMPTS + 1):
        try:
            with urllib.request.urlopen(request, timeout=150) as response:
                return json.load(response)
        except urllib.error.HTTPError as err:
            if err.code not in BUSY_STATUSES or attempt == MAX_ATTEMPTS:
                raise SystemExit(f"FAIL: HTTP {err.code}: {err.read().decode(errors='replace')[:300]}") from None
            wait = int(err.headers.get("Retry-After", "5"))
            print(f"(server busy, HTTP {err.code}; retrying in {wait}s as Retry-After asks)")
            time.sleep(wait)
    raise AssertionError("unreachable")


def rpc(endpoint: str, method: str, params: dict) -> dict:
    body = json.dumps({"jsonrpc": "2.0", "id": str(uuid.uuid4()), "method": method, "params": params}).encode()
    request = urllib.request.Request(
        endpoint, data=body, headers={"Content-Type": "application/json", "A2A-Version": "1.0"}
    )
    reply = post(request)
    if "error" in reply:
        raise SystemExit(f"FAIL: {method} returned {reply['error']}")
    return reply["result"]


def send(endpoint: str, data: dict, task_id: str | None = None) -> dict:
    message = {"messageId": str(uuid.uuid4()), "role": "ROLE_USER", "parts": [{"data": data}]}
    if task_id:
        message["taskId"] = task_id
    result = rpc(endpoint, "SendMessage", {"message": message})
    task = result.get("task", result)
    state = task["status"]["state"]
    texts = [
        part.get("text", "")
        for artifact in task.get("artifacts", [])
        if artifact.get("name") == "result"
        for part in artifact.get("parts", [])
    ]
    print(f"\n> SendMessage {json.dumps(data)}\n{state}\n" + "\n".join(texts))
    return task


def expect(condition: bool, message: str) -> None:
    if not condition:
        raise SystemExit(f"FAIL: {message}")
    print(f"CHECK OK: {message}")


def main() -> None:
    try:
        card = json.load(urllib.request.urlopen(f"{A2A}/.well-known/agent-card.json", timeout=5))
    except OSError:
        print(f"SKIP: no A2A server at {A2A}; start one with `ostler a2a --allow-domain 127.0.0.1`.")
        sys.exit(3)
    # Discover the JSON-RPC endpoint from the agent card rather than hard-coding it.
    endpoint = next(i["url"] for i in card["supportedInterfaces"] if i["protocolBinding"] == "JSONRPC")
    print(f"Agent card: {card['name']} {card.get('version', '')}, JSON-RPC at {endpoint}")

    # Fill and ask to submit; Ostler stops and asks for confirmation instead of submitting.
    task = send(
        endpoint,
        {
            "url": f"{BASE}/basic.html",
            "values": {"Full name": "Ada Lovelace", "Email": "ada@example.com"},
            "submit": True,
        },
    )
    expect(task["status"]["state"] == "TASK_STATE_INPUT_REQUIRED", "the task waits for confirmation")

    # Same task: the caller (or its human) agrees.
    task = send(endpoint, {"confirm": True}, task_id=task["id"])
    expect(task["status"]["state"] == "TASK_STATE_COMPLETED", "submitted after confirm")


main()
