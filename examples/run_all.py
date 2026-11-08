"""Run every example against the bundled fixture pages and print a summary.

Prerequisites: `ostler` on PATH (or OSTLER_BIN), `ostler fixtures` serving on 127.0.0.1:8787,
and for example 11 `ostler a2a --allow-domain 127.0.0.1`. Exit code 0 only if nothing failed.
"""

import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent
SKIP_EXIT_CODE = 3


def main() -> int:
    only = set(sys.argv[1:])
    results = []
    scripts = sorted(ROOT.glob("[0-9][0-9]-*/run.py")) + sorted(ROOT.glob("agents/*/test_offline.py"))
    for script in scripts:
        name = str(script.parent.relative_to(ROOT))
        if only and not any(name.startswith(prefix) for prefix in only):
            continue
        started = time.monotonic()
        proc = subprocess.run([sys.executable, str(script)], capture_output=True, text=True, timeout=300)
        seconds = time.monotonic() - started
        if proc.returncode == 0:
            status = "pass"
        elif proc.returncode == SKIP_EXIT_CODE:
            status = "skip"
        else:
            status = "FAIL"
        results.append((name, status, seconds))
        print(f"{status:4}  {name}  ({seconds:.1f}s)")
        if status == "FAIL":
            print(proc.stdout[-3000:], proc.stderr[-3000:], sep="\n")
    failed = [name for name, status, _ in results if status == "FAIL"]
    passed = sum(1 for _, status, _ in results if status == "pass")
    print(f"\n{passed} passed, {len(failed)} failed, {len(results) - passed - len(failed)} skipped")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
