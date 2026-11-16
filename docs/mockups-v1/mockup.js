/* smart-review mockups — shared behaviour. No network, no dependencies. */
(function () {
  'use strict';
  var $ = function (s, r) { return (r || document).querySelector(s); };
  var $$ = function (s, r) { return Array.prototype.slice.call((r || document).querySelectorAll(s)); };
  var B = document.body, D = B.dataset;
  var store = {
    get: function (k, d) { try { var v = sessionStorage.getItem(k); return v === null ? d : JSON.parse(v); } catch (e) { return d; } },
    set: function (k, v) { try { sessionStorage.setItem(k, JSON.stringify(v)); } catch (e) { /* ignore */ } }
  };
  var pending = store.get('pending', 3);

  /* ---------- shared chrome ---------- */
  function link(k, href, t) { return '<a href="' + href + '"' + (D.view === k ? ' aria-current="page"' : '') + '>' + t + '</a>'; }
  function buildChrome() {
    var pr = D.chrome !== 'min', head = D.head || '9f3c2e1', prog = D.progress || '17';
    var h = document.createElement('header');
    h.className = 'topbar';
    h.setAttribute('role', 'banner');
    h.innerHTML = '<a class="brand" href="01-home.html">smart<span>-</span>review</a>' +
      (pr ? '<div class="pr"><a class="sha-l" href="03-overview.html">acme/node-client</a><span class="muted">#482</span>' +
        '<strong title="feat(rpc): retry with backoff on timeout">feat(rpc): retry with backoff on timeout</strong>' +
        '<code class="sha-l" title="head commit">' + head + '</code>' +
        '<a class="b b-ci" href="03-overview.html#ci" title="1 failing: test (integration)">CI: 1 of 6 failing</a></div>' +
        '<nav class="views" aria-label="View">' + link('groups', '03-overview.html', 'Groups') + link('tour', '05-tour.html', 'Tour') + link('files', '06-files.html', 'Files') + '</nav>' +
        '<div class="prog" title="Risk-weighted progress: high-risk fragments count more"><span>Reviewed</span>' +
        '<span class="bar" role="progressbar" aria-label="Risk-weighted review progress" aria-valuemin="0" aria-valuemax="100" aria-valuenow="' + prog + '"><i id="prog-bar" style="width:' + prog + '%"></i></span><b id="prog-val">' + prog + '%</b></div>' +
        '<a class="pend" href="08-pending-review.html" aria-label="Pending review comments">Pending <b id="pend-n">' + pending + '</b></a>'
        : '<div class="pr"></div><a href="09-settings.html" class="muted">Settings</a>') +
      '<button class="iconbtn" id="help-btn" aria-label="Keyboard shortcuts" title="Keyboard shortcuts (?)">?</button>' +
      '<a class="muted" href="09-settings.html" aria-label="User menu: alice, settings" title="Signed in as alice">alice &#9662;</a>';
    var skip = document.createElement('a');
    skip.className = 'skip'; skip.href = '#main'; skip.textContent = 'Skip to content';
    B.insertBefore(h, B.firstChild);
    B.insertBefore(skip, B.firstChild);
    var f = document.createElement('footer');
    f.className = 'mock-foot';
    f.textContent = 'Mockup — static example data';
    B.appendChild(f);
    var dlg = document.createElement('dialog');
    dlg.id = 'help'; dlg.setAttribute('aria-labelledby', 'help-t');
    dlg.innerHTML = '<h2 id="help-t">Keyboard shortcuts</h2><table><tbody>' +
      '<tr><td><kbd>j</kbd> <kbd>k</kbd></td><td>next / previous fragment (tour: step)</td></tr>' +
      '<tr><td><kbd>n</kbd> <kbd>p</kbd></td><td>next / previous group</td></tr>' +
      '<tr><td><kbd>x</kbd></td><td>mark fragment reviewed</td></tr>' +
      '<tr><td><kbd>d</kbd></td><td>show / hide old code</td></tr>' +
      '<tr><td><kbd>c</kbd></td><td>comment on pinned annotation</td></tr>' +
      '<tr><td><kbd>a</kbd></td><td>ask / deepen</td></tr>' +
      '<tr><td><kbd>?</kbd></td><td>this help</td></tr><tr><td><kbd>Esc</kbd></td><td>close</td></tr></tbody></table>' +
      '<p class="muted small">Shortcuts are active on the group workspace; j/k also on the tour.</p>' +
      '<form method="dialog"><button class="btn pri">Close</button></form>';
    B.appendChild(dlg);
    var t = document.createElement('div');
    t.id = 'toast'; t.setAttribute('role', 'status'); t.hidden = true;
    B.appendChild(t);
    $('#help-btn').addEventListener('click', function () { dlg.showModal(); });
  }
  function toast(msg) {
    var t = $('#toast'); t.textContent = msg; t.hidden = false;
    clearTimeout(toast.t); toast.t = setTimeout(function () { t.hidden = true; }, 2600);
  }
  function setPending(n) { pending = n; store.set('pending', n); var e = $('#pend-n'); if (e) e.textContent = n; }
  function setProgress(v) {
    var b = $('#prog-bar'); if (b) { b.style.width = v + '%'; b.parentNode.setAttribute('aria-valuenow', v); }
    var t = $('#prog-val'); if (t) t.textContent = v + '%';
  }

  /* ---------- "mockup states" switcher ---------- */
  function states() {
    var bar = $('.states'); if (!bar) return;
    var btns = $$('[data-s]', bar);
    function set(s) {
      B.dataset.state = s;
      btns.forEach(function (b) { b.setAttribute('aria-pressed', b.dataset.s === s ? 'true' : 'false'); });
      $$('[data-for]').forEach(function (e) { e.hidden = e.dataset.for.split(' ').indexOf(s) < 0; });
    }
    btns.forEach(function (b) { b.addEventListener('click', function () { set(b.dataset.s); }); });
    var q = /[?&]s=([\w-]+)/.exec(location.search);
    set(q && btns.some(function (b) { return b.dataset.s === q[1]; }) ? q[1] : btns[0].dataset.s);
  }

  /* ---------- group workspace ---------- */
  function workspace() {
    var tip = document.createElement('div');
    tip.id = 'tip'; tip.setAttribute('role', 'tooltip'); tip.hidden = true; B.appendChild(tip);
    var panels = $$('.insp-panel'), cur = null, curFrag = null;
    function info(el) { var p = $('#p-' + el.dataset.a); return p ? p.dataset : (el.dataset || {}); }
    function pin(id, scroll) {
      var p = $('#p-' + id); if (!p) return;
      panels.forEach(function (x) { x.hidden = x !== p; });
      $$('[data-a]').forEach(function (h) { h.classList.toggle('pinned', h.dataset.a === id); });
      $('#pin-title').textContent = p.dataset.title;
      $('#cmt-loc').value = p.dataset.loc;
      $('#insp-scroll').scrollTop = 0;
      $$('[data-fb]').forEach(function (b) { b.setAttribute('aria-pressed', 'false'); });
      $('#fb-note').hidden = true;
      cur = id;
      if (scroll) { var t = $('[data-a="' + id + '"]'); if (t) t.scrollIntoView({ block: 'center' }); }
    }
    function showTip(e) {
      var el = e.target.closest('[data-a],[data-tip]'); if (!el || (el.dataset.a && !$('#p-' + el.dataset.a))) return;
      var d = info(el);
      tip.innerHTML = '';
      tip.appendChild(document.createTextNode(el.dataset.tip || d.tip));
      var b = document.createElement('b'); b.textContent = (el.dataset.risk || d.risk) + ' · click to pin'; tip.appendChild(b);
      tip.hidden = false;
      var r = el.getBoundingClientRect(), x = e.clientX ? Math.min(e.clientX + 12, innerWidth - 320) : r.left + 40;
      tip.style.left = x + 'px';
      tip.style.top = Math.min((e.clientY ? e.clientY : r.bottom) + 16, innerHeight - 80) + 'px';
    }
    document.addEventListener('mouseover', showTip);
    document.addEventListener('focusin', showTip);
    document.addEventListener('mouseout', function (e) { if (e.target.closest('[data-a],[data-tip]')) tip.hidden = true; });
    document.addEventListener('focusout', function () { tip.hidden = true; });
    document.addEventListener('click', function (e) {
      var h = e.target.closest('[data-a]');
      if (h && !e.target.closest('button,a,summary,input,textarea')) pin(h.dataset.a);
    });
    document.addEventListener('keydown', function (e) {
      var h = e.target.closest && e.target.closest('[data-a]');
      if (h && (e.key === 'Enter' || e.key === ' ') && e.target === h) { e.preventDefault(); pin(h.dataset.a); }
    });
    $('#unpin') && $('#unpin').addEventListener('click', function () { pin('a1'); toast('Mockup: inspector reset to the first annotation'); });

    /* progress */
    function recount() {
      var tot = 0, done = 0;
      $$('.frag[data-w]').forEach(function (f) { var w = +f.dataset.w; tot += w; if (f.classList.contains('reviewed')) done += w; });
      $$('.grp').forEach(function (g) {
        var fs = $$('.frag[data-w]', g), d = fs.filter(function (f) { return f.classList.contains('reviewed'); }).length;
        var e = $('[data-gp]', g); if (e) e.textContent = d + ' of ' + fs.length + ' fragments reviewed';
        var nav = $('.glist [href="#' + g.id + '"] .tick'); if (nav) nav.hidden = !(fs.length && d === fs.length);
      });
      setProgress(Math.round(100 * done / tot));
    }
    function setRev(f, on) {
      f.classList.toggle('reviewed', on);
      var b = $('[data-act="rev"]', f);
      if (b) { b.setAttribute('aria-pressed', on); b.firstChild.textContent = on ? 'Reviewed ✓ (undo) ' : 'Mark reviewed '; }
    }
    function toggleOld(f) {
      var o = $('.old', f), b = $('[data-act="old"]', f); if (!o) return;
      o.hidden = !o.hidden; b.setAttribute('aria-pressed', !o.hidden);
      b.firstChild.textContent = o.hidden ? 'Show old code ' : 'Hide old code ';
    }
    document.addEventListener('click', function (e) {
      var b = e.target.closest('[data-act]'); if (!b) return;
      var f = b.closest('.frag'), a = b.dataset.act;
      if (a === 'rev') { setRev(f, !f.classList.contains('reviewed')); recount(); }
      if (a === 'old') toggleOld(f);
      if (a === 'col') { var c = f.classList.toggle('collapsed'); b.setAttribute('aria-expanded', !c); b.textContent = c ? '▸' : '▾'; }
      if (a === 'grp') { var g = b.closest('.grp'); $$('.frag[data-w]', g).forEach(function (x) { setRev(x, true); }); recount(); }
      if (a === 'unfold') { var n = b.nextElementSibling; if (n) { n.hidden = false; b.hidden = true; } }
      if (a === 'mvtoggle') { var bd = $('.frag-body', f); bd.hidden = !bd.hidden; b.setAttribute('aria-expanded', !bd.hidden); b.textContent = bd.hidden ? 'Show code' : 'Hide code'; }
    });

    /* feedback, ask, composer */
    $$('[data-fb]').forEach(function (b) {
      b.addEventListener('click', function () {
        $$('[data-fb]').forEach(function (x) { x.setAttribute('aria-pressed', x === b ? 'true' : 'false'); });
        $('#fb-note').hidden = b.dataset.fb === 'accurate';
        toast('Feedback "' + b.dataset.fb + '" saved locally (mock) and queued for MemCan');
      });
    });
    var canned = {
      callers: 'Found 3 callers, all outside the diff: wallet::send_payment, sync::fetch_headers, mempool::broadcast_tx.',
      coverage: 'No test covers a request delivered but timed out. tests/rpc_retry.rs exercises 2 of 4 retry paths.',
      verify: 'Suspicion holds: mempool::broadcast_tx reaches send() with no idempotency check; node-side dedup is not visible in this repo. Confidence 0.70 → 0.75.'
    };
    function addQA(q, a) {
      var li = document.createElement('div'); li.className = 'qa';
      li.innerHTML = '<div class="q"></div><div class="a muted">Working… <span class="spin"></span></div>';
      $('.q', li).textContent = q;
      $('#ask-log').insertBefore(li, $('#ask-log').firstChild);
      setTimeout(function () { $('.a', li).innerHTML = ''; $('.a', li).appendChild(document.createTextNode(a + ' ')); var s = document.createElement('span'); s.className = 'b b-llm'; s.textContent = 'LLM-claimed'; $('.a', li).appendChild(s); $('.a', li).classList.remove('muted'); }, 900);
    }
    $$('[data-ask]').forEach(function (b) { b.addEventListener('click', function () { addQA(b.textContent, canned[b.dataset.ask]); }); });
    $('#ask-form').addEventListener('submit', function (e) {
      e.preventDefault(); var i = $('#ask-input'); if (!i.value.trim()) return;
      addQA(i.value.trim(), 'Mock answer: the agent would read the checkout and reply here, pinned to this annotation.'); i.value = '';
    });
    $('#cmt-form').addEventListener('submit', function (e) {
      e.preventDefault();
      var t = $('#cmt-text'), err = $('#cmt-err');
      if (!t.value.trim()) { err.hidden = false; t.setAttribute('aria-invalid', 'true'); t.focus(); return; }
      err.hidden = true; t.removeAttribute('aria-invalid');
      setPending(pending + 1); t.value = '';
      toast('Added to pending review (' + pending + ' drafts). Publish it on GitHub.');
    });

    /* keyboard */
    function frags() { return $$('.frag'); }
    function goFrag(d) {
      var fs = frags(), i = fs.indexOf(curFrag);
      i = Math.max(0, Math.min(fs.length - 1, i + d));
      if (curFrag) curFrag.classList.remove('cur');
      curFrag = fs[i]; curFrag.classList.add('cur'); curFrag.scrollIntoView({ block: 'start', behavior: 'smooth' });
      var a = $('[data-a]', curFrag); if (a) pin(a.dataset.a);
    }
    function goGroup(d) {
      var gs = $$('.grp'), y = $('.ws-main').scrollTop, i = 0;
      gs.forEach(function (g, k) { if (g.offsetTop - $('.ws-main').offsetTop <= y + 40) i = k; });
      i = Math.max(0, Math.min(gs.length - 1, i + d)); location.hash = '#' + gs[i].id;
    }
    document.addEventListener('keydown', function (e) {
      if (/input|textarea|select/i.test(e.target.tagName) || e.ctrlKey || e.metaKey || e.altKey) return;
      var k = e.key;
      if (k === 'j') goFrag(1); else if (k === 'k') goFrag(-1);
      else if (k === 'n') goGroup(1); else if (k === 'p') goGroup(-1);
      else if (k === 'x' && curFrag) { setRev(curFrag, !curFrag.classList.contains('reviewed')); recount(); }
      else if (k === 'd' && curFrag) toggleOld(curFrag);
      else if (k === 'c') { e.preventDefault(); $('#cmt-text').focus(); }
      else if (k === 'a') { e.preventDefault(); $('#ask-input').focus(); }
    });

    /* hash deep links: #h-a1 (annotation), #g3 (group) */
    function fromHash() {
      var h = location.hash;
      if (/^#h-/.test(h)) { pin(h.slice(3), true); }
      else if (/^#g\d/.test(h)) { var g = $(h); var a = g && $('[data-a]', g); if (a) pin(a.dataset.a); }
    }
    window.addEventListener('hashchange', fromHash);
    $$('.frag.reviewed').forEach(function (f) { setRev(f, true); });
    pin('a1'); recount(); fromHash();
  }

  /* ---------- tour ---------- */
  function tour() {
    var steps = $$('.step'), idx = 0;
    var inc = $('#inc-noise');
    function vis() { return steps.filter(function (s) { return !s.dataset.noise || inc.checked; }); }
    function render() {
      var v = vis(); idx = Math.max(0, Math.min(v.length - 1, idx));
      steps.forEach(function (s) { s.hidden = s !== v[idx]; });
      $$('#steps li').forEach(function (li) {
        var s = $('#' + li.dataset.step); li.hidden = !v.includes(s);
        li.classList.toggle('cur', s === v[idx]);
        if (s === v[idx]) $('a', li).setAttribute('aria-current', 'step'); else $('a', li).removeAttribute('aria-current');
      });
      $('#step-k').textContent = 'Step ' + (idx + 1) + ' of ' + v.length;
      $('#prev').disabled = idx === 0; $('#next').disabled = idx === v.length - 1;
      $('#skipped').hidden = inc.checked;
    }
    $('#next').addEventListener('click', function () { idx++; render(); });
    $('#prev').addEventListener('click', function () { idx--; render(); });
    inc.addEventListener('change', render);
    $$('#steps a').forEach(function (a) {
      a.addEventListener('click', function (e) { e.preventDefault(); var s = $(a.getAttribute('href')); idx = vis().indexOf(s); render(); });
    });
    $$('[data-act="rev"]').forEach(function (b) {
      b.addEventListener('click', function () { var on = b.getAttribute('aria-pressed') !== 'true'; b.setAttribute('aria-pressed', on); b.firstChild.textContent = on ? 'Reviewed ✓ (undo) ' : 'Mark reviewed '; });
    });
    document.addEventListener('keydown', function (e) {
      if (/input|textarea|select/i.test(e.target.tagName)) return;
      if (e.key === 'j' || e.key === 'ArrowRight') { idx++; render(); } else if (e.key === 'k' || e.key === 'ArrowLeft') { idx--; render(); }
    });
    render();
  }

  /* ---------- files ---------- */
  function files() {
    var bs = $$('.ftree button[data-file]');
    function open(id) {
      bs.forEach(function (b) { b.setAttribute('aria-current', b.dataset.file === id ? 'true' : 'false'); });
      $$('[data-pane]').forEach(function (p) { p.hidden = p.dataset.pane !== id; });
    }
    bs.forEach(function (b) { b.addEventListener('click', function () { open(b.dataset.file); }); });
    $$('.fold[data-unfold]').forEach(function (f) { f.addEventListener('click', function () { var n = f.nextElementSibling; n.hidden = !n.hidden; f.hidden = true; }); });
    var tip = document.createElement('div'); tip.id = 'tip'; tip.setAttribute('role', 'tooltip'); tip.hidden = true; B.appendChild(tip);
    document.addEventListener('mouseover', function (e) {
      var el = e.target.closest('[data-tip]'); if (!el) return;
      tip.textContent = el.dataset.tip; var b = document.createElement('b'); b.textContent = el.dataset.risk; tip.appendChild(b);
      tip.hidden = false; tip.style.left = Math.min(e.clientX + 12, innerWidth - 320) + 'px'; tip.style.top = (e.clientY + 16) + 'px';
    });
    document.addEventListener('mouseout', function (e) { if (e.target.closest('[data-tip]')) tip.hidden = true; });
    open(bs[0].dataset.file);
  }

  /* ---------- pending review ---------- */
  function pendingPage() {
    function refresh() {
      var n = $$('.draft').length; setPending(n);
      var c = $('#draft-n'); if (c) c.textContent = n;
      if (n === 0 && B.dataset.state === 'drafts') $('#empty-msg').hidden = false;
      $('#push').disabled = n === 0;
    }
    document.addEventListener('click', function (e) {
      var b = e.target.closest('[data-d]'); if (!b) return;
      var d = b.closest('.draft');
      if (b.dataset.d === 'del') { d.remove(); refresh(); toast('Draft deleted'); }
      if (b.dataset.d === 'edit') {
        var p = $('.txt', d), ta = $('textarea', d), ed = ta.hidden;
        ta.hidden = !ed; p.hidden = ed;
        if (ed) ta.focus(); else p.textContent = ta.value;
        b.textContent = ed ? 'Save' : 'Edit';
      }
    });
    $('#push').addEventListener('click', function () { var s = $$('.states [data-s]'); s.forEach(function (x) { if (x.dataset.s === 'pushed') x.click(); }); });
    refresh();
  }

  /* ---------- home ---------- */
  function home() {
    var f = $('#analyze-form');
    f.addEventListener('submit', function (e) {
      e.preventDefault();
      var v = $('#pr-url').value.trim();
      var ok = /^https:\/\/github\.com\/[\w.-]+\/[\w.-]+\/pull\/\d+/.test(v);
      var s = $$('.states [data-s]');
      if (!v || !ok) { s.forEach(function (x) { if (x.dataset.s === 'error') x.click(); }); $('#pr-url').focus(); return; }
      s.forEach(function (x) { if (x.dataset.s === 'loading') x.click(); });
      setTimeout(function () { location.href = '02-analysis-progress.html'; }, 1200);
    });
  }

  /* ---------- settings ---------- */
  function settings() {
    var sel = $('#lang');
    sel.addEventListener('change', function () { $('#lang-en').hidden = sel.value !== 'en'; $('#lang-pl').hidden = sel.value !== 'pl'; });
  }

  buildChrome();
  states();
  var pg = D.page;
  if (pg === 'workspace') workspace();
  if (pg === 'tour') tour();
  if (pg === 'files') files();
  if (pg === 'pending') pendingPage();
  if (pg === 'home') home();
  if (pg === 'settings') settings();
  document.addEventListener('keydown', function (e) {
    if (/input|textarea|select/i.test(e.target.tagName)) return;
    var dlg = $('#help');
    if (e.key === '?') { if (dlg.open) dlg.close(); else dlg.showModal(); }
  });
}());
