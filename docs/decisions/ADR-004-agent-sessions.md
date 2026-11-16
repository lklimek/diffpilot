# ADR-004: Agent sessions — archive, resume, compaction, loss, deletion

- Status: accepted (coordinator, 2026-11-16, after spikes S1b, S7)
- Source: `docs/spikes/S1b-codex-sessions-isolation.md` findings 1, 3; `S7-llm-capability.md` finding 7;
  brief §5 Agent sessions; plan §5.3, §5.4 T8.

## Decisions
- **One session per analysis.** The session archive is the set of Codex rollout files of the thread, stored by
  the runner per analysis id under uid `runner`. Only that analysis's files are copied into a job's
  `CODEX_HOME`; `auth.json` is never part of the archive.
- **Every job of the analysis resumes the session** (`codex exec resume <thread_id>`): `plan`, `pr_context`,
  each `analyse_flow` and every `ask`. `JobSpec.session` carries `{analysis_id, thread_id}`; the first job has
  none. `JobResult.session` returns the thread id and `resumed | started | started_after_loss`.
- **Compaction** at the end of every job: one extra `exec resume <id> -c model_auto_compact_token_limit=<small>`
  with a trivial prompt. It runs inside the provider (`LlmProvider::run`/`resume` end with it). S7: context 50–95k
  → 17–24k, about 1.5–2 minutes per job; it counts against the 45-minute job cap (ADR-005).
- **Loss.** A missing or unusable archive is a cache miss: the job starts a new session seeded with
  `JobSpec.context_pack` (always sent with a resumed job) and reports `started_after_loss`.
- **One running job per session**, enforced at claim (database partial unique index on running jobs per
  analysis).
- **Deletion.** When an analysis is cancelled or superseded by a re-analysis the server marks its session for
  deletion; the runner learns it from `ClaimResponse.drop_sessions`, deletes the files and acknowledges with
  `ClaimRequest.dropped_sessions` (at-least-once, idempotent). Retention deletion is runner-local.
- A re-analysis on a moved head starts a new session.

## Consequences
- The server never needs the session files; per-job containers later ship the archive with the job.
