# ADR-004: Agent sessions — archive, resume, compaction, loss, deletion

- Status: accepted (coordinator, 2026-11-16, after spikes S1b, S7; amended 2026-11-16 after the WP1-A contract
  review)
- Source: `docs/spikes/S1b-codex-sessions-isolation.md` findings 1, 3; `S7-llm-capability.md` finding 7;
  brief §5 Agent sessions; plan §5.3, §5.4 T8.

## Decisions
- **One session per pull request**, shared across heads, Retry rounds, languages and Ask on any analysis.
  `pull_requests.id` (unique for repository + PR number, never reused) is the archive key:
  `<state>/sessions/<pull_request_id>/YYYY/MM/DD/rollout-…-<thread>.jsonl`, owned by uid `runner`.
  `auth.json` is never archived. `JobSpec.pull_request_id` carries the key, and `SessionRef` is
  `{pull_request_id, thread_id}`. The server resolves the current thread at claim time.
- **Automatic compaction only.** Every useful new or resumed Codex turn sets
  `model_auto_compact_token_limit = 400000`. No extra turn, trivial prompt or deadline reserve.
  Measured forced compaction cost was 35–147 s per job; the owner approved removing it on 2026-11-17.
- **Checkpoint each successful useful turn.** After process cleanup and credential settlement, archive the
  rollout independently of compaction. Usage is the useful turn's cumulative-token delta, including cached
  input. A failed useful turn leaves the previous checkpoint. Failed archival after valid output leaves the
  job succeeded with no session reference, so the next job can recover through its context pack.
- **Output before archival.** `ProviderEvent::OutputReady` still publishes parsed output when the useful turn
  ends. The final result is authoritative; ordering and retries are in `docs/contracts/runner-protocol.md`.
  [deviation] Brief §5 describes streaming Ask answers; Codex exec returns one final structured answer.
- **Loss.** Missing or unusable archives fall back to a new session seeded with `JobSpec.context_pack` and
  report `started_after_loss`. Every prompt states the current base/head and tells the agent to re-read code
  when the remembered head differs. The checkout is the source of truth.
- **One running job per PR.** The atomic claim excludes another claimed/running job of the same PR, analysis
  or credential. Database triggers also enforce the PR constraint on inserts and updates. Cancelled running
  jobs keep the exclusion until completion or lease expiry; stale results cannot overwrite a newer thread.
- **Cleanup.** Cancel and Re-analyze preserve the session. Existing runner retention deletes archives after
  `SR_RUNNER_SESSION_RETENTION_DAYS` (default 14) without a write. Explicit removal may request a PR-keyed
  drop through `drop_sessions` / `dropped_sessions`; no PR deletion UI/API is added here. A retained DB thread
  whose archive expired takes the normal context-pack fallback. PR keys use SQLite AUTOINCREMENT, so a deleted
  PR's retained archive cannot be attached to a newly inserted PR.
- **Logs.** One runner info summary per job identifies new/resumed/context-pack-after-loss, job id and kind,
  completed session turns when available in rollout records, and input/cached-input/output tokens. No new
  protocol fields are needed for logging.

## Consequences
- The server stores the thread reference at PR level; only the runner handles the files.
- The single rewritable migration moves thread/lifecycle columns from analyses to pull_requests; local
  databases must be recreated. Analysis documents, review state and skills commit remain per analysis.
