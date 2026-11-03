"""One 'ideal agent' per server: the shortest sensible sequence, with refs read from the tool's own output.

Each driver returns None on success of the *script* (the page outcome is judged separately from the
fixture server's record, never from the tool's claim).
"""
import re

# ---------- helpers ----------

def norm(s):
    return re.sub(r"\s*\*\s*$", "", s).strip().lower()


def find_ref(text, label, roles, ref_re):
    """Return the ref of the first line whose role is in roles and whose quoted name matches label."""
    want = norm(label)
    for line in text.splitlines():
        m = re.search(r'(\w+) "([^"]*)"', line)
        if not m or m.group(1) not in roles:
            continue
        if norm(m.group(2)) == want:
            r = re.search(ref_re, line)
            if r:
                return r.group(1)
    return None


ROLE = {"text": ("textbox", "spinbutton", "searchbox", "Date", "date"), "check": ("checkbox",), "select": ("combobox", "listbox")}


def _chunks(items, task, step_index):
    k = task.get("split_after")
    if k and step_index == 0:
        return [items[:k], items[k:]]
    return [items]


class Fail(Exception):
    pass


# ---------- Playwright MCP ----------

PW_REF = r"\[ref=(\w+)\]"


def playwright(m, url, task):
    c = m.call("browser_navigate", url=url)
    snap = ""  # this version returns the snapshot as a file link on navigate; an MCP-only agent calls browser_snapshot
    steps, submits = task["steps"], task["submit"]
    if isinstance(submits, str):
        submits = [submits]
    for i, step in enumerate(steps):
        snap = m.call("browser_snapshot").text
        fields = []
        for label, kind, val in step:
            ref = find_ref(snap, label, ROLE[kind], PW_REF)
            if not ref:
                raise Fail(f"no ref for {label!r}")
            ftype = {"text": "textbox", "check": "checkbox", "select": "combobox"}[kind]
            v = ("true" if val else "false") if kind == "check" else val
            fields.append({"name": label, "type": ftype, "target": ref, "value": v})
        for chunk in _chunks(fields, task, i):
            r = m.call("browser_fill_form", fields=chunk)
            if r.is_error:
                raise Fail("fill_form: " + r.text[:200])
        btn = find_ref(snap, submits[i], ("button",), PW_REF)
        if not btn:
            raise Fail(f"no button {submits[i]!r}")
        r = m.call("browser_click", target=btn)
        if r.is_error:
            raise Fail("click: " + r.text[:200])


# ---------- Chrome DevTools MCP ----------

DT_REF = r"uid=(\S+)"


def devtools(m, url, task):
    m.call("navigate_page", pageId=1, type="url", url=url)
    steps, submits = task["steps"], task["submit"]
    if isinstance(submits, str):
        submits = [submits]
    for i, step in enumerate(steps):
        snap = m.call("take_snapshot", pageId=1).text
        els = []
        for label, kind, val in step:
            ref = find_ref(snap, label, ROLE[kind], DT_REF)
            if not ref:
                raise Fail(f"no uid for {label!r}")
            v = ("true" if val else "false") if kind == "check" else val
            els.append({"uid": ref, "value": v})
        for j, chunk in enumerate(_chunks(els, task, i)):
            if j and task.get("wait_text"):
                # Async option list: an idiomatic agent waits for it (hidden option text counts as "on the page").
                m.call("wait_for", pageId=1, text=[task["wait_text"]])
            r = m.call("fill_form", pageId=1, elements=chunk, includeSnapshot=False)
            if r.is_error:
                raise Fail("fill_form: " + r.text[:200])
        btn = find_ref(snap, submits[i], ("button",), DT_REF)
        if not btn:
            raise Fail(f"no button {submits[i]!r}")
        r = m.call("click", pageId=1, uid=btn, includeSnapshot=False)
        if r.is_error:
            raise Fail("click: " + r.text[:200])


# ---------- Ostler ----------

def _values(step):
    return {label: val for label, kind, val in step}


def ostler(m, url, task, inspect=False):
    if inspect:
        m.call("navigate", url=url, observe="none")
        m.call("form_inspect")
    else:
        m.call("navigate", url=url, observe="interactive")
    for step in task["steps"]:
        r = m.call("form_fill", values=_values(step))
        if r.is_error or "FAILED" in r.text or "UNMATCHED" in r.text:
            raise Fail("form_fill: " + r.text[:200])
        r = m.call("form_submit")
        if r.is_error or r.text.startswith(("NOT SUBMITTED", "NOT ACCEPTED")):
            raise Fail("form_submit: " + r.text[:200])


def ostler_inspect(m, url, task):
    ostler(m, url, task, inspect=True)
