#!/bin/sh
# End-to-end test of the install paths against a fake local release. Publishes nothing.
#   scripts/test-installers.sh [path-to-ostler-binary]   (builds one if not given)
# Checks: install.sh (happy path, upgrade, tampered download, missing version) and the npm
# launcher (download+cache, pure JSON-RPC on stdout through `npx`, tampered download).
set -eu
[ -n "${DEBUG:-}" ] && set -x
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
bin="${1:-}"
if [ -z "$bin" ]; then cargo build --release --locked -p ostler >&2; bin="$root/target/release/ostler"; fi
target="$(rustc -vV | sed -n 's/^host: //p')"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
work="$(mktemp -d)"; srv=""
# shellcheck disable=SC2086  # $srv is a list of PIDs
trap '[ -n "$srv" ] && kill $srv 2>/dev/null; rm -rf "$work"' EXIT
pass() { printf '  ok   %s\n' "$1"; }
fail() { printf '  FAIL %s\n' "$1" >&2; exit 1; }

mkdir -p "$work/good/download/v$version" "$work/bad/download/v$version"
scripts/package.sh "$target" "$bin" "$work/good/download/v$version" >/dev/null
cp "$work"/good/download/v"$version"/* "$work/bad/download/v$version/"
echo tamper >> "$work/bad/download/v$version/ostler-v$version-$target.tar.gz"

port="$(python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1])')"
cat > "$work/serve.py" <<PY
import http.server, sys
root, port = sys.argv[1], int(sys.argv[2])
class H(http.server.SimpleHTTPRequestHandler):
    def __init__(s, *a, **k): super().__init__(*a, directory=root, **k)
    def do_GET(s):
        if s.path == '/latest':
            s.send_response(302); s.send_header('Location', '/tag/v$version'); s.end_headers(); return
        if s.path.startswith('/tag/'):
            s.send_response(200); s.end_headers(); s.wfile.write(b'ok'); return
        super().do_GET()
    def log_message(s, *a): pass
http.server.ThreadingHTTPServer(('127.0.0.1', port), H).serve_forever()
PY
python3 "$work/serve.py" "$work/good" "$port" >/dev/null 2>&1 & srv=$!
port_bad="$(python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1])')"
python3 "$work/serve.py" "$work/bad" "$port_bad" >/dev/null 2>&1 & srv="$srv $!"
sleep 1
good="http://127.0.0.1:$port"; bad="http://127.0.0.1:$port_bad"

echo "install.sh"
OSTLER_RELEASE_BASE=$good OSTLER_INSTALL_DIR="$work/i1" sh site/install.sh >/dev/null 2>&1 || fail "install latest"
[ "$("$work/i1/ostler" --version)" = "ostler $version" ] && pass "installs the latest release and it runs" || fail "version"
OSTLER_RELEASE_BASE=$good OSTLER_INSTALL_DIR="$work/i1" OSTLER_VERSION=$version sh site/install.sh >/dev/null 2>&1 && pass "re-running upgrades in place" || fail "rerun"
if OSTLER_RELEASE_BASE=$bad OSTLER_INSTALL_DIR="$work/i2" OSTLER_VERSION=$version sh site/install.sh >/dev/null 2>&1; then fail "tampered archive was installed"; fi
[ ! -e "$work/i2/ostler" ] && pass "refuses a tampered download and installs nothing" || fail "tampered left a file"
if OSTLER_RELEASE_BASE=$good OSTLER_INSTALL_DIR="$work/i3" OSTLER_VERSION=99.0.0 sh site/install.sh >/dev/null 2>&1; then fail "missing version succeeded"; fi
pass "fails clearly for an unpublished version"

echo "npm launcher"
(cd npm && npm pack --silent --pack-destination "$work" >/dev/null)
tgz="$work/ostler-$version.tgz"
nx() { npx --yes --package "$tgz" -- ostler "$@"; }
OSTLER_RELEASE_BASE=$good OSTLER_CACHE_DIR="$work/cache" nx --version 2>"$work/err1" | grep -qx "ostler $version" && pass "first run downloads, verifies and runs" || fail "npx first run"
grep -q downloading "$work/err1" && pass "progress goes to stderr" || fail "no progress on stderr"
OSTLER_RELEASE_BASE=$bad OSTLER_CACHE_DIR="$work/cache" nx --version 2>"$work/err2" | grep -qx "ostler $version" && pass "second run uses the cache (no download)" || fail "cache"
[ ! -s "$work/err2" ] && pass "cached run is silent" || fail "cached run wrote to stderr"
if OSTLER_RELEASE_BASE=$bad OSTLER_CACHE_DIR="$work/cache-bad" nx --version >/dev/null 2>&1; then fail "npm ran a tampered binary"; fi
pass "refuses a tampered download"
OSTLER_RELEASE_BASE=$good OSTLER_CACHE_DIR="$work/cache" python3 - "$tgz" <<'PY' && pass "MCP handshake through npx: stdout is pure JSON-RPC" || fail "MCP handshake"
import json, subprocess, sys
p = subprocess.Popen(["npx", "--yes", "--package", sys.argv[1], "--", "ostler", "serve", "--memory", "off"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
def rpc(i, m, params):
    p.stdin.write(json.dumps({"jsonrpc": "2.0", "id": i, "method": m, "params": params}) + "\n"); p.stdin.flush()
    return json.loads(p.stdout.readline())
assert rpc(1, "initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}})["result"]["serverInfo"]["name"] == "ostler"
assert len(rpc(2, "tools/list", {})["result"]["tools"]) > 10
p.stdin.close(); p.wait(timeout=20)
PY
echo "all installer checks passed"
