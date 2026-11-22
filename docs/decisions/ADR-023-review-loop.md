# ADR-023: Processing PR comments — PR page, Comments view, Codex assessment, Watch, decision feed for external agents

- Status: proposed (2026-11-22). Owner model and answers RQ1–RQ22 of 2026-11-22 recorded in
  `docs/review-workflow-plan.md` (§0, §11); RQ23 (notification channel without an open tab) open. Design in HTML
  first; no implementation before the owner's review.
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
   Dependencies and works without an analysis. Modules: `server::comments` (items, queue, effects, rounds, carry-overs,
   watch, feed), `server::github::comments` (reads), `db::repo::comments`, contracts `api::comments` and
   `agent::verify_comments`, web `components/comments/**`, `screens/pr/**`, `screens/comments/**`. No new crate.
2. **On demand, plus Watch.** Any pull request is processed on demand; diffpilot shows GitHub's state and records
   only what is decided and executed through it. A user may **watch** pull requests he is working on: for a watched PR
   the server detects new comments and reviews itself, runs the assessment at once and notifies the user; unwatched
   PRs are read only when a page asks.
3. **Codex assesses, never implements.** A new job kind `verify_comments` (`JobTarget::PullRequestComments`) runs on
   the existing `JobExecutor` and runner: checkout of the head, the read-only PR-scoped installation token, no session
   restore or save, no MemCan, no GitHub write. It writes per item the assessment, the recommendation and the code
   context, and answers the owner's questions. Unchanged items reuse their assessment. It is the only source of
   assessments (RQ20). Every detection that yields new items spends LLM tokens; this is bounded: one assessment job
   per PR at a time, debounced while thepastaclaw still reviews that PR, a minimum spacing per PR and a daily cap.
4. **Detection for watched PRs reuses refresh machinery** at bounded GitHub cost: one alias-batched GraphQL probe
   (10 PRs per request) per interval with the user's token, triggered also by plug-in change signals (PR Hygiene
   change marks, thepastaclaw round end) and by the daily scheduled refresh; a full read only for PRs whose probe
   changed. The token pair is kept for this under ADR-020's bound (30 days after the last sign-in).
5. **Decisions are the signed-in users' only.** Decisions, supersedes, gate marks, "Resolve this thread" and
   PR-level questions need a browser session with CSRF; no token can decide.
6. **Effects computed by the server.** A pure function maps (decision, author kind, source kind, gate, override) to the
   effects of WM §6.5, each with an executor and order. Gate threads (G-FUNDS, G-SECRET, G-CRYPTO, G-DATA) get a resolve
   only when the decision says so; "Accept despite the gate" is a reject with a gate override whose reason is posted
   and whose thread is always resolved (RQ6). Gate items are never grouped, never in "Decide the rest", and need their
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
11. **Notifications** are per-user events (questions ready, round executed) delivered to open tabs as browser
    notifications under the existing setting (RQ18); a channel for users without an open tab is a later consumer of
    the same events (RQ23 open).
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
| Full read of every watched PR per interval | costs a threads/reviews/comments read per PR; the batched probe reads three counters and two ids |
| diffpilot jobs fix code, push, reply | the runner is read-only with no code execution (brief §2, T3); a prompt-injected PR would gain write access |
| Ask (private session) for owner questions | needs an analysis; Comments must work without one |
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
