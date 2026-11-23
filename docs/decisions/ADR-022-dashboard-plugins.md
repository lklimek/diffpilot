# ADR-022: Dashboard plug-ins — external per-PR statements; PR Hygiene and thepastaclaw status

- Status: accepted (owner request v1.2, 2026-11-21: plug-in interface and PR Hygiene; contract and route changes
  approved; coordinator decisions on the Ready to merge cross-check and on polling only while the dashboard is in
  use). Plan `docs/v1.1-dashboard-plan.md` §L. Extends ADR-017 (data sources) and ADR-020 (refresh machinery).
- Amendment v1.3 (2026-11-22, `docs/review-workflow-plan.md` §2.8, §8, RQ16): second plug-in thepastaclaw status
  (§10); watched pull requests are a second presence source (§3).
- Amendment DR1 (owner, 2026-11-22; plan K15, K17): the held-back list is removed (a held pull request stays in the
  ranking with its note, §6); a statement may say whose move a pull request is, which decides Waiting on others
  (K15 W3b, §11).
- Overrides `CLAUDE.md` "no abstraction beyond the seams the brief names" for exactly this seam.

## Context
The dashboard knows only what its GitHub scan reads. Some organisations run their own PR process engine whose
verdict explains why a pull request is blocked: PR Hygiene (`https://pr-hygiene.services.dash.org`, code
`dashpay/stale_prs_are_bad` `service/`) publishes per PR an engine state, stage, blockers and the next action.
Today diffpilot only sees its "hygiene" check as a policy gate (`score::text::is_policy_check`) without the reason.
The owner wants (a) the reason on work items, (b) a cheap change signal that refreshes just the changed pull requests
instead of waiting for the next full scan, without more GitHub searches, and (c) Ready to merge (K17) to respect
the engine's verdict. Further sources of the same kind are expected, so the owner asked for an interface.

## Decision
1. **Seam.** `server::dashboard::plugin::DashboardPlugin` (trait object, `BoxFuture` like `Ranker`):
   `id()` (stable, URL-safe), `name()` (shown before every statement), `home()` (link of its status),
   `poll()` → `Unchanged` or `Data { as_of, prs }`. Per pull request a plug-in may return a `PrStatement`:
   a note (summary, details, labelled facts, evidence URL), a Ready to merge opinion (`agrees` or `holds` with a
   reason) and a change mark. A pull request without a statement is not covered. Plug-ins live in
   `server::dashboard::plugin::<name>`; wire types (`PluginNote`, `AwaitingReason`, `PluginStatus`) in
   `contracts::api::dashboard`. Dashboard core (view, score, refresh) depends on the host's output types only, so a
   second plug-in adds a module and a config line, nothing else.
2. **What a plug-in may do.** Read its own external source over HTTP(S), unauthenticated. It never receives a
   GitHub token, a user token, a session, user data or the list of users; it never writes anywhere; it has no
   database access; nothing it returns is stored (in-memory only, lost on restart, rebuilt by the next poll).
   It may not change scores, tiers, tags, facts, judgements or the LLM's inputs; its only effects are its attributed
   notes, a Ready to merge hold (point 6), whose move it is for Waiting on others (point 11) and change signals
   (point 5).
3. **Lifecycle.** The host (`PluginHost`, in `AppState`) is built from configuration at start-up; one in-process
   task polls every plug-in at most once per 60 s server-wide (never per browser tab or user), and **only while a
   user is actively using the dashboard**: a summary (R1) or items (R1b) read in a browser session marks the reader
   active for 3 minutes (a personal access token read does not: an automated client is not a user in the session);
   with nobody active the task sends no request at all. **Watch presence (v1.3):** the task also polls while any user
   watches a pull request; the watch list reports this through `PluginHost::set_watching` (off until it reports, so
   without the Watch feature only dashboard reads count). When a user becomes active, or a first watch appears, and
   the last poll is older than the interval, the next poll starts at once. The browser re-reads the summary every
   60 s only while the summary lists a plug-in and the tab is visible, which keeps an open dashboard active and shows
   new statements; while a listed plug-in is `waiting` or `stale` it re-reads every 5 s, at most 6 times in a row, so
   a returning reader sees the statements of the poll their visit started within seconds. Ready to merge names a
   plug-in whose statements are not applied: `waiting` ("PR Hygiene: being read") when it was not read yet or its
   data only aged while nobody used the dashboard, `stale` ("PR Hygiene: unavailable or out of date") only after a
   failed poll or for data older than the 1 hour bound; nothing for a current one.
4. **Freshness and failures.** A plug-in's data is used only while fresh: its last successful poll (200 or 304) is
   at most 5 minutes old and its `as_of` at most 1 hour old. A timeout, a non-2xx answer (503 before the source's
   first snapshot included), an answer over 8 MiB or malformed JSON is a failed poll: the previous data ages out, the
   dashboard is otherwise unaffected. A failure, a recovery and a stale/fresh change are logged once per change of
   state, never per poll; so is the count of source records left out as unreadable (shape or repository name). Stale data, an unconfigured plug-in and an uncovered pull request all behave exactly as
   without the plug-in.
5. **Change signal → targeted re-read.** After a poll with new data, the host compares each pull request's change
   mark with the previous poll's (a statement added, changed or removed; not on the first poll). For every active
   user whose current snapshot holds a changed pull request, the host re-reads at most 40 of them with that user's
   token through the scan's GraphQL details query (no search, no enumeration, no context reads), rebuilds their facts
   with the stored context (relations, stack relations, ownership verdict, milestone catalogue kept from the scan),
   drops pull requests GitHub reports closed or merged, and writes the snapshot back only while it is still the
   user's current one (`db::repo::dashboard::replace_current_snapshot`; a newer scan wins). Skipped while the user's
   refresh is scanning or rate-limited. No ranking job is created: judgements are reused at read time as before, and
   the next full refresh re-judges by the usual fingerprint rule. GitHub search usage does not change.
6. **Ready to merge cross-check** (coordinator decision; DR1): a pull request K17 would list and a fresh plug-in holds
   (PR Hygiene: governed, engine state other than `ready-to-merge`) is left out of `ready_to_merge`; it stays in the
   ranking only, with the plug-in's note (no held-back list). Uncovered pull requests, `agrees`, stale data and an
   unconfigured plug-in leave K17 as today. A plug-in never adds a pull request to Ready to merge.
7. **Trust and wording** (brief §1). Every statement is shown as an attributed external statement ("PR Hygiene: …"),
   never as a diffpilot badge, tag, fact or verdict; no numeric confidence, no "verified"/"no risk". Each note links to
   its evidence: the source's record of the pull request, built by the server from the configured base URL and the
   validated repository and number. External text is plain text: control characters and invisible format characters
   (bidi controls, zero-width) removed, summary and each detail cut to 300 characters, at most 8 details and 6 facts
   (values 100 characters). The source is untrusted data.
8. **Configuration.** One variable per plug-in, unset = off (today's behaviour exactly, no request, no new field on
   the wire). PR Hygiene: `SR_PR_HYGIENE_URL` = the service's base URL (`http`/`https`). thepastaclaw status:
   `SR_THEPASTACLAW_STATUS_URL` = the URL of its `status.json` (`http`/`https`). No secret is involved and none is
   sent.
9. **PR Hygiene mapping.** One `GET /api/v1/prs` (all open pull requests of its covered repositories) and one
   `GET /api/v1/stages` per poll, each with `If-None-Match`; both 304 → `Unchanged`. Pull requests of a repository
   in `stale_repos` (stale or `fetch_error`) and ungoverned ones (`engine_state` null) get no statement. Note: summary
   = `next_action`, details = `blockers`, facts = stage, whose move (the stage's owner from `/stages`) and lateness;
   evidence `<base>/api/v1/prs/{owner}/{repo}/{n}`. Opinion: `agrees` for `ready-to-merge`, else `holds` with
   `next_action` (or the stage) as the reason. Change mark: the record minus time-only values (`lateness`, `idle`).
10. **thepastaclaw status mapping** (v1.3, `server::dashboard::plugin::thepastaclaw`). One `GET` of the status file
   (`https://thepastaclaw.github.io/review-system/data/status.json`, GitHub Pages, `ETag`) per poll with
   `If-None-Match`; 304 → `Unchanged`. `as_of` = `data_as_of` (the file trails by up to about 20 minutes, inside the
   1-hour bound; required, else the poll fails as malformed). A pull request in `live.active[]` gets summary
   "reviewing (<phase>, step <n> of <m>)" (n = steps started, m = n + upcoming; the fractional `progress` is not
   shown: brief §1 allows no percentages) and facts Since (`started_at`, UTC) and Head (short `sha`); one in
   `live.queued[]` gets "queued (<reason>)" and facts Eligible (`eligible_at`, UTC), Position and Head; one in both
   (a newer head queued during a run) keeps the reviewing summary with a detail "queued again for head …". A pull
   request in neither list gets no statement. Evidence: the human status page (the directory above the file's,
   `review-system/`) at `#run=<id>` for a run, `#queue` for a queued one. **No Ready to merge opinion**: the status
   of a bot's review says nothing about readiness. Change mark: reviewing with run id and head, queued with head;
   phase, steps, progress and clocks are left out (they advance within a run, and the browser's summary re-read
   already shows them), so a finished round (the pull request leaves `live.active`), a new run and a new queued head
   are change signals. Records of another shape or an invalid repository name are left out and counted (logged once
   per change of the count).

11. **Whose move** (DR1, K15 W3b): a statement may say the move is the author's or the reviewers'/maintainers', with
   the logins owing a review. PR Hygiene: the stage's owner from `/stages` (`author`; `reviewers` or `maintainers`
   with `asks[].approvers`, valid GitHub logins, at most 10, once each; `bots` and `nobody` say nothing) and the same
   reason as its hold. A fresh statement of the reviewers' or maintainers' move lists the user's pull request under
   Waiting on others (reason "PR Hygiene: …" with the evidence link) when nothing is the user's move; one of the
   author's move adds nothing and vetoes nothing (neither the ready label nor a requested review). The judge's input
   applies the same rule without statements, so every ranked pull request is judged and the input stays GitHub's.

## Alternatives considered
| Alternative | Why not |
|---|---|
| Hard-wire PR Hygiene into the scan | owner asked for an interface; a second source would need core changes again |
| Per-user queries (`?author=`, `?reviewer=`) | one request per login and minute instead of one 304 for all; the full list is ~260 KB and cached by ETag |
| Browser polls the service directly (CORS `*`) | per tab, leaks the user's interest to a third party, no change signal on the server |
| Full refresh on every change | raises GitHub search usage and LLM ranking cost |
| Store plug-in data in the database | not needed: statements are recomputed on read from the in-memory poll; no migration |
| Poll continuously | owner: no requests to the service while nobody uses the dashboard |
| Plug-in verdict replaces K17 | an outage or uncovered repository would empty Ready to merge; a hold-only cross-check keeps today's behaviour as the floor |

## Consequences
- `WorkItem.plugin_notes`, `DashboardSummary.plugins` (omitted when empty, so the wire is unchanged with no
  plug-in), `AwaitingReason::Plugin` (DR1; `SnapshotMeta.ready_to_merge_held` and `HeldBack` were removed); the
  snapshot document gains `prs_reread_at` (default absent, no version bump); the summary/items revision includes the
  notes, the plug-in waits and, with them, the Ready to merge list, so a new statement changes the `ETag`.
- One more outbound HTTP destination from the server (not the runner) per configured plug-in; with nobody active and
  nothing watched, none.
- Targeted re-reads use the user's GitHub GraphQL budget (one query per 10 changed pull requests) only while that user
  is active.
