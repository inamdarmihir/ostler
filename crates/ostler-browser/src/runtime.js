// Ostler in-page runtime. Injected into every document before page scripts run.
// Everything the Rust side needs from inside the page lives behind window.__wm.
(() => {
  if (window.__wm) return;

  // ---------------------------------------------------------------- activity tracking
  // We count in-flight fetch/XHR and short timers so "settle" can wait for real
  // work to finish instead of sleeping for a fixed time.
  const W = { fetches: 0, lastActivity: performance.now() };
  const touch = () => { W.lastActivity = performance.now(); };
  const activeTimers = new Set();

  const origFetch = window.fetch;
  if (origFetch) {
    window.fetch = function (...args) {
      W.fetches++;
      const done = () => { W.fetches--; touch(); };
      let p;
      try { p = origFetch.apply(this, args); } catch (e) { done(); throw e; }
      p.then(done, done);
      return p;
    };
  }
  const xhrSend = XMLHttpRequest.prototype.send;
  XMLHttpRequest.prototype.send = function (...args) {
    W.fetches++;
    this.addEventListener('loadend', () => { W.fetches--; touch(); }, { once: true });
    return xhrSend.apply(this, args);
  };
  const origST = window.setTimeout.bind(window);
  const origCT = window.clearTimeout.bind(window);
  window.setTimeout = function (fn, delay, ...rest) {
    const d = Number(delay) || 0;
    if (typeof fn !== 'function' || d > 1500) return origST(fn, delay, ...rest);
    let id;
    id = origST(function () {
      activeTimers.delete(id);
      touch();
      return fn.apply(this, rest);
    }, d);
    activeTimers.add(id);
    return id;
  };
  window.clearTimeout = function (id) { activeTimers.delete(id); return origCT(id); };
  // Our own polling must not count as page activity, so it uses the unwrapped timer.
  const sleep = (ms) => new Promise((r) => origST(r, ms));
  const invalidSeen = new WeakSet();
  document.addEventListener('invalid', (e) => { invalidSeen.add(e.target); }, true);

  // ---------------------------------------------------------------- refs
  const refs = new Map();
  const ids = new WeakMap();
  const groups = new Map(); // group ref -> member elements (radio / checkbox groups)
  let counter = 0;
  function refOf(el) {
    let r = ids.get(el);
    if (!r) { r = 'e' + (++counter); ids.set(el, r); refs.set(r, el); }
    return r;
  }
  function elOf(ref) {
    const el = refs.get(String(ref).replace(/^@/, ''));
    if (!el) throw new Error(`unknown ref ${ref}; take a fresh snapshot`);
    if (!el.isConnected) throw new Error(`ref ${ref} is stale (element left the page); take a fresh snapshot`);
    return el;
  }

  // ---------------------------------------------------------------- DOM helpers
  const clean = (s) => (s || '').replace(/\s+/g, ' ').trim();
  const SKIP = new Set(['SCRIPT', 'STYLE', 'NOSCRIPT', 'TEMPLATE', 'HEAD', 'META', 'LINK', 'TITLE', 'OPTION', 'OPTGROUP', 'DATALIST']);

  function kids(node) {
    if (node.nodeType === 1) {
      if (node.shadowRoot) return [...node.shadowRoot.childNodes];
      if (node.tagName === 'SLOT') {
        const a = node.assignedNodes({ flatten: true });
        return a.length ? a : [...node.childNodes];
      }
      if (node.tagName === 'IFRAME' || node.tagName === 'FRAME') {
        try { const d = node.contentDocument; return d && d.documentElement ? [d.documentElement] : []; } catch { return []; }
      }
    }
    return [...node.childNodes];
  }
  function deepQueryAll(root, selector, out = []) {
    const visit = (n) => {
      if (n.nodeType === 1) {
        if (n.matches(selector)) out.push(n);
      }
      for (const k of kids(n)) visit(k);
    };
    visit(root);
    return out;
  }
  function styleOf(el) { return el.ownerDocument.defaultView.getComputedStyle(el); }
  function hiddenSelf(el) {
    if (el.getAttribute('aria-hidden') === 'true') return true;
    const cs = styleOf(el);
    if (cs.display === 'contents') return false;
    return cs.display === 'none' || cs.visibility === 'hidden' || cs.visibility === 'collapse';
  }
  function rendered(el) {
    try {
      if (styleOf(el).display === 'contents') return true;
      if (el.checkVisibility) return el.checkVisibility({ checkVisibilityCSS: true, contentVisibilityAuto: true });
    } catch { /* fall through */ }
    return !hiddenSelf(el);
  }
  function rootOf(el) { return el.getRootNode(); }
  function byId(el, id) {
    const r = rootOf(el);
    return (r.getElementById ? r.getElementById(id) : null) || el.ownerDocument.getElementById(id);
  }
  function textOfIds(el, list) {
    return clean(list.split(/\s+/).map((id) => { const t = byId(el, id); return t ? ownText(t) : ''; }).join(' '));
  }
  // Text of an element without the text of embedded form controls.
  function ownText(el) {
    const c = el.cloneNode(true);
    if (c.querySelectorAll) c.querySelectorAll('select,textarea,input,button,option,script,style').forEach((n) => n.remove());
    return clean(c.textContent);
  }
  function stripLabel(raw) {
    let t = clean(raw);
    let required = false;
    if (/\*\s*$/.test(t) || /\(required\)\s*$/i.test(t)) required = true;
    t = t.replace(/\s*\*+\s*$/, '').replace(/\s*\(required\)\s*$/i, '').replace(/\s*:\s*$/, '');
    return { text: clean(t), required };
  }
  function nearbyText(el) {
    let node = el;
    for (let depth = 0; depth < 3 && node; depth++, node = node.parentElement) {
      let s = node.previousSibling;
      for (let hops = 0; s && hops < 3; hops++, s = s.previousSibling) {
        if (s.nodeType === 3) { const t = clean(s.nodeValue); if (t) return t.slice(0, 80); continue; }
        if (s.nodeType !== 1 || SKIP.has(s.tagName)) continue;
        if (s.querySelector && s.querySelector('input,select,textarea,button')) break;
        if (!rendered(s)) continue;
        const t = ownText(s);
        if (t && t.length <= 80) return t;
      }
      // Text directly inside the parent before the control (e.g. "Email <input>")
      const p = node.parentElement;
      if (p && p.childNodes.length <= 6) {
        for (const c of p.childNodes) {
          if (c === node) break;
          if (c.nodeType === 3) { const t = clean(c.nodeValue); if (t) return t.slice(0, 80); }
        }
      }
    }
    return '';
  }

  // ---------------------------------------------------------------- roles / names
  const HEADING = /^H[1-6]$/;
  const LANDMARK = { MAIN: 'main', NAV: 'navigation', HEADER: 'banner', FOOTER: 'contentinfo', ASIDE: 'complementary', DIALOG: 'dialog' };
  // Mirrors Field::is_sensitive in ostler-forms: a typed secret must not come back in a snapshot.
  const SECRET_RE = /password|passcode|card.?number|cvv|cvc|security code|\bssn\b|social security|pin code|secret|iban|routing number|one-time|\botp\b/i;
  function looksSecret(el, label) {
    const ac = (el.getAttribute('autocomplete') || '').toLowerCase();
    if (ac.includes('password') || ac.includes('cc-') || ac.includes('one-time-code')) return true;
    return SECRET_RE.test([label || '', el.name || '', el.id || ''].join(' '));
  }

  function roleOf(el) {
    const ar = el.getAttribute('role');
    if (ar) return ar.split(/\s+/)[0];
    const t = el.tagName;
    if (HEADING.test(t)) return 'heading';
    if (LANDMARK[t]) return LANDMARK[t];
    switch (t) {
      case 'A': return el.hasAttribute('href') ? 'link' : null;
      case 'BUTTON': case 'SUMMARY': return 'button';
      case 'SELECT': return el.multiple ? 'listbox' : 'combobox';
      case 'TEXTAREA': return 'textbox';
      case 'IMG': return 'img';
      case 'FORM': return 'form';
      case 'TABLE': return 'table';
      case 'TR': return 'row';
      case 'IFRAME': case 'FRAME': return 'iframe';
      case 'INPUT': {
        const ty = (el.type || 'text').toLowerCase();
        if (['button', 'submit', 'reset', 'image'].includes(ty)) return 'button';
        if (ty === 'checkbox') return 'checkbox';
        if (ty === 'radio') return 'radio';
        if (ty === 'range') return 'slider';
        if (ty === 'hidden') return null;
        if (ty === 'search') return 'searchbox';
        if (ty === 'number') return 'spinbutton';
        if (ty === 'file') return 'file';
        return el.getAttribute('list') ? 'combobox' : 'textbox';
      }
      default:
        if (el.isContentEditable && el.getAttribute('contenteditable') !== null) return 'textbox';
        if (el.hasAttribute('onclick') || (el.hasAttribute('tabindex') && el.getAttribute('tabindex') !== '-1')) return 'clickable';
        return null;
    }
  }
  const INTERACTIVE = new Set(['link', 'button', 'textbox', 'searchbox', 'spinbutton', 'checkbox', 'radio', 'combobox', 'listbox', 'slider', 'switch', 'tab', 'menuitem', 'option', 'file', 'clickable', 'form', 'iframe', 'treeitem', 'menuitemcheckbox', 'menuitemradio']);

  function labelFor(el) {
    const lb = el.getAttribute('aria-labelledby');
    if (lb) { const t = textOfIds(el, lb); if (t) return t; }
    const al = el.getAttribute('aria-label');
    if (al && clean(al)) return clean(al);
    if (el.labels && el.labels.length) {
      const t = clean([...el.labels].map(ownText).join(' '));
      if (t) return t;
    }
    const near = nearbyText(el);
    if (near) return near;
    return clean(el.placeholder || el.title || el.getAttribute('name') || '');
  }
  function nameOf(el) {
    const role = roleOf(el);
    const lb = el.getAttribute('aria-labelledby');
    if (lb) { const t = textOfIds(el, lb); if (t) return t; }
    const al = el.getAttribute('aria-label');
    if (al && clean(al)) return clean(al);
    if (['textbox', 'searchbox', 'spinbutton', 'checkbox', 'radio', 'combobox', 'listbox', 'slider', 'switch', 'file'].includes(role) && /^(INPUT|SELECT|TEXTAREA)$/.test(el.tagName)) {
      return stripLabel(labelFor(el)).text;
    }
    if (el.tagName === 'INPUT') return clean(el.value || el.title || el.alt || '');
    if (el.tagName === 'IMG') return clean(el.alt || el.title);
    const t = clean(el.innerText || el.textContent);
    return (t || clean(el.title)).slice(0, 100);
  }

  // ---------------------------------------------------------------- snapshot
  function snapshot(opts = {}) {
    const lines = [];
    const budget = opts.maxChars || 20000;
    const interactiveOnly = !!opts.interactiveOnly;
    let used = 0, truncated = false;
    const emit = (depth, text) => {
      if (truncated) return;
      const line = '  '.repeat(depth) + text;
      used += line.length + 1;
      if (used > budget) { truncated = true; return; }
      lines.push(line);
    };
    const q = (s) => JSON.stringify(s);
    function ctl(el, role, depth) {
      const r = refOf(el);
      const name = nameOf(el);
      let s = `${role}${name ? ' ' + q(name) : ''} [${r}]`;
      if (role === 'heading') s = `heading ${q(name)}`;
      const attrs = [];
      if (el.disabled || el.getAttribute('aria-disabled') === 'true') attrs.push('disabled');
      if (el.required || el.getAttribute('aria-required') === 'true') attrs.push('required');
      if (el.getAttribute('aria-invalid') === 'true') attrs.push('invalid');
      if (el.readOnly) attrs.push('readonly');
      if (role === 'checkbox' || role === 'radio' || role === 'switch') {
        const on = el.checked !== undefined ? el.checked : el.getAttribute('aria-checked') === 'true';
        if (on) attrs.push('checked');
      } else if (el.tagName === 'SELECT') {
        const sel = [...el.selectedOptions].map((o) => clean(o.text));
        attrs.push(`value=${q(el.multiple ? sel : (sel[0] || ''))}`);
        const labels = [...el.options].map((o) => clean(o.text));
        attrs.push(labels.length <= 10 ? `options=${q(labels)}` : `options=${labels.length}`);
      } else if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') {
        const ty = (el.type || '').toLowerCase();
        if (!['button', 'submit', 'reset', 'image', 'file', 'checkbox', 'radio'].includes(ty)) {
          const v = (ty === 'password' || looksSecret(el, name)) ? (el.value ? '•••' : '') : el.value;
          if (v) attrs.push(`value=${q(v.length > 80 ? v.slice(0, 80) + '…' : v)}`);
          if (el.placeholder) attrs.push(`placeholder=${q(el.placeholder)}`);
          if (ty && !['text', 'textarea'].includes(ty)) attrs.push(`type=${ty}`);
        }
      } else if (role === 'link') {
        const href = el.getAttribute('href') || '';
        if (href) attrs.push(`href=${q(href.length > 100 ? href.slice(0, 100) + '…' : href)}`);
      } else if (role === 'heading') {
        attrs.push(`level=${el.tagName[1] || el.getAttribute('aria-level') || '?'}`);
      }
      emit(depth, s + (attrs.length ? ' ' + attrs.join(' ') : ''));
    }
    function walk(node, depth, noText) {
      if (truncated) return;
      if (node.nodeType === 3) {
        if (!interactiveOnly && !noText) { const t = clean(node.nodeValue); if (t) emit(depth, `text ${q(t.slice(0, 300))}`); }
        return;
      }
      if (node.nodeType !== 1) return;
      const el = node;
      if (SKIP.has(el.tagName) || hiddenSelf(el)) return;
      // A label that owns a control: keep the control (it may be wrapped), drop the duplicate text.
      if (el.tagName === 'LABEL' && el.control) noText = true;
      const role = roleOf(el);
      const isFrame = el.tagName === 'IFRAME' || el.tagName === 'FRAME';
      if (isFrame) {
        let accessible = true;
        try { accessible = !!el.contentDocument; } catch { accessible = false; }
        const r = refOf(el);
        emit(depth, accessible ? `iframe [${r}]:` : `iframe [${r}] (cross-origin: content not accessible)`);
        if (accessible) for (const k of kids(el)) walk(k, depth + 1, noText);
        return;
      }
      if (role === 'heading') { if (!interactiveOnly || true) ctl(el, role, depth); return; }
      if (role === 'img') { if (!interactiveOnly && clean(el.alt)) emit(depth, `img ${q(clean(el.alt))}`); return; }
      if (role && INTERACTIVE.has(role) && role !== 'form' && role !== 'clickable') {
        // Composite widgets (e.g. role=combobox wrapping an input) still recurse for their popup/options.
        ctl(el, role, depth);
        if (!/^(INPUT|SELECT|TEXTAREA|BUTTON)$/.test(el.tagName) && el.children.length && role !== 'link' && role !== 'button') {
          for (const k of kids(el)) walk(k, depth + 1, noText);
        }
        return;
      }
      if (role === 'clickable') { ctl(el, role, depth); return; }
      let d = depth;
      if (role === 'form') { const r = refOf(el); const n = clean(el.getAttribute('aria-label') || '') || (el.getAttribute('aria-labelledby') && textOfIds(el, el.getAttribute('aria-labelledby'))) || el.getAttribute('name') || ''; emit(depth, `form${n ? ' ' + q(n) : ''} [${r}]:`); d++; }
      else if (LANDMARK[el.tagName] || (role && ['main', 'navigation', 'dialog', 'alert', 'alertdialog', 'status', 'banner', 'region', 'listbox', 'menu', 'tablist'].includes(role))) {
        const n = clean(el.getAttribute('aria-label') || '');
        emit(depth, `${role || LANDMARK[el.tagName]}${n ? ' ' + q(n) : ''}:`); d++;
      } else if (el.tagName === 'FIELDSET') {
        const lg = el.querySelector(':scope > legend');
        emit(depth, `group${lg ? ' ' + q(clean(lg.textContent)) : ''}:`); d++;
      } else if (role === 'table') { emit(depth, 'table:'); d++; }
      else if (!role && !el.shadowRoot && el.tagName !== 'SLOT' && el.tagName !== 'LEGEND') {
        // Plain container with nothing interesting inside: collapse to one text line.
        if (!el.querySelector('a[href],button,input,select,textarea,[role],[contenteditable],[tabindex],[onclick],summary,h1,h2,h3,h4,h5,h6,img,form,iframe,table,dialog,fieldset')) {
          if (!interactiveOnly && !noText) { const t = clean(el.innerText || el.textContent); if (t) emit(depth, `text ${q(t.slice(0, 300))}`); }
          return;
        }
      }
      if (el.tagName === 'LEGEND') return;
      for (const k of kids(el)) walk(k, d, noText);
    }
    const root = opts.ref ? elOf(opts.ref) : document.documentElement;
    walk(root, 0, false);
    return { url: location.href, title: document.title, lines, truncated, refs: counter };
  }

  // ---------------------------------------------------------------- forms
  const CONTROL_SEL = 'input,select,textarea,[contenteditable=""],[contenteditable="true"],[role=combobox],[role=switch],[role=checkbox],[role=radio],[role=textbox]';
  const SKIP_INPUT = new Set(['hidden', 'submit', 'button', 'reset', 'image']);

  function kindOf(el) {
    const t = el.tagName;
    const role = el.getAttribute('role');
    if (t === 'SELECT') return el.multiple ? 'multi_select' : 'select';
    if (t === 'TEXTAREA') return 'textarea';
    if (t === 'INPUT') {
      const ty = (el.type || 'text').toLowerCase();
      if (role === 'combobox' || (el.getAttribute('aria-autocomplete') && el.getAttribute('aria-controls'))) return 'combobox';
      const map = { text: 'text', email: 'email', tel: 'tel', url: 'url', search: 'search', password: 'password', number: 'number', date: 'date', time: 'time', 'datetime-local': 'datetime_local', month: 'month', week: 'week', range: 'range', color: 'color', file: 'file', checkbox: 'checkbox', radio: 'radio' };
      return map[ty] || 'text';
    }
    if (role === 'switch') return 'switch';
    if (role === 'checkbox') return 'checkbox';
    if (role === 'radio') return 'radio';
    if (role === 'combobox') return 'combobox';
    return 'contenteditable';
  }
  function eligible(el) {
    if (el.tagName === 'INPUT' && SKIP_INPUT.has((el.type || '').toLowerCase())) return false;
    if (el.closest && el.closest('[role=listbox],[role=option],[role=menu]') && el.getAttribute('role') !== 'combobox') return false;
    return true;
  }
  function visibleControl(el) {
    if (rendered(el)) return true;
    // Visually hidden native checkbox/radio/file: the associated label is what users see.
    if (el.tagName === 'INPUT' && ['checkbox', 'radio', 'file'].includes((el.type || '').toLowerCase())) {
      const hidden = styleOf(el).display === 'none' || styleOf(el).visibility === 'hidden';
      if (hidden) return false;
      return !!(el.labels && [...el.labels].some(rendered));
    }
    return false;
  }
  // Visual-only error cues: pages that signal failure with colour alone (no ARIA, no error class).
  function parseRgb(c) {
    const m = /rgba?\(\s*(\d+)[,\s]+(\d+)[,\s]+(\d+)(?:[,\s/]+([\d.]+))?/.exec(c || '');
    return m ? { r: +m[1], g: +m[2], b: +m[3], a: m[4] === undefined ? 1 : +m[4] } : null;
  }
  function isRed(c) {
    const k = parseRgb(c);
    return !!k && k.a > 0.3 && k.r >= 150 && k.r - k.g >= 70 && k.r - k.b >= 60 && Math.abs(k.g - k.b) < 90;
  }
  function redBorder(el) {
    const cs = styleOf(el);
    for (const side of ['Top', 'Right', 'Bottom', 'Left']) {
      if (parseFloat(cs['border' + side + 'Width']) > 0 && cs['border' + side + 'Style'] !== 'none' && isRed(cs['border' + side + 'Color'])) return 'red border';
    }
    if (parseFloat(cs.outlineWidth) > 0 && cs.outlineStyle !== 'none' && isRed(cs.outlineColor)) return 'red outline';
    if (cs.boxShadow && cs.boxShadow !== 'none' && (cs.boxShadow.match(/rgba?\([^)]*\)/g) || []).some(isRed)) return 'red glow';
    return null;
  }
  function redTextNear(el) {
    // Only text that follows this control with no other control in between, so a message is never
    // attributed to a neighbouring field.
    const own = new Set([el, ...(el.labels || [])]);
    const scope = el.parentElement && el.parentElement.parentElement ? el.parentElement.parentElement : el.parentElement;
    if (!scope) return null;
    const nodes = [...scope.querySelectorAll('*')];
    const idx = nodes.indexOf(el);
    for (let i = idx + 1; i < nodes.length; i++) {
      const cand = nodes[i];
      if (cand.matches('input,select,textarea,button') || cand.querySelector('input,select,textarea,button')) {
        if (cand.contains(el)) continue;
        break;
      }
      if (own.has(cand) || cand.tagName === 'LABEL' || !rendered(cand)) continue;
      const t = ownText(cand);
      if (!t || t.length > 200 || /^[*\s]*$/.test(t)) continue;
      if (isRed(styleOf(cand).color)) return t;
    }
    return null;
  }
  function visualCue(el) {
    if (!rendered(el)) return null;
    const border = redBorder(el);
    const text = redTextNear(el);
    if (!border && !text) return null;
    return { cue: border || 'red text', text };
  }
  const ALERT_WORDS = /\b(error|errors|invalid|failed|failure|required|could not|couldn't|cannot|can't|unable|incorrect|denied|not valid|try again|please (?:enter|fix|correct|select|provide|check))\b/i;
  function toasts() {
    const out = [];
    for (const n of document.querySelectorAll('body *')) {
      if (n.closest('form') && !/toast|snack|flash|banner|notif/i.test(n.className || '')) continue;
      const cs = styleOf(n);
      const floating = cs.position === 'fixed' || cs.position === 'sticky';
      const live = ['alert', 'status'].includes(n.getAttribute('role')) || n.getAttribute('aria-live');
      const named = /toast|snack|flash|banner|notif|alert/i.test(String(n.className || '') + ' ' + (n.id || ''));
      if (!(floating || live || named) || !rendered(n)) continue;
      if (n.querySelector('input,select,textarea') || n.children.length > 6) continue;
      const t = ownText(n);
      if (!t || t.length > 300) continue;
      const bg = parseRgb(cs.backgroundColor);
      const redBg = isRed(cs.backgroundColor) || (bg && bg.a > 0.3 && bg.r >= 200 && bg.g <= 120 && bg.b <= 120);
      if (isRed(cs.color) || redBg || ALERT_WORDS.test(t)) { if (!out.includes(t)) out.push(t); }
      if (out.length >= 3) break;
    }
    return out;
  }
  function errorText(el) {
    const grab = (id) => { const t = byId(el, id); return t && rendered(t) ? ownText(t) : ''; };
    const em = el.getAttribute('aria-errormessage');
    if (em) { const t = grab(em); if (t) return t; }
    const invalid = el.getAttribute('aria-invalid') === 'true';
    const db = el.getAttribute('aria-describedby');
    if (invalid && db) { const t = db.split(/\s+/).map(grab).filter(Boolean).join(' '); if (t) return t; }
    if (el.validity && el.validity.customError && el.validationMessage) return el.validationMessage;
    if (el.validity && !el.validity.valid && invalidSeen.has(el)) return el.validationMessage;
    // Nearby visible error element (common framework patterns).
    const scopes = [el.parentElement, el.parentElement && el.parentElement.parentElement].filter(Boolean);
    for (const s of scopes) {
      for (const cand of s.querySelectorAll('[role=alert],.error,.invalid-feedback,.field-error,.error-message,.help-block,[class*="error-"],[class*="-error"],[data-error]')) {
        if (cand.contains(el) || cand.querySelector('input,select,textarea') || !rendered(cand)) continue;
        const t = ownText(cand);
        if (t && t.length < 200) return t;
      }
    }
    if (invalid) return el.validationMessage || 'invalid';
    const v = visualCue(el);
    if (v) return v.text || ('marked invalid visually (' + v.cue + ')');
    return null;
  }
  function optionsOf(el) {
    return [...el.options].map((o) => ({ value: o.value, label: clean(o.text), selected: o.selected, disabled: o.disabled || (o.parentElement && o.parentElement.disabled) || false }));
  }
  function fileNames(el) { return el.files ? [...el.files].map((f) => f.name) : []; }
  function baseField(el) {
    const kind = kindOf(el);
    const lab = stripLabel(labelFor(el));
    const f = {
      ref: refOf(el), kind, label: lab.text,
      name: el.getAttribute('name') || null, id: el.id || null,
      required: !!(el.required || el.getAttribute('aria-required') === 'true' || lab.required),
      disabled: !!(el.disabled || el.getAttribute('aria-disabled') === 'true'),
      readonly: !!el.readOnly,
      placeholder: el.getAttribute('placeholder') || null,
      autocomplete: (() => { const a = el.getAttribute('autocomplete'); return a && a !== 'off' && a !== 'on' ? a : null; })(),
      pattern: el.getAttribute('pattern') || null,
      min: el.getAttribute('min'), max: el.getAttribute('max'), step: el.getAttribute('step'),
      minlength: el.minLength > 0 ? el.minLength : null, maxlength: el.maxLength > 0 ? el.maxLength : null,
      options: [], value: '', section: null, error: errorText(el), hidden: false,
    };
    const fs = el.closest && el.closest('fieldset');
    if (fs) { const lg = fs.querySelector(':scope > legend'); if (lg) f.section = clean(lg.textContent); }
    return f;
  }
  function readValue(el, kind) {
    switch (kind) {
      case 'select': return el.selectedOptions[0] ? el.selectedOptions[0].value : '';
      case 'multi_select': return [...el.selectedOptions].map((o) => o.value);
      case 'checkbox': return el.checked !== undefined && el.type === 'checkbox' ? el.checked : el.getAttribute('aria-checked') === 'true';
      case 'switch': return el.checked !== undefined && el.type === 'checkbox' ? el.checked : el.getAttribute('aria-checked') === 'true';
      case 'file': return fileNames(el);
      case 'contenteditable': return clean(el.innerText);
      default: return el.value;
    }
  }
  function groupLabel(members, fallback) {
    const first = members[0];
    const fs = first.closest('fieldset');
    if (fs) { const lg = fs.querySelector(':scope > legend'); if (lg && clean(lg.textContent)) return stripLabel(lg.textContent); }
    const grp = first.closest('[role=radiogroup],[role=group]');
    if (grp) {
      const l = grp.getAttribute('aria-label') || (grp.getAttribute('aria-labelledby') && textOfIds(grp, grp.getAttribute('aria-labelledby')));
      if (l) return stripLabel(l);
      const near = nearbyText(grp);
      if (near) return stripLabel(near);
    }
    const near = nearbyText(members[0].closest('label') || members[0].parentElement || members[0]);
    return stripLabel(near || fallback || '');
  }
  const isOn = (m) => (m.tagName === 'INPUT' ? m.checked : m.getAttribute('aria-checked') === 'true');
  const valOf = (m) => (m.tagName === 'INPUT' ? m.value : (m.getAttribute('data-value') || m.getAttribute('value') || clean(m.textContent)));
  const labOf = (m) => (m.tagName === 'INPUT' ? stripLabel(labelFor(m)).text : clean(m.getAttribute('aria-label') || m.textContent));
  function buildGroup(members, type) {
    const f = baseField(members[0]);
    const lab = groupLabel(members, members[0].getAttribute('name'));
    f.label = lab.text;
    f.required = members.some((m) => m.required) || lab.required;
    f.kind = type === 'radio' ? 'radio' : 'multi_select';
    f.options = members.map((m) => ({ value: valOf(m), label: labOf(m) || valOf(m), selected: isOn(m), disabled: !!m.disabled || m.getAttribute('aria-disabled') === 'true' }));
    f.value = type === 'radio' ? ((members.find(isOn) && valOf(members.find(isOn))) || '') : members.filter(isOn).map(valOf);
    f.disabled = f.options.every((o) => o.disabled);
    f.error = members.map(errorText).find(Boolean) || null;
    groups.set(f.ref, members);
    return f;
  }
  function buildField(el) {
    const f = baseField(el);
    f.value = readValue(el, f.kind);
    if (f.kind === 'select' || f.kind === 'multi_select') f.options = optionsOf(el);
    if (f.kind === 'combobox') {
      const lid = el.getAttribute('aria-controls') || el.getAttribute('aria-owns');
      const lb = lid && byId(el, lid);
      if (lb && rendered(lb)) f.options = [...lb.querySelectorAll('[role=option]')].map((o) => ({ value: o.getAttribute('data-value') || clean(o.textContent), label: clean(o.textContent), selected: o.getAttribute('aria-selected') === 'true', disabled: o.getAttribute('aria-disabled') === 'true' }));
      if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') f.value = el.value;
      else f.value = clean(el.innerText);
    }
    return f;
  }

  const SUBMIT_RE = /^(submit|send|save|continue|next|finish|finish up|review|apply|register|sign ?up|sign ?in|log ?in|create|pay|confirm|book|place order|complete|done|proceed|get started|subscribe|checkout|search|go)\b/i;
  const NEXT_RE = /\b(next|continue|proceed|review|step)\b/i;
  const NOT_SUBMIT_RE = /\b(back|previous|prev|cancel|reset|clear|close|skip)\b/i;
  function submitControls(scopeEl, root) {
    const out = [];
    const cands = deepQueryAll(root || scopeEl, 'button,input[type=submit],input[type=button],input[type=image],[role=button]');
    for (const b of cands) {
      if (!rendered(b)) continue;
      const label = nameOf(b);
      const type = (b.getAttribute('type') || (b.tagName === 'BUTTON' ? 'submit' : '')).toLowerCase();
      if (NOT_SUBMIT_RE.test(label) && type !== 'submit') continue;
      if (b.tagName === 'BUTTON' && type === 'reset') continue;
      const isSubmitType = type === 'submit' || type === 'image';
      if (!(isSubmitType || SUBMIT_RE.test(label))) continue;
      if (NOT_SUBMIT_RE.test(label)) continue;
      out.push({ ref: refOf(b), label, role: NEXT_RE.test(label) && !/submit|send|pay|place order|confirm/i.test(label) ? 'next' : 'submit' });
    }
    return out;
  }
  function stepInfo(container) {
    const txt = (container.innerText || '').slice(0, 1500);
    const m = txt.match(/step\s+(\d+)\s*(?:of|\/)\s*(\d+)/i);
    if (m) return { index: +m[1], total: +m[2] };
    const pb = container.querySelector('[role=progressbar][aria-valuenow][aria-valuemax]');
    if (pb) return { index: +pb.getAttribute('aria-valuenow'), total: +pb.getAttribute('aria-valuemax') };
    return null;
  }
  function pageErrors(container) {
    const out = [];
    for (const n of container.querySelectorAll('[role=alert],.alert-danger,.error-summary,.form-error,.alert-error')) {
      if (!rendered(n) || n.querySelector('input,select,textarea')) continue;
      const t = ownText(n);
      if (t && t.length < 300 && !out.includes(t)) out.push(t);
      if (out.length >= 3) break;
    }
    return out;
  }
  function headingsIn(container) {
    const hs = [...container.querySelectorAll('h1,h2,h3,legend')].filter(rendered).map((h) => clean(h.textContent)).filter(Boolean);
    return hs.slice(0, 3);
  }

  function forms() {
    groups.clear();
    const controls = deepQueryAll(document.documentElement, CONTROL_SEL).filter(eligible);
    const containers = new Map(); // key (form element or root node) -> {el, controls[]}
    for (const el of controls) {
      const form = el.form || (el.closest && el.closest('form'));
      const key = form || rootOf(el);
      if (!containers.has(key)) containers.set(key, { form: form || null, root: rootOf(el), controls: [] });
      containers.get(key).controls.push(el);
    }
    const result = [];
    for (const c of containers.values()) {
      const seenGroup = new Set();
      const fields = [];
      let hiddenCount = 0;
      for (const el of c.controls) {
        if (!visibleControl(el)) { hiddenCount++; continue; }
        const ty = el.tagName === 'INPUT' ? (el.type || '').toLowerCase() : '';
        if ((ty === 'radio' || ty === 'checkbox') && el.name) {
          const key = ty + '|' + el.name;
          if (seenGroup.has(key)) continue;
          const members = c.controls.filter((m) => m.tagName === 'INPUT' && (m.type || '').toLowerCase() === ty && m.name === el.name && visibleControl(m));
          if (ty === 'radio' || members.length > 1) {
            seenGroup.add(key);
            fields.push(buildGroup(members, ty));
            continue;
          }
        }
        if (el.tagName !== 'INPUT' && el.getAttribute('role') === 'radio') {
          const grp = el.closest('[role=radiogroup]');
          const key = 'aria-radio|' + (grp ? refOf(grp) : 'none');
          if (seenGroup.has(key)) continue;
          seenGroup.add(key);
          const members = c.controls.filter((m) => m.tagName !== 'INPUT' && m.getAttribute('role') === 'radio' && m.closest('[role=radiogroup]') === grp && visibleControl(m));
          fields.push(buildGroup(members, 'radio'));
          continue;
        }
        fields.push(buildField(el));
      }
      if (!fields.length) continue;
      const scope = c.form || (c.root.documentElement || c.root.host || document.body);
      const container = c.form || (c.root.nodeType === 9 ? c.root.body : c.root);
      const formEl = c.form;
      const submitRoot = formEl || container;
      result.push({
        ref: formEl ? refOf(formEl) : 'page',
        name: formEl ? (formEl.getAttribute('name') || formEl.getAttribute('aria-label') || formEl.id || null) : null,
        action: formEl ? (formEl.getAttribute('action') || null) : null,
        method: formEl ? (formEl.getAttribute('method') || 'get').toLowerCase() : null,
        headings: headingsIn(container && container.querySelectorAll ? container : document.body),
        fields, hidden_fields: hiddenCount,
        submit: submitControls(scope, submitRoot),
        step: stepInfo(container && container.querySelectorAll ? container : document.body),
        errors: pageErrors(container && container.querySelectorAll ? container : document.body).concat(toasts()).filter((t, i, a) => a.indexOf(t) === i).slice(0, 4),
      });
    }
    return { url: location.href, title: document.title, forms: result };
  }
  function field(ref) {
    const el = elOf(ref);
    const members = groups.get(ref);
    if (members) {
      const ty = members[0].tagName === 'INPUT' ? (members[0].type || '').toLowerCase() : 'radio';
      return buildGroup(members, ty);
    }
    return buildField(el);
  }

  // ---------------------------------------------------------------- acting on fields
  function nativeSet(el, value) {
    const proto = el.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : el.tagName === 'SELECT' ? HTMLSelectElement.prototype : HTMLInputElement.prototype;
    const desc = Object.getOwnPropertyDescriptor(proto, 'value');
    if (desc && desc.set) desc.set.call(el, value); else el.value = value;
  }
  function fire(el, types) {
    for (const t of types) el.dispatchEvent(new Event(t, { bubbles: true, cancelable: false }));
  }
  function scrollInto(el) { try { el.scrollIntoView({ block: 'center', inline: 'center' }); } catch { /* ignore */ } }

  function prepare(ref) {
    const el = elOf(ref);
    scrollInto(el);
    el.focus({ preventScroll: true });
    if (el.isContentEditable && !('value' in el)) {
      const r = el.ownerDocument.createRange();
      r.selectNodeContents(el);
      const s = el.ownerDocument.defaultView.getSelection();
      s.removeAllRanges(); s.addRange(r);
    } else if (typeof el.select === 'function') {
      try { el.select(); } catch { /* some input types cannot select() */ }
    }
    const focused = (el.getRootNode().activeElement === el) || el.ownerDocument.activeElement === el;
    return { focused, readonly: !!el.readOnly, disabled: !!el.disabled };
  }
  function finish(ref) {
    const el = elOf(ref);
    if (typeof el.blur === 'function') el.blur();
    return true;
  }
  function setNative(ref, value) {
    const el = elOf(ref);
    scrollInto(el);
    if (el.isContentEditable && !('value' in el)) { el.textContent = value; fire(el, ['input']); fire(el, ['change']); return true; }
    nativeSet(el, value);
    fire(el, ['input', 'change']);
    return true;
  }
  function clickEl(el) {
    for (const t of ['pointerdown', 'mousedown', 'pointerup', 'mouseup']) el.dispatchEvent(new MouseEvent(t, { bubbles: true, cancelable: true, view: el.ownerDocument.defaultView }));
    el.click();
  }
  // Apply a non-typed value. `indices` are option indexes resolved on the Rust side.
  function apply(ref, kind, payload) {
    const el = elOf(ref);
    scrollInto(el);
    if (kind === 'select' || kind === 'multi_select') {
      const members = groups.get(ref);
      if (members) { // checkbox group
        const want = new Set(payload);
        members.forEach((m, i) => { if (m.checked !== want.has(i)) clickEl(m); });
        return true;
      }
      const want = new Set(payload);
      [...el.options].forEach((o, i) => { o.selected = want.has(i); });
      fire(el, ['input', 'change']);
      return true;
    }
    if (kind === 'radio') {
      const members = groups.get(ref) || [el];
      const m = members[payload];
      if (!m) throw new Error('radio option index out of range');
      if (!isOn(m)) clickEl(m);
      return true;
    }
    if (kind === 'checkbox' || kind === 'switch') {
      const cur = el.type === 'checkbox' ? el.checked : el.getAttribute('aria-checked') === 'true';
      if (cur !== payload) clickEl(el);
      return true;
    }
    throw new Error('apply: unsupported kind ' + kind);
  }

  // Calendar widgets on read-only inputs: open the popup, navigate to the month, click the day.
  async function pickDate(ref, iso) {
    const [Y, Mo, D] = iso.split('-').map(Number);
    const MONTHS = ['january','february','march','april','may','june','july','august','september','october','november','december'];
    const el = elOf(ref);
    scrollInto(el);
    clickEl(el);
    const header = (root) => {
      const m = clean(root.textContent).toLowerCase().match(/(jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)[a-z]*\.?,?\s+(\d{4})/);
      return m ? { y: +m[2], m: MONTHS.findIndex((n) => n.startsWith(m[1])) } : null;
    };
    const POP = '[role=dialog],[role=grid],[class*=calendar i],[class*=datepicker i],[class*=date-picker i],[class*=picker i],[class*=flatpickr i]';
    const find = () => deepQueryAll(document.documentElement, POP).filter((p) => rendered(p) && header(p)).sort((a, b) => a.querySelectorAll('*').length - b.querySelectorAll('*').length)[0];
    let pop = null;
    for (let i = 0; i < 20 && !(pop = find()); i++) await sleep(50);
    if (!pop) return { ok: false, why: 'no calendar popup appeared' };
    const btns = () => [...pop.querySelectorAll('button,[role=button],a,span,div')].filter((b) => rendered(b) && b.children.length === 0 || b.tagName === 'BUTTON');
    for (let i = 0; i < 48; i++) {
      const h = header(pop);
      const diff = (Y * 12 + Mo - 1) - (h.y * 12 + h.m);
      if (diff === 0) break;
      const want = diff > 0 ? /next|forward|›|»|>|→/i : /prev|back|‹|«|<|←/i;
      const b = btns().find((x) => want.test((x.getAttribute('aria-label') || '') + ' ' + (x.getAttribute('title') || '')) || want.test(clean(x.textContent)) && clean(x.textContent).length <= 2);
      if (!b) return { ok: false, why: 'cannot find month navigation' };
      clickEl(b);
      await sleep(30);
      pop = find() || pop;
    }
    const mname = MONTHS[Mo - 1];
    const dayRe = new RegExp('\\b' + D + '\\b');
    const cells = [...pop.querySelectorAll('button,[role=gridcell],[role=button],td,a,span,div')].filter((c) => rendered(c) && !(c.children.length > 1));
    const bad = (c) => c.disabled || c.getAttribute('aria-disabled') === 'true' || /outside|disabled|other-month|prevMonth|nextMonth|muted/i.test(c.className || '');
    let cell = cells.find((c) => !bad(c) && (c.getAttribute('aria-label') || '').toLowerCase().includes(mname) && dayRe.test(c.getAttribute('aria-label')));
    if (!cell) cell = cells.find((c) => !bad(c) && clean(c.textContent) === String(D));
    if (!cell) return { ok: false, why: 'day ' + D + ' not selectable in that month' };
    clickEl(cell);
    await sleep(60);
    return { ok: true };
  }

  async function comboPick(ref, wanted, timeoutMs) {
    const el = elOf(ref);
    const norm = (s) => clean(s).toLowerCase().replace(/[^a-z0-9]+/g, ' ').trim();
    const w = norm(wanted);
    const lid = el.getAttribute('aria-controls') || el.getAttribute('aria-owns');
    const deadline = performance.now() + (timeoutMs || 1500);
    let options = [];
    while (performance.now() < deadline) {
      let lb = lid && byId(el, lid);
      let pool = lb && rendered(lb) ? [...lb.querySelectorAll('[role=option]')] : [];
      if (!pool.length) pool = deepQueryAll(document.documentElement, '[role=option]').filter((o) => rendered(o));
      options = pool.filter(rendered);
      if (options.length) break;
      await sleep(40);
    }
    if (!options.length) return { picked: null, available: [] };
    const labels = options.map((o) => clean(o.textContent));
    let idx = labels.findIndex((l) => norm(l) === w);
    if (idx < 0) { const hits = labels.map((l, i) => [l, i]).filter(([l]) => norm(l).includes(w) || w.includes(norm(l))); if (hits.length === 1) idx = hits[0][1]; }
    if (idx < 0) return { picked: null, available: labels.slice(0, 15) };
    clickEl(options[idx]);
    return { picked: labels[idx], available: [] };
  }

  // ---------------------------------------------------------------- pointer info
  function absoluteCenter(el) {
    const r = el.getBoundingClientRect();
    let x = r.left + r.width / 2, y = r.top + r.height / 2;
    let w = el.ownerDocument.defaultView;
    while (w && w !== w.top && w.frameElement) {
      const fr = w.frameElement.getBoundingClientRect();
      x += fr.left + w.frameElement.clientLeft; y += fr.top + w.frameElement.clientTop;
      w = w.parent;
    }
    return { x, y, w: r.width, h: r.height };
  }
  function deepElementFromPoint(doc, x, y) {
    let el = doc.elementFromPoint(x, y);
    while (el && el.shadowRoot) {
      const inner = el.shadowRoot.elementFromPoint(x, y);
      if (!inner || inner === el) break;
      el = inner;
    }
    return el;
  }
  function clickInfo(ref) {
    const el = elOf(ref);
    scrollInto(el);
    const c = absoluteCenter(el);
    const info = { x: c.x, y: c.y, zero: c.w < 1 || c.h < 1, covered: false, tag: el.tagName.toLowerCase(), disabled: !!el.disabled };
    if (info.zero) return info;
    const top = el.ownerDocument === document ? deepElementFromPoint(document, c.x, c.y) : null;
    if (top && top !== el && !el.contains(top) && !(top.contains && top.contains(el) && false)) {
      // Different element is on top at the click point (overlay, sticky header, modal).
      const rootHit = top.getRootNode() === el.getRootNode() ? top : null;
      const labelOf = el.labels && [...el.labels].some((l) => l.contains(top));
      if (!labelOf && !(rootHit && rootHit.closest && rootHit.closest('label') && rootHit.closest('label').control === el)) {
        info.covered = true;
        info.covered_by = `${top.tagName.toLowerCase()}${top.id ? '#' + top.id : ''}${top.className && typeof top.className === 'string' ? '.' + top.className.split(/\s+/).filter(Boolean).slice(0, 2).join('.') : ''}`;
      }
    }
    return info;
  }
  function jsClick(ref) { const el = elOf(ref); scrollInto(el); el.click(); return true; }
  function rect(ref) {
    const el = elOf(ref); scrollInto(el);
    const r = el.getBoundingClientRect();
    return { x: r.left + window.scrollX, y: r.top + window.scrollY, width: r.width, height: r.height };
  }

  // ---------------------------------------------------------------- settle / wait
  function pendingDeep() {
    let n = W.fetches + activeTimers.size;
    try {
      for (const f of document.querySelectorAll('iframe')) {
        const w = f.contentWindow;
        if (w && w.__wm) n += w.__wm.pending();
      }
    } catch { /* cross-origin */ }
    return n;
  }
  async function settle(maxMs = 3000, quietMs = 150) {
    const start = performance.now();
    let last = Math.max(W.lastActivity, start - quietMs + 10);
    const mo = new MutationObserver(() => { last = performance.now(); });
    mo.observe(document, { subtree: true, childList: true, attributes: true, characterData: true });
    try {
      for (;;) {
        const now = performance.now();
        const quiet = now - Math.max(last, W.lastActivity) >= quietMs;
        if (document.readyState !== 'loading' && quiet && pendingDeep() === 0) break;
        if (now - start > maxMs) return { settled: false, waited: Math.round(now - start), pending: pendingDeep() };
        await sleep(25);
      }
    } finally { mo.disconnect(); }
    return { settled: true, waited: Math.round(performance.now() - start), pending: 0 };
  }
  function textPresent(needle) {
    const n = needle.toLowerCase();
    let found = false;
    const visit = (node) => {
      if (found) return;
      if (node.nodeType === 3) { if (node.nodeValue.toLowerCase().includes(n)) found = true; return; }
      if (node.nodeType === 1 && (SKIP.has(node.tagName) || hiddenSelf(node))) return;
      for (const k of kids(node)) visit(k);
    };
    visit(document.documentElement);
    if (!found && document.body && (document.body.innerText || '').toLowerCase().includes(n)) found = true;
    return found;
  }
  function selectorPresent(sel) { try { return deepQueryAll(document.documentElement, sel).some(rendered); } catch { return false; } }

  window.__wm = {
    snapshot, forms, field, prepare, finish, setNative, apply, comboPick, pickDate, clickInfo, jsClick, rect,
    settle, textPresent, selectorPresent,
    el: (ref) => elOf(ref),
    pending: pendingDeep,
    version: 1,
  };
})();
