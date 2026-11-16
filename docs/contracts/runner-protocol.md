# Runner protocol (v1)

Types: `crates/contracts/src/job/**`. Served only on the runner listener (`SR_RUNNER_LISTEN_ADDR`); never routed
through the reverse proxy. The runner is long-lived and pulls work; the server never calls the runner.

## Messages
| Method | Path | Auth | Body | Reply |
|---|---|---|---|---|
| POST | `/runner/v1/claim` | `Bearer <SR_RUNNER_SECRET>` | `ClaimRequest` | `ClaimResponse` |
| POST | `/runner/v1/jobs/{id}/events` | `Bearer <job token>` | `EventBatch` (empty = heartbeat) | `EventsAck` |
| POST | `/runner/v1/jobs/{id}/output` | `Bearer <job token>` | `OutputReady` | `EventsAck` |
| POST | `/runner/v1/jobs/{id}/result` | `Bearer <job token>` | `JobResult` | 204 |

Errors use `ApiError`: 401 wrong secret or token (compared in constant time against the stored hash); 404 unknown
job; 409 `conflict` for a stale attempt, a job that is no longer claimed or running, or a different output for
the same attempt; 400 malformed body.

## Claim
- The runner polls `claim` every 5 s while idle, reporting the credentials it holds (`usable` false after a
  failed refresh, ADR-003) and the sessions it deleted (`dropped_sessions`).
- The server picks the next runnable job: `queued`, credential usable, not paused and idle, no running job of
  the same analysis (partial unique indexes), Ask (`priority` 0) before batch (1), then oldest. It increments
  `attempt` (every claim, including after a pause or a lease expiry), mints a random job token (stored hashed),
  sets the lease, and fills `spec.access` at claim time:
  - `github`: an installation token for the **PR repository only** (explicit `repositories` and read-only
    `permissions`, 1 h, ADR-005) — or `null` when the GitHub App is not installed on the PR repository; then the
    runner configures no `github-mcp-server` and the prompt says related work is not checked;
  - `memcan`: URL and token from configuration (runner config limits the MCP to search and `add_memory`).
- `drop_sessions`: analyses whose session files the runner deletes (cancelled or superseded, ADR-004); it
  acknowledges in the next claim's `dropped_sessions` (at least once; idempotent).
- The job token is for the runner only: never placed in the agent's environment, config, logs or events.

## State machine
`JobState::transition` is the single source (`crates/contracts/src/job/state.rs`):

| From | Event | To |
|---|---|---|
| queued | claim | claimed |
| claimed | first events | running |
| claimed, running | result `succeeded` | succeeded |
| claimed, running | result `failed` | failed |
| claimed, running | result `usage_limited` | paused (`resumes_at`; the credential pauses too) |
| paused | `resumes_at` passed | queued |
| claimed, running | lease expired, attempts left | queued |
| claimed, running | lease expired, `attempt ≥ max_attempts` (3) | failed |
| queued, paused | cancel | cancelled at once |
| claimed, running | cancel | stays; `cancel_requested` in every ack; ends `cancelled` on any result |

## Liveness and fencing
- Lease 60 s, renewed by every accepted `events` or `output` request. The runner sends a batch (or an empty
  heartbeat) at least every 15 s.
- **Self-fencing.** If the runner has not had a successful ack for 45 s, it stops the agent (SIGTERM, SIGKILL
  after 10 s), discards the attempt and does not post a result. 45 s < 60 s, so the old process is gone before
  the server can hand the job to a new claim. The server revokes the installation token when a lease expires
  or a result arrives.
- **Credential copy-back** (ADR-003) happens only after the agent process has exited, with compare-and-swap on
  the master `auth.json`. On start the runner copies back any newer `auth.json` left in per-job directories
  of a crashed run (same CAS), then deletes those directories.

## Events
- `seq` starts at 1 per attempt and increases by one; the server stores `(job, attempt, seq)` and ignores
  duplicates, so a retried batch is harmless. Events of a stale attempt are rejected (409).
- `summary` ≤ 200 characters, plain text; never raw command output, file contents or prompt text.
- `EventsAck.cancel_requested`: stop the agent (`LlmProvider::cancel`) and post `cancelled`.

## Output before compaction (ARCH-005)
- When the model turn ends, the provider emits `ProviderEvent::OutputReady` and the runner posts
  `OutputReady {attempt, output}` **before** compaction, for every job kind. `output` is the parsed final JSON
  (Codex enforced the output schema); the server validates and verifies it exactly as a result.
- Ordering: after every event of that turn; at most once per attempt; before the result.
- Retry: on a network error or 5xx the runner retries with the same body (backoff 1, 2, 4 s, within the lease).
  The server stores it once per attempt (`jobs.ready_output`); an identical repeat answers 200, a different
  output for the same attempt 409. If it never arrives, nothing is lost: the result carries the output.
- Publication: the server may publish a verified ready output at once (Ask answer `answered`, flow progress);
  persistent analysis documents are written from the result.
- Reconciliation: `JobResult` is authoritative. `succeeded` must carry the same output; if it differs the
  server uses the result's output and republishes. A requeued attempt (lease expiry) or a `cancelled` result
  discards the ready output and its publication. When cancel was requested before the output arrived the
  output is stored but not published (`OutputRecorded::Stored {cancel_requested: true}`).
- A failed compaction after the output does not fail the job: the result is `succeeded` with
  `session: null` (lost; the next job uses the context pack).

## Result
- Posted once per attempt; retried with the same body until 204 or a 4xx. The server applies it idempotently
  (`Completion::AlreadyRecorded` for a repeat). A result for a stale attempt is 409 and the runner drops it.
- Cancel wins: if cancel was requested, any result (also `succeeded`) ends the job `cancelled` and the output is
  discarded.
- `llm_call` (last provider call outcome) updates the LLM status in Settings; `usage` is the job's own delta.

## Secrets and scrubbing
The runner scrubs every known secret value (installation token, MemCan token, job token, runner secret,
Codex access and refresh tokens) from events, ready outputs, results and the session files it stores, before
they leave the job directory. Summaries never carry raw command output. `Secret` values are redacted in
`Debug` and never logged by either side.
