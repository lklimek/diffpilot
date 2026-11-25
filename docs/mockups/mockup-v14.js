/* diffpilot v1.4 mockups - menus, popovers, filter chips and small toggles. No network. Loads after mockup.js. */
(function () {
  'use strict';
  var $$ = function (s, r) { return Array.prototype.slice.call((r || document).querySelectorAll(s)); };
  var open = null;

  /* Menu buttons and popover triggers: [aria-controls] + [aria-haspopup]; Esc and outside click close. */
  function close(refocus) {
    if (!open) return;
    var t = open, p = document.getElementById(t.getAttribute('aria-controls'));
    t.setAttribute('aria-expanded', 'false'); if (p) p.hidden = true; open = null;
    if (refocus) t.focus();
  }
  function items(p) { return $$('[role^=menuitem]:not([aria-disabled=true])', p); }
  function show(t, focusFirst) {
    if (open && open !== t) close(false);
    var p = document.getElementById(t.getAttribute('aria-controls')); if (!p) return;
    t.setAttribute('aria-expanded', 'true'); p.hidden = false; open = t;
    if (focusFirst && t.getAttribute('aria-haspopup') === 'menu') { var f = items(p)[0]; if (f) f.focus(); }
  }
  document.addEventListener('click', function (e) {
    var t = e.target.closest('[aria-haspopup][aria-controls]');
    if (t) { e.preventDefault(); if (t.getAttribute('aria-expanded') === 'true') close(false); else show(t, e.detail === 0); return; }
    if (open && !e.target.closest('.menu,.pop')) close(false);
  });
  document.addEventListener('keydown', function (e) {
    if (!open) return;
    var p = document.getElementById(open.getAttribute('aria-controls'));
    if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); close(true); return; }
    if (!p || open.getAttribute('aria-haspopup') !== 'menu') return;
    var l = items(p), i = l.indexOf(document.activeElement);
    if (e.key === 'ArrowDown') { e.preventDefault(); (l[i + 1] || l[0]).focus(); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); (l[i - 1] || l[l.length - 1]).focus(); }
    else if (e.key === 'Home') { e.preventDefault(); l[0].focus(); }
    else if (e.key === 'End') { e.preventDefault(); l[l.length - 1].focus(); }
  }, true);

  /* Radio items set the trigger label; checkbox items toggle; plain items close. */
  document.addEventListener('click', function (e) {
    var it = e.target.closest('.menu [role^=menuitem]'); if (!it || it.getAttribute('aria-disabled') === 'true') return;
    var menu = it.closest('.menu'), role = it.getAttribute('role');
    if (role === 'menuitemradio') {
      $$('[role=menuitemradio]', menu).forEach(function (x) { x.setAttribute('aria-checked', x === it ? 'true' : 'false'); var c = x.querySelector('.ck'); if (c) c.textContent = x === it ? '✓' : ''; });
      var lbl = document.querySelector('[aria-controls="' + menu.id + '"] .val'); if (lbl && it.dataset.label) lbl.textContent = it.dataset.label;
    } else if (role === 'menuitemcheckbox') {
      var on = it.getAttribute('aria-checked') !== 'true'; it.setAttribute('aria-checked', on ? 'true' : 'false');
      var ck = it.querySelector('.ck'); if (ck) ck.textContent = on ? '✓' : '';
      if (it.dataset.on) it.querySelector('.k').firstChild.textContent = on ? it.dataset.on : it.dataset.off;
    }
    if (it.dataset.toast) toast(it.dataset.toast);
    close(true);
  });
  function toast(msg) { var t = document.getElementById('toast'); if (!t) return; t.textContent = msg; t.hidden = false; clearTimeout(toast.t); toast.t = setTimeout(function () { t.hidden = true; }, 3200); }

  /* Filter chips: [data-filter="field|value|label"] adds a chip; chip body flips include/exclude; x removes. */
  var bar = document.getElementById('chips');
  function renderCount() {
    var n = bar ? $$('.chip', bar).length : 0, c = document.getElementById('chip-count'), h = document.getElementById('chip-hint');
    if (c) c.hidden = n === 0; if (h) h.hidden = n > 0;
    var u = document.getElementById('chip-url');
    if (u) u.textContent = n ? '?f=' + $$('.chip', bar).map(function (x) { return (x.classList.contains('ex') ? '-' : '') + x.dataset.f; }).join(',') : '(no filter)';
  }
  function addChip(field, value, label, ex) {
    if (!bar) return;
    var key = field + ':' + value; if ($$('.chip', bar).some(function (x) { return x.dataset.f === key; })) { toast('Already filtered by ' + label + '.'); return; }
    var li = document.createElement('li'); li.className = 'chip' + (ex ? ' ex' : ''); li.dataset.f = key;
    li.innerHTML = '<button type="button" aria-pressed="' + (ex ? 'true' : 'false') + '" data-tip="Click to switch between include and exclude"><span class="sym">' + (ex ? '≠ ' : '') + '</span>' + field + ': <b></b></button><button type="button" aria-label="Remove filter ' + field + ' ' + label + '">×</button>';
    li.querySelector('b').textContent = label;
    bar.insertBefore(li, document.getElementById('chip-count')); renderCount();
  }
  document.addEventListener('click', function (e) {
    var f = e.target.closest('[data-filter]');
    if (f) { var p = f.dataset.filter.split('|'); addChip(p[0], p[1], p[2], e.altKey); return; }
    var chip = e.target.closest('.chip'); if (!chip) { if (e.target.id === 'chip-clear') { $$('.chip', bar).forEach(function (x) { x.remove(); }); renderCount(); } return; }
    var b = e.target.closest('button'); if (!b) return;
    if (b === chip.lastElementChild) { chip.remove(); renderCount(); return; }
    var ex = !chip.classList.contains('ex'); chip.classList.toggle('ex', ex); b.setAttribute('aria-pressed', ex ? 'true' : 'false');
    chip.querySelector('.sym').textContent = ex ? '≠ ' : ''; renderCount();
  });
  if (bar) bar.addEventListener('keydown', function (e) { if (e.key === 'Escape') { $$('.chip', bar).forEach(function (x) { x.remove(); }); renderCount(); } });
  function seed() { if (bar && bar.dataset.seed && !$$('.chip', bar).length) bar.dataset.seed.split(';').forEach(function (s) { var p = s.split('|'); addChip(p[0], p[1], p[2], p[3] === 'ex'); }); }
  renderCount();

  /* Judgement mark: outdated -> judging. Watch toggles. Settings sub-nav selects the mockup state. */
  document.addEventListener('click', function (e) {
    var j = e.target.closest('button.jm[data-rejudge]');
    if (j) { j.outerHTML = '<span class="jm run" data-tip="The LLM is judging this item again; the old text stays until it is done.">◔ judging…</span>'; toast('Re-judging ' + j.dataset.rejudge + ' (mock): 1 of today’s 120 items.'); return; }
    var w = e.target.closest('[data-watch]');
    if (w) { var on = w.getAttribute('aria-pressed') !== 'true'; w.setAttribute('aria-pressed', on); w.querySelector('.eye').textContent = on ? '◉' : '○'; w.querySelector('.wt').textContent = on ? 'Watching' : (w.dataset.off || 'Watch'); return; }
    var nc = e.target.closest('[data-navc]');
    if (nc) { var on2 = document.querySelector('.shell').classList.toggle('navc'); nc.setAttribute('aria-pressed', on2); return; }
    var x = e.target.closest('[data-hide]');
    if (x) { var bn = x.closest('[data-banner]'); bn.hidden = true; var nf = document.getElementById(x.dataset.hide); if (nf) nf.focus(); toast('Hidden until the assessment state changes (mock).'); return; }
    var tb = e.target.closest('[data-toast]');
    if (tb && !tb.closest('.menu')) { toast(tb.dataset.toast); return; }
    var s = e.target.closest('[data-tab]');
    if (s) { e.preventDefault(); var b = document.querySelector('.states [data-s="' + s.dataset.tab + '"]'); if (b) b.click(); }
  });
  var sel = document.getElementById('subsel');
  if (sel) sel.addEventListener('change', function () { var b = document.querySelector('.states [data-s="' + sel.value + '"]'); if (b) b.click(); });
  var states = document.querySelector('.states');
  if (states) states.addEventListener('click', function () {
    var cur = document.body.dataset.state;
    $$('nav.app .sub a[data-tab]').forEach(function (a) { if (a.dataset.tab === cur.split('-')[0]) a.setAttribute('aria-current', 'page'); else a.removeAttribute('aria-current'); });
    if (sel) sel.value = cur;
    $$('[data-banner]').forEach(function (b) { b.hidden = false; });
    if (bar && bar.dataset.seedOn === cur) seed();
    $$('[data-open-on]').forEach(function (t) { if (t.dataset.openOn.split(' ').indexOf(cur) >= 0) setTimeout(function () { show(t, false); }, 0); else if (open === t) close(false); });
  });
  setTimeout(function () { if (states) states.dispatchEvent(new Event('click')); }, 0);
}());
