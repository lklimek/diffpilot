# Database schema (summary)

> v1.3 common amendment: [container contracts](containers.md) and [ADR-025](../decisions/ADR-025-container-runtime.md)
> govern payer/container identity, gateway boundaries, in-place 0030 cleanup and report 4.2. Runtime integration is
> owned by W3/W4/W5/W8; the older runtime description below applies until those packages replace it.


Source of truth: `crates/db/migrations/` (`0001_init.sql`, then additive migrations that upgrade an existing
database in place and keep its data: `0002_flow_auto_retries.sql`, `0003_dashboard.sql`, `0004_access_tokens.sql`, `0005_ranking_attempt.sql`, `0006_ask_effort.sql`, `0007_jobs_targets.sql`, `0008_ranking_sent.sql`, `0009_known_bots.sql`,
`0010_language_ru.sql`, `0011_hidden_mentions.sql`, …, `0017_review_loop.sql`). An applied migration file is never edited
(sqlx compares its checksum at start). A server binary older than the database (an applied migration it does not
know) refuses to start; going back needs a database copy of that version. SQLite in WAL mode, foreign keys on, `STRICT` tables, timestamps as RFC 3339 UTC text. Only
`crates/db` uses sqlx; everything else goes through the repository API (`crates/db/src/repo/*.rs`, one module per
aggregate) and sees `DbError {NotFound, Conflict, NotImplemented, Json, Storage}` — sqlx types stay private.

## Tables
| Table | Holds | Key constraints |
|---|---|---|
| `tenants` | tenants; `default` seeded | – |
| `tenant_allowed_logins` | sign-in allowlist (ADR-007); made equal to `SR_ALLOWED_LOGINS` at every start (v1.2; migration 0016 drops the unbound seed rows) | login `NOCASE`; `github_id` bound once at first sign-in with `bound_at`; `UNIQUE(tenant_id, github_id)` |
| `users` | GitHub users (id, login, name, avatar, LLM language `en`, `pl` or `ru`; `ru` since migration 0010) | no `tenant_id` (memberships) |
| `memberships` | user ↔ tenant | – |
| `oauth_logins` | OAuth state + encrypted PKCE verifier, short-lived | – |
| `sessions` | cookie sessions, token hash only, CSRF token | – |
| `user_tokens` | GitHub user tokens, AES-GCM ciphertext | `version` = CAS counter of the serialised refresh |
| `repos`, `pull_requests` | known repositories (Settings list) and PR snapshots; PR-level analytic `agent_thread_id`, `analytic_generation` (default 0), and `session_state`, shared across analyses; PR id uses AUTOINCREMENT for archive keys | – |
| `analyses` | one analysis per PR head and language: state, progress, skills commit, `round` (incremented by each Retry), nullable `user_paused_by` and `user_paused_at` | partial unique index: one live (not cancelled) per (PR, head, language) |
| `documents` | validated agent output and pre-pass as JSON: `prepass`, `plan`, `pr_context`, `flow` (key = flow id), `github_snapshot` (threads, checks), `unsent` (lines the server did not send to the LLM with their reason; key `plan` or a flow id), `memcan_profile` (the analysis's MemCan project profile lookup: `not_checked`, or `checked` with the bounded profile text or none; a Retry looks a `not_checked` one up again); `dependencies` (the Cargo map and conversion facts of the head, ADR-021; migration 0015 widened the kinds in place); `github_access` (an accepted agent output's attempt ran with an installation token; NULL for server-built documents) | `json_valid` |
| `controlled_owners` | installation-wide controlled GitHub owners (ADR-021, migration 0015) and, in `controlled_owners_changed`, who changed the list and when | owner `NOCASE`; PK `(tenant_id, owner)` |
| `llm_selection` | singleton installation model, reasoning effort, dashboard ranking `judge_effort` (default `low`, migration 0003), Ask jobs' `ask_effort` (default `medium`, migration 0006), changer login and time | primary key must be 1; effort low/medium/high; no tenant scope |
| `llm_credentials` | credential ids runners hold (never the secret), pause, unusable, last call outcome | kind `codex_chatgpt` |
| `jobs` | the queue: owner `analysis_id` (analysis jobs) or `refresh_id` (dashboard `judge_work` jobs; cascade on refresh removal; ADR-018, migration 0007 rebuilt the table in place keeping every row), kind, priority (0 Ask and ranking, 1 batch), state, spec (no secrets; `target` names the pull request or refresh), attempt/max, lease, job-token hash, cancel flag, `resumes_at`, `ready_output` + `ready_output_attempt`, result, `stage` (`plan:<n>`, `pr_context`, `flow:<id>`, `ask:<id>`, `judge:<refresh>:<batch>`), `generation` (0 first run, 1..2 repair runs), `round` (the analysis's round, or the refresh's ranking attempt, at enqueue), `github_access_granted` (the last claim got an installation token; the queue executor reports it with the result) | exactly one owner; `judge_work` exactly for refresh-owned jobs; partial unique indexes: one claimed/running job per analysis and per credential (so judge and analysis jobs never run together), one live or succeeded job per (analysis, stage, generation, round) and per (refresh, stage, generation, round); PR-level insert/update triggers exclude simultaneous claimed/running jobs across analyses and credentials; claim-order, by-analysis and by-refresh indexes; a judge job is claimed only while its refresh is `ranking` and its `round` is the refresh's current `ranking_attempt` |
| `installation_tokens` | AES-GCM ciphertext and expiry of the installation token per (job, attempt), kept until revocation is confirmed | PK `(job_id, attempt)`; listed for revocation once the attempt is no longer claimed or running |
| `job_events` | bounded activity log (last 500 per job) | PK `(job_id, attempt, seq)` |
| `flow_work` | durable flow demands and failures; analysis background switch lives on `analyses` | PK `(analysis_id, flow_id)`; cascade on analysis removal |
| `review_states` | ticks and last viewed block per user and analysis | – |
| `questions` | private Ask questions and answers; lists and quotas scoped by authenticated user; fallback history spans the PR, latest 20 final answers; anchor by `kind` (migration 0012 rebuilt the table for step and lines anchors, 0014 again for `kind`, `flow_id` and `parent_id`, every existing question keeping its kind): `step` (`step_id`), `lines` (`path` + `side`), both with optional `start_line`/`end_line`, `flow` (`flow_id`) or `pr`; `touched_steps` (0013): JSON step ids of a lines question, chosen at submit; `parent_id`: a follow-up's thread's first question, which it repeats the anchor of | `user_id` selects the owner; `step_id`, `path` and `flow_id` set exactly for their kind; `side` exactly with `path`; `start_line` and `end_line` together, `1 ≤ start_line ≤ end_line`, only for `step` and `lines`; `parent_id` references `questions` (cascade), never the row itself; same analysis and user checked by the server; index `questions_by_parent` |
| `working_sessions` | private Ask thread and fork's analytic generation, with `tenant_id` | PK `(pull_request_id, user_id)`; cascading PR/user foreign keys |
| `feedback` | one entry per (user, analysis, target, value) (CCR-F1); `revision` (+1 per replacement or removal); `memcan_state` pending/written/failed of the current revision (exposed as `Feedback.memory`); `removed_at` marks a removed entry kept until MemCan deleted its memory; `attempts` / `next_attempt_at` schedule retries per entry; `unsettled_until` is the end of the settle period of a write with unknown outcome | `target_key`: step id, annotation id, or `<side>:<line>:<path>` for lines; `overrated` / `underrated` exclusive per (user, analysis, target) |
| `comment_requests` | pending-review idempotency: request hash + response per (user, key) | – |
| `dashboard_settings` | per-user dashboard settings (v1.1, plan §F.6): JSON lists of linked logins, enumerate owners, `repositories` (`RepositoryRule[]`), excluded authors (`skip_authors`), known bots (`known_bots`, migration 0009, default the three review bots), extra MemCan projects, Slack names (`slack_names`, `SlackName[]`, migration 0027, default `[]`); look-back days, ready label, TODOs and schedule switches, `schedule_time`, IANA `time_zone` (null until known); the last scheduled run (`last_scheduled_on` local date, `last_scheduled_outcome`, reason) | PK `user_id` (cascade); absent row = defaults; outcome and date set together |
| `dashboard_refreshes` | one scan + ranking per row (states: `http-api.md` "Refresh states"): trigger (`manual`, `scheduled`, `automatic`: migration 0029 rebuilt the table, with `dashboard_judgements` and `jobs` and its children, keeping every row), state, `progress` `{steps, percent}`, `log` (≤ 200 lines), failure code/message/`retry_at`, `ranking_state`, judged/reused/left-provisional counts, `ranking_attempt` (migration 0005; server-internal, not on the wire), `sent` (migration 0008; server-internal: items sent to judge jobs across the refresh's attempts, reserved before each job against both ranking caps), `snapshot` (one validated JSON document, null until scanned), start/scan/end times | partial unique index: one `scanning`/`ranking` row per user; `failed` exactly when a failure code is set; snapshot and ranking state exactly on `ranking`/`finished` rows (live ranking states on `ranking`, ending ones on `finished`); newest 10 per user kept, plus the one holding the latest snapshot and every one started in the last 48 h with `judged` or `sent` > 0 (the daily ranking cap sums `sent` of the refreshes that ranked in the local day: started, live or ended in it); `ranking_attempt` ≥ 0, incremented by Retry ranking, and ranking results update the refresh only for the current attempt (a late result of an earlier attempt or a superseded ranking is stale and only raises `judged`; a newer attempt or a refresh that never ranked is a conflict) |
| `dashboard_judgements` | stored LLM judgement per (user, item key in its normative form, `http-api.md` "Item keys"): judged-input fingerprint (SHA-256 hex), input and prompt versions, `body` JSON, refresh (set null when deleted), job id (no foreign key), model, `judged_at`, `last_seen_at` (deleted 90 days after) | PK `(user_id, item_key)` |
| `dashboard_hidden_mentions` | per-post hides (migration 0011, STRICT), `hidden_at`; independent of refresh history | PK `(tenant_id, user_id, mention_id)`; user cascade |
| `access_tokens` | personal access tokens (ADR-019): public 8-character `id`, name, SHA-256 `token_hash` (32 bytes, never the token), `scopes` JSON, created/expiry/last-used/revoked times | `token_hash` unique; name unique per user among tokens not revoked; cascade with the user |
| `comment_prs` | review loop (ADR-023, migration 0017): per pull request the last GitHub read (time, failure, `github_snapshot`), bot `round_count`, `next_item_no` (`C<no>` never reused), the latest pass's `purpose` (LLM) | PK `pull_request_id` (cascade); failure and its time set together |
| `comment_items` | items (thread, review-body item, PR-comment item, PR-level question): `item_no`, `stable_key` (`dedupe=` marker, thread id, review id + ordinal, `pr:<round>:…`), author and kind, location, title, body, `body_hash` (normalised text, spike S13-1 K1), first head and round, bot severity, `gate_marks` JSON, `github` facts JSON, `state`, `revision` (+1 per change), `outside` JSON, `repeat_of` | `UNIQUE (pull_request_id, item_no)` and `(pull_request_id, stable_key)`; `state = 'outside'` exactly with `outside`; lines need a path |
| `comment_assessments` | LLM assessments (`CommentAssessment` JSON) per item and pass with `fingerprint`, job id (no foreign key), head, prompt version; the newest is current | index `(item_id, id)` |
| `comment_passes` | `verify_comments` passes: PR, charged user, cause, head, items, state, job | one `queued`/`running` pass per PR (partial unique index); `finished_at` exactly when ended |
| `comment_decisions` | decisions, append-only (an `UPDATE` trigger aborts): kind, reason, `target`/`override`/`recommendation` JSON, `resolve_ticked`, reply text, the owner's `comment`, `via`, `carried_from`, `supersedes`, head, the item's `body_hash` and lines at decision time | `via = 'carried'` exactly with `carried_from`; a decision is superseded at most once |
| `comment_effects` | effects per decision in `seq` order (`after_seq`), kind, executor, `params` JSON, state, claim (`claim_token_id`, `lease_until`), reference, message, `seen_on_github_at` | `UNIQUE (decision_id, seq)`; `claimed` exactly with a claim |
| `comment_effect_reports` | agent reports per effect: token, outcome, reference, message, idempotency key | `UNIQUE (token_id, idempotency_key)` |
| `comment_carry_events` | carried decisions and repeats asked again with reason and `evidence` JSON | `asked_again` exactly with a reason |
| `agent_subscriptions` | feed subscriptions of `review_loop_execute` tokens: capabilities, last seen, optional push target (kind, URL, topic, `push_secret` ciphertext) and its delivery health | PK `token_id` (cascade); push fields only with a kind |
| `pr_watches` | watched PRs per user: since, last probe values and time, latest automatic pass, `state`, `ended_at`; `pass_due_at` (an automatic pass is due since; the 90-minute wait for the bot's round counts from it) and `notified_for` (the round of the last "questions ready" event) (migration 0023) | PK `(user_id, pull_request_id)`; `ended` exactly with `ended_at`; the 20-PR limit is checked by the repository |
| `user_events` | per-user events (`questions_ready`, `round_executed`, `test`) with count, text, link | `test` exactly without a PR |
| `notification_channels` | a user's ntfy and webhook channels: URL, topic, `secret` ciphertext, events, on/off, delivery health (last delivery, last error, failing since) | `UNIQUE (user_id, kind)` |
| `notification_deliveries` | one row per (user event, channel) or (decision, agent token): attempts, state, next attempt, HTTP status, error | exactly one source and its matching target; `pending` exactly with a next attempt; once per pair (partial unique indexes) |

Tenant-owned tables carry a `NOT NULL tenant_id` (schema test). Decision origins inherit ownership through their
parent. User-wide assessment configuration and quota and installation-wide infrastructure have separate scopes.
`CHECK` lists mirror closed contract enums.

## v1.4 migrations

| Migration | Storage | Constraints |
|---|---|---|
| 0031 | `github_deliveries` retains the delivery id, body SHA-256, event, action, installation id, typed change list, state, attempts and timestamps. `app_repositories` stores the GitHub repository id, installation id, suspension and check time for tracked repositories. | Installation-wide. Delivery id is unique. Received time and body hash with received time are indexed for retention and replay checks. The repository map is keyed by case-insensitive owner/name with a unique numeric GitHub repository id for lookup after a rename. Delivery state `not_allowed` records untracked repositories. Delivery errors use typed codes. |
| 0032 | `dashboard_sends` records reader, refresh, attempt, item count, job and send/release times. `dashboard_ranking_resets` records the reader's latest reset. `dashboard_judgements.rejudge_requested_at` is nullable. | Back-fill one row per refresh with `sent > 0` started within 48 hours, using its start as send time. Retain ledger entries across refresh and job pruning. Day usage will count unreleased sends since local midnight or the last reset, whichever is later. |
| 0033 | `comment_items.status_comment` defaults to 0. `comment_decision_origins` records imported GitHub evidence or the primary decision a follower uses. | The flag is 0 or 1. An origin belongs to its decision. A trigger rejects updates, and deleting the decision cascades to its origin. The stored `via` for these rows is `question`; the origin selects the wire variant. |
| 0034 | `pr_page_openings` holds `last_opened_at` once per tenant member and PR. | Composite foreign keys require the member and PR to belong to the same tenant. Removing either parent removes the opening. |

## Feedback → MemCan
The single writer (`server::memcan::writer`) takes the entries due at `now` (`pending_memcan(now, limit)`: not
`written`, `next_attempt_at` unset or reached; fewest attempts first, then oldest) with their `revision`, writes
them and reports `record_memcan_outcome(id, revision, outcome, now)`, which applies only if the revision is still
current; a replacement or removal made meanwhile stays pending (`MemcanAck::Superseded`) and is written next. The
MemCan memory id is the feedback id: a live entry is stored with MemCan's `_import_records` (synchronous embed and
upsert by id, no LLM rewriting; metadata `feedback_id`, `revision`), so retries are idempotent and the newest
revision replaces the memory in place (also for overrated / underrated calibration examples); a removed entry's
memory is deleted with `delete_memory`. An operation counts only on MemCan's exact answer: import
`{"imported":1,"skipped":0,"errors":[]}`; delete `{"status":"deleted","memory_id":<id>}`, or the invalid-params
error `delete_memory failed: memory not found: <id>` (never written or already deleted).

Outcomes: `Confirmed`; `Failed` (MemCan rejected the call: `failed`, `attempts + 1`, retry at a back-off of 30 s
doubling to 30 min per entry, so one failing entry never blocks the others); `Unknown` (timeout, broken
connection or unexpected answer: as `Failed`, and `unsettled_until` is raised to now + the settle period of ten
call timeouts, 200 s, also for a superseded revision). MemCan may still carry out an operation of unknown outcome,
so a confirmation before `unsettled_until` is not final: the entry stays `pending` until then and the operation is
repeated once, so a late import cannot revive a removed memory and a late older revision cannot outlive a newer
one. A settled confirmation sets `written` (live) or deletes the tombstone row (removed). When MemCan is
unreachable or times out, the sweep stops and the writer backs off (30 s doubling to 30 min); a rejected entry
does not stop the sweep.

## Ownership
v1.1 frozen signatures (WP11-A; bodies return `NotImplemented` until filled): `dashboard` (WP11-DB; every function
takes `(tenant_id, user_id)` except the server-internal `schedule_candidates`, `interrupt_live` and
`delete_unseen_judgements`), `access_tokens` (WP11-TOKENS). `judge_effort` reads and writes in `settings` belong to
WP11-DB. Frozen cross-WP signatures (bodies return `NotImplemented` until filled): `jobs` (WP2-JOBS), `analyses`
(WP2-JOBS / WP2-INGEST), `documents` (WP2-INGEST), `feedback` (WP3-MEMCAN; WP3-REVIEW records
`missed_by_llm`), `llm_credentials` (WP2-JOBS). Other modules are owned and shaped by their work package.
Schema changes are change requests to the coordinator.

`analyses.failure_reason` preserves the optional API failure code, including `configuration`; state transitions
clear it together with the failure message. `llm_credentials.last_call_failure` accepts `configuration` for a
required agent skill that could not be provided. These remain additions to the single pre-deployment schema.

User pause keeps existing jobs and documents. The claim query excludes pipeline jobs of analyses with `user_paused_at` (private Ask remains available);
state updates preserve the user pause even when a provider limit arrives. Resume clears only the user
pause and keeps an unexpired job usage-limit pause. Jobs retain the model choice present at submission;
`models_used` groups the existing JSON specs of claimed jobs without another table or counter.

Analytic completion advances the generation even when it keeps the same thread id. Ask completion updates
only `working_sessions` using the job's durable question owner. A stale fork is not selected at claim. Successful
analytic output without an archive clears its reference and invalidates forks. Cancel/Re-analyze keep archives;
explicit PR removal or PR-wide retention deletes them.

Lazy review adds `analyses.background_analysis` (boolean, column default 1; a new analysis is inserted with 0,
existing rows keep their value) and `flow_work` keyed by analysis/flow,
with tenant, nullable requested timestamp and nullable plain error. Lazy scheduling adds no fields to stored
plan or flow documents; the separate concise-assessment amendment makes low assessment details optional.
Accepted flow documents override scheduling rows. Newest demand precedes plan rank; a failed flow is scheduled
again by background work until `flow_work.auto_retries` (integer, default 0, migration 0002) reaches 2, each
automatic retry counting one, and after that only on request; a requested flow that fails keeps `requested_at`
while the counter is below 2 (the failure counts one) and so runs again whatever the background switch; a flow with an `unsent` document (too large for
the agent request) only on request. A flow retry advances the job round only between serial pipeline jobs, in the
transaction that chose the flow; succeeded jobs of other flows that have neither a flow document nor a failure
(an output waiting for its postponed repair) move to the new round with it (`jobs.round`).

### `dashboard_hidden_mentions` (migration 0011)
Primary key `(tenant_id, user_id, mention_id)`, with `hidden_at` (UTC timestamp). Tenant/user foreign keys;
user deletion cascades. `mention_id` is SHA-256 of canonical item key and post anchor, so different posts on the
same item remain independent. Hiding is an idempotent insert; no read-modify-write transaction is needed.
Refresh pruning does not delete hides. Snapshot JSON gains `mentions` (default `[]` for old documents); reads
filter it with this user's hidden identities, then derive the revision. Ranking/judge never consume mentions.
