# ADR-004: Agent sessions — archive, resume, compaction, loss, deletion

- Status: accepted (coordinator, 2026-11-16, after spikes S1b, S7; amended 2026-11-16 after the WP1-A contract
  review)
- Source: `docs/spikes/S1b-codex-sessions-isolation.md` findings 1, 3; `S7-llm-capability.md` finding 7;
  brief §5 Agent sessions; plan §5.3, §5.4 T8.

## Decisions
- **One analytic session per PR**, shared across heads, Retry rounds and languages, for plan, PR context,
  flow and repair jobs only. It never receives local questions, answers, feedback notes or comment drafts.
  Archive: `<state>/sessions/<pull_request_id>/analytic/YYYY/MM/DD/rollout-…-<thread>.jsonl`.
- **One private working session per (PR, user)** for Ask. Archive:
  `<state>/sessions/<pull_request_id>/users/<sha256-user-id>/YYYY/MM/DD/rollout-…-<thread>.jsonl`.
  Lazy fork copies only the analytic rollout into a fresh job home; the useful turn saves only the working
  archive. A working thread is eligible only at its recorded analytic generation. Missing working archive
  falls back to the analytic archive; missing analytic archive falls back to the context pack. Every re-fork
  receives the asker's latest 20 final Q&A across the PR, further bounded and fenced as untrusted data by the
  existing Ask context-pack renderer. No other user's history is selected.
- **Generation and publication.** Each effective analytic checkpoint advances `analytic_generation`, even
  if the provider thread id is unchanged. Successful output without an archive clears the analytic reference
  and invalidates forks. Ask completion updates only its user's working reference. Replayed/stale results
  never rewrite references. A cancelled checkpoint invalidates forks without accepting its thread.
- **Self-contained routing.** `JobSpec.session` always names the analytic source; `working_session` is required
  for Ask and absent otherwise. Its `user_key`, `analytic_generation`, and optional working `thread_id` select
  restore-first and save locations. Runner rejects missing/mismatched routing and non-hex archive keys.
  The server derives ownership from the authenticated question, never from runner output.
- **No indirect private-memory channel.** Agents have MemCan access, split by job kind: analytic jobs (plan,
  PR context, flows, repairs) get search tools and `add_memory`; Ask jobs, which see user-typed text, get
  search tools only (`allow_write: false` in the job's MemCan endpoint, enforced by the runner's Codex
  `enabled_tools`), so an Ask cannot write text that a later analysis reads. The server also reads
  bounded project profiles and calibration examples. Feedback notes stay local; shared calibration contains
  only the fixed feedback value and public analysis facts, marked `private_text_excluded: true`. Legacy
  calibration records without that marker are ignored. The bearer keeps its full permissions (AR-1); the
  Ask restriction limits what the model is offered, not what the bearer can do.
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
- **Cleanup.** Pause preserves results and both PR sessions. Cancel discards analysis documents and cancels
  unfinished jobs; Cancel and Re-analyze preserve analytic and private working sessions. Existing runner retention deletes archives after
  `SR_RUNNER_SESSION_RETENTION_DAYS` (default 14) without a write. Explicit removal may request a PR-keyed
  drop through `drop_sessions` / `dropped_sessions`; no PR deletion UI/API is added here. A retained DB thread
  whose archive expired takes the normal context-pack fallback. PR keys use SQLite AUTOINCREMENT, so a deleted
  PR's retained archive cannot be attached to a newly inserted PR.
- **Logs.** One runner info summary per job identifies new/resumed/context-pack-after-loss, job id and kind,
  completed session turns when available in rollout records, and input/cached-input/output tokens. No new
  protocol fields are needed for logging.

## Consequences
- The server stores the analytic reference/generation at PR level and working references per PR/user; only the runner handles files.
- The single rewritable migration moves thread/lifecycle columns from analyses to pull_requests; local
  databases must be recreated. Analysis documents, review state and skills commit remain per analysis.

## Privacy and concurrency (owner decision, 2026-11-17)

HTTP lists and SSE snapshots/live events expose only the signed-in user's questions, feedback notes and
review state. GitHub pending comments use that user's token and require `viewerDidAuthor`; foreign ids
return 404. Unsent comment drafts live only in the browser component. Published GitHub comments remain
public PR data and may enter analysis; pending comments are excluded from the shared snapshot.

Jobs remain serialised per PR and credential, including two users' Ask jobs. The archive model permits
parallel working sessions later; v1 does not enable that concurrency. Cancel and Re-analyze keep both kinds
of archive. Explicit PR removal and the existing 14-day retention remove the whole PR archive tree.
Removing sessions when a user is deleted or removed from the allowlist is out of scope; references have
foreign-key cleanup, but archive retention remains PR-wide. Existing databases must be recreated (the single
rewritable migration); old analytic references must not be reused because they may contain earlier Ask text.

## Addendum 2026-11-18: one compaction when an Ask working session is forked (owner decision)

- **Decision.** The first turn of a new working session — the lazy fork on a user's first question, and every
  re-fork after the analytic generation advanced or the working archive was lost — runs with
  `model_auto_compact_token_limit = 80000` instead of 400000. Codex then compacts the forked analytic history
  before it answers (pre-turn compaction, S1b finding 3) and answers within the same turn: no extra turn. The
  compacted session is what the working archive keeps. Later Ask turns use the 400000 limit (automatic
  compaction only); analytic jobs never use the lower limit. 80000 lies above a compacted context (17–24k in S7)
  plus one Ask turn, so the turn does not compact again.
- **Visibility.** The runner posts the activity line "Preparing the conversation (first question after an
  analysis)" before that turn; the Ask panel says that a first question, or the first after the analysis
  changed, takes longer.
- **Fallback.** If the compacting turn fails (provider failure), the runner answers once more from the
  uncompacted fork with the 400000 limit; a fork without an analytic archive starts from the context pack and
  is not compacted. The retry follows any provider failure of that turn, not only a failed compaction (the
  agent is read-only, so a second attempt has no side effects). The failed turn's tokens are not in the job's
  usage: the provider reports no usage for a failed turn; the runner logs the retry at warn.
- **Unproven.** The 80000 limit is not yet measured on the deployed Codex with a ~300k-token fork: compaction
  time (S7: 47–130 s from 50–95k) and whether it happens before the first request must be checked on the real
  stack (job phase timings and input tokens of the first and a follow-up Ask).
- **Reason.** Ask turns resumed the full analytic history: on a large pull request (dashpay/platform, plan +
  PR context + 2 flows) every model request re-read about 300k tokens, and answers took 70–115 s against about
  30 s on a 100k-token session. The removal of forced compaction above stays valid for analytic jobs (35–147 s per
  job); one compaction per working fork is paid once per user and analytic generation.

## Addendum 2026-11-19: Ask threads carry their context in the job (owner decision)

- **Decision.** A follow-up under an Ask question (one level: every follow-up names its thread's first question)
  carries its thread in the `ask` job's prompt: the asker's earlier questions of that thread with their answers,
  oldest first. The first question always stays; then the latest follow-ups (at most 6) that fit 32 KiB, each answer
  clipped to 4000 characters, with a note on omitted ones.
- **Reason.** The working session holds the thread only while it lives. A re-fork after the analytic generation
  advanced, a lost working archive or a context pack would drop it; the owner requires that a follow-up remembers
  the earlier questions and answers of its thread. Putting the thread in the job makes it independent of the
  session; the cost is bounded by the limits above.
- **Context packs stay bounded and built from stored documents only** (no Tour or diff work at claim): a lines
  question's touched steps are chosen at submit and stored with the question; a flow question's pack holds that flow
  in full within the asked-flow budget; a PR question's pack holds the usual stored results.
