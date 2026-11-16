# ADR-004: Agent sessions — archive, resume, compaction, loss, deletion

- Status: accepted (coordinator, 2026-11-16, after spikes S1b, S7; amended 2026-11-16 after the WP1-A contract
  review, ARCH-005)
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
- **Output before compaction** (ARCH-005). Compaction takes 47–130 s (S7), so the answer must not wait for it.
  When the model turn ends, the provider sends `ProviderEvent::OutputReady(output)` and the runner posts it to
  `POST /runner/v1/jobs/{id}/output` (`OutputReady {attempt, output}`, scrubbed like a result) before
  compaction starts, for every job kind. The server stores it per attempt and may publish it (e.g. an Ask answer
  shown as ready); `JobResult` stays authoritative and idempotent. Ordering, retry and reconciliation rules:
  `docs/contracts/runner-protocol.md`. A failed compaction after a valid output does not fail the job: the
  result is `succeeded` and the session counts as lost (the next job uses the context pack).
  For the owner: [deviation] brief §5 says Ask streams the answer; Codex `exec` delivers the structured output
  only at the end, so v1 has no token streaming: the answer appears in one piece as soon as the model turn ends.
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
