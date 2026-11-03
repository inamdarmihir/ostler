#!/bin/sh
# Run a command against a throwaway Qdrant that has an API key and JWT RBAC enabled.
#   scripts/ci-qdrant-rbac.sh <qdrant-binary> <command> [args...]
# QDRANT_URL and OSTLER_QDRANT_ADMIN_KEY are exported to <command> only. The key is random per run, is
# never printed, and the server is stopped (by PID, not by name) when the command finishes.
# Fails if the command's output contains "SKIPPED", so a suite cannot pass by silently skipping.
set -eu
[ "$#" -ge 2 ] || { echo "usage: $0 <qdrant-binary> <command> [args...]" >&2; exit 2; }
bin="$1"; shift
work="$(mktemp -d)"
log="$work/test.log"
pid=""
cleanup() {
  [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
  [ -n "$pid" ] && wait "$pid" 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT INT TERM

key="$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')"
[ -n "${GITHUB_ACTIONS:-}" ] && echo "::add-mask::$key"
http=6433; grpc=6434

(
  cd "$work"
  export QDRANT__SERVICE__HTTP_PORT="$http" QDRANT__SERVICE__GRPC_PORT="$grpc"
  export QDRANT__SERVICE__API_KEY="$key" QDRANT__SERVICE__JWT_RBAC=true
  export QDRANT__STORAGE__STORAGE_PATH="$work/storage" QDRANT__STORAGE__SNAPSHOTS_PATH="$work/snaps"
  export QDRANT__TELEMETRY_DISABLED=true
  exec "$bin" >"$work/qdrant.log" 2>&1
) &
pid=$!

# /readyz is unauthenticated in Qdrant; wait up to 60s.
i=0
until curl -sf "http://127.0.0.1:$http/readyz" >/dev/null 2>&1; do
  i=$((i + 1))
  if [ "$i" -gt 120 ] || ! kill -0 "$pid" 2>/dev/null; then
    echo "qdrant (RBAC instance) did not become ready; last log lines:" >&2
    tail -n 20 "$work/qdrant.log" | sed "s/$key/***/g" >&2
    exit 1
  fi
  sleep 0.5
done

status=0
QDRANT_URL="http://127.0.0.1:$http" OSTLER_QDRANT_ADMIN_KEY="$key" "$@" >"$log" 2>&1 || status=$?
cat "$log"
if [ "$status" -ne 0 ]; then
  echo "command failed (exit $status)" >&2; exit "$status"
fi
# Pass --nocapture to the test binary so skip notices are visible here.
if grep -q "SKIPPED" "$log"; then
  echo "a test reported SKIPPED; the RBAC suite must really run" >&2; exit 1
fi
