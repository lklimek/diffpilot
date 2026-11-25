# ADR-003: LLM credential handling and usage limits

## v1.3 container amendment (2026-11-24)

Each payer links a ChatGPT plan through Settings and the gateway; `mode: chatgpt_plan` is the only mode.
No API-key mode, paid fallback, CLI login, host credential import or worker credential copy exists. Linking may
log Codex out on other machines; probability and scope are unknown. The gateway owns the encrypted dedicated
refresh chain: one owner, near-expiry refresh only, durable in-flight marker, persist before use. Unknown
outcome quarantines the generation for relinking; no forced/replayed refresh. Upstream revoke requires explicit
unlink. Virtual keys expire within 90 minutes and work only on worker networks. Usage pauses are per payer;
turn deltas include compaction and exclude fork inheritance. Auth acceptance uses fakes only.

This amendment and [ADR-025](ADR-025-container-runtime.md) take precedence over the historical text below.


> Amended for v1.3 by [ADR-025](ADR-025-container-runtime.md): per-payer containers, app-server,
> gateway-only real credentials, file-based read-only GitHub tokens and host report 4.2.
> KL1-A selects gateway-run ChatGPT-plan device linking only. No API-key mode or paid fallback;
> credentials never enter workers. The master-copy lifecycle below describes the superseded runtime.


- Status: accepted (coordinator, 2026-11-16, after spikes S1a, S1b)
- Source: `docs/spikes/S1a-codex-basics.md` findings 1–2, 9; `S1b-codex-sessions-isolation.md` finding 10;
  plan §5.4 T1.

## Decisions
- **One credential kind in v1:** `codex_chatgpt` (the owner's ChatGPT Pro subscription). No paid API key; no
  credits are ever bought or spent.
- **Master credential** (`auth.json`) lives only in the runner's store (uid `runner`). Each job gets a copy in
  its per-job `CODEX_HOME`. After the job the runner writes the copy back if its refresh token changed, with a
  compare-and-swap against the master read at job start. The per-job `CODEX_HOME` lives on a disk-backed
  Docker volume that survives restarts (0700 per job, never in RAM; ADR-013 "Scratch volumes"); on start the runner recovers leftover copies with the
  same CAS, so a crash between rotation and copy-back does not lose the credential
  (`docs/contracts/runner-protocol.md`).
- **Jobs are serialised per credential.** Refresh tokens are single-use; two copies refreshing concurrently
  would burn the master. The server claims at most one job per credential at a time (database partial unique
  index on running jobs per `credential_id`, plan `docs/contracts/runner-protocol.md`).
- **Refresh failure** (`refresh_token_reused|expired|invalidated`) marks the credential unusable; the runner
  reports it in `ClaimRequest.credentials` and `JobFailureKind::credential_unusable`; the owner logs in again
  (runbook). Settings shows the plain error as LLM status.
- **Usage limit.** Detected from `turn.failed` whose message starts "You’ve hit your usage limit" or "Your
  workspace is out of credits". `resets_at` comes from the latest rollout `rate_limits`, never from the text.
  The job result is `usage_limited { resets_at }`; the server pauses the credential and the analysis until then.
  If `resets_at` is unknown the server retries after 30 minutes and the UI says "resumes later".
- **Usage accounting.** `turn.completed.usage` is cumulative per thread; per-job usage is the delta of the
  cumulative totals (or the sum of rollout `last_token_usage`).

## Consequences
- With one credential, batch and Ask jobs share one queue slot; Ask is claimed before every other queued job (dashboard ranking, then batch); a running job is never pre-empted (plan §5.3).
