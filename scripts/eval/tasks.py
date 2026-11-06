"""Task loading against the contract shared with crates/ostler-fixtures/packs/<pack>/tasks.json.

task = {"id","pack","title","difficulty","instruction" (may contain {BASE}),"start","tags",
        "outcome": {...}, "expect_gaps": [], "risk": "..."}
outcome = {"kind": "records", "must": [{"method","path","body_contains":{}}], "must_not": [{"path"}]}
        | {"kind": "answer",  "must_contain": [], "must_cite": [], "must_not_contain": []}
        | {"kind": "both", ...both sets of fields, flat or under "records"/"answer"}
"""
import glob
import json
import os

REQUIRED = ("id", "instruction", "outcome")
KINDS = ("records", "answer", "both")


class TaskError(ValueError):
    pass


def validate_task(t, where="task"):
    for k in REQUIRED:
        if k not in t:
            raise TaskError(f"{where}: missing required key {k!r}")
    o = t["outcome"]
    if not isinstance(o, dict) or o.get("kind") not in KINDS:
        raise TaskError(f"{where} {t['id']}: outcome.kind must be one of {KINDS}")
    for m in _must(o):
        if "path" not in m:
            raise TaskError(f"{where} {t['id']}: every 'must' entry needs a 'path'")
    return t


def _must(o):
    r = o.get("records", o)
    return list(r.get("must", []) or [])


def normalise_task(t, pack=None):
    t = dict(t)
    t.setdefault("pack", pack or "")
    t.setdefault("title", t["id"])
    t.setdefault("difficulty", "")
    t.setdefault("start", "")
    t.setdefault("tags", [])
    t.setdefault("expect_gaps", [])
    t.setdefault("risk", "")
    return t


def load_file(path):
    with open(path) as f:
        d = json.load(f)
    items = d["tasks"] if isinstance(d, dict) and "tasks" in d else d
    if not isinstance(items, list):
        raise TaskError(f"{path}: expected a list of tasks or {{'tasks': [...]}}")
    pack = os.path.basename(os.path.dirname(os.path.abspath(path)))
    return [validate_task(normalise_task(t, pack), where=path) for t in items]


def load_packs(packs_dir, packs=None, ids=None, tags=None):
    """Load every packs_dir/<pack>/tasks.json, optionally filtered by pack names, task ids or tags."""
    out = []
    for p in sorted(glob.glob(os.path.join(packs_dir, "*", "tasks.json"))):
        pack = os.path.basename(os.path.dirname(p))
        if packs and pack not in packs:
            continue
        out.extend(load_file(p))
    if ids:
        out = [t for t in out if t["id"] in ids]
    if tags:
        out = [t for t in out if set(tags) & set(t["tags"])]
    seen = set()
    for t in out:
        key = (t["pack"], t["id"])
        if key in seen:
            raise TaskError(f"duplicate task {key}")
        seen.add(key)
    return out


def task_key(t):
    return f"{t['pack']}/{t['id']}" if t.get("pack") else t["id"]


def render(text, base):
    return (text or "").replace("{BASE}", base.rstrip("/"))


def start_url(t, base):
    s = render(t.get("start", ""), base)
    if not s:
        return base.rstrip("/") + "/"
    if s.startswith(("http://", "https://")):
        return s
    return base.rstrip("/") + "/" + s.lstrip("/")
