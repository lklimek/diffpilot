# ADR-003: LLM credential handling and usage limits

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
