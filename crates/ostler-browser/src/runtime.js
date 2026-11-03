// Ostler in-page runtime. Injected into every document before page scripts run.
// Everything the Rust side needs from inside the page lives behind window.__wm.
(() => {
  if (window.__wm) return;

  // ---------------------------------------------------------------- activity tracking
  // We count in-flight fetch/XHR and short timers so "settle" can wait for real
  // work to finish instead of sleeping for a fixed time.
  const W = { fetches: 0, lastActivity: performance.now(), lastRel: 0 };
  // lastRel moves only for activity a fill should wait for; lastActivity moves for everything (plain settle).
  const touch = (rel = true) => { const t = performance.now(); W.lastActivity = t; if (rel) W.lastRel = t; };
  // In-flight requests and short timers, with enough detail for a scoped settle to tell work the page
  // started because of us from background chatter (polling, analytics, long polls, tickers).
  const reqs = new Set();
  const activeTimers = new Map(); // timer id -> entry
  const hist = new Map();         // recurrence key -> recent start times
  const BG_HOSTS = /(^|\.)(google-analytics|googletagmanager|doubleclick|googlesyndication|googleadservices|adservice\.google|facebook\.net|hotjar|segment\.(io|com)|mixpanel|sentry\.io|clarity\.ms|newrelic|nr-data|datadoghq|amplitude|taboola|outbrain|criteo|scorecardresearch|quantserve|adnxs|fullstory|heap(analytics)?|optimizely|intercom|hubspot|clickagy|adsrvr|rubiconproject|pubmatic|openx)\b/i;
  const BG_PATH = /\/(beacon|collect|telemetry|analytics|heartbeat|rum|csp-report)\b/i;
  function recurs(key, t) {
    let a = hist.get(key);
    if (!a) { a = []; hist.set(key, a); }
    a.push(t);
    while (a.length && t - a[0] > 6000) a.shift();
    if (hist.size > 400) hist.delete(hist.keys().next().value);
    return a.length;
  }
  function reqKey(method, url, body) {
    let u;
    try { u = new URL(String(url), location.href); } catch { return method + ' ' + String(url); }
    if (method === 'GET') {
      // Cache-busting stamps must not make every poll look new.
      const q = [...u.searchParams].map(([k, v]) => k + '=' + (/^\d{8,}$|^0\.\d+$/.test(v) ? '*' : v)).join('&');
      return method + ' ' + u.origin + u.pathname + '?' + q;
    }
    return method + ' ' + u.origin + u.pathname + ' ' + bodyKey(body);
  }
  function bodyKey(body) {
    try {
      if (typeof body === 'string') return body.slice(0, 200);
      if (body instanceof URLSearchParams) return body.toString().slice(0, 200);
      if (typeof FormData !== 'undefined' && body instanceof FormData) {
        return [...body.entries()].map(([k, v]) => k + '=' + (typeof v === 'string' ? v : 'file')).join('&').slice(0, 200);
      }
      if (body && typeof body === 'object') return '[' + (body.constructor && body.constructor.name) + (body.size != null ? ':' + body.size : '') + ']';
    } catch { /* ignore */ }
    return '';
  }
  // A user/fill action (input, change, click, key) starts a new causal window: whatever the page fires in
  // response must not be mistaken for repeated chatter, so recurrence counting restarts here. A burst is a run
  // of actions less than 5 s apart (one fill); work that began before the burst is what the page was already doing.
  const ambient = new Set(); // keys known to be background chatter (long-pending before the fill, or recurring with no input)
  let burstStart = 0, lastAct = 0, settleStart = 0;
  for (const type of ['input', 'change', 'click', 'keydown', 'pointerdown']) {
    document.addEventListener(type, () => {
      const t = performance.now();
      if (!lastAct || t - lastAct > 5000) burstStart = t;
      lastAct = t;
      hist.clear();
    }, true);
  }
  const preBurst = (e) => e.start < (lastAct ? burstStart : settleStart);
  function trackReq(method, url, body, keepalive) {
    method = String(method || 'GET').toUpperCase();
    const t = performance.now();
    const e = { start: t, bg: !!keepalive, key: reqKey(method, url, body) };
    try {
      const u = new URL(String(url), location.href);
      if (BG_HOSTS.test(u.hostname) || BG_PATH.test(u.pathname)) e.bg = true;
    } catch { /* ignore */ }
    e.n = recurs(e.key, t);
    reqs.add(e);
    W.fetches++;
    return () => { reqs.delete(e); W.fetches--; touch(!isBg(e, performance.now(), 700)); };
  }
  // Background = known beacon/ad traffic, the same thing fired over and over with no input in between, or work that
  // was already pending (and long) before the fill began. Work the fill itself set going is never aged out by
  // duration alone: a slow uniqueness check or a debounce is exactly what a fill must wait for.
  function isBg(e, now, bgMs) {
    if (e.bg || ambient.has(e.key)) return true;
    if (e.n >= 4) { ambient.add(e.key); if (ambient.size > 300) ambient.delete(ambient.values().next().value); return true; }
    if (preBurst(e) && (now - e.start > bgMs || (e.delay || 0) > bgMs)) { e.bg = true; ambient.add(e.key); return true; }
    return false;
  }

  const origFetch = window.fetch;
  if (origFetch) {
    window.fetch = function (...args) {
      let method = 'GET', url = args[0], body, ka = false;
      try {
        const r = args[0], init = args[1] || {};
        if (typeof Request !== 'undefined' && r instanceof Request) { url = r.url; method = r.method; ka = r.keepalive; }
        method = init.method || method; body = init.body; ka = init.keepalive || ka;
      } catch { /* ignore */ }
      const done = trackReq(method, url, body, ka);
      let p;
      try { p = origFetch.apply(this, args); } catch (e) { done(); throw e; }
      p.then(done, done);
      return p;
    };
  }
  const xhrSend = XMLHttpRequest.prototype.send;
  const xhrOpen = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function (m, u, ...rest) { this.__wmReq = [m, u]; return xhrOpen.call(this, m, u, ...rest); };
  XMLHttpRequest.prototype.send = function (...args) {
    const r = this.__wmReq || [];
    const done = trackReq(r[0], r[1], args[0], false);
    this.addEventListener('loadend', done, { once: true });
    return xhrSend.apply(this, args);
  };
  const origST = window.setTimeout.bind(window);
  const origCT = window.clearTimeout.bind(window);
  window.setTimeout = function (fn, delay, ...rest) {
    const d = Number(delay) || 0;
    if (typeof fn !== 'function' || d > 2500) return origST(fn, delay, ...rest);
    let id;
    const t = performance.now();
    let key = 'T';
    try { key = 'T ' + Function.prototype.toString.call(fn).slice(0, 160); } catch { /* ignore */ }
    const e = { start: t, delay: d, bg: false, key, n: recurs(key, t) };
    id = origST(function () {
      activeTimers.delete(id);
      touch(!isBg(e, performance.now(), 700));
      return fn.apply(this, rest);
    }, d);
    activeTimers.set(id, e);
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
    if (!el) throw new Error(`unknown ref ${ref}: the page changed or the last snapshot was truncated before it; call snapshot again (refs stay stable on the same page)`);
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

  // ---- concealed text -------------------------------------------------------------------------------
  // Text a person cannot see but a model would read: the usual hiding place for an instruction aimed at an
  // agent. Only *text* is judged. Controls are never dropped (a native checkbox at opacity 0 under a custom
  // one is normal), and screen-reader-only text (clip / 1px boxes) is kept: it is meant to be read.
  let memo = null; // per-snapshot cache; null outside a snapshot
  const newCache = () => ({ c: new WeakMap(), bg: new WeakMap() });
  // Solid colour painted behind `el`, or null when it cannot be told (gradient, image, translucent layer).
  function solidBackground(el, cache) {
    if (cache && cache.bg.has(el)) return cache.bg.get(el);
    const cs = styleOf(el);
    const c = parseRgb(cs.backgroundColor);
    let res;
    if (cs.backgroundImage && cs.backgroundImage !== 'none') res = null;
    else if (c && c.a >= 0.99) res = c;
    else if (c && c.a > 0) res = null;
    else if (el.parentElement) res = solidBackground(el.parentElement, cache);
    else {
      // Nothing painted: the canvas. Assume white unless the page opts into a dark scheme.
      const dark = /dark/.test(cs.colorScheme || '') || (window.matchMedia && matchMedia('(prefers-color-scheme: dark)').matches);
      res = dark ? null : { r: 255, g: 255, b: 255, a: 1 };
    }
    if (cache) cache.bg.set(el, res);
    return res;
  }
  function concealed(el, cache = memo) {
    if (cache && cache.c.has(el)) return cache.c.get(el);
    let v = false;
    const cs = styleOf(el);
    if (parseFloat(cs.opacity) === 0 || parseFloat(cs.fontSize) < 2) v = true;
    if (!v) {
      const fg = parseRgb(cs.color);
      if (fg && fg.a === 0) v = true;
      else if (fg) {
        const bg = solidBackground(el, cache);
        if (bg && Math.abs(fg.r - bg.r) < 8 && Math.abs(fg.g - bg.g) < 8 && Math.abs(fg.b - bg.b) < 8) v = true;
      }
    }
    if (!v) {
      // The old "left:-9999px" hiding idiom. Far off the page only, so a carousel one slide away stays readable.
      const r = el.getBoundingClientRect();
      if (r.width + r.height > 0 && (r.right + window.scrollX < -2000 || r.bottom + window.scrollY < -2000)) v = true;
    }
    if (!v && el.parentElement) v = concealed(el.parentElement, cache);
    if (cache) cache.c.set(el, v);
    return v;
  }
  // Text under `el` that a person can see, plus how many text runs were left out as concealed.
  function readableText(el) {
    const parts = [];
    let omitted = 0;
    const tw = el.ownerDocument.createTreeWalker(el, NodeFilter.SHOW_TEXT);
    for (let n = tw.nextNode(); n; n = tw.nextNode()) {
      const p = n.parentElement;
      const t = clean(n.nodeValue);
      if (!p || !t || SKIP.has(p.tagName) || !rendered(p)) continue;
      if (concealed(p)) { omitted++; continue; }
      parts.push(t);
    }
    return { text: parts.join(' '), omitted };
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
    if (el.querySelectorAll && c.querySelectorAll) {
      // The clone has no styles: judge the live nodes, drop their twins. Controls go anyway, so skip them.
      const cache = newCache();
      const live = el.querySelectorAll('*');
      const twins = c.querySelectorAll('*');
      live.forEach((n, i) => { if (twins[i] && !n.matches('select,textarea,input,button,option,script,style') && concealed(n, cache)) twins[i].remove(); });
      c.querySelectorAll('select,textarea,input,button,option,script,style').forEach((n) => n.remove());
    }
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
        // A neighbouring control (or one inside the neighbour) has its own label; this text is not ours.
        if (/^(INPUT|SELECT|TEXTAREA|BUTTON)$/.test(s.tagName) || (s.querySelector && s.querySelector('input,select,textarea,button'))) break;
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

  // ---- checkbox / radio labels --------------------------------------------------------------------
  // A bare checkbox is almost always written `<input type=checkbox> its label`: the text AFTER the control
  // names it. Reading the text before it (the previous control's label, or the section heading) names the
  // wrong box, and a fill then flips a different control than the one asked for.
  const INLINE = new Set(['A', 'B', 'I', 'EM', 'STRONG', 'SPAN', 'SMALL', 'FONT', 'U', 'CODE', 'ABBR', 'MARK', 'SUB', 'SUP', 'LABEL']);
  const isChoice = (el) => el.tagName === 'INPUT' && ['checkbox', 'radio'].includes((el.type || '').toLowerCase());
  const nextContent = (n) => { for (let s = n.nextSibling; s; s = s.nextSibling) { if (s.nodeType === 3 ? clean(s.nodeValue) : s.nodeType === 1) return true; } return false; };
  function afterText(el) {
    let node = el;
    // `<span class=custom><input></span> Label`: the text follows the wrapper.
    for (let up = 0; up < 2 && !nextContent(node) && node.parentElement && INLINE.has(node.parentElement.tagName); up++) node = node.parentElement;
    let out = '';
    for (let n = node.nextSibling, hops = 0; n && hops < 10; n = n.nextSibling, hops++) {
      if (n.nodeType === 3) { const t = clean(n.nodeValue); if (t) out = clean(out + ' ' + t); continue; }
      if (n.nodeType !== 1) continue;
      if (n.tagName === 'BR' || !INLINE.has(n.tagName) || n.querySelector('input,select,textarea,button')) break;
      if (SKIP.has(n.tagName) || !rendered(n)) continue;
      const t = ownText(n);
      if (t) out = clean(out + ' ' + t);
    }
    return out.length <= 80 ? out : '';
  }
  // Mirror of afterText: the inline text directly before the control, up to a <br>, block or another control.
  const prevContent = (n) => { for (let s = n.previousSibling; s; s = s.previousSibling) { if (s.nodeType === 3 ? clean(s.nodeValue) : s.nodeType === 1) return true; } return false; };
  function beforeText(el) {
    let node = el;
    for (let up = 0; up < 2 && !prevContent(node) && node.parentElement && INLINE.has(node.parentElement.tagName); up++) node = node.parentElement;
    let out = '';
    for (let n = node.previousSibling, hops = 0; n && hops < 10; n = n.previousSibling, hops++) {
      if (n.nodeType === 3) { const t = clean(n.nodeValue); if (t) out = clean(t + ' ' + out); continue; }
      if (n.nodeType !== 1) continue;
      if (n.tagName === 'BR' || !INLINE.has(n.tagName) || n.querySelector('input,select,textarea,button')) break;
      if (SKIP.has(n.tagName) || !rendered(n)) continue;
      const t = ownText(n);
      if (t) out = clean(t + ' ' + out);
    }
    return out.length <= 80 ? out : '';
  }
  // Text on both sides of a bare checkbox (`Email <input> Phone <input>`): which side names the box is a
  // property of the whole group's layout. The last control has trailing text only when every label follows its
  // control; otherwise every label precedes it. When that cannot be told, return '' so the caller refuses to guess.
  function choiceSide(el) {
    let box = el.parentElement, group = [el];
    for (let up = 0; up < 3 && box; up++, box = box.parentElement) {
      const g = [...box.querySelectorAll('input[type=checkbox],input[type=radio]')];
      if (g.length > 1) { group = g; break; }
    }
    if (group.length < 2) return 'after'; // lone control: leading text is a lead-in, the trailing text is its label
    const first = group[0], last = group[group.length - 1];
    if (afterText(last)) return 'after';
    return beforeText(first) ? 'before' : '';
  }
  // Text that is some other checkbox's own trailing label cannot also name this one.
  function claimedByAnother(el, text) {
    const scope = el.form || el.getRootNode();
    if (!scope || !scope.querySelectorAll) return false;
    for (const c of scope.querySelectorAll('input[type=checkbox],input[type=radio]')) {
      if (c !== el && afterText(c) === text) return true;
    }
    return false;
  }

  function labelFor(el) {
    const lb = el.getAttribute('aria-labelledby');
    if (lb) { const t = textOfIds(el, lb); if (t) return t; }
    const al = el.getAttribute('aria-label');
    if (al && clean(al)) return clean(al);
    if (el.labels && el.labels.length) {
      const t = clean([...el.labels].map(ownText).join(' '));
      if (t) return t;
    }
    if (isChoice(el)) {
      const after = afterText(el);
      if (after) {
        const adj = beforeText(el);
        if (adj) {
          // Text on both sides: the group's layout decides which one is ours.
          const side = choiceSide(el);
          if (side === 'after') return after;
          if (side === 'before') return adj;
          return clean(el.title || el.getAttribute('name') || '');
        }
        return after;
      }
      // No trailing text: in a group whose labels all precede their controls, the text just before is ours.
      const adjBefore = beforeText(el);
      if (adjBefore && choiceSide(el) === 'before') return adjBefore;
      const before = nearbyText(el);
      if (before && !claimedByAnother(el, before)) return before;
      return clean(el.title || el.getAttribute('name') || '');
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

  // Search engines wrap result links in tracking redirects (Bing /ck/a?u=a1<base64>, Google /url?q=,
  // DuckDuckGo /l/?uddg=, Yahoo /RU=<url>/RK). Show the real destination so the agent can judge and
  // quote it; clicking still goes through the element itself.
  function displayHref(el) {
    const raw = el.getAttribute('href') || '';
    if (!raw || raw.startsWith('#') || raw.startsWith('javascript:')) return raw;
    try {
      const u = new URL(raw, location.href);
      const p = u.searchParams;
      let dest = null;
      if (/(^|\.)bing\.com$/.test(u.hostname) && u.pathname === '/ck/a') {
        const v = p.get('u') || '';
        if (v.startsWith('a1')) {
          const b = v.slice(2).replace(/-/g, '+').replace(/_/g, '/');
          dest = decodeURIComponent(escape(atob(b + '='.repeat((4 - (b.length % 4)) % 4))));
        }
      } else if (/(^|\.)google\.[a-z.]+$/.test(u.hostname) && u.pathname === '/url') {
        dest = p.get('q') || p.get('url');
      } else if (/(^|\.)duckduckgo\.com$/.test(u.hostname) && u.pathname.startsWith('/l/')) {
        dest = p.get('uddg');
      } else if (/(^|\.)search\.yahoo\.com$/.test(u.hostname)) {
        const m = u.pathname.match(/\/RU=([^/]+)\/R[KV]=/);
        if (m) dest = decodeURIComponent(m[1]);
      }
      if (dest && /^https?:\/\//i.test(dest)) return dest;
      // Absolute form for same-origin relative links keeps the host visible only when it differs.
      return raw.startsWith('http') ? raw : (u.origin === location.origin ? raw : u.href);
    } catch { return raw; }
  }

  // ---------------------------------------------------------------- snapshot
  function snapshot(opts = {}) {
    const lines = [];
    const budget = opts.maxChars || 20000;
    const interactiveOnly = !!opts.interactiveOnly;
    const root = opts.ref ? elOf(opts.ref) : document.documentElement;
    let used = 0, truncated = false, omittedText = 0;
    memo = newCache();
    const emit = (depth, text) => {
      if (truncated) return;
      const line = '  '.repeat(depth) + text;
      used += line.length + 1;
      if (used > budget) { truncated = true; return; }
      lines.push(line);
    };
    const q = (s) => JSON.stringify(s);
    // Long text becomes several lines at word boundaries; anything cut gets an explicit marker.
    const CHUNK = 500, TEXT_CAP = 3000;
    function emitText(depth, t) {
      let rest = t.length > TEXT_CAP ? t.slice(0, TEXT_CAP) : t;
      const cut = t.length - rest.length;
      while (rest.length > CHUNK) {
        let i = rest.lastIndexOf(' ', CHUNK);
        if (i < CHUNK / 2) i = CHUNK;
        emit(depth, `text ${q(rest.slice(0, i).trim())}`);
        rest = rest.slice(i).trim();
      }
      if (rest) emit(depth, `text ${q(rest)}`);
      if (cut > 0) emit(depth, `text "… [${cut} more chars cut]"`);
    }
    const seenEmptyHref = new Set();
    function ctl(el, role, depth) {
      const r = refOf(el);
      if (truncated) return; // keep numbering stable: refs are still assigned past the cut
      const name = nameOf(el);
      // Image-only duplicates of a link already listed (product cards repeat the same href) add noise.
      if (role === 'link' && !name) {
        const h = el.getAttribute('href') || '';
        if (!h || seenEmptyHref.has(h)) return;
        seenEmptyHref.add(h);
      }
      let s = `${role}${name ? ' ' + q(name) : ''} [${r}]`;
      if (role === 'heading') s = `heading ${q(name)} [${r}]`;
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
        const href = displayHref(el);
        if (href) attrs.push(`href=${q(href.length > 100 ? href.slice(0, 100) + '…' : href)}`);
      } else if (role === 'heading') {
        attrs.push(`level=${el.tagName[1] || el.getAttribute('aria-level') || '?'}`);
      }
      emit(depth, s + (attrs.length ? ' ' + attrs.join(' ') : ''));
    }
    function walk(node, depth, noText) {
      if (node.nodeType === 3) {
        if (!interactiveOnly && !noText && !truncated) {
          const t = clean(node.nodeValue);
          if (t) { if (node.parentElement && concealed(node.parentElement)) omittedText++; else emitText(depth, t); }
        }
        return;
      }
      if (node.nodeType !== 1) return;
      const el = node;
      if (SKIP.has(el.tagName) || hiddenSelf(el)) return;
      // A label that owns a control: keep the control (it may be wrapped), drop the duplicate text.
      if (el.tagName === 'LABEL' && el.control) noText = true;
      let role = roleOf(el);
      // A page-level element with a click handler (common in single-page apps) would swallow everything
      // inside it as one control. When it contains real controls, treat it as a container instead.
      if (role === 'clickable' && el.querySelector('a[href],button,input:not([type=hidden]),select,textarea,summary,[role=button],[role=link],h1,h2,h3')) role = null;
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
      else if (opts.collapseFooter && el !== root && (role === 'contentinfo' || (el.tagName === 'FOOTER' && !el.closest('article,aside'))) && el.querySelectorAll('a[href]').length >= 5) {
        const n = el.querySelectorAll('a[href]').length;
        const f = el.querySelectorAll('input:not([type=hidden]),select,textarea').length;
        const r = refOf(el);
        emit(depth, `footer [${r}]: ${n} link${n === 1 ? '' : 's'}${f ? ` and ${f} field${f === 1 ? '' : 's'}` : ''} omitted; snapshot ref=${r} to expand`);
        return;
      }
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
          if (!interactiveOnly && !noText && !truncated) {
            const r = readableText(el);
            omittedText += r.omitted;
            const t = r.omitted ? r.text : (clean(el.innerText) || r.text);
            if (t) emitText(depth, t);
          }
          return;
        }
      }
      if (el.tagName === 'LEGEND') return;
      for (const k of kids(el)) walk(k, d, noText);
    }
    walk(root, 0, false);
    // A heading's ref reads its whole section: the siblings after it up to the next heading of the same
    // or a higher level (Wikipedia wraps each heading in a div, so step out of such a wrapper first).
    if (opts.ref && /^H[1-6]$/.test(root.tagName)) {
      const level = +root.tagName[1];
      const headingIn = (n) => {
        if (/^H[1-6]$/.test(n.tagName)) return n;
        const h = n.firstElementChild;
        return h && /^H[1-6]$/.test(h.tagName) && n.children.length <= 3 ? h : null;
      };
      let start = root;
      if (root.parentElement && root.parentElement !== document.body && headingIn(root.parentElement) === root) start = root.parentElement;
      for (let n = start.nextElementSibling; n && !truncated; n = n.nextElementSibling) {
        const h = headingIn(n);
        if (h && +h.tagName[1] <= level) break;
        walk(n, 0, false);
      }
    }
    memo = null;
    if (omittedText) emit(0, `note: ${omittedText} invisible text run(s) omitted (opacity 0, zero font size, far off-screen or same colour as the background)`);
    return { url: location.href, title: document.title, lines, truncated, refs: counter, omittedText };
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
    // Walk forward from the control a bounded number of elements: its message sits right after it. Scanning
    // the whole grandparent per field made large flat forms quadratic.
    const tw = el.ownerDocument.createTreeWalker(scope, NodeFilter.SHOW_ELEMENT);
    tw.currentNode = el;
    for (let steps = 0, cand = tw.nextNode(); cand && steps < 60; cand = tw.nextNode(), steps++) {
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
  // True when `cand` is this control's own message: the control is the only one in `scope`, or the message
  // sits after it and before the next different control. Stops one invalid field's message being
  // attached to its neighbours (a shared wrapper holds several fields and several messages).
  function messageBelongsTo(el, cand, scope) {
    const foreign = [...scope.querySelectorAll('input,select,textarea')].filter((c) => {
      if (c === el) return false;
      const ty = (c.type || '').toLowerCase();
      if (['hidden', 'submit', 'button', 'reset', 'image'].includes(ty)) return false;
      return !(c.name && c.name === el.name && ty === (el.type || '').toLowerCase()); // same radio/checkbox group
    });
    if (!foreign.length) return true;
    if (!(el.compareDocumentPosition(cand) & Node.DOCUMENT_POSITION_FOLLOWING)) return false;
    return foreign.every((c) => !(el.compareDocumentPosition(c) & Node.DOCUMENT_POSITION_FOLLOWING) || (cand.compareDocumentPosition(c) & Node.DOCUMENT_POSITION_FOLLOWING));
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
    // Nearby visible error element (common framework patterns). `.help-block` is help text in Bootstrap 3
    // unless the group is marked `.has-error`.
    const scopes = [el.parentElement, el.parentElement && el.parentElement.parentElement].filter(Boolean);
    for (const s of scopes) {
      for (const cand of s.querySelectorAll('[role=alert],.error,.invalid-feedback,.field-error,.error-message,.has-error .help-block,[class*="error-"],[class*="-error"],[data-error]')) {
        if (cand.contains(el) || cand.querySelector('input,select,textarea') || !rendered(cand)) continue;
        if (!messageBelongsTo(el, cand, s)) continue;
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
  // A text box that wants a date in a particular shape says so in its placeholder or a widget attribute.
  const FMT_RE = /^(?:d{1,2}|m{1,2}|y{2}|y{4})([\/.\-])(?:d{1,2}|m{1,2}|y{2}|y{4})\1(?:d{1,2}|m{1,2}|y{2}|y{4})$/;
  function dateFormat(el) {
    if (el.tagName !== 'INPUT' || (el.type || 'text').toLowerCase() !== 'text') return null;
    for (const a of ['placeholder', 'data-date-format', 'data-format', 'data-inputmask-inputformat']) {
      const t = clean(el.getAttribute(a) || '').toLowerCase();
      if (FMT_RE.test(t) && /d/.test(t) && /m/.test(t) && /y/.test(t)) return t;
    }
    if ((el.getAttribute('data-provide') || '').toLowerCase() === 'datepicker') return 'mm/dd/yyyy?'; // bootstrap-datepicker default; the trailing ? marks it as assumed, not stated
    return null;
  }
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
      options: [], value: '', section: null, error: errorText(el), hidden: false, grouped: false,
      format: dateFormat(el),
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
    f.grouped = true;
    f.format = null;
    f.options = members.map((m) => ({ value: valOf(m), label: labOf(m) || valOf(m), selected: isOn(m), disabled: !!m.disabled || m.getAttribute('aria-disabled') === 'true' }));
    f.value = type === 'radio' ? ((members.find(isOn) && valOf(members.find(isOn))) || '') : members.filter(isOn).map(valOf);
    f.disabled = f.options.every((o) => o.disabled);
    f.error = members.map(errorText).find(Boolean) || null;
    groups.set(f.ref, members);
    return f;
  }
  // react-select and friends keep an empty search <input> and show the choice in a sibling "single value"
  // element. Only looked at when the input itself is empty, and never across another combobox.
  function shownChoice(el) {
    let box = el.parentElement;
    for (let i = 0; i < 4 && box && !box.matches('form,body'); i++, box = box.parentElement) {
      if (box.querySelectorAll('[role=combobox],input[aria-autocomplete]').length > 1) break;
      const hits = [...box.querySelectorAll('[class*="single-value" i],[class*="singlevalue" i],[class*="selected-value" i],[class*="selectedvalue" i]')]
        .filter((n) => !n.contains(el) && !n.querySelector('input') && rendered(n));
      const t = clean(hits.map(ownText).join(' '));
      if (t) return t;
    }
    return '';
  }
  function buildField(el) {
    const f = baseField(el);
    f.value = readValue(el, f.kind);
    if (f.kind === 'select' || f.kind === 'multi_select') f.options = optionsOf(el);
    if (f.kind === 'combobox') {
      const lid = el.getAttribute('aria-controls') || el.getAttribute('aria-owns');
      const lb = lid && byId(el, lid);
      if (lb && rendered(lb)) f.options = [...lb.querySelectorAll('[role=option]')].map((o) => ({ value: o.getAttribute('data-value') || clean(o.textContent), label: clean(o.textContent), selected: o.getAttribute('aria-selected') === 'true', disabled: o.getAttribute('aria-disabled') === 'true' }));
      if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') f.value = el.value || shownChoice(el);
      else f.value = clean(el.innerText);
    }
    return f;
  }

  const SUBMIT_RE = /^(submit|send|save|continue|next|finish|finish up|review|apply|register|sign ?up|sign ?in|log ?in|create|pay|confirm|book|place order|complete|done|proceed|get started|subscribe|checkout|search|go)\b/i;
  const NEXT_RE = /\b(next|continue|proceed|review|step)\b/i;
  const NOT_SUBMIT_RE = /\b(back|previous|prev|cancel|reset|clear|close|skip)\b/i;
  const CHROME_LANDMARK = 'header,nav,[role=banner],[role=navigation],[role=search],[role=menubar]';
  // `hasForm` is false for the synthetic "page" form made of controls outside any <form>. There a button
  // does not submit anything by default, so a type-less <button> only counts when its label says so, and
  // the site's header/navigation/search chrome is never a candidate.
  function submitControls(scopeEl, root, hasForm) {
    const out = [];
    const cands = deepQueryAll(root || scopeEl, 'button,input[type=submit],input[type=button],input[type=image],[role=button]');
    for (const b of cands) {
      if (!rendered(b)) continue;
      if (!hasForm && b.closest(CHROME_LANDMARK)) continue;
      const label = nameOf(b);
      const declared = (b.getAttribute('type') || '').toLowerCase();
      const owned = !!(b.form || (b.closest && b.closest('form')));
      const type = declared || (b.tagName === 'BUTTON' && (hasForm || owned) ? 'submit' : '');
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

  // The <form> a control belongs to. A control inside a web component's shadow root has no `.form` and no
  // form ancestor in its own tree, but its host (or the host's host) may sit inside a <form>.
  function formOf(el) {
    let f = el.form || (el.closest && el.closest('form'));
    let root = el.getRootNode && el.getRootNode();
    while (!f && root && root.host) {
      const h = root.host;
      f = h.form || (h.closest && h.closest('form'));
      root = h.getRootNode && h.getRootNode();
    }
    return f || null;
  }

  function forms() {
    groups.clear();
    const controls = deepQueryAll(document.documentElement, CONTROL_SEL).filter(eligible);
    const containers = new Map(); // key (form element or root node) -> {el, controls[]}
    for (const el of controls) {
      const form = formOf(el);
      const key = form || rootOf(el);
      if (!containers.has(key)) containers.set(key, { form: form || null, root: rootOf(el), controls: [] });
      containers.get(key).controls.push(el);
    }
    const result = [];
    for (const c of containers.values()) {
      const seenGroup = new Set();
      const fields = [];
      let hiddenCount = 0;
      const concealed = [];
      for (const el of c.controls) {
        if (!visibleControl(el)) {
          hiddenCount++;
          if (concealed.length < 40) {
            const l = el.labels && el.labels[0];
            const name = clean((l && l.textContent) || el.getAttribute('aria-label') || el.getAttribute('placeholder') || '');
            if (name && !concealed.includes(name)) concealed.push(name);
          }
          continue;
        }
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
        fields, hidden_fields: hiddenCount, concealed,
        submit: submitControls(scope, submitRoot, !!formEl),
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

  // What the control itself says about the value it now holds (constraint validation), or null when it is fine.
  // Never fires events and never changes anything.
  function validity(ref) {
    const el = elOf(ref);
    if (!el.validity || el.validity.valid || el.disabled) return null;
    if (el.tagName === 'INPUT' && ['checkbox', 'radio', 'file'].includes((el.type || '').toLowerCase())) return null;
    const val = el.value;
    if (val === '' || val === undefined) return null;
    const v = el.validity;
    const min = el.getAttribute('min') || '', max = el.getAttribute('max') || '';
    if (v.rangeUnderflow || v.rangeOverflow) return `value ${val} is outside the allowed range ${min}..${max}`;
    if (v.typeMismatch) return `value ${val} is not a valid ${({ email: 'email address', url: 'URL' })[el.type] || el.type}`;
    if (v.patternMismatch) return `value ${val} does not match the required pattern ${el.getAttribute('pattern')}${el.title ? ' (' + clean(el.title) + ')' : ''}`;
    if (v.tooLong) return `value is longer than the allowed ${el.maxLength} characters`;
    if (v.tooShort) return `value is shorter than the required ${el.minLength} characters`;
    if (v.stepMismatch) return `value ${val} does not fit the step ${el.getAttribute('step') || 1}`;
    if (v.badInput) return `value ${val} cannot be read as ${el.type}`;
    if (v.customError) return el.validationMessage || 'value rejected by the page';
    return el.validationMessage || 'value is not valid';
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
    // Widgets such as bootstrap-datepicker only learn the typed text from a keyup; without one they
    // rewrite the box from their own (empty) state when they close. Send one harmless keyup.
    if ((el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') && el.value) {
      try { el.dispatchEvent(new KeyboardEvent('keyup', { bubbles: true, cancelable: true, key: 'a', code: 'KeyA', keyCode: 65, which: 65 })); } catch {}
    }
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
    // Exact (normalized) match only: a partial pick would diverge from the request, and the verifier
    // refuses substring equivalence. Misses return the available labels so the caller can choose.
    // Address suggestions list the whole address ("1600 Amphitheatre Parkway, Mountain View, CA 94043"):
    // the request names its first part. Accept that only when exactly one suggestion starts with it
    // followed by a comma. The caller still reads the box back, so a pick that shows more than was asked
    // for is reported as a mismatch, not as success.
    if (idx < 0) {
      const head = clean(wanted).toLowerCase() + ',';
      const starts = labels.map((l, i) => (clean(l).toLowerCase().startsWith(head) ? i : -1)).filter((i) => i >= 0);
      if (starts.length === 1) idx = starts[0];
    }
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
  // opts (fill mode): { scope: ref of the form/control whose changes matter, bgMs: age at which pending work is
  // treated as background }. Without opts the behaviour is the plain "everything must be quiet" wait.
  function pendingDeep(opts) {
    let n;
    if (!opts) n = W.fetches + activeTimers.size;
    else {
      const now = performance.now(), bg = opts.bgMs || 700;
      n = 0;
      for (const e of reqs) if (!isBg(e, now, bg)) n++;
      for (const e of activeTimers.values()) if (!isBg(e, now, bg)) n++;
    }
    try {
      for (const f of document.querySelectorAll('iframe')) {
        const w = f.contentWindow;
        if (w && w.__wm) n += w.__wm.pending(opts);
      }
    } catch { /* cross-origin */ }
    return n;
  }
  const CTRL_SEL = 'input,select,textarea,[role=listbox],[role=option],[role=combobox],[role=menu],[role=dialog],[role=alertdialog]';
  // Would this DOM change matter to a fill? Anything inside the scope does, as does a rebuild of the scope
  // itself or a popup (listbox, dialog) appearing; a ticker elsewhere on the page does not.
  function relevantMutation(m, scope, noisy) {
    const t = m.target;
    const inScope = scope.contains(t);
    if (inScope) {
      // A node rewriting itself over and over (clock, counter) is chatter, not a response to us.
      const n = (noisy.get(t) || 0) + 1;
      noisy.set(t, n);
      return n <= 6;
    }
    if (m.type !== 'childList') return false;
    const hit = (n) => n.nodeType === 1 && (n.contains(scope) || (n.matches(CTRL_SEL) || n.querySelector(CTRL_SEL)));
    for (const n of m.removedNodes) if (n.nodeType === 1 && n.contains(scope)) return true;
    for (const n of m.addedNodes) if (hit(n)) return true;
    return false;
  }
  async function settle(maxMs = 3000, quietMs = 150, opts) {
    const start = performance.now();
    settleStart = start;
    let last = Math.max(opts ? W.lastRel : W.lastActivity, start - quietMs + 10);
    let scope = null;
    if (opts && opts.scope) {
      const el = refs.get(String(opts.scope).replace(/^@/, ''));
      if (el && el.isConnected) scope = el;
    }
    const noisy = new Map();
    const mo = new MutationObserver((recs) => {
      if (!opts || !scope) { last = performance.now(); return; }
      for (const m of recs) if (relevantMutation(m, scope, noisy)) { last = performance.now(); return; }
    });
    mo.observe(document, { subtree: true, childList: true, attributes: true, characterData: true });
    try {
      for (;;) {
        const now = performance.now();
        const quiet = now - Math.max(last, opts ? W.lastRel : W.lastActivity) >= quietMs;
        if (document.readyState !== 'loading' && quiet && pendingDeep(opts) === 0) break;
        if (now - start > maxMs) return { settled: false, waited: Math.round(now - start), pending: pendingDeep(opts) };
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

  // ---------------------------------------------------------------- where to click
  const TARGET_SEL = 'a[href],button,input:not([type=hidden]),select,textarea,summary,[role=button],[role=link],[role=tab],[role=menuitem],[role=checkbox],[role=radio],[role=switch],[role=combobox],[role=option],[onclick],[tabindex]:not([tabindex="-1"]),[contenteditable=""],[contenteditable=true]';
  function targets(max) {
    const vw = window.innerWidth, vh = window.innerHeight;
    const out = [];
    let covered = 0;
    const seen = new Set();
    for (const el of deepQueryAll(document.documentElement, TARGET_SEL)) {
      if (el.ownerDocument !== document || seen.has(el) || !visibleControl(el)) continue;
      seen.add(el);
      // A visually hidden native checkbox/radio is operated through its label.
      const box = rendered(el) ? el : ((el.labels && [...el.labels].find(rendered)) || el);
      const r = box.getBoundingClientRect();
      if (r.width < 4 || r.height < 4 || r.bottom < 0 || r.right < 0 || r.top > vh || r.left > vw) continue;
      const cx = Math.min(vw - 1, Math.max(0, r.left + r.width / 2)), cy = Math.min(vh - 1, Math.max(0, r.top + r.height / 2));
      const top = deepElementFromPoint(document, cx, cy);
      if (top && top !== box && !box.contains(top) && !top.contains(box) && !(top.closest && top.closest('label') && top.closest('label').control === el)) { covered++; continue; }
      const role = roleOf(el) || 'clickable';
      const type = (el.getAttribute('type') || '').toLowerCase();
      const name = nameOf(el) || clean(el.getAttribute('placeholder') || '') || clean(el.getAttribute('title') || '');
      let purpose;
      if (['textbox', 'searchbox', 'spinbutton'].includes(role) || el.tagName === 'TEXTAREA') purpose = 'type here';
      else if (['combobox', 'listbox'].includes(role)) purpose = 'choose an option';
      else if (['checkbox', 'radio', 'switch'].includes(role)) purpose = 'toggle';
      else if (role === 'file') purpose = 'attach a file';
      else if (role === 'link') purpose = el.closest('nav,header,footer,[role=navigation]') ? 'navigation link' : 'link';
      else if (role === 'tab' || role === 'menuitem') purpose = 'switch view';
      else if (role === 'button') {
        const sub = (el.form || el.closest('form')) && (type === 'submit' || (el.tagName === 'BUTTON' && !type));
        purpose = sub ? (NEXT_RE.test(name) && !/submit|send|pay|place order|confirm/i.test(name) ? 'next step' : 'submits the form') : 'button';
        if (NOT_SUBMIT_RE.test(name)) purpose = 'dismiss/secondary';
      } else purpose = 'clickable';
      const state = [];
      if (el.disabled || el.getAttribute('aria-disabled') === 'true') state.push('disabled');
      if (el.required || el.getAttribute('aria-required') === 'true') state.push('required');
      if (el.checked === true || el.getAttribute('aria-checked') === 'true') state.push('checked');
      const err = ['textbox', 'searchbox', 'spinbutton', 'combobox'].includes(role) ? errorText(el) : null;
      if (err) state.push('error: ' + err.slice(0, 80));
      const val = (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') && !['button', 'submit', 'checkbox', 'radio', 'file', 'password'].includes(type) ? el.value : '';
      out.push({ ref: refOf(el), role, name: name.slice(0, 60), purpose, state, value: val ? val.slice(0, 40) : '', x: Math.round(r.left), y: Math.round(r.top), w: Math.round(r.width), h: Math.round(r.height) });
    }
    out.sort((a, b) => (Math.round(a.y / 12) - Math.round(b.y / 12)) || (a.x - b.x));
    return { viewport: { w: vw, h: vh }, items: out.slice(0, max || 40), total: out.length, covered };
  }
  function mark(list) {
    unmark();
    const host = document.createElement('div');
    host.id = '__wm_marks';
    host.style.cssText = 'position:fixed;inset:0;pointer-events:none;z-index:2147483647';
    const color = (t) => t.state.some((x) => x.startsWith('error')) ? '#d32f2f' : t.purpose === 'submits the form' || t.purpose === 'next step' ? '#2e7d32' : t.purpose === 'type here' || t.purpose === 'choose an option' ? '#1565c0' : t.purpose.includes('link') || t.purpose === 'link' ? '#6a1b9a' : '#ef6c00';
    for (const t of list.items) {
      const c = color(t);
      const b = document.createElement('div');
      b.style.cssText = `position:absolute;left:${t.x}px;top:${t.y}px;width:${t.w}px;height:${t.h}px;outline:2px solid ${c};outline-offset:1px;box-sizing:border-box`;
      const tag = document.createElement('span');
      tag.textContent = t.ref;
      tag.style.cssText = `position:absolute;left:0;top:${t.y < 16 ? '0' : '-16px'};background:${c};color:#fff;font:bold 11px/14px system-ui,sans-serif;padding:0 4px;border-radius:3px;white-space:nowrap`;
      b.appendChild(tag);
      host.appendChild(b);
    }
    document.documentElement.appendChild(host);
    return list.items.length;
  }
  function unmark() { const h = document.getElementById('__wm_marks'); if (h) h.remove(); return true; }

  // ---------------------------------------------------------------- human-verification challenges
  // Ostler never solves these. It reports them so the agent stops retrying and hands off to the user.
  // Only walls that block the page count: the invisible reCAPTCHA badge and hidden widgets do not.
  const CHALLENGE_SRC = [
    [/google\.com\/recaptcha|recaptcha\.net/i, 'reCAPTCHA'],
    [/hcaptcha\.com/i, 'hCaptcha'],
    [/challenges\.cloudflare\.com/i, 'Cloudflare Turnstile'],
    [/arkoselabs\.com|funcaptcha\.com/i, 'Arkose'],
    [/captcha-delivery\.com/i, 'DataDome'],
  ];
  const CHALLENGE_BOX = [
    ['.g-recaptcha', 'reCAPTCHA'], ['.h-captcha', 'hCaptcha'], ['.cf-turnstile', 'Cloudflare Turnstile'],
  ];
  function bigVisible(el) {
    if (!rendered(el)) return false;
    if (el.closest && el.closest('.grecaptcha-badge')) return false; // the invisible-v3 badge
    const r = el.getBoundingClientRect();
    return r.width >= 200 && r.height >= 50;
  }
  function challenge() {
    for (const f of deepQueryAll(document.documentElement, 'iframe')) {
      const src = f.getAttribute('src') || '';
      const hit = CHALLENGE_SRC.find(([re]) => re.test(src));
      if (hit && bigVisible(f)) return { vendor: hit[1], kind: 'widget' };
    }
    for (const [sel, vendor] of CHALLENGE_BOX) {
      for (const el of deepQueryAll(document.documentElement, sel)) if (bigVisible(el)) return { vendor, kind: 'widget' };
    }
    // Interstitials replace the whole page, so they are short and say so in the title or body.
    const text = (document.body && document.body.innerText || '').trim();
    const title = document.title || '';
    if (text.length < 2500) {
      // A real wall has no ordinary content: a page with a form on it that merely mentions verification is not one.
      const hasInputs = deepQueryAll(document.documentElement, 'input:not([type=hidden]):not([type=submit]):not([type=button]),textarea,select').some(rendered);
      if (/^(just a moment|attention required|verifying you are human|are you a robot|robot check|security check)/i.test(title)
        || (!hasInputs && /(verify(ing)? (that )?you are (a )?human|checking if the site connection is secure|unusual traffic from your computer|confirm you are not a robot|press (&|and) hold|bots use duckduckgo too|confirm this search was made by a human)/i.test(text))) {
        const vendor = /duckduckgo/i.test(text) ? 'DuckDuckGo' : /cloudflare/i.test(text + title) ? 'Cloudflare' : location.hostname.replace(/^www\./, '');
        return { vendor, kind: 'interstitial' };
      }
    }
    // DuckDuckGo serves a result-less page (menu and promo links only) to browsers it distrusts.
    if (/(^|\.)duckduckgo\.com$/.test(location.hostname) && /[?&]q=/.test(location.search) && !/^\/(html|lite)\//.test(location.pathname)
        && !document.querySelector('[data-testid="result"],article[data-nrn],.react-results--main li,.result,#links .result,.results_links')) {
      return { vendor: 'duckduckgo.com (no results were served to this browser)', kind: 'block' };
    }
    // Block pages from sites that refuse automated browsers outright. They hold no content, so a short
    // page whose title/URL/text says "denied/blocked/suspended/captcha" is a wall, not a result.
    if (text.length < 1500) {
      const blob = title + ' ' + location.href + ' ' + text.slice(0, 600);
      if (/^(captcha\b|access denied|access to this page has been denied|access temporarily suspended|request blocked|pardon our interruption|403 forbidden|you have been blocked|sorry, you have been blocked)|captcha - |blocked\.html|\/sorry\/|access temporarily suspended|automated (access|queries)|detected unusual (traffic|activity)|your request has been blocked/i.test(blob)) {
        const vendor = location.hostname.replace(/^www\./, '');
        return { vendor, kind: 'block' };
      }
    }
    return null;
  }
  // Visible text that says a submission went through ("Thank you", "has been sent", ...). Compared before
  // and after a click: new confirmation text is evidence the page accepted the data even when the URL
  // did not change (XHR forms) or the next page still has inputs (a newsletter box on a thank-you page).
  const CONFIRM_RE = /\b(thank you|thanks for|successfully|has been (?:sent|submitted|received|saved|created|placed|registered)|was (?:sent|submitted|received|saved|created)|we(?:'ll| will) (?:be in touch|get back|contact)|confirmation (?:number|email|code)|order (?:confirmed|placed|received)|you(?:'re| are) (?:all set|registered|subscribed))\b/i;
  function confirmations() {
    const out = [];
    for (const n of deepQueryAll(document.documentElement, '[role=status],[role=alert],[aria-live],.alert-success,.success,.toast,.notification,[class*=success],[class*=confirm],[class*=thank],h1,h2,h3,h4,p,li')) {
      if (!rendered(n) || n.querySelector('input,select,textarea,form')) continue;
      const t = ownText(n);
      if (!t || t.length > 200 || !CONFIRM_RE.test(t)) continue;
      // An error that merely contains the word ("could not be sent successfully") is not a confirmation.
      if (ALERT_WORDS.test(t) && !/\b(thank you|thanks for)\b/i.test(t)) continue;
      if (!out.includes(t)) out.push(t);
      if (out.length >= 3) break;
    }
    return out;
  }

  // Identity of this document: a server round trip that lands on the same URL (a failed login redirecting back
  // to the login page) is still a new document, which the URL alone cannot show.
  const DOC_ID = Math.random().toString(36).slice(2) + Date.now().toString(36);
  const PROBLEM_WORDS = /\b(invalid|incorrect|failed|failure|not valid|wrong|denied|unable|error|errors)\b/i;
  const STRONG_SEL = '[role=alert],[class*=error i],[class*=flash i],[class*=alert i],[class*=danger i],[class*=invalid i],[class*=fail i]';
  // Error-like messages on the page now. `strong` ones announce themselves (role=alert, an error/flash/danger
  // class); `weak` ones are only short text that says something went wrong, so a caller should ignore any
  // that were already there before.
  function problems() {
    const strong = [], weak = [];
    const skipEl = (n) => !rendered(n) || n.matches('script,style,form,input,select,textarea,button,option,label') || n.querySelector('input,select,textarea,button,form');
    for (const n of document.querySelectorAll(STRONG_SEL)) {
      if (strong.length >= 4) break;
      const cls = String(n.className || '');
      if (/success|info|\bok\b|notice|valid-feedback/i.test(cls) && !n.matches('[role=alert]')) continue;
      if (skipEl(n)) continue;
      const t = ownText(n);
      if (!t || t.length > 200) continue;
      const loud = n.matches('[role=alert]') || /error|danger|invalid|fail/i.test(cls);
      if ((loud || PROBLEM_WORDS.test(t)) && !strong.includes(t)) strong.push(t);
    }
    // Plain text only answers a submit when it sits with the form (same landmark region) and is not an item in a
    // list or table: a results page that happens to quote "invalid certificate" is content, not a verdict.
    const LM = 'main,header,nav,footer,aside,dialog,[role=main],[role=banner],[role=navigation],[role=contentinfo],[role=complementary],[role=dialog]';
    const regions = [...document.forms].filter((f) => rendered(f)).map((f) => (f.parentElement && f.parentElement.closest(LM)) || document.body);
    const nearForm = (n) => !n.closest('ul,ol,li,table,dl,[role=list],[role=listitem],[role=row],[role=grid],article') && regions.some((r) => r.contains(n));
    let seen = 0;
    for (const n of document.querySelectorAll('p,li,span,div,td,small,strong,b,em,h4,h5,h6')) {
      if (weak.length >= 4 || ++seen > 4000) break;
      if (n.childElementCount > 2 || !PROBLEM_WORDS.test(n.textContent || '') || (n.textContent || '').length > 200) continue;
      if (skipEl(n) || !nearForm(n)) continue;
      const t = ownText(n);
      if (t && t.length <= 200 && PROBLEM_WORDS.test(t) && !strong.includes(t) && !weak.includes(t)) weak.push(t);
    }
    return { strong, weak };
  }

  window.__wm = {
    docId: () => DOC_ID, problems,
    snapshot, forms, field, validity, prepare, finish, setNative, apply, comboPick, pickDate, clickInfo, jsClick, rect, targets, mark, unmark,
    settle, textPresent, selectorPresent, challenge, confirmations,
    el: (ref) => elOf(ref),
    pending: pendingDeep,
    version: 1,
  };
})();
