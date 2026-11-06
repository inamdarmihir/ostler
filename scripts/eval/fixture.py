"""Client for the fixture server started by `ostler fixtures` (default http://127.0.0.1:8787).

Contract (owned by crates/ostler-fixtures, not by this directory):
    GET  /__records  -> JSON list of {ts, method, path, query, body, pack}
    POST /__reset    -> clears the records
"""
import json
import urllib.error
import urllib.request

DEFAULT_BASE = "http://127.0.0.1:8787"


class FixtureError(RuntimeError):
    pass


class Fixture:
    def __init__(self, base=DEFAULT_BASE, timeout=10):
        self.base = base.rstrip("/")
        self.timeout = timeout

    def _req(self, method, path, body=None):
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(self.base + path, data=data, method=method,
                                     headers={"Content-Type": "application/json"} if data is not None else {})
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as r:
                return r.status, r.read()
        except urllib.error.HTTPError as e:
            return e.code, e.read()
        except (urllib.error.URLError, OSError) as e:
            raise FixtureError(f"cannot reach the fixture server at {self.base} ({e}). "
                               "Start it with `ostler fixtures` (default http://127.0.0.1:8787).")

    def reset(self):
        status, body = self._req("POST", "/__reset", {})
        if status >= 300:
            raise FixtureError(f"POST /__reset returned {status}: {body[:200]!r}")

    def records(self):
        status, body = self._req("GET", "/__records")
        if status >= 300:
            raise FixtureError(f"GET /__records returned {status}: {body[:200]!r}")
        try:
            d = json.loads(body)
        except ValueError:
            raise FixtureError("GET /__records did not return JSON")
        if isinstance(d, dict) and "records" in d:
            d = d["records"]
        if not isinstance(d, list):
            raise FixtureError("GET /__records did not return a list")
        return d

    def send(self, method, path, body=None):
        """Used only by the scripted oracle (harness self-test). Real agents act through a browser."""
        return self._req(method.upper(), path, body if method.upper() != "GET" else None)
