/* diffpilot mockups v2 - shared behaviour. No network, no dependencies. */
(function () {
  'use strict';
  var $ = function (s, r) { return (r || document).querySelector(s); };
  var $$ = function (s, r) { return Array.prototype.slice.call((r || document).querySelectorAll(s)); };
  var B = document.body, D = B.dataset, GH = 'https://github.com/acme/node-client', SHA = '9f3c2e1';
  function store(kind) {
    return {
      get: function (k, d) { try { var v = window[kind].getItem(k); return v === null ? d : JSON.parse(v); } catch (e) { return d; } },
      set: function (k, v) { try { window[kind].setItem(k, JSON.stringify(v)); } catch (e) { /* storage unavailable */ } }
    };
  }
  var ls = store('localStorage'), ss = store('sessionStorage');
  function esc(s) { return String(s).replace(/[&<>"]/g, function (c) { return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]; }); }

  /* ---------- tooltips: one mechanism, [data-tip], hover and keyboard focus ---------- */
  var TIP = {
    HIGH: 'Risk HIGH at system scale: a production user or operator would clearly notice. LLM rating; the reason is under Assessment.',
    MEDIUM: 'Risk MEDIUM at system scale: noticeable in some situations or by some users. LLM rating; the reason is under Assessment.',
    LOW: 'Risk LOW at system scale: small or contained effect. LLM rating; the reason is still given under Assessment.',
    added: 'Added: the code is new in this PR (taken from the diff).', changed: 'Changed: lines were modified in this PR (taken from the diff).',
    unchanged: 'Unchanged code, shown only because it connects the changed code.', removed: 'Removed: code deleted by this PR (taken from the diff).',
    moved: 'Moved: a block that exists elsewhere in the base version (text comparison).',
    q: 'A suspicion phrased as a question for you to check. It is not a verdict.',
    valid: 'The LLM judges the thread still applies to the current code.', stale: 'The LLM judges the comment refers to code that no longer exists here.',
    addressed: 'The LLM judges the thread was already fixed in the current code.',
    ident: 'Deterministic check: the block text is identical after whitespace normalisation.',
    missed: 'Also saved as feedback: the LLM had no remark on this line.', llma: 'An answer written by the LLM. Not a deterministic check.',
    ood: 'The code is not part of the diff; the LLM says the change affects it.', diffb: 'Deterministic text comparison with the base version.'
  };
  function bd(cls, text, tip) { return '<span class="b ' + cls + '" data-tip="' + esc(TIP[tip] || tip) + '" tabindex="0">' + text + '</span>'; }
  var RC = { HIGH: 'b-h', MEDIUM: 'b-m', LOW: 'b-l' };
  function rb(level) { return bd(RC[level], level, level); }
  var tipEl = $('#tip'), tipFor = null;
  function tipText(el) {
    if (el.dataset.a) { var d = INSP[el.dataset.a]; return d && D.state !== 'noq' ? esc(d.tip) + '<small>Click to open in the inspector</small>' : null; }
    if (el.classList.contains('n')) return 'Comment on this line<small>adds to your GitHub pending review</small>';
    return el.dataset.tip ? esc(el.dataset.tip) : null;
  }
  function showTip(e) {
    var el = e.target.closest && e.target.closest('[data-tip],[data-a],.code button.n'); if (!el || !tipEl) return;
    if (el.classList.contains('n') && D.page !== 'workspace') return;
    var t = tipText(el); if (!t) return;
    tipEl.innerHTML = t; tipEl.hidden = false; tipFor = el; el.setAttribute('aria-describedby', 'tip');
    var r = el.getBoundingClientRect(), mouse = e.type === 'mouseover' && e.clientX;
    var x = mouse ? e.clientX + 12 : r.left, y = mouse ? e.clientY + 18 : r.bottom + 6;
    tipEl.style.left = Math.max(4, Math.min(x, innerWidth - 336)) + 'px';
    tipEl.style.top = Math.min(y, innerHeight - tipEl.offsetHeight - 6) + 'px';
  }
  function hideTip() { if (tipEl) tipEl.hidden = true; if (tipFor) { tipFor.removeAttribute('aria-describedby'); tipFor = null; } }
  function initTips(root) {
    $$('[data-tip]', root).forEach(function (el) { if (!el.matches('a,button,input,select,textarea,[tabindex]')) el.tabIndex = 0; });
  }
  document.addEventListener('mouseover', showTip);
  document.addEventListener('focusin', showTip);
  document.addEventListener('mouseout', function (e) { if (e.target.closest && e.target.closest('[data-tip],[data-a],.code button.n')) hideTip(); });
  document.addEventListener('focusout', hideTip);
  document.addEventListener('keydown', function (e) { if (e.key === 'Escape') hideTip(); });
  function toast(msg) {
    var t = $('#toast'); t.textContent = msg; t.hidden = false;
    clearTimeout(toast.t); toast.t = setTimeout(function () { t.hidden = true; }, 3200);
  }

  /* ---------- collapsible sections, state kept in localStorage ---------- */
  function secState(k) { var s = ls.get('sr2.sec', {}); return k in s ? s[k] : (k === 'assess' || k === 'qs' ? false : true); }
  function applySecs(root) { $$('details[data-sec]', root).forEach(function (d) { d.open = secState(d.dataset.sec); }); }
  document.addEventListener('toggle', function (e) {
    var d = e.target; if (!d.dataset || !d.dataset.sec) return;
    var s = ls.get('sr2.sec', {}); s[d.dataset.sec] = d.open; ls.set('sr2.sec', s);
  }, true);

  /* ---------- counters shared across pages ---------- */
  function setK(k) { var e = $('#gcount'); if (e) e.textContent = k; ss.set('sr2.k', k); }
  function setPend(n) { var e = $('#pend-n'); if (e) e.textContent = n; $$('.pcn').forEach(function (c) { c.textContent = n; }); ss.set('sr2.pend', n); }
  setK(ss.get('sr2.k', 1)); setPend(D.page === 'workspace' ? $$('#pc-list li').length : ss.get('sr2.pend', 2));

  /* ---------- help ---------- */
  var help = $('#help');
  var hb = $('#help-btn'); if (hb) hb.addEventListener('click', function () { help.showModal(); });
  document.addEventListener('keydown', function (e) {
    if (/input|textarea|select/i.test(e.target.tagName) || e.ctrlKey || e.metaKey || e.altKey) return;
    if (e.key === '?') { if (help.open) help.close(); else help.showModal(); }
  });

  /* ---------- states switcher ---------- */
  var stateHook = function () {};
  function states() {
    var bar = $('.states'); if (!bar) return;
    var btns = $$('[data-s]', bar);
    function set(s) {
      B.dataset.state = s;
      btns.forEach(function (b) { b.setAttribute('aria-pressed', b.dataset.s === s ? 'true' : 'false'); });
      $$('[data-for]').forEach(function (e) { e.hidden = e.dataset.for.split(' ').indexOf(s) < 0; });
      stateHook(s);
    }
    btns.forEach(function (b) { b.addEventListener('click', function () { set(b.dataset.s); }); });
    var q = /[?&]s=([\w-]+)/.exec(location.search);
    return function () { set(q && btns.some(function (b) { return b.dataset.s === q[1]; }) ? q[1] : btns[0].dataset.s); };
  }

  /* ---------- inspector data ---------- */
  function CH(f, l, n) { return [f, l, n]; }
  var INSP = {
    '1-1': { t: 'Trigger: callers of send', file: 'src/mempool/mod.rs', a: 126, z: 132, kind: 'unchanged',
      tip: 'Was: a timeout on this call came back once as Timeout. Is: the same call may be sent up to 3 times.',
      by: 'a user action in the wallet or the relay (outside the diff)', dec: 'Nothing here; it builds the request and calls <code>client.send</code>.', next: 'step 2',
      why: 'Shows who reaches the retry loop, in particular a call that is not safe to repeat.',
      was: 'One call, one attempt.', is: 'One call, up to 3 attempts.', diff: null,
      conseq: ['<code>broadcast_tx</code> now reaches a retrying <code>send</code>.'],
      callers: [['wallet::send_payment', 'src/wallet/pay.rs', 57], ['mempool::broadcast_tx', 'src/mempool/mod.rs', 130]], callees: [['RpcClient::send', 'src/rpc/client.rs', 90]],
      qs: ['Does the node reject a repeated <code>broadcast_tx</code> for the same transaction, so that a second send is harmless? Not visible in this repository.'], as: null, th: [] },
    '1-2': { t: 'Attempt: build the request and send once', file: 'src/rpc/client.rs', a: 88, z: 96, kind: 'changed',
      tip: 'Was: one request built and sent once. Is: built and sent once per loop turn, with an attempt counter.',
      by: 'step 1', dec: 'Counts the attempt; rebuilds the request on every turn.', next: 'step 3',
      why: 'The counter feeds the retry decision; the request is rebuilt because <code>execute</code> consumes it.',
      was: 'Built the request and sent it once.', is: 'Builds and sends once per loop turn; the turn count is kept in <code>attempt</code>.',
      diff: [['@', '', '@@ -52,5 +88,9 @@ impl RpcClient'], [' ', 52, '    pub async fn send(&self, req: Request) -> Result<Response, RpcError> {'], ['-', 53, '        let http = self.build_request(&req)?;'], ['-', 54, '        let resp = self.transport.execute(http, self.config.timeout).await?;'], ['+', 91, '        let policy = &self.retry;'], ['+', 92, '        let mut attempt: u32 = 0;'], ['+', 93, '        loop {'], ['+', 94, '            attempt += 1;'], ['+', 95, '            let http = self.build_request(&req)?;'], ['+', 96, '            let timeout = self.config.timeout;']],
      conseq: ['Every attempt gets the full per-attempt timeout, so the worst case is 3 &times; timeout plus delays.'],
      callers: [['send callers', 'src/wallet/pay.rs', 57]], callees: [['RpcClient::build_request', 'src/rpc/client.rs', 112], ['Transport::execute', 'src/rpc/transport.rs', 40]],
      qs: [], as: null, th: [] },
    '1-3': { t: 'Outcome of the attempt?', file: 'src/rpc/client.rs', a: 97, z: 97, kind: 'changed',
      tip: 'Was: any error was returned as it came. Is: the result is matched; only Timeout can lead to another attempt.',
      by: 'step 2', dec: 'Branches on the result: Ok, Timeout, other error.', next: 'step 4 (Ok), step 5 (Timeout), step 8 (other error)',
      why: 'Only timeouts are retried; every other error keeps the old behaviour.',
      was: '<code>execute(...).await?</code> returned every error to the caller.', is: '<code>match</code> on the result with three outcomes.',
      diff: [['@', '', '@@ -54,1 +97,1 @@'], ['-', 54, '        let resp = self.transport.execute(http, self.config.timeout).await?;'], ['+', 97, '            match self.transport.execute(http, timeout).await {']],
      conseq: [], callers: [], callees: [['Transport::execute', 'src/rpc/transport.rs', 40]], qs: [], as: null, th: [] },
    '1-4': { t: 'Return the response', file: 'src/rpc/client.rs', a: 98, z: 98, kind: 'changed',
      tip: 'Was: the response was returned after one attempt. Is: the response is returned from inside the loop.',
      by: 'step 3 (Ok)', dec: 'Nothing; ends the call.', next: 'back to the caller', why: 'A successful attempt ends the loop at once.',
      was: 'Returned the response.', is: 'Returns the response from any attempt.', diff: [['@', '', '@@ -54,1 +98,1 @@'], ['-', 54, '        let resp = self.transport.execute(http, self.config.timeout).await?;'], ['+', 98, '                Ok(resp) => return Ok(resp),']], conseq: [], callers: [], callees: [], qs: [], as: null, th: [] },
    '1-5': { t: 'Retry allowed?', file: 'src/rpc/client.rs', a: 99, z: 99, kind: 'added',
      tip: 'Was: a timeout was returned to the caller. Is: a timeout is retried while attempt < 3, for every method.',
      by: 'step 3 (Timeout)', dec: 'Asks <code>RetryPolicy::should_retry(attempt)</code>; yes goes to step 6, no goes to step 7.', next: 'step 6 or step 7',
      why: 'The loop needs a rule for when to stop; the rule is a plain attempt limit.',
      was: 'No retry decision; the timeout went straight to the caller.', is: 'Retries while <code>attempt &lt; max_attempts</code> (3). The method of the request is not an input.',
      diff: [['@', '', '@@ -0,0 +99,1 @@'], ['+', 99, '                Err(RpcError::Timeout) if policy.should_retry(attempt) => {'], ['@', '', '@@ -0,0 +21,5 @@ src/rpc/retry.rs (new file)'], ['+', 23, '    pub fn should_retry(&self, attempt: u32) -> bool {'], ['+', 24, '        attempt < self.max_attempts'], ['+', 25, '    }']],
      conseq: ['<code>mempool::broadcast_tx</code> may submit the same transaction twice if attempt 1 reached the node and only the reply was late.', 'Callers that cannot tolerate a repeat have no way to opt out; <code>send_once()</code> is removed (flow 4).'],
      callers: [['RpcClient::send', 'src/rpc/client.rs', 99]], callees: [],
      qs: ['Can <code>broadcast_tx</code> be sent twice when the reply arrives late? <code>should_retry</code> receives only the attempt number; no method or idempotency input reaches it.'],
      as: { notice: 'A wallet user could see one payment broadcast twice after a slow node reply. An operator would see duplicate-transaction rejections in the node log.', risk: 'HIGH',
        reason: 'A repeated value-moving request at system scale (wallet and relay both call it). It needs a reply delayed past 5 s, which flow 2 makes more likely.',
        basis: 'Read: 4 files (client.rs, retry.rs, mempool/mod.rs, wallet/pay.rs). Not checked: node-side deduplication (not in this repository).',
        orig: 'introduced by this PR', blocks: 'blocks this PR', fix: 'Pass an idempotency input into <code>should_retry</code> (or skip retry for <code>broadcast_tx</code>); add a test with a transport that delivers the request and then times out.' },
      th: [['bob', 'Should we retry non-idempotent calls at all?', 'valid', 'src/rpc/client.rs:99']] },
    '1-6': { t: 'Wait, then try again', file: 'src/rpc/client.rs', a: 100, z: 103, kind: 'added',
      tip: 'Was: no waiting. Is: sleeps backoff(attempt), 200 ms doubling, capped at 4 s plus jitter, then loops to step 2.',
      by: 'step 5 (yes)', dec: 'Computes the delay and sleeps.', next: 'step 2 (loop)',
      why: 'Spaces the attempts so a briefly overloaded node can recover.',
      was: 'No delay.', is: 'Delay = jitter(min(200 ms &times; 2^(attempt-1), 4 s)).',
      diff: [['@', '', '@@ -0,0 +100,4 @@'], ['+', 100, '                    let delay = policy.backoff(attempt);'], ['+', 101, '                    tracing::warn!(attempt, ?delay, method = %req.method, "rpc timeout; retrying");'], ['+', 102, '                    tokio::time::sleep(delay).await;'], ['+', 103, '                }']],
      conseq: ['Jitter is applied after the cap, so a sleep can exceed <code>max_delay</code> by up to 50% (6 s instead of 4 s).'],
      callers: [['RpcClient::send', 'src/rpc/client.rs', 100]], callees: [['RetryPolicy::backoff', 'src/rpc/retry.rs', 28], ['with_jitter', 'src/rpc/retry.rs', 45]],
      qs: ['With jitter applied after the cap, can one sleep exceed <code>max_delay</code> (4 s)?'],
      as: { notice: 'On a slow link a retry may wait up to 6 s instead of 4 s. Few users would notice.', risk: 'LOW', reason: 'Bounded (at most 1.5 &times; the cap) and only on the third attempt path.',
        basis: 'Read: 2 files (retry.rs, client.rs).', orig: 'introduced by this PR', blocks: 'follow-up', fix: 'Apply the jitter before <code>min(max_delay)</code>.' },
      th: [['carol', 'Use a seeded RNG for the jitter so tests are stable.', 'stale', 'src/rpc/retry.rs:30']] },
    '1-7': { t: 'Give up: Exhausted', file: 'src/rpc/client.rs', a: 104, z: 106, kind: 'added',
      tip: 'Was: the call ended with Timeout. Is: the call ends with Exhausted { attempts } after the last attempt.',
      by: 'step 5 (no)', dec: 'Returns the new error instead of <code>Timeout</code>.', next: 'back to the caller',
      why: 'Callers need to tell a retried failure from a single timeout.',
      was: 'Returned <code>Err(Timeout)</code>.', is: 'Returns <code>Err(Exhausted { attempts })</code> once the policy refuses another attempt.',
      diff: [['@', '', '@@ -0,0 +104,3 @@'], ['+', 104, '                Err(RpcError::Timeout) => {'], ['+', 105, '                    return Err(RpcError::Exhausted { attempts: attempt });'], ['+', 106, '                }']],
      conseq: ['Callers that matched on <code>Timeout</code> no longer see it from <code>send</code>.', 'Two sites outside the diff swallow the new variant (flow 3).'],
      callers: [['RpcClient::send', 'src/rpc/client.rs', 104]], callees: [['RpcError::Exhausted', 'src/rpc/error.rs', 13]],
      qs: ['Do all callers of <code>send</code> still handle the case that used to be <code>Timeout</code>? Two wildcard sites were found (flow 3).'],
      as: { notice: 'Wallet users get a generic RPC error instead of the &ldquo;offline&rdquo; message once retries run out.', risk: 'MEDIUM', reason: 'User-visible message change on a common failure; not data-affecting.',
        basis: 'Read: 3 files. Not checked: callers in other repositories.', orig: 'introduced by this PR', blocks: 'follow-up', fix: 'Map <code>Exhausted</code> to the same wallet error as <code>Timeout</code>.' },
      th: [] },
    '1-8': { t: 'Other errors returned as before', file: 'src/rpc/client.rs', a: 107, z: 107, kind: 'changed',
      tip: 'Was: every error was returned by <code>?</code>. Is: non-timeout errors are returned from the match, never retried.',
      by: 'step 3 (other error)', dec: 'Nothing; ends the call.', next: 'back to the caller', why: 'Only timeouts are worth another attempt.',
      was: 'Returned any error.', is: 'Returns any non-timeout error at once.', diff: [['@', '', '@@ -54,1 +107,1 @@'], ['-', 54, '        let resp = self.transport.execute(http, self.config.timeout).await?;'], ['+', 107, '                Err(e) => return Err(e),']], conseq: [], callers: [], callees: [], qs: [], as: null, th: [] },
    '2-1': { t: 'Default timeout 30 s &rarr; 5 s', file: 'src/config.rs', a: 12, z: 26, kind: 'changed',
      tip: 'Was: every request waited up to 30 s. Is: every attempt waits up to 5 s.',
      by: 'RpcClient::new (src/rpc/client.rs:40)', dec: 'Sets the default per-attempt timeout.', next: 'every <code>send</code> call', why: 'The wallet UI froze for 30 s on an unreachable peer (issue #470).',
      was: 'timeout = 30 s.', is: 'timeout = 5 s, for each of up to 3 attempts.',
      diff: [['@', '', '@@ -23,1 +23,1 @@ impl Default for RpcConfig'], ['-', 23, '            timeout: Duration::from_secs(30),'], ['+', 23, '            timeout: Duration::from_secs(5),']],
      conseq: ['<code>sync::fetch_headers</code> relied on the longer wait (see the code below the fragment).', 'CI: <code>sync_headers::slow_peer_2000_headers</code> fails with a timeout after 5 s.'],
      callers: [['RpcClient::new', 'src/rpc/client.rs', 40]], callees: [],
      qs: ['Does <code>sync::fetch_headers</code> finish within 5 s for 2000 headers on slow peers? Its own comment says 10&ndash;20 s, and the failing CI test is about this.'],
      as: { notice: 'Header sync from slow peers may fail where it used to finish; an operator would see sync stall on slow links.', risk: 'MEDIUM', reason: 'Affects node sync on slow links, not wallet funds; CI already shows a failing test for it.',
        basis: 'Read: 3 files (config.rs, sync/mod.rs, CI log). Not checked: real peer latencies.', orig: 'introduced by this PR', blocks: 'blocks this PR until the failing test is resolved', fix: 'Keep a longer timeout for <code>get_headers</code> or pass a per-request timeout in <code>fetch_headers</code>.' },
      th: [] },
    '2-2': { t: 'Affected code: sync::fetch_headers', file: 'src/sync/mod.rs', a: 61, z: 67, kind: 'unchanged',
      tip: 'Was: waits up to 30 s for a reply with 2000 headers. Is: gives up after 5 s per attempt, then retries.',
      by: 'the sync loop', dec: 'Requests up to 2000 headers in one reply.', next: 'decodes the headers', why: 'Shown because the default timeout reaches it, although the file is not in the diff.',
      was: 'One wait of up to 30 s.', is: 'Up to 3 waits of 5 s each, then <code>Exhausted</code>.', diff: null,
      conseq: ['A reply that needs 10&ndash;20 s now times out on every attempt.'], callers: [['sync loop', 'src/sync/mod.rs', 41]], callees: [['RpcClient::send', 'src/rpc/client.rs', 90]],
      qs: ['Is a 2000-header reply from a slow peer expected to arrive within 5 s?'], as: null, th: [] },
    '3-1': { t: 'New error variant RpcError::Exhausted', file: 'src/rpc/error.rs', a: 7, z: 16, kind: 'added',
      tip: 'Was: send could fail with Timeout or Transport. Is: a third variant, Exhausted, exists.',
      by: 'RpcClient::send (step 7 of flow 1)', dec: 'Adds a variant that carries the attempt count.', next: 'every <code>match</code> on <code>RpcError</code>', why: 'Callers need to tell &ldquo;retried and failed&rdquo; from one timeout.',
      was: 'Two failure variants.', is: 'Three; wildcard arms treat the new one as a generic failure.',
      diff: [['@', '', '@@ -11,3 +11,5 @@ pub enum RpcError'], [' ', 11, '    Timeout,'], ['+', 12, '    #[error("gave up after {attempts} attempts")]'], ['+', 13, '    Exhausted { attempts: u32 },'], [' ', 14, '    #[error("transport failure: {0}")]']],
      conseq: ['Two wildcard <code>match</code> sites outside the diff swallow it (wallet and peers).'], callers: [['RpcClient::send', 'src/rpc/client.rs', 105]], callees: [],
      qs: ['Should the two wildcard sites handle <code>Exhausted</code> explicitly?'],
      as: { notice: 'A wallet user sees a generic RPC error; a peer that keeps timing out is silently ignored instead of being marked failed.', risk: 'MEDIUM', reason: 'Wrong handling of a common failure in two places; no data loss.',
        basis: 'Read: 3 files, searched the repository for <code>match</code> on <code>send</code> results.', orig: 'introduced by this PR', blocks: 'follow-up', fix: 'Handle <code>Exhausted</code> explicitly in both <code>match</code> sites.' },
      th: [] },
    '3-2': { t: 'Affected code: wildcard match on send()', file: 'src/wallet/mod.rs', a: 118, z: 122, kind: 'unchanged',
      tip: 'Was: Err(_) caught Transport errors only. Is: Err(_) also catches Exhausted.',
      by: 'wallet payment path', dec: 'Maps any other error to <code>WalletError::Rpc</code>.', next: 'returns to the wallet UI', why: 'Same pattern &times; 2; the second site is <code>src/sync/peers.rs:204</code>.',
      was: 'Wildcard arm saw one other error.', is: 'Wildcard arm also receives <code>Exhausted</code>.', diff: null,
      conseq: ['The user message differs from the <code>Timeout</code> case.'], callers: [], callees: [['RpcClient::send', 'src/rpc/client.rs', 90]],
      qs: ['Should <code>Exhausted</code> map to <code>WalletError::Offline</code> like <code>Timeout</code>?'], as: null, th: [] },
    '4-1': { t: 'Removed the single-attempt API', file: 'src/rpc/client.rs', a: 52, z: 61, kind: 'removed',
      tip: 'Was: send_once() sent a request exactly once. Is: removed; send() always retries.',
      by: 'callers that must not retry (none left in this repository)', dec: 'Nothing; the code is gone.', next: 'callers now use <code>send()</code>', why: 'One entry point instead of two.',
      was: 'Two ways to send: with and without retry.', is: 'One way, always with retry.',
      diff: [['@', '', '@@ -52,10 +0,0 @@ impl RpcClient'], ['-', 52, '    /// Sends exactly once; for callers that must not retry.'], ['-', 53, '    pub async fn send_once(&self, req: Request) -> Result<Response, RpcError> {'], ['-', '', '    ... 8 more removed lines']],
      conseq: ['No way is left to send a request exactly once (see flow 1, step 5).', 'Downstream crates that call <code>send_once</code> stop compiling.'],
      callers: [['search for send_once in this repository: 0 hits', 'src/', 0]], callees: [],
      qs: ['Does <code>acme/node-sdk</code> (PR #118) still call <code>send_once()</code>?', 'How does a caller now send a request that must not be repeated?'],
      as: { notice: 'A downstream developer gets a compile error; a user of <code>broadcast_tx</code> loses the only no-retry path.', risk: 'MEDIUM', reason: 'Public API removal with a known downstream user; ties to the double-send question.',
        basis: 'Read: 2 files, searched this repository and the stacked PR title. Not checked: other downstream crates.', orig: 'introduced by this PR', blocks: 'blocks this PR', fix: 'Keep <code>send_once</code> (or add a per-call retry opt-out) until the SDK has moved.' },
      th: [] },
    '5-1': { t: 'Backoff helpers moved', file: 'src/rpc/retry.rs', a: 38, z: 95, kind: 'moved',
      tip: 'Was: jitter of up to 10% of the delay. Is: jitter of up to 50% of the delay.',
      by: 'RetryPolicy::backoff (flow 1, step 6)', dec: 'Computes the exponential delay and the jitter.', next: 'returns the delay to <code>backoff</code>', why: 'Keeps the helpers next to the policy that uses them.',
      was: '<code>JITTER = 0.1</code>.', is: '<code>JITTER = 0.5</code>.',
      diff: [['@', '', '@@ -22,2 +49,2 @@ moved from src/util/backoff.rs'], ['-', 22, '// Jitter is uniform in [-JITTER, +JITTER], i.e. +/-10% of the delay.'], ['-', 23, 'const JITTER: f64 = 0.1;'], ['+', 49, '// Jitter is uniform in [-JITTER, +JITTER], i.e. +/-50% of the delay.'], ['+', 50, 'const JITTER: f64 = 0.5;']],
      conseq: ['Retry times of concurrent clients are spread over a wider window.'], callers: [['RetryPolicy::backoff', 'src/rpc/retry.rs', 30]], callees: [],
      qs: ['Is the jump from &plusmn;10% to &plusmn;50% intended in a PR that is described as a move?'],
      as: { notice: 'Retries wait noticeably longer or shorter from run to run; few users would see it.', risk: 'LOW', reason: 'A tuning change inside a bounded delay.',
        basis: 'Read: 2 files (old and new location).', orig: 'introduced by this PR', blocks: 'follow-up', fix: 'Say it in the PR description, or keep 0.1 in the move.' },
      th: [] }
  };
  var ALT = { wrong: { '3-1': { as: { notice: 'Every downstream crate that matches <code>RpcError</code> exhaustively stops compiling.', risk: 'HIGH',
    reason: 'Adding a variant is a breaking change for all consumers of the crate.', basis: 'Read: <code>src/rpc/error.rs</code> lines 9&ndash;16 only. Not checked: attributes on the enum.',
    orig: 'introduced by this PR', blocks: 'blocks this PR', fix: 'Bump the major version or box the error.' } } } };
  function getI(id) {
    var d = INSP[id]; if (!d) return null;
    var alt = ALT[D.state] && ALT[D.state][id];
    if (alt) { var c = {}; for (var k in d) c[k] = d[k]; for (k in alt) c[k] = alt[k]; return c; }
    return d;
  }

  /* ---------- inspector rendering ---------- */
  var KB = { added: ['b-add', 'added'], changed: ['b-chg', 'changed'], unchanged: ['b-same', 'unchanged'], removed: ['b-rm', 'removed'], moved: ['b-mv', 'moved'] };
  function ghl(f, a, z) { return GH + '/blob/' + SHA + '/' + f + '#L' + a + (z && z !== a ? '-L' + z : ''); }
  function glink(f, l, txt) { return '<a href="' + ghl(f, l) + '" target="_blank" rel="noopener noreferrer" aria-label="Open ' + esc(f) + ' line ' + l + ' on GitHub, new tab">' + txt + '</a>'; }
  function diffHTML(rows) {
    var o = rows.map(function (r) {
      var c = r[0] === '-' ? 'rm' : r[0] === '+' ? 'ad2' : r[0] === '@' ? 'hh' : '';
      return '<div class="l ' + c + '"><span class="n">' + r[1] + '</span><span class="g"></span><code>' + esc(r[0] === '@' ? r[2] : r[0] + r[2]) + '</code></div>';
    }).join('');
    return '<div class="old"><div class="code" tabindex="0" role="region" aria-label="Classic diff of the selected range"><div class="ci">' + o + '</div></div></div>';
  }
  function treeHTML(title, arr) {
    if (!arr.length) return '';
    return '<li><strong>' + title + '</strong><ul>' + arr.map(function (x) {
      return '<li>' + (x[2] ? glink(x[1], x[2], '<code>' + esc(x[0]) + '</code>') + ' <span class="mut">' + esc(x[1]) + ':' + x[2] + '</span>' : '<code>' + esc(x[0]) + '</code> <span class="mut">' + esc(x[1]) + '</span>') + '</li>';
    }).join('') + '</ul></li>';
  }
  function assessHTML(d) {
    if (!d.as) return '<p class="small muted">No LLM assessment for this step: it is unchanged context or a plain mechanical part of a flow that has its own assessment.</p>';
    var a = d.as;
    return '<dl class="asx"><dt>What a production user or operator would notice</dt><dd>' + a.notice + '</dd>' +
      '<dt>Risk at system scale</dt><dd>' + rb(a.risk) + ' ' + a.reason + '</dd>' +
      '<dt>Basis</dt><dd>' + a.basis + '</dd><dt>Origin</dt><dd>' + a.orig + '</dd><dt>Merge impact</dt><dd>' + a.blocks + '</dd>' +
      '<dt>Smallest correction</dt><dd>' + a.fix + '</dd></dl>';
  }
  function answersHTML(id) {
    var a = (S.answers[id] || []);
    if (!a.length) return '<p class="small muted">No questions asked about this block yet. Answers stay pinned here.</p>';
    return a.map(function (x) { return '<div class="qa"><div class="q">' + esc(x.q) + '</div><div>' + esc(x.a) + ' ' + bd('b-llm', 'LLM answer', 'llma') + '</div></div>'; }).join('');
  }
  function sec(key, title, inner) { return '<details class="sec" data-sec="' + key + '"><summary>' + title + '</summary><div>' + inner + '</div></details>'; }
  function titleOf(key) {
    var b = $('.blk[data-key="' + key + '"]'); if (!b) return key;
    return b.dataset.title.replace(/^(Step \d+): /, '$1 · ').replace(/^Flow (\d+): /, 'Flow $1 · ');
  }
  function tgt() { return S.sel || S.blk; }
  var DEPS = { 1: 'flow 3 (the variant) and flow 5 (the helpers)' };
  function summaryHTML(k) {
    var noq = D.state === 'noq', body = '', t = titleOf(k);
    var m = /^g(\d)$/.exec(k);
    if (m) {
      var n = m[1], keys = Object.keys(INSP).filter(function (x) { return x.charAt(0) === n && $('.blk[data-key="' + x + '"]'); }), files = [], qn = 0;
      keys.forEach(function (x) { var d = getI(x); if (files.indexOf(d.file) < 0) files.push(d.file); qn += noq ? 0 : d.qs.length; });
      var rt = $('#gl-' + n + ' .rate'), kinds = keys.map(function (x) { var d = getI(x), kb = KB[d.kind]; return '<li><button type="button" class="linkbtn" data-go="' + x + '">' + esc(titleOf(x)) + '</button> ' + bd(kb[0], kb[1], d.kind) + '</li>'; }).join('');
      body = sec('fsum', 'Steps of this flow', '<ul class="small" style="list-style:none;padding:0">' + kinds + '</ul>') +
        sec('ffiles', 'Files touched', '<ul class="small">' + files.map(function (f) { return '<li><code>' + esc(f) + '</code></li>'; }).join('') + '</ul>') +
        sec('fq', 'Questions to check', '<p class="small">' + (qn ? qn + ' question' + (qn > 1 ? 's' : '') + ' across the steps of this flow. Open a step to read them.' : 'The LLM raised no questions. That is not evidence of no defects.') + '</p>') +
        sec('fdep', 'Depends on', '<p class="small">' + (DEPS[n] || 'No other flow.') + '</p>');
      t += rt ? ' ' + rb(rt.textContent) : '';
    } else if (k === 'ctx') {
      body = sec('c1', 'At a glance', '<ul class="small"><li>CI: 1 of 6 checks failing (<code>sync_headers::slow_peer_2000_headers</code>)</li><li>Related work: 3 items (competing PR #479, issue #470, stacked PR node-sdk#118)</li><li>Fit questions to check: 3</li><li>Counters: 12 lines removed from tests, 3 dependencies changed</li><li>Project profile: ' + (D.state === 'noprofile' ? 'none (ratings are component-level)' : 'found') + '</li><li>5 flows, 14 steps</li></ul>');
    } else if (k === 'pending') {
      body = sec('p1', 'Pending review', '<p class="small">' + $$('#pc-list li').length + ' comments are in your GitHub pending review. Edit and delete act on GitHub. Submit the review on GitHub.</p>');
    } else if (k === 'q-unexp') {
      body = sec('q1', 'Unexplained lines', '<p class="small">6 changed lines in <code>src/rpc/metrics.rs</code> (L41\u201346) were not assigned to any flow. The analysis did not explain them; read them yourself. You can comment on any line.</p><p class="small muted">No LLM remark exists for these lines, so a comment here is also recorded as missed by the LLM.</p>');
    } else if (k === 'q-build') {
      body = sec('q1', 'Moved block, text identical', '<p class="small"><code>build_request()</code>: 41 lines, body text identical after whitespace normalisation. The call site changed in flow 1, step 2.</p>');
    } else if (k === 'q-tests') {
      body = sec('q1', 'Not analysed: Tests', '<p class="small">4 files recognised by path, 12 lines removed from tests. Shown as a count only; the agent may read tests as evidence.</p>');
    } else if (k === 'q-docs') {
      body = sec('q1', 'Not analysed: Docs and comments', '<p class="small">3 files by path or line rule. Not interpreted.</p>');
    } else if (k === 'q-mech') {
      body = sec('q1', 'Not analysed: Mechanical and generated', '<p class="small"><code>Cargo.lock</code>: 3 dependencies changed. Counted, not interpreted.</p>');
    }
    return '<div class="insp"><h2>' + t + '</h2><p class="llmline">LLM analysis unless marked otherwise.</p>' + body +
      sec('ask', 'Ask: pinned answers', '<div id="answers">' + answersHTML(k) + '</div>') + '</div>';
  }
  function renderInsp() {
    var box = $('#insp-body'), id = S.sel, k = S.blk;
    $('#insp-empty').hidden = true; box.hidden = false;
    if (id && getI(id)) { render(id, 'ann'); return; }
    if (getI(k) && /^\d-\d$/.test(k)) { render(k, 'step'); return; }
    box.innerHTML = summaryHTML(k); applySecs(box); initTips(box); $('#insp').scrollTop = 0;
  }
  function render(id, mode) {
    var d = getI(id), box = $('#insp-body'), ann = mode === 'ann';
    var kb = KB[d.kind], noq = D.state === 'noq';
    var q = (noq || !d.qs.length) ? '<p class="small">The LLM raised no questions. That is not evidence of no defects.</p>' : '<ul>' + d.qs.map(function (x) { return '<li>' + bd('b-q', 'question', 'q') + ' ' + x + '</li>'; }).join('') + '</ul>';
    var th = d.th.length ? d.th.map(function (t) { return '<div class="thread"><strong>@' + t[0] + '</strong> on <code>' + t[3] + '</code><br>' + esc(t[1]) + '<br>' + bd(t[2] === 'valid' ? 'b-chg' : 'b-same', t[2] === 'valid' ? 'still valid' : t[2], t[2] === 'valid' ? 'valid' : t[2]) + ' <span class="small muted">LLM status</span></div>'; }).join('') : '';
    var rate = (d.as && !noq) ? ' ' + rb(d.as.risk) : '';
    var title = ann ? 'Annotation · ' + d.t : titleOf(id);
    var back = ann ? '<p style="margin:0 0 6px"><button type="button" class="linkbtn" data-back>&larr; Back to step</button> <span class="small muted">' + esc(titleOf(S.blk)) + '</span></p>' : '';
    var diff = sec('diff', ann ? 'Classic diff of this range' : 'Classic diff of this step', d.diff ? diffHTML(d.diff) : '<p class="small muted">No diff: this is unchanged code shown for context, or a moved block with identical text.</p>');
    var beh = sec('beh', 'Behaviour: was &rarr; is', '<div class="wi"><div><h3>Was</h3>' + d.was + '</div><div><h3>Is</h3>' + d.is + '</div></div>');
    var thr = sec('threads', 'GitHub threads at this location', th || '<p class="small muted">No GitHub threads at this location.</p>');
    var qs = sec(ann ? 'aqs' : 'qs', 'Questions to check', q), as = sec('assess', 'Assessment', assessHTML(d));
    var calls = sec('calls', 'Call hierarchy', '<p class="small muted" data-tip="Callers and callees will come from static analysis in a later version." tabindex="0">n/a \u2014 needs a language plugin</p>');
    var mid = ann ? '' : sec('why', 'Why', '<p>' + d.why + '</p>') +
      sec('conseq', 'Consequences', d.conseq.length ? '<ul>' + d.conseq.map(function (x) { return '<li>' + x + '</li>'; }).join('') + '</ul>' : '<p class="small muted">None found by the LLM.</p>') +
      calls;
    box.innerHTML = '<div class="insp">' + back + '<h2>' + title + '</h2><p class="row" style="margin:0 0 2px">' + bd(kb[0], kb[1], d.kind) + rate +
      ' <a class="loclink" href="' + ghl(d.file, d.a, d.z) + '" target="_blank" rel="noopener noreferrer" aria-label="Open ' + esc(d.file) + ' on GitHub at the head commit, new tab">' + esc(d.file) + ':' + d.a + (d.z !== d.a ? '&ndash;' + d.z : '') + ' &#8599;</a></p>' +
      '<p class="llmline">LLM analysis unless marked otherwise.</p>' +
      (ann ? diff + beh + calls + qs + as + thr : diff + beh + mid + (th ? thr : '') + qs + as) +
      sec('ask', 'Ask: pinned answers', '<div id="answers">' + answersHTML(S.blk) + '</div>') + '</div>';
    applySecs(box); initTips(box); $('#insp').scrollTop = 0;
  }

  /* ---------- workspace: one block at a time ---------- */
  var S = { sel: null, fb: {}, answers: {}, cmt: null, blk: null, cur: null, ticks: ss.get('sr2.ticks', ['f2-1']) };
  function workspace() {
    var scroller = $('#scroller'), blks = $$('.blk'), ORDER = [$('.blk[data-key="ctx"]')].concat($$('.grp .blk'), [$('.blk[data-key="q-unexp"]')]);
    function byKey(k) { return $('.blk[data-key="' + k + '"]'); }
    ORDER.forEach(function (b, i) {
      var p = ORDER[i - 1], n = ORDER[i + 1], d = document.createElement('div'); d.className = 'bnav';
      d.innerHTML = '<button class="btn" type="button" data-nav="-1"' + (p ? '' : ' disabled') + '>&larr; Previous' + (p ? ': ' + esc(p.dataset.title) : '') + '</button><span class="sp"></span>' +
        '<button class="btn pri" type="button" data-nav="1"' + (n ? '' : ' disabled') + '>Next' + (n ? ': ' + esc(n.dataset.title) : '') + ' &rarr;</button>';
      b.appendChild(d);
    });
    function groupOf(k) { var m = /^g?(\d)/.exec(k); return /^(g\d|\d-\d)$/.test(k) && m ? +m[1] : null; }
    function showBlock(key) {
      var t = byKey(key); if (!t) return;
      blks.forEach(function (b) { b.hidden = b !== t; });
      S.blk = key; var g = groupOf(key);
      $$('.gitem,.qitem,.sitem').forEach(function (x) { x.setAttribute('aria-current', x.dataset.go === key ? 'true' : 'false'); });
      $$('.substeps').forEach(function (o, i) { o.hidden = (i + 1) !== g; });
      S.cur = $('.frag', t); scroller.scrollTop = 0; select(null); clearCmtActive();
      var dp = $('#dpane'); dp.hidden = g === null; $$('.dgw').forEach(function (d) { d.hidden = +d.dataset.g !== g; });
      $('#dbar-f').textContent = g ? '(flow ' + g + ')' : ''; markNodes(); scrollNode(); renderInsp(); updateFb();
    }
    function go(d) { var i = ORDER.map(function (b) { return b.dataset.key; }).indexOf(S.blk); if (i >= 0 && ORDER[i + d]) showBlock(ORDER[i + d].dataset.key); }
    function goGroup(d) { if (S.blk === 'ctx' && d < 0) return; var g = groupOf(S.blk) || (d > 0 ? 0 : 6); g = Math.max(1, Math.min(5, g + d)); showBlock('g' + g); }
    function route() {
      var h = location.hash, m;
      if ((m = /^#open=([\d-]+)/.exec(h))) showBlock(m[1]);
      else if (/^#g\d$/.test(h)) showBlock(h.slice(1));
      else if (h === '#ctx') showBlock('ctx');
      else if ((m = /^#f(\d-\d)$/.exec(h))) showBlock(m[1]);
      else if (h === '#pending') showBlock('pending');
    }
    window.addEventListener('hashchange', route);
    document.addEventListener('click', function (e) {
      var nd = e.target.closest('.node,.mn'); if (nd) { showBlock(nd.dataset.step); return; }
      var g = e.target.closest('[data-go]'); if (g) { showBlock(g.dataset.go); return; }
      var n = e.target.closest('[data-nav]'); if (n) { go(+n.dataset.nav); return; }
      var h = e.target.closest('.hl[data-a],.delsite[data-a]');
      if (h && !e.target.closest('button,a,summary') && D.state !== 'noq') select(h.dataset.a);
    });
    document.addEventListener('keydown', function (e) {
      var t = e.target; if (e.key !== 'Enter' && e.key !== ' ') return;
      if (t.matches && t.matches('.node,.mn')) { if (t.tagName !== 'BUTTON') { e.preventDefault(); showBlock(t.dataset.step); } }
      else if (t.matches && t.matches('.hl[data-a],.delsite[data-a]') && D.state !== 'noq') { e.preventDefault(); select(t.dataset.a); }
    });

    /* diagram pane: hidden by default, state and height remembered */
    var dgOpen = ls.get('sr2.dg', false), dh = ls.get('sr2.dh', null);
    function applyDiagram() {
      var body = $('#dbody'), bar = $('#dbar'); body.hidden = !dgOpen; bar.setAttribute('aria-expanded', dgOpen);
      $('#dbar-t').textContent = dgOpen ? 'Hide flow diagram' : 'Show flow diagram'; $('.chev', bar).innerHTML = dgOpen ? '&#9662;' : '&#9656;';
      body.style.height = (dh || 300) + 'px'; markNodes(); scrollNode();
    }
    $('#dbar').addEventListener('click', function () { dgOpen = !dgOpen; ls.set('sr2.dg', dgOpen); applyDiagram(); });
    function clampH(v) { return Math.round(Math.max(130, Math.min(v, $('#col-main').getBoundingClientRect().height * 0.7))); }
    var gh = $('#gut-h'), hdrag = false;
    gh.addEventListener('pointerdown', function (e) { hdrag = true; gh.setPointerCapture(e.pointerId); gh.classList.add('drag'); e.preventDefault(); });
    gh.addEventListener('pointermove', function (e) { if (!hdrag) return; dh = clampH(e.clientY - $('#dbody').getBoundingClientRect().top); $('#dbody').style.height = dh + 'px'; });
    function hend() { if (hdrag) { hdrag = false; gh.classList.remove('drag'); ls.set('sr2.dh', dh); } }
    gh.addEventListener('pointerup', hend); gh.addEventListener('pointercancel', hend);
    gh.addEventListener('dblclick', function () { dh = null; ls.set('sr2.dh', null); applyDiagram(); });
    gh.addEventListener('keydown', function (e) { if (e.key !== 'ArrowUp' && e.key !== 'ArrowDown') return; e.preventDefault(); dh = clampH((dh || 300) + (e.key === 'ArrowDown' ? 24 : -24)); ls.set('sr2.dh', dh); applyDiagram(); });
    function scrollNode() {
      var body = $('#dbody'); if (body.hidden) return;
      var n = $('.dgw:not([hidden]) .node.sel, .dgw:not([hidden]) .mn.sel'); if (!n) { body.scrollTop = 0; return; }
      var b = body.getBoundingClientRect(), r = n.getBoundingClientRect();
      if (r.top < b.top + 8 || r.bottom > b.bottom - 16) body.scrollTop += r.top - b.top - (b.height - r.height) / 2;
    }
    function markNodes() {
      $$('.node,.mn').forEach(function (n) {
        var st = n.dataset.step, rv = S.ticks.indexOf('f' + st) >= 0;
        n.classList.toggle('sel', st === S.blk); n.classList.toggle('rv', rv);
        if (st === S.blk) n.setAttribute('aria-current', 'step'); else n.removeAttribute('aria-current');
        var old = n.querySelector('.rvm'); if (old) n.removeChild(old);
        if (rv && n.tagName === 'g' && dgOpen && !n.closest('[hidden]')) {
          var bb = n.querySelector('.sh').getBBox(), t = document.createElementNS('http://www.w3.org/2000/svg', 'text');
          t.setAttribute('class', 'rvm'); t.setAttribute('x', bb.x + bb.width - 8); t.setAttribute('y', bb.y + 13); t.setAttribute('text-anchor', 'end'); t.textContent = '☑ reviewed'; n.appendChild(t);
        }
      });
    }
    /* comment highlight: the line being commented stays highlighted while the box is open */
    function clearCmtActive() { $$('.l.cmt-active').forEach(function (x) { x.classList.remove('cmt-active'); }); }

    /* columns: hide, full screen, drag-resize (sizes kept in localStorage) */
    function defRight() { return ls.get('sr2.insph', false) ? 'h' : (innerWidth >= 1440 ? 'n' : 'h'); }
    var cs = { left: 'n', main: 'n', right: defRight() }, W = ls.get('sr2.w', { l: null, r: null });
    function defW(k) { return k === 'l' ? Math.max(260, Math.min(400, Math.round(innerWidth * 0.17))) : Math.max(370, Math.min(620, Math.round(innerWidth * 0.27))); }
    function layout() {
      var ws = $('#ws'), full = Object.keys(cs).filter(function (k) { return cs[k] === 'f'; })[0], hid = {};
      ['left', 'main', 'right'].forEach(function (k) { hid[k] = full ? k !== full : cs[k] === 'h'; });
      var wl = W.l || defW('l'), wr = W.r || defW('r'), tpl = [];
      tpl.push(hid.left ? '34px' : (hid.main ? 'minmax(0,1fr)' : wl + 'px'));
      tpl.push(hid.left || hid.main ? '0px' : '6px');
      tpl.push(hid.main ? '34px' : 'minmax(0,1fr)');
      tpl.push(hid.right || hid.main ? '0px' : '6px');
      tpl.push(hid.right ? '34px' : (hid.main ? 'minmax(0,1fr)' : wr + 'px'));
      ws.style.gridTemplateColumns = tpl.join(' ');
      $('#gut-l').classList.toggle('off', hid.left || hid.main); $('#gut-r').classList.toggle('off', hid.right || hid.main);
      ['left', 'main', 'right'].forEach(function (k) {
        var c = $('#col-' + k); c.classList.toggle('is-rail', hid[k]);
        var fb = $('[data-colbtn="full"]', c); fb.setAttribute('aria-pressed', cs[k] === 'f'); fb.firstChild.textContent = cs[k] === 'f' ? 'Exit full screen' : 'Full screen';
      });
    }
    function openInsp() { var full = Object.keys(cs).some(function (k) { return cs[k] === 'f'; }); if (full) { cs.left = cs.main = 'n'; } if (full || cs.right !== 'n') { cs.right = 'n'; layout(); } }
    document.addEventListener('click', function (e) {
      var b = e.target.closest('[data-colbtn]'); if (!b) return; var k = b.dataset.col, a = b.dataset.colbtn;
      if (k === 'right' && (a === 'hide' || a === 'show')) ls.set('sr2.insph', a === 'hide');
      if (a === 'hide') { var vis = Object.keys(cs).filter(function (x) { return cs[x] !== 'h' && x !== k; }); if (!vis.length) { toast('At least one column must stay visible.'); return; } if (Object.keys(cs).some(function (x) { return cs[x] === 'f'; })) { cs.left = cs.main = 'n'; cs.right = 'n'; } cs[k] = 'h'; }
      if (a === 'show') { if (Object.keys(cs).some(function (x) { return cs[x] === 'f'; })) { cs.left = cs.main = 'n'; cs.right = 'h'; } cs[k] = 'n'; }
      if (a === 'full') { var on = cs[k] !== 'f'; var r = cs.right; cs.left = cs.main = 'n'; cs.right = r === 'f' ? 'n' : r; if (on) cs[k] = 'f'; }
      layout();
    });
    document.addEventListener('keydown', function (e) {
      if (e.key === 'Escape' && !(help && help.open) && Object.keys(cs).some(function (x) { return cs[x] === 'f'; })) { cs.left = cs.main = 'n'; cs.right = 'n'; layout(); }
    });
    var drag = null;
    function clampW(k, v) { var tot = $('#ws').getBoundingClientRect().width, other = k === 'l' ? (cs.right === 'h' ? 34 : (W.r || defW('r'))) : (cs.left === 'h' ? 34 : (W.l || defW('l'))); return Math.round(Math.max(k === 'l' ? 200 : 300, Math.min(v, tot - other - 12 - 320))); }
    $$('.gut').forEach(function (g) {
      var k = g.dataset.gut;
      g.addEventListener('pointerdown', function (e) { drag = k; g.setPointerCapture(e.pointerId); g.classList.add('drag'); e.preventDefault(); });
      g.addEventListener('pointermove', function (e) {
        if (drag !== k) return; var r = $('#ws').getBoundingClientRect();
        W[k] = clampW(k, k === 'l' ? e.clientX - r.left : r.right - e.clientX); layout();
      });
      function end() { if (drag === k) { drag = null; g.classList.remove('drag'); ls.set('sr2.w', W); } }
      g.addEventListener('pointerup', end); g.addEventListener('pointercancel', end);
      g.addEventListener('dblclick', function () { W[k] = null; ls.set('sr2.w', W); layout(); });
      g.addEventListener('keydown', function (e) {
        if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return; e.preventDefault();
        var d = (e.key === 'ArrowRight' ? 1 : -1) * (k === 'l' ? 1 : -1) * 24, cur = W[k] || defW(k); W[k] = clampW(k, cur + d); ls.set('sr2.w', W); layout();
      });
    });

    /* inspector selection: only from a clicked highlight */
    function select(id) {
      $$('.hl.sel,.delsite.sel').forEach(function (x) { x.classList.remove('sel'); });
      S.sel = id; B.dataset.sel = id || '';
      if (!id || !getI(id)) { S.sel = null; B.dataset.sel = ''; renderInsp(); updateFb(); return; }
      $$('.hl[data-a="' + id + '"],.delsite[data-a="' + id + '"]').forEach(function (x) { x.classList.add('sel'); });
      renderInsp(); updateFb(); openInsp();
    }
    window.__select = select;
    document.addEventListener('click', function (e) { if (e.target.closest('[data-back]')) select(null); });

    function fillFrags() {
      $$('.facts[data-meta]').forEach(function (dl) {
        var d = getI(dl.dataset.meta); if (!d) return;
        dl.innerHTML = '<dt>Called by</dt><dd>' + d.by + '</dd><dt>Decides or changes</dt><dd>' + d.dec + '</dd><dt>Goes next</dt><dd>' + d.next + '</dd><dt>Why</dt><dd class="why">' + d.why + '</dd>';
      });
      $$('.frag-assess').forEach(function (x) { x.querySelector('.assess-body').innerHTML = assessHTML(getI(x.dataset.assess)); initTips(x); });
    }
    function groupFrags(g) { return $$('.frag', g); }
    function applyTicks() {
      var k = 0;
      $$('.frag').forEach(function (f) {
        var on = S.ticks.indexOf(f.id) >= 0; f.classList.toggle('reviewed', on);
        var b = $('[data-act="rev"]', f); b.setAttribute('aria-pressed', on); b.firstChild.textContent = on ? 'Reviewed (undo) ' : 'Mark reviewed ';
      });
      $$('.sitem').forEach(function (x) { x.classList.toggle('done', S.ticks.indexOf('f' + x.dataset.go) >= 0); });
      $$('.grp').forEach(function (g) {
        var fs = groupFrags(g), d = fs.filter(function (f) { return f.classList.contains('reviewed'); }).length, all = d === fs.length;
        $('[data-gp]', g).textContent = d + ' of ' + fs.length + ' steps reviewed';
        var gb = $('[data-act="grp"]', g); gb.setAttribute('aria-pressed', all); gb.textContent = all ? 'Flow reviewed (undo)' : 'Mark flow reviewed';
        var t = $('#gl-' + g.id.slice(1) + ' .gt'); if (t) t.hidden = !all;
        if (all) k++;
      });
      setK(k); ss.set('sr2.ticks', S.ticks); markNodes();
    }
    function toggleTick(f) { var i = S.ticks.indexOf(f.id); if (i >= 0) S.ticks.splice(i, 1); else S.ticks.push(f.id); applyTicks(); }
    function toggleOld(f) {
      var o = $('.old', f), b = $('[data-act="old"]', f); if (!o || !b) return;
      o.hidden = !o.hidden; b.setAttribute('aria-pressed', !o.hidden); b.firstChild.textContent = o.hidden ? 'Show old code ' : 'Hide old code ';
    }
    document.addEventListener('click', function (e) {
      var b = e.target.closest('[data-act]'); if (!b) return; var a = b.dataset.act;
      if (a === 'rev') toggleTick(b.closest('.frag'));
      if (a === 'old') toggleOld(b.closest('.frag'));
      if (a === 'grp') {
        var g = b.closest('.grp'), fs = groupFrags(g), all = fs.every(function (f) { return S.ticks.indexOf(f.id) >= 0; });
        fs.forEach(function (f) { var i = S.ticks.indexOf(f.id); if (all && i >= 0) S.ticks.splice(i, 1); if (!all && i < 0) S.ticks.push(f.id); });
        applyTicks();
      }
    });

    /* feedback */
    function updateFb() {
      var t = tgt(), cur = S.fb[t], ok = !!getI(t);
      $$('[data-fb]').forEach(function (b) { b.disabled = !ok; b.setAttribute('aria-pressed', cur === b.dataset.fb ? 'true' : 'false'); });
      $('#fb-note-r').hidden = !(cur && cur !== 'correct');
      var lab = S.sel ? 'annotation \u00b7 ' + getI(S.sel).t : titleOf(S.blk);
      $('#ask-for').textContent = 'about ' + lab + '; answers stay pinned in the inspector';
      $('#ask-input').placeholder = 'Ask about ' + lab + '…';
      $('#fb-for').textContent = ok ? '' : 'feedback applies to steps';
    }
    $$('[data-fb]').forEach(function (b) {
      b.addEventListener('click', function () {
        var t = tgt(); if (!getI(t)) return;
        var k = b.dataset.fb; if (S.fb[t] === k) delete S.fb[t]; else S.fb[t] = k;
        updateFb();
        var g = t.split('-')[0], m = $('#gl-' + g + ' .mine'); if (m) { m.hidden = !S.fb[t]; m.textContent = S.fb[t] ? 'you marked: ' + S.fb[t] : ''; }
        if (S.fb[t]) toast(/rated/.test(k) ? 'Saved as a severity calibration example (mock): ' + k + '.' : 'Feedback saved locally and queued for MemCan (mock): ' + k + '.');
      });
    });
    function grow(t) { t.style.height = 'auto'; t.style.height = Math.min(t.scrollHeight + 2, 170) + 'px'; }
    $$('#ask-input,#fb-note,#cmt-text').forEach(function (t) { t.addEventListener('input', function () { grow(t); }); });
    $('#ask-input').addEventListener('keydown', function (e) { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); $('#ask-form').requestSubmit(); } });
    $('#ask-form').addEventListener('submit', function (e) {
      e.preventDefault(); var i = $('#ask-input'), q = i.value.trim(); if (!q) return;
      var id = S.blk; (S.answers[id] = S.answers[id] || []).unshift({ q: q, a: 'I read the files around this step and searched for callers. I did not check other repositories.' });
      i.value = ''; grow(i); var an = $('#answers'); if (an) an.innerHTML = answersHTML(id);
      var sd = $('.sec[data-sec="ask"]'); if (sd) sd.open = true;
      toast('Answer pinned in the inspector (mock).');
    });

    /* comments */
    function startComment(btn, prefill) {
      var row = btn.closest('.l'), code = row.closest('.code'); if (!code || !code.dataset.file) return;
      var old = row.dataset.side === 'old', ann = !!row.closest('.hl,.delsite');
      S.cmt = { file: code.dataset.file, ln: row.dataset.ln, old: old, ann: ann, row: row };
      clearCmtActive(); row.classList.add('cmt-active'); row.scrollIntoView({ block: 'nearest' });
      openInsp();
      $('#cmt-loc').textContent = code.dataset.file + ':' + row.dataset.ln + (old ? ' (removed line, old numbering)' : '');
      $('#cmt-miss').hidden = ann; $('#cmt-form').hidden = false; $('#cmt-hint').hidden = true; $('#cmt-err').hidden = true;
      var t = $('#cmt-text'); if (prefill) { t.value = prefill; grow(t); } t.focus();
      $('#dock').scrollTop = $('#dock').scrollHeight;
    }
    document.addEventListener('click', function (e) { var n = e.target.closest('.code button.n'); if (n) startComment(n); });
    $('#cmt-cancel').addEventListener('click', function () { clearCmtActive(); $('#cmt-form').hidden = true; $('#cmt-hint').hidden = false; S.cmt = null; });
    $('#cmt-form').addEventListener('submit', function (e) {
      e.preventDefault(); var t = $('#cmt-text'), c = S.cmt;
      if (!t.value.trim()) { $('#cmt-err').hidden = false; t.setAttribute('aria-invalid', 'true'); t.focus(); return; }
      t.removeAttribute('aria-invalid'); $('#cmt-err').hidden = true;
      var li = document.createElement('li'), loc = c.file + ':' + c.ln;
      li.innerHTML = '<span class="loc">' + esc(loc) + '</span> ' + (c.ann ? '' : bd('b-me', 'recorded as missed by the LLM', 'missed')) +
        '<p class="txt" style="margin:3px 0"></p><textarea hidden rows="2" aria-label="Edit comment"></textarea><button class="btn sm" type="button" data-pc="edit">Edit</button> <button class="btn sm" type="button" data-pc="del" aria-label="Delete comment on ' + esc(loc) + '">Delete</button>';
      $('.txt', li).textContent = t.value.trim();
      $('#pc-list').insertBefore(li, $('#pc-list').firstChild); initTips(li);
      setPend($$('#pc-list li').length); clearCmtActive(); c.row.classList.add('cm'); t.value = ''; grow(t);
      $('#cmt-form').hidden = true; $('#cmt-hint').hidden = false; S.cmt = null;
      toast('Added to your GitHub pending review (mock)' + (c.ann ? '.' : ' and recorded as missed by the LLM.'));
    });
    document.addEventListener('click', function (e) {
      var b = e.target.closest('[data-pc]'); if (!b) return; var li = b.closest('li');
      if (b.dataset.pc === 'del') { li.remove(); setPend($$('#pc-list li').length); toast('Deleted from the GitHub pending review (mock).'); }
      else { var p = $('.txt', li), ta = $('textarea', li), ed = ta.hidden; if (ed) { ta.value = p.textContent; } else { p.textContent = ta.value; toast('Saved to the GitHub pending review (mock).'); } ta.hidden = !ed; p.hidden = ed; b.textContent = ed ? 'Save' : 'Edit'; if (ed) ta.focus(); }
    });

    /* keyboard */
    document.addEventListener('keydown', function (e) {
      if (/input|textarea|select/i.test(e.target.tagName) || e.ctrlKey || e.metaKey || e.altKey || (help && help.open)) return;
      var k = e.key;
      if (k === 'j') go(1); else if (k === 'k') go(-1);
      else if (k === 'n') goGroup(1); else if (k === 'p') goGroup(-1);
      else if (k === 'x' && S.cur) toggleTick(S.cur);
      else if (k === 'd' && S.cur) toggleOld(S.cur);
      else if (k === 'c') { e.preventDefault(); if (S.cmt) { openInsp(); $('#cmt-text').focus(); } else { var n = $('.blk:not([hidden]) .code button.n'); if (n) startComment(n); else toast('Open a step with code first.'); } }
      else if (k === 'a') { e.preventDefault(); openInsp(); $('#ask-input').focus(); }
    });

    function rerate() {
      $$('.rate').forEach(function (r) {
        var lv = r.dataset.r; if (D.state === 'noq') lv = 'LOW'; else if (D.state === 'wrong' && r.dataset.rw) lv = r.dataset.rw;
        r.className = 'b rate ' + RC[lv]; r.textContent = lv; r.dataset.tip = TIP[lv];
      });
    }
    stateHook = function (s) {
      rerate(); fillFrags(); clearCmtActive(); $('#cmt-form').hidden = true; $('#cmt-hint').hidden = false; S.cmt = null;
      cs.left = cs.main = 'n'; cs.right = defRight(); layout(); showBlock('ctx');
      if (s === 'wrong') showBlock('3-1');
      if (s === 'noq') showBlock('g1');
      if (s === 'resume') showBlock('1-5');
      if (s === 'missed') {
        showBlock('1-2');
        var n = $('.blk[data-key="1-2"] .l[data-ln="96"] .n'); if (n) startComment(n, 'The per-attempt timeout is reused for every attempt and the caller\'s deadline is ignored: 3 attempts of 5 s plus delays can exceed the 10 s deadline that wallet::send_payment passes.');
      }
    };
    applySecs(document); applyTicks(); layout(); applyDiagram(); fillFrags(); rerate(); updateFb(); showBlock('ctx');
    $$('.code .n').forEach(function (n) { var r = n.closest('.l'), c = n.closest('.code'); if (n.tagName === 'BUTTON' && c && c.dataset.file) n.setAttribute('aria-label', 'Comment on ' + c.dataset.file + ' line ' + r.dataset.ln); });
    setTimeout(route, 0);
  }

  /* ---------- home ---------- */
  function home() {
    var btn = function (s) { var x = $('.states [data-s="' + s + '"]'); if (x) x.click(); };
    $('#analyze-form').addEventListener('submit', function (e) {
      e.preventDefault(); var v = $('#pr-url').value.trim();
      if (!/^https:\/\/github\.com\/[\w.-]+\/[\w.-]+\/pull\/\d+/.test(v)) { btn('error'); $('#pr-url').focus(); return; }
      if (B.dataset.state === 'resume') { location.href = '05-tour.html'; return; }
      if (B.dataset.state === 'moved') { btn('moved'); return; }
      toast('Opening the analysis (mock)'); setTimeout(function () { location.href = '02-analysis-progress.html'; }, 900);
    });
    $('#signin').addEventListener('click', function (e) { e.preventDefault(); btn('new'); });
    $('#reanalyze').addEventListener('click', function () { toast('Fresh analysis started (mock); ticks are not carried over.'); setTimeout(function () { location.href = '02-analysis-progress.html'; }, 900); });
  }

  initTips(document); applySecs(document);
  var start = states();
  if (D.page === 'workspace') workspace();
  if (D.page === 'home') home();
  if (start) start();
}());
