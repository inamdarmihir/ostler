#!/usr/bin/env bash
# One command: Ostler vs Playwright MCP vs Chrome DevTools MCP on the local fixtures.
# Needs: cargo, node/npm, python3, Google Chrome. Network is used only for `npm install`.
# Override versions: PLAYWRIGHT_MCP_VERSION=latest CHROME_DEVTOOLS_MCP_VERSION=latest scripts/bench-compare.sh
# Output: bench-results/compare.json and bench-results/compare.md
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
PW_VER="${PLAYWRIGHT_MCP_VERSION:-0.0.84}"
DT_VER="${CHROME_DEVTOOLS_MCP_VERSION:-1.12.0}"
REPS="${REPS:-3}"
PORT="${PORT:-8765}"
WORK="$(mktemp -d)"
FX_PID=""
cleanup() { [ -n "$FX_PID" ] && kill "$FX_PID" 2>/dev/null || true; rm -rf "$WORK"; }
trap cleanup EXIT

cargo build --release -p ostler -p ostler-bench
(cd "$WORK" && npm init -y >/dev/null && npm_config_cache="$WORK/cache" npm i --silent \
  "@playwright/mcp@$PW_VER" "chrome-devtools-mcp@$DT_VER")

./target/release/ostler-bench serve-fixtures --port "$PORT" >/dev/null &
FX_PID=$!
for _ in $(seq 50); do curl -sf "http://127.0.0.1:$PORT/__submissions" >/dev/null && break; sleep 0.1; done

mkdir -p bench-results
python3 scripts/bench-compare/run.py --fixtures "http://127.0.0.1:$PORT" \
  --ostler ./target/release/ostler --npm-dir "$WORK" --reps "$REPS" --out bench-results/compare.json
python3 scripts/bench-compare/report.py bench-results/compare.json | tee bench-results/compare.md
