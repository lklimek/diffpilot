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
| `repos`, `pull_requests` | known repositories (Settings list) and PR snapshots | – |
| `analyses` | one analysis per PR head and language: state, progress, agent thread id, `session_state`, skills commit | partial unique index: one live (not cancelled) per (PR, head, language) |
| `documents` | validated agent output and pre-pass as JSON: `prepass`, `plan`, `pr_context`, `flow` (key = flow id), `github_snapshot` (threads, checks) | `json_valid` |
| `llm_credentials` | credential ids runners hold (never the secret), pause, unusable, last call outcome | kind `codex_chatgpt` |
| `jobs` | the queue: kind, priority (0 Ask, 1 batch), state, spec (no secrets), attempt/max, lease, job-token hash, cancel flag, `resumes_at`, encrypted installation token (for revocation), `ready_output` + `ready_output_attempt`, result, `stage` (`plan:<n>`, `pr_context`, `flow:<id>`, `ask:<id>`) | partial unique indexes: one claimed/running job per analysis and per credential, one live or succeeded job per (analysis, stage); claim-order and by-analysis indexes |
| `job_events` | bounded activity log (last 500 per job) | PK `(job_id, attempt, seq)` |
| `review_states` | ticks and last viewed block per user and analysis | – |
| `questions` | Ask questions and answers | – |
| `feedback` | one entry per (user, analysis, target); `revision` (+1 per replacement); `memcan_state` pending/written/failed of the current revision | `target_key`: step id, annotation id, or `<side>:<line>:<path>` for lines |
| `comment_requests` | pending-review idempotency: request hash + response per (user, key) | – |

Every table except `tenants` and `users` has a `NOT NULL tenant_id` (schema test). `CHECK` lists mirror the
closed contract enums (schema test compares them with the serde names).

## Feedback → MemCan
The writer takes `pending_memcan` entries with their `revision`, writes them and acknowledges with
`set_memcan_state(id, revision, state)`, which applies only if the revision is still current; a replacement
made meanwhile stays pending (`MemcanAck::Superseded`) and is written next. The MemCan memory is keyed by the
feedback id (metadata `feedback_id`, `revision`): the writer updates the existing memory of that id instead of
adding a second one, and never overwrites a memory carrying a higher revision, so retries and reordered writes
are idempotent and the newest correction wins (also for overrated / underrated calibration examples).

## Ownership
Frozen cross-WP signatures (bodies return `NotImplemented` until filled): `jobs` (WP2-JOBS), `analyses`
(WP2-JOBS / WP2-INGEST), `documents` (WP2-INGEST), `feedback` (WP3-MEMCAN; WP3-REVIEW records
`missed_by_llm`), `llm_credentials` (WP2-JOBS). Other modules are owned and shaped by their work package.
Schema changes are change requests to the coordinator.
