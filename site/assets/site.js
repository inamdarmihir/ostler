/* Ostler site behaviour. Vanilla JS, no dependencies, no network except the
   same-origin search index (fetched lazily the first time search opens). */
(function () {
  'use strict';
  var doc = document, root = doc.documentElement;

  /* ---------- live region for announcements ---------- */
  var live = doc.createElement('div');
  live.className = 'visually-hidden';
  live.setAttribute('aria-live', 'polite');
  doc.body.appendChild(live);
  function announce(msg) { live.textContent = ''; setTimeout(function () { live.textContent = msg; }, 30); }

  /* ---------- theme ---------- */
  var toggle = doc.querySelector('.theme-toggle');
  function currentTheme() { return root.dataset.theme === 'dark' ? 'dark' : 'light'; }
  function syncToggle() {
    if (!toggle) return;
    var dark = currentTheme() === 'dark';
    toggle.setAttribute('aria-pressed', String(dark));
    toggle.setAttribute('aria-label', dark ? 'Dark theme on. Switch to light theme' : 'Light theme on. Switch to dark theme');
    toggle.title = dark ? 'Switch to light theme' : 'Switch to dark theme';
  }
  if (toggle) {
    syncToggle();
    toggle.addEventListener('click', function () {
      var next = currentTheme() === 'dark' ? 'light' : 'dark';
      root.dataset.theme = next;
      try { localStorage.setItem('ostler-theme', next); } catch (e) {}
      syncToggle();
      announce(next + ' theme');
    });
  }
  // Follow the OS when the visitor has not chosen
  if (window.matchMedia) {
    var mq = matchMedia('(prefers-color-scheme: dark)');
    var onChange = function (e) {
      var stored = null;
      try { stored = localStorage.getItem('ostler-theme'); } catch (err) {}
      if (stored !== 'light' && stored !== 'dark') { root.dataset.theme = e.matches ? 'dark' : 'light'; syncToggle(); }
    };
    if (mq.addEventListener) mq.addEventListener('change', onChange);
  }

  /* ---------- copy buttons ---------- */
  function copyText(text, btn, label) {
    var orig = btn.dataset.label || btn.textContent;
    btn.dataset.label = orig;
    function ok() { btn.textContent = 'Copied'; announce((label || 'Text') + ' copied'); setTimeout(function () { btn.textContent = orig; }, 1800); }
    function fail() { btn.textContent = 'Press Ctrl+C'; setTimeout(function () { btn.textContent = orig; }, 2500); }
    if (navigator.clipboard && window.isSecureContext !== false) {
      navigator.clipboard.writeText(text).then(ok, function () { legacy(text) ? ok() : fail(); });
    } else { legacy(text) ? ok() : fail(); }
  }
  function legacy(text) {
    var ta = doc.createElement('textarea');
    ta.value = text; ta.setAttribute('readonly', ''); ta.style.cssText = 'position:fixed;left:-9999px;top:0';
    doc.body.appendChild(ta); ta.select();
    var done = false;
    try { done = doc.execCommand('copy'); } catch (e) {}
    doc.body.removeChild(ta);
    return done;
  }
  doc.querySelectorAll('.code').forEach(function (block) {
    var pre = block.querySelector('pre');
    if (!pre) return;
    var b = doc.createElement('button');
    b.type = 'button'; b.className = 'copy-btn'; b.textContent = 'Copy';
    var lang = block.getAttribute('data-lang');
    b.setAttribute('aria-label', 'Copy ' + (lang ? lang + ' ' : '') + 'code block');
    b.addEventListener('click', function () { copyText(pre.innerText.replace(/\n$/, ''), b, 'Code'); });
    block.appendChild(b);
  });
  doc.querySelectorAll('[data-copy-from]').forEach(function (b) {
    b.addEventListener('click', function () {
      var el = doc.getElementById(b.getAttribute('data-copy-from'));
      if (el) copyText(el.value !== undefined && el.tagName === 'TEXTAREA' ? el.value : el.textContent, b, b.getAttribute('aria-label') || 'Text');
    });
  });
  doc.querySelectorAll('[data-copy-text]').forEach(function (b) {
    b.addEventListener('click', function () { copyText(b.getAttribute('data-copy-text'), b, b.getAttribute('aria-label')); });
  });

  /* ---------- tabs (landing page) ---------- */
  doc.querySelectorAll('[role="tablist"]').forEach(function (list) {
    var tabs = Array.prototype.slice.call(list.querySelectorAll('[role="tab"]'));
    function select(tab, focus) {
      tabs.forEach(function (t) {
        var on = t === tab;
        t.setAttribute('aria-selected', String(on));
        t.tabIndex = on ? 0 : -1;
        var panel = doc.getElementById(t.getAttribute('aria-controls'));
        if (panel) panel.hidden = !on;
      });
      if (focus) tab.focus();
    }
    tabs.forEach(function (t, i) {
      t.addEventListener('click', function () { select(t); });
      t.addEventListener('keydown', function (e) {
        var n = null;
        if (e.key === 'ArrowRight') n = tabs[(i + 1) % tabs.length];
        else if (e.key === 'ArrowLeft') n = tabs[(i - 1 + tabs.length) % tabs.length];
        else if (e.key === 'Home') n = tabs[0];
        else if (e.key === 'End') n = tabs[tabs.length - 1];
        if (n) { e.preventDefault(); select(n, true); }
      });
    });
  });

  /* ---------- mobile navigation drawer ---------- */
  var navBtn = doc.querySelector('.nav-toggle'), sidebar = doc.getElementById('sidebar'), scrim = doc.querySelector('[data-scrim]');
  function setNav(open) {
    if (!sidebar) return;
    doc.body.classList.toggle('nav-open', open);
    if (navBtn) { navBtn.setAttribute('aria-expanded', String(open)); navBtn.setAttribute('aria-label', open ? 'Close navigation' : 'Open navigation'); }
    if (scrim) scrim.hidden = !open;
    if (open) { var first = sidebar.querySelector('a'); if (first) first.focus(); } else if (navBtn && doc.activeElement && sidebar.contains(doc.activeElement)) navBtn.focus();
  }
  if (navBtn && sidebar) {
    navBtn.addEventListener('click', function () { setNav(!doc.body.classList.contains('nav-open')); });
    if (scrim) scrim.addEventListener('click', function () { setNav(false); });
    var closeBtn = sidebar.querySelector('.sidebar-close');
    if (closeBtn) closeBtn.addEventListener('click', function () { setNav(false); });
    sidebar.addEventListener('click', function (e) { if (e.target.closest('a')) setNav(false); });
    doc.addEventListener('keydown', function (e) { if (e.key === 'Escape' && doc.body.classList.contains('nav-open')) setNav(false); });
    if (window.matchMedia) matchMedia('(min-width: 901px)').addEventListener('change', function (e) { if (e.matches) setNav(false); });
  }
  // keep the current page visible in a long sidebar
  var curLink = sidebar && sidebar.querySelector('[aria-current="page"]');
  if (curLink && curLink.scrollIntoView) { try { curLink.scrollIntoView({ block: 'nearest' }); } catch (e) {} }

  /* ---------- "on this page" scroll spy ---------- */
  var tocLinks = Array.prototype.slice.call(doc.querySelectorAll('.toc a'));
  if (tocLinks.length && 'IntersectionObserver' in window) {
    var map = {};
    tocLinks.forEach(function (a) { map[a.getAttribute('href').slice(1)] = a; });
    var heads = Object.keys(map).map(function (id) { return doc.getElementById(id); }).filter(Boolean);
    var setActive = function (id) {
      tocLinks.forEach(function (a) { a.classList.remove('active'); a.removeAttribute('aria-current'); });
      if (map[id]) { map[id].classList.add('active'); map[id].setAttribute('aria-current', 'location'); }
    };
    var visible = {};
    var io = new IntersectionObserver(function (entries) {
      entries.forEach(function (en) { visible[en.target.id] = en.isIntersecting; });
      for (var i = 0; i < heads.length; i++) { if (visible[heads[i].id]) { setActive(heads[i].id); return; } }
    }, { rootMargin: '-70px 0px -70% 0px', threshold: 0 });
    heads.forEach(function (h) { io.observe(h); });
  }

  /* ---------- search ---------- */
  var dialog = doc.getElementById('search-dialog');
  if (dialog && typeof dialog.showModal === 'function') {
    var input = doc.getElementById('search-input'), results = doc.getElementById('search-results');
    var base = doc.body.getAttribute('data-search') || 'docs/search-index.json';
    var data = null, loading = null, sel = -1, hits = [];
    var STOP = { the: 1, a: 1, an: 1, of: 1, to: 1, and: 1, is: 1, in: 1, it: 1, for: 1, on: 1, or: 1 };

    function load() {
      if (data) return Promise.resolve(data);
      if (!loading) {
        loading = fetch(base, { credentials: 'omit' }).then(function (r) { if (!r.ok) throw new Error(r.status); return r.json(); })
          .then(function (j) {
            data = j.map(function (e) { return { e: e, p: e.p.toLowerCase(), s: e.s.toLowerCase(), t: e.t.toLowerCase() }; });
            return data;
          }).catch(function (err) { loading = null; throw err; });
      }
      return loading;
    }
    function terms(q) {
      return q.toLowerCase().split(/[^a-z0-9_\-.:]+/).filter(function (w) { return w && !STOP[w]; })
        .map(function (w) { return w.length > 5 ? w.replace(/(ing|ed|es|s)$/, '') : w; });
    }
    function score(d, ws, phrase) {
      var s = 0;
      for (var i = 0; i < ws.length; i++) {
        var w = ws[i], hit = false;
        if (d.s.indexOf(w) >= 0) { s += 12; hit = true; if (d.s === w) s += 8; else if (d.s.indexOf(w) === 0) s += 4; }
        if (d.p.indexOf(w) >= 0) { s += 6; hit = true; }
        var ti = d.t.indexOf(w);
        if (ti >= 0) { s += 3; hit = true; var n = 0, k = ti; while (k >= 0 && n < 5) { n++; k = d.t.indexOf(w, k + 1); } s += n * 0.4; }
        if (!hit) return 0;
      }
      if (ws.length > 1 && (d.s.indexOf(phrase) >= 0 || d.t.indexOf(phrase) >= 0)) s += 10;
      return s;
    }
    function escHtml(s) { return s.replace(/[&<>"]/g, function (c) { return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]; }); }
    function mark(text, ws) {
      var out = escHtml(text);
      ws.forEach(function (w) {
        if (w.length < 2) return;
        out = out.replace(new RegExp('(' + w.replace(/[.*+?^${}()|[\]\\]/g, '\\$&') + ')(?![^<]*>)', 'ig'), '<mark>$1</mark>');
      });
      return out;
    }
    function snippet(t, ws) {
      var low = t.toLowerCase(), pos = -1;
      for (var i = 0; i < ws.length; i++) { pos = low.indexOf(ws[i]); if (pos >= 0) break; }
      var start = Math.max(0, pos < 0 ? 0 : pos - 40), end = Math.min(t.length, start + 150);
      return (start > 0 ? '…' : '') + t.slice(start, end) + (end < t.length ? '…' : '');
    }
    function render(q) {
      var ws = terms(q);
      results.innerHTML = '';
      sel = -1; hits = [];
      input.setAttribute('aria-expanded', 'false');
      input.removeAttribute('aria-activedescendant');
      if (!ws.length) {
        results.innerHTML = '<p class="search-empty">Type to search every docs page: commands, flags, tool names, errors.</p>';
        status('');
        return;
      }
      var phrase = ws.join(' ');
      var best = {};
      data.forEach(function (d) {
        var s = score(d, ws, phrase);
        if (s > 0 && (!best[d.e.u] || s > best[d.e.u][0])) best[d.e.u] = [s, d.e];
      });
      var scored = Object.keys(best).map(function (k) { return best[k]; });
      scored.sort(function (a, b) { return b[0] - a[0]; });
      hits = scored.slice(0, 12).map(function (x) { return x[1]; });
      if (!hits.length) {
        results.innerHTML = '<p class="search-empty">No results for “' + escHtml(q) + '”.</p>';
        status('No results');
        return;
      }
      var frag = doc.createDocumentFragment();
      hits.forEach(function (h, i) {
        var a = doc.createElement('a');
        a.className = 'search-hit'; a.id = 'hit-' + i; a.setAttribute('role', 'option'); a.setAttribute('aria-selected', 'false');
        a.href = urlFor(h.u);
        a.innerHTML = '<span class="hit-path">' + escHtml(h.p) + '</span><span class="hit-title">' + mark(h.s || h.p, ws) + '</span><span class="hit-snip">' + mark(snippet(h.t, ws), ws) + '</span>';
        a.addEventListener('mousemove', function () { choose(i, false); });
        frag.appendChild(a);
      });
      results.appendChild(frag);
      input.setAttribute('aria-expanded', 'true');
      choose(0, false);
      status(hits.length + ' result' + (hits.length === 1 ? '' : 's'));
    }
    function status(msg) { var s = doc.getElementById('search-status'); if (s) s.textContent = msg; }
    function urlFor(u) {
      // index URLs are relative to the docs directory; the landing page lives one level up
      var inDocs = /\/docs\/[^/]*$/.test(location.pathname) || /\/docs\/?$/.test(location.pathname);
      return (inDocs ? '' : 'docs/') + u;
    }
    function choose(i, scroll) {
      var items = results.querySelectorAll('.search-hit');
      if (!items.length) return;
      sel = (i + items.length) % items.length;
      items.forEach(function (el, k) { el.setAttribute('aria-selected', String(k === sel)); });
      input.setAttribute('aria-activedescendant', items[sel].id);
      if (scroll !== false) items[sel].scrollIntoView({ block: 'nearest' });
    }
    function open() {
      if (!dialog.open) dialog.showModal();
      input.focus(); input.select();
      load().then(function () { render(input.value); }, function () {
        results.innerHTML = '<p class="search-empty">Search needs the page to be served over http(s). Browse the sidebar instead.</p>';
      });
    }
    doc.querySelectorAll('[data-search-open]').forEach(function (b) { b.addEventListener('click', open); });
    input.addEventListener('input', function () { if (data) render(input.value); });
    input.addEventListener('keydown', function (e) {
      if (e.key === 'ArrowDown') { e.preventDefault(); choose(sel + 1); }
      else if (e.key === 'ArrowUp') { e.preventDefault(); choose(sel - 1); }
      else if (e.key === 'Escape') { e.preventDefault(); dialog.close(); }
      else if (e.key === 'Enter') { var items = results.querySelectorAll('.search-hit'); if (items[sel]) { e.preventDefault(); location.href = items[sel].href; } }
    });
    results.addEventListener('click', function () { dialog.close(); });
    dialog.addEventListener('click', function (e) { if (e.target === dialog) dialog.close(); });
    doc.addEventListener('keydown', function (e) {
      var t = e.target, typing = t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable);
      if (((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k') || (e.key === '/' && !typing && !e.ctrlKey && !e.metaKey && !e.altKey)) {
        e.preventDefault(); open();
      }
    });
    // prefetch on intent
    doc.querySelectorAll('[data-search-open]').forEach(function (b) {
      ['pointerenter', 'focus'].forEach(function (ev) { b.addEventListener(ev, function () { load().catch(function () {}); }, { once: true }); });
    });
    // `?q=` deep link
    var qp = /[?&]q=([^&]+)/.exec(location.search);
    if (qp) { try { input.value = decodeURIComponent(qp[1].replace(/\+/g, ' ')); } catch (e) {} open(); }
  } else {
    doc.querySelectorAll('[data-search-open]').forEach(function (b) { b.hidden = true; });
  }
})();
