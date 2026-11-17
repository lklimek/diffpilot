# Database schema (summary)

Source of truth: `crates/db/migrations/0001_init.sql` (one file, rewritten in place until deployment; no
versioning). SQLite in WAL mode, foreign keys on, `STRICT` tables, timestamps as RFC 3339 UTC text. Only
`crates/db` uses sqlx; everything else goes through the repository API (`crates/db/src/repo/*.rs`, one module per
aggregate) and sees `DbError {NotFound, Conflict, NotImplemented, Json, Storage}` — sqlx types stay private.

## Tables
| Table | Holds | Key constraints |
|---|---|---|
| `tenants` | tenants; `default` seeded | – |
| `tenant_allowed_logins` | sign-in allowlist (ADR-007); seeds `lklimek`, `Claudius-Maginificent` | login `NOCASE`; `github_id` bound once at first sign-in with `bound_at`; `UNIQUE(tenant_id, github_id)` |
| `users` | GitHub users (id, login, name, avatar, LLM language) | no `tenant_id` (memberships) |
| `memberships` | user ↔ tenant | – |
| `oauth_logins` | OAuth state + encrypted PKCE verifier, short-lived | – |
| `sessions` | cookie sessions, token hash only, CSRF token | – |
| `user_tokens` | GitHub user tokens, AES-GCM ciphertext | `version` = CAS counter of the serialised refresh |
| `repos`, `pull_requests` | known repositories (Settings list) and PR snapshots; PR-level `agent_thread_id` and `session_state`, shared across analyses; PR id uses AUTOINCREMENT for archive keys | – |
| `analyses` | one analysis per PR head and language: state, progress, skills commit, `round` (incremented by each Retry), nullable `user_paused_by` and `user_paused_at`, `cancel_cleanup_done` | partial unique index: one live (not cancelled) per (PR, head, language) |
| `documents` | validated agent output and pre-pass as JSON: `prepass`, `plan`, `pr_context`, `flow` (key = flow id), `github_snapshot` (threads, checks), `unsent` (lines the server did not send to the LLM with their reason; key `plan` or a flow id), `memcan_profile` (the analysis's MemCan project profile lookup: `not_checked`, or `checked` with the bounded profile text or none; a Retry looks a `not_checked` one up again); `github_access` (an accepted agent output's attempt ran with an installation token; NULL for server-built documents) | `json_valid` |
| `llm_selection` | singleton installation model, reasoning effort, changer login and time | primary key must be 1; effort low/medium/high; no tenant scope |
| `llm_credentials` | credential ids runners hold (never the secret), pause, unusable, last call outcome | kind `codex_chatgpt` |
| `jobs` | the queue: kind, priority (0 Ask, 1 batch), state, spec (no secrets), attempt/max, lease, job-token hash, cancel flag, `resumes_at`, `ready_output` + `ready_output_attempt`, result, `stage` (`plan:<n>`, `pr_context`, `flow:<id>`, `ask:<id>`), `generation` (0 first run, 1..2 repair runs), `round` (the analysis's round at enqueue), `github_access_granted` (the last claim got an installation token; the queue executor reports it with the result) | partial unique indexes: one claimed/running job per analysis and per credential, one live or succeeded job per (analysis, stage, generation, round); PR-level insert/update triggers exclude simultaneous claimed/running jobs across analyses and credentials; claim-order and by-analysis indexes |
| `installation_tokens` | AES-GCM ciphertext and expiry of the installation token per (job, attempt), kept until revocation is confirmed | PK `(job_id, attempt)`; listed for revocation once the attempt is no longer claimed or running |
| `job_events` | bounded activity log (last 500 per job) | PK `(job_id, attempt, seq)` |
| `review_states` | ticks and last viewed block per user and analysis | – |
| `questions` | Ask questions and answers | – |
| `feedback` | one entry per (user, analysis, target, value) (CCR-F1); `revision` (+1 per replacement or removal); `memcan_state` pending/written/failed of the current revision (exposed as `Feedback.memory`); `removed_at` marks a removed entry kept until MemCan deleted its memory; `attempts` / `next_attempt_at` schedule retries per entry; `unsettled_until` is the end of the settle period of a write with unknown outcome | `target_key`: step id, annotation id, or `<side>:<line>:<path>` for lines; `overrated` / `underrated` exclusive per (user, analysis, target) |
| `comment_requests` | pending-review idempotency: request hash + response per (user, key) | – |

Every table except `tenants`, `users` and installation-wide `llm_selection` has a `NOT NULL tenant_id` (schema test). `CHECK` lists mirror the
closed contract enums (schema test compares them with the serde names).

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
Frozen cross-WP signatures (bodies return `NotImplemented` until filled): `jobs` (WP2-JOBS), `analyses`
(WP2-JOBS / WP2-INGEST), `documents` (WP2-INGEST), `feedback` (WP3-MEMCAN; WP3-REVIEW records
`missed_by_llm`), `llm_credentials` (WP2-JOBS). Other modules are owned and shaped by their work package.
Schema changes are change requests to the coordinator.

`analyses.failure_reason` preserves the optional API failure code, including `configuration`; state transitions
clear it together with the failure message. `llm_credentials.last_call_failure` accepts `configuration` for a
required agent skill that could not be provided. These remain additions to the single pre-deployment schema.

User pause keeps existing jobs and documents. The claim query excludes analyses with `user_paused_at`;
state updates preserve the user pause even when a provider limit arrives. Resume clears only the user
pause and keeps an unexpired job usage-limit pause. Jobs retain the model choice present at submission;
`models_used` groups the existing JSON specs of claimed jobs without another table or counter.

`cancel_cleanup_done` makes session deletion and document cleanup one durable transaction. Repeated
cleanup removes late documents but never drops a later analysis's new PR session. Claims wait for a
pending session deletion acknowledgement; deletion is offered only once no PR job is running.
