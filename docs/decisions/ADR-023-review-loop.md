# ADR-023: Processing PR comments — PR page, Comments view, Codex assessment, Watch, decision feed for external agents

> Amended for v1.3 by [ADR-025](ADR-025-container-runtime.md): per-payer containers, app-server,
> gateway-only real credentials, file-based read-only GitHub tokens and host report 4.2.
> KL1 gates credential mode; older master-credential/no-paid-API rules do not decide that open question.


- Status: accepted (2026-11-22; the owner reviewed the mockup and started development). Owner model and answers
  RQ1–RQ23 of 2026-11-22 recorded in `docs/review-workflow-plan.md` (§0, §11). Spikes: `docs/spikes/S13-1-*`,
  `S13-2-*`. Contract names: `api::review_loop` and `db::repo::review_loop` (`api::comments` and
  `repo::comments` hold the ADR-006 pending review).
- Extends ADR-018 (new `JobTarget`), ADR-019 (one write scope for tokens), ADR-020 (the token pair is also kept while
  the user watches PRs), ADR-022 (second plug-in; plug-ins also poll while watches exist) and, in phase 3, ADR-005
  (a server-only write installation token).
- Plan: `docs/review-workflow-plan.md`. Mockup: `docs/mockups/10-review-loop.html`. Workflow source: the working
  model `/data/artifacts/dash-evo-tool/2026-11-22/handoff/working-model.md` ("WM").

## Context
The owner decides every fix made in answer to a PR review comment, per ID and one question at a time (WM §6.4),
about 178 times a day (WM M5); 40% of blocker entries repeat decided items (M3); decisions live in a chat and a log
file without stable IDs (U1, U2). diffpilot already reads GitHub threads with the user token, hosts plug-ins
(ADR-022), refreshes on a schedule (ADR-020), runs read-only Codex jobs (ADR-005, ADR-018) and lets the user's agents
read with tokens (ADR-019).

## Decision
1. **PR page and Comments view.** My work leads to a PR page (replacing the progress page) with "Review a PR" (the
   analysis and its status) and "Process PR comments". Comments is a view of the review menu beside Flows, Files and
   Dependencies and works without an analysis; its one-at-a-time page is "Questions", beside Table, Rounds,
   Carry-overs and the decision log. The question card shows the flows containing the commented code and icons for
   notes about it from other modules (each opening in a new tab); every section heading is collapsible; PR state badges
   use a five-state palette with text labels. Modules: `server::review_loop` (items, queue, effects, rounds,
   carry-overs, watch, feed), `server::github::review_loop` (reads), `db::repo::review_loop`, contracts
   `api::review_loop` and `agent::verify_comments`, web `components/comments/**`, `screens/pr/**`,
   `screens/comments/**`. No new crate.
2. **On demand, plus Watch.** Any pull request is processed on demand; diffpilot shows GitHub's state and records
   only what is decided and executed through it. A user may **watch** pull requests he is working on: for a watched PR
   the server detects new comments and reviews itself, runs the assessment at once and notifies the user; unwatched
   PRs are read only when a page asks.
3. **Codex assesses, never implements.** A new job kind `verify_comments` (`JobTarget::PullRequestComments`) runs on
   the existing `JobExecutor` and runner: checkout of the head, the read-only PR-scoped installation token, no session
   restore or save, no MemCan, no GitHub write. It writes per item the assessment, the recommendation and the code
   context. The owner asks about a comment in the shared floating Ask chat (subject kind `comment_item`): with an
   analysis of the head the Ask session answers; without one a `verify_comments` question pass answers into the same
   chat. Unchanged items reuse their assessment. It is the only source of
   assessments (RQ20). Every detection that yields new items spends LLM tokens; this is bounded: one assessment job
   per PR at a time, debounced while thepastaclaw still reviews that PR, a minimum spacing per PR and a daily cap.
4. **Detection for watched PRs reuses refresh machinery** at bounded GitHub cost: one alias-batched GraphQL probe
   (10 PRs per request) per interval with the user's token, triggered also by plug-in change signals (PR Hygiene
   change marks, thepastaclaw round end) and by the daily scheduled refresh; a full read only for PRs whose probe
   changed. The token pair is kept for this under ADR-020's bound (30 days after the last sign-in).
5. **Decisions are the signed-in users' only.** Decisions, supersedes, gate marks, "Resolve this thread" and
   PR-level questions need a browser session with CSRF; no token can decide.
6. **Effects computed by the server.** A pure function maps (decision, author kind, source kind, gate, override) to the
   effects of WM §6.5, each with an executor and order. "Resolve this thread" is shown on every thread decision,
   ticked by default; the submitted state is the explicit resolve decision, for gate threads (G-FUNDS, G-SECRET,
   G-CRYPTO, G-DATA) too, and nothing is resolved without the owner's submit. "Accept despite the gate" is a reject
   with a gate override whose reason is posted and whose thread is always resolved (RQ6). "Create follow-up PR" asks
   for no link (the agent creates the PR); "Handled in #N" names an existing PR. Every decision carries an optional
   owner's comment: stored, shown in the log and table, delivered to the executing agent, never posted to GitHub. Gate items are never grouped, never in "Decide the rest", and need their
   own choice on the table page (RQ4, RQ5, RQ21).
7. **Decision feed for external agents.** Implementing agents outside diffpilot's control subscribe with a personal
   access token (SSE or polling, filtered by their capabilities), claim a decision under a lease, execute its effects
   and report each. ADR-019 gains the scope `review_loop:execute`, valid on the claim and report routes only. Agents
   upload nothing else.
8. **Repeats are carried.** A finding whose `dedupe=` marker matches a decided item takes that decision without a
   question; it is asked again only when the bot edited the comment text or a later commit changed the code at the
   thread's lines. Every carry and re-ask is logged with its evidence and counted per PR and overall (RQ3).
9. **Rounds and PR-level questions.** Rounds are thepastaclaw final reviews; the header counts them. After round 3 the
   queue asks "Merge now, remaining items as follow-ups?"; at the end of a round, when a bot's change request has only
   decided items, it asks "Dismiss stale bot reviews?" (RQ10, RQ11). Merging stays the owner's own act on GitHub.
10. **thepastaclaw status plug-in** (RQ16): a second ADR-022 plug-in reading the review system's `status.json`;
    statements "reviewing (phase, progress)" or "queued (reason)", attributed, no Ready to merge opinion. Plug-ins also
    poll while any user watches a PR (ADR-022 amendment).
11. **Notifications** (RQ18, RQ23): per-user events (questions ready on a watched PR, a round's decisions all
    executed) go to open tabs as browser notifications and to push channels the user configures in Settings: ntfy
    (first-class: server URL, topic, optional token) and a generic webhook (short text and a PR link). An agent's
    `review_loop:execute` token may register its own ntfy or webhook push target, woken by claimable decisions with
    IDs and a fetch link only. Channel tokens are server-side secrets (AES-GCM, masked, never in a `JobSpec`).
    Delivery: 10 s timeout, 3 attempts, failure state shown in Settings and logged once per state change; `http`/`https`
    only, no redirects, private addresses allowed unless the operator denies them (LAN targets such as Home Assistant),
    loopback, link-local and reserved addresses never; 30 messages an hour per target, counted from stored sends.
12. **Phase 3: diffpilot[bot] posts** (RQ2): replies, PR-level comments, resolves and dismissals of stored decisions are
    posted by the server as the diffpilot GitHub App, with a footer naming diffpilot and the maintainer's decision. The
    App gains `pull_requests: write`; the write installation token is minted per effect for the PR repository only,
    never enters a `JobSpec` or response, and is revoked after use. The runner keeps its read-only token.
13. **triage-findings is superseded** for PR comments; it stays for local review reports without a PR.
14. **Trust and wording** (brief §1): LLM text tagged "LLM" ("LLM assessment", never "verified"); GitHub text tagged
    "GitHub"; badges only for facts diffpilot read; a bot's "Blocking" is the bot's claim; no numeric confidence;
    counts, never percentages; agent reports attributed to the token's name; plug-in statements attributed.
15. **Retention:** none now; a planned system-wide 13-month policy will cover all diffpilot data (RQ14, RQ22).

## Alternatives considered
| Alternative | Why not |
|---|---|
| The coordinator uploads its own verifications | owner model: one assessment source, diffpilot's Codex; external agents only execute (RQ20) |
| Track every PR the user touches | owner model: on demand, plus an explicit per-user Watch for PRs being worked on |
| GitHub webhooks for watched PRs | a public inbound endpoint and App webhook configuration; the probe with plug-in signals is enough at this scale |
| E-mail or Web Push as the push channel | needs SMTP or VAPID keys and a service worker; the owner already uses ntfy, and a webhook covers Home Assistant |
| Strict SSRF filter (deny private addresses always) | the targets the owner uses live on the LAN; hosts are configured by the user or the operator; denial stays an operator switch |
| Full read of every watched PR per interval | costs a threads/reviews/comments read per PR; the batched probe reads three counters and two ids |
| diffpilot jobs fix code, push, reply | the runner is read-only with no code execution (brief §2, T3); a prompt-injected PR would gain write access |
| A separate ask box on the question card | the owner wants one Ask, the floating chat of Flows and Files; without an analysis a question pass answers into it |
| A third agent-session kind for comments | ADR-004 allows analytic and Ask sessions only; a context pack per job is enough |
| Server posts with the owner's App user token | the owner chose the App bot (RQ2) |
| Free-text shorthand ("C01 fix, C02 reject") | owner chose per-row selects with one submit (RQ21) |
| Keep triage-findings with a cross-PR index | no memory across rounds, no queue, actions lack D4–D8 and PR-level questions |

## Consequences
- CCRs: `JobKind`, `JobTarget`, `select_model` arm, runner path (ADR-018 pattern); routes; one migration; ADR-019,
  ADR-020 and ADR-022 amendments; phase 3 ADR-005 amendment and an App permission each installation owner accepts.
- The progress screen becomes part of the PR page; `/analyses/:id/progress` redirects; brief §4 gains an amendment.
- Watching costs GitHub points per interval and LLM tokens per detected change, both bounded and shown to the user.
- Implementing agents need a small feed client (claudius scripts can execute effects); the decision log makes the
  owner's agreement with recommendations and the repeats measurable (WM U1, RR R11(b)).

## Amendment (2026-11-22, WP13-WATCH and SK1)
- **Watch.** A probe every 5 minutes (ADR-020 scheduler task) reads up to 10 watched PRs per aliased GraphQL query
  (head, `updatedAt`, thread, comment and latest-review counts, state) with the watcher's token; only a PR whose
  values changed is read in full. A change makes an automatic pass due (`pr_watches.pass_due_at`, migration 0023);
  it waits while thepastaclaw reviews the PR (its status plug-in or its gate comment in progress or queued on the
  head), for at most 90 minutes from the change, then goes through `request_pass(Watch)`: one pass per PR, 10-minute
  spacing, 80 passes per user and 24 hours (shown in RW2; spike S13-3). A plug-in change signal (PR Hygiene, the end of a
  thepastaclaw round) probes its watched PRs at once. A user whose GitHub rate limit is used up is not probed until
  GitHub's reset or retry time. A merged or closed PR ends the watch; a user without a usable token pair pauses.
- **Questions ready.** A watch pass that leaves undecided questions (or a due change an on-demand pass already
  assessed) writes one `questions_ready` user event per round and watcher (`pr_watches.notified_for`: `round:<n>`,
  or `head:<sha>` before any bot round); delivery to channels is the notification task's work. RW3 streams them as
  SSE events named `user_event` and `heartbeat` (`UserStreamEvent`).
- **SK1** (superseded 2026-11-23: no skills, ADR-002 amendment of that date). `verify_comments` reads the claudius
  skills `severity` and `check-pr-comments` (ADR-002 amendment); each assessment keeps the claudius commit it was made
  with as provenance, never as a reason to re-assess (owner Q28 = c).
- **"Assess again" while a pass runs** answers 409 `conflict` instead of being dropped.

## Amendment (2026-11-23, owner decisions on the comparison report R2, R3, R8, R12, R13 and spike S13-3 O10)
- **Input** (`VerifyItem`, all optional and left out when empty): `commit`, the commit the comment was written on, from
  the read's `written` fact that the queue shows (the head of the bot round whose own review holds it, else its review's
  commit); `bot_status`, the bot's own thread status; `replies` (at most 8, 600 characters each, author kind `human`,
  `bot` or `owner` = the PR's author) and `earlier_replies`, from the replies the Q-L2 read already holds, for open
  threads. `EarlierDecision.decision` is null when a reply in the item's thread records the maintainer's decision
  ("maintainer decision", "deferred", "won't fix"); `reason` then quotes it. Bot bodies lose HTML, agent prompts and
  `<details>` sections other than findings (CodeRabbit's nitpick and outside-diff sections with their file sections keep
  their text, as claudius check-pr-comments takes each finding as an item) and suggested fixes (summary and code; a
  section that holds one keeps the finding it belongs to); provenance and metadata sections are dropped; code is kept as
  written; the description loses CodeRabbit's summary and the PR Hygiene block. Stored texts and hashes are unchanged.
- **Prompt v4** states the gates, the category catalogue, the necessity rule and the checking rules itself and the
  job mounts no skill (ADR-002 amendment 2026-11-23). `resolve` names a commit in `<commit>..HEAD`.
- **Fingerprint** format 2: purpose hash of the cleaned description, the item's comment, its bot status class, the
  replies of people, whole (the snapshot keeps them uncut; the input cuts them), which bots conceded in a reply and its
  lines. Bot reply text is not part of it: thepastaclaw re-reviews and the maintainer's agent answers every open thread
  each round, so a hash of their text, or of their first lines, changes at every head. A bot without a status marker
  (CodeRabbit) withdraws a point only in words, so a bot reply whose first line concedes ("you're right", "thanks for
  the clarification") counts by its author. The prompt version is not part of it and is not compared: a new prompt alone
  re-assesses nothing (owner decision 2026-11-23; v1.3 migration 0030 cleans all earlier assessments in place,
  so no earlier format needs a match; [ADR-025](ADR-025-container-runtime.md)). The pass is charged to its user as before.
- **Logs.** `Usage.requests` (job result) estimates the model requests of a call from the provider's events (one per
  batch of tool calls and one for the answer); the server's `job token usage` line and the runner's session summary
  name model, effort, requests and, for `verify_comments`, the item count.
- **Public text** (first real use, dashpay/rust-dashcore#1336). `recommendation.reason` and `title` pre-fill a
  decision's posted reason and follow-up title, and `reply_drafts` are replies; prompt v4 keeps notes about the
  assessor, its data handling and checks it did not run in the owner-facing texts. Ingest drops each sentence of those
  three fields that hits `analysis::verify::wording::ASSESSOR_NOTES` and logs the rule; the current rules apply again
  whenever a stored assessment is read for display or pre-fill, since assessments are reused across builds. The rules
  target only the assessor's own process (first person, "this assessment", its input, an agentless "no tests were
  run" or a bare "tests were not run" sentence): CI, prompt and agent vocabulary is ordinary review text in the
  owner's projects. The guard is a best-effort denylist; a passive or translated note can pass, and the owner's preview
  of the pre-filled text is the gate. A reason or TODO title that was only such sentences is left for the owner to
  write and the assessment is kept, so the item is not sent again. Dropping the sentence rather than rejecting the
  item: a rejected item would lose its evidence and gate flags and be sent again under the same prompt.
- **Conventions.** The repository's convention files at the base commit (`AGENTS.md`, `CLAUDE.md`, `CONTRIBUTING.md`,
  `.github/copilot-instructions.md`, …) are the binding rules for comments about convention or style and are cited
  as evidence. They stay data for the job's own output rules, and the PR's changes to them are not rules. The
  checkout already holds the base commit, so the input carries nothing more.

