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
    runner configures no `github-mcp-server` and the prompt says related work is not checked. The same holds
    when the App is unusable as configured (key file, key or App id; logged as an error). Any other mint
    failure, a minted token that differs from the request (permissions, repository, visibility, expiry outside
    the job's runtime cap and GitHub's hour), or GitHub's part of the claim exceeding its 6 s budget fails the
    claim (`github_unavailable`); the job is retried after its lease, and a token received after the budget is
    still stored and revoked by the worker once that lease expires. A rejected token is never handed out: it
    is stored encrypted like any other and revoked;
  - `memcan`: URL and token from configuration (runner config limits the MCP to search and `add_memory`).
- `drop_sessions`: analyses whose session files the runner deletes (cancelled or superseded, ADR-004); it
  acknowledges in the next claim's `dropped_sessions` (at least once; idempotent).
- The job token is for the runner only: never placed in the agent's environment, config, logs or events.

## State machine
`JobState::transition` is the single source (`crates/contracts/src/job/state.rs`):

| From | Event | To |
|---|---|---|
| queued | claim | claimed |
| claimed | first non-empty events or the `output` | running (an empty heartbeat does not start the job) |
| claimed, running | result `succeeded` | succeeded |
| claimed, running | result `failed` | failed |
| claimed, running | result `usage_limited` | paused (`resumes_at`; the credential pauses too) |
| paused | `resumes_at` passed | queued |
| queued | the credential is paused (any job's usage limit) | stays queued; the follower reports `Paused { resumes_at: paused_until }` once per pause end |
| claimed, running | lease expired, cancel requested | cancelled (never re-claimed) |
| claimed, running | lease expired, attempts left | queued |
| claimed, running | lease expired, `attempt ≥ max_attempts` (3) | failed |
| queued, paused | cancel | cancelled at once |
| claimed, running | cancel | stays; `cancel_requested` in every ack; ends `cancelled` on any result or at lease expiry |

At lease expiry (`Jobs::expire_leases`) the server clears the job token and the ready output, revokes the
installation token and ends the executor stream; a job with cancel requested ends with a server-built
`cancelled` result.

## Liveness and fencing
Constants: `LeaseTiming::V1` (`crates/contracts/src/job/protocol.rs`; a test checks the inequality).

| Value | Seconds |
|---|---|
| lease granted from the server's receipt of an accepted `events` / `output` request | 60 |
| longest gap between runner requests (empty batch = heartbeat) | 15 |
| runner fence: after the **send** time of the last acked request | 35 |
| SIGTERM → SIGKILL grace | 10 |
| SIGKILL, reaping, credential copy-back | 5 |
| margin before the lease can end | ≥ 10 |

- The runner keeps, per job, the send time (monotonic clock) of the last request that got a 2xx ack. Its
  local deadline is that send time + 35 s. A request is sent before the server receives it, so the server's
  lease (receipt + 60 s) always ends after send + 35 + 10 + 5 + 10 s. A late ack never extends local validity;
  `lease_expires_at` in replies is informational.
- At the deadline the runner stops the agent (SIGTERM, SIGKILL after 10 s), copies the credential back (CAS),
  discards the attempt and posts no result. Only then may the server hand the job to a new claim, so two
  processes never hold the same refresh token. Two lost heartbeats (30 s) still stay inside the deadline.
- The server revokes the attempt's installation token after a lease expires or a result arrives (recovery:
  "Installation-token revocation" below); claims and results never wait for revocations.

## Credential home (ADR-003)
- The per-job `CODEX_HOME` (copy of `auth.json`, generated config, the analysis's session files, CAS data)
  lives on the runner's **persistent private volume** in a per-job directory (uid `runner`, 0700; the agent
  uid gets access only to its own job's directory), never on tmpfs or under `/tmp`. Only the checkout / work
  directory may be tmpfs.
- Copy-back happens after the agent process has exited, with compare-and-swap on the master `auth.json`
  (written back only if the master is unchanged since the copy).
- Recovery: on start, before claiming, the runner applies the same CAS to every per-job `CODEX_HOME` left by
  a crash (a rotated refresh token is the only valid one), then deletes those directories. A container
  restart therefore never loses a rotated credential.

## Events
- `seq` starts at 1 per attempt and increases by one; the server stores `(job, attempt, seq)` and ignores
  duplicates, so a retried batch is harmless. Events of a stale attempt are rejected (409).
- `summary` ≤ 200 characters, plain text; never raw command output, file contents or prompt text.
- `EventsAck.cancel_requested`: stop the agent (`LlmProvider::cancel`) and post `cancelled`.

## Output before compaction
- When the model turn ends, the provider emits `ProviderEvent::OutputReady` and the runner posts
  `OutputReady {attempt, output}` **before** compaction, for every job kind. `output` is the parsed final JSON
  (Codex enforced the output schema); the server validates and verifies it exactly as a result.
- Ordering: after every event of that turn; at most once per attempt; before the result.
- Retry: on a network error or 5xx the runner retries with the same body (backoff 1, 2, 4 s, within the local
  deadline).
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

## Agent session (ADR-004)
- One session per analysis. The server sets `spec.session` at claim from the analysis's recorded session (none
  before the first report, none once a drop was requested); the stored spec never holds it. One running job per
  analysis, so queued jobs never fork the session.
- The result's `session` is recorded as the analysis's session in the transaction that completes the job, only
  for the first effective, not cancelled result of the current attempt (a repeat or a result after a cancel
  request changes nothing); a session already asked to be dropped stays dropped.
- `context_pack` is built at claim from the plan, PR context and accepted flow analyses stored at that moment
  (`jobs::claim::ContextPacks`, implemented by `pipeline::context::StoredPacks`); never at enqueue. Only the runner
  knows whether its session files survived, so every job carries it; the runner uses it only when it starts a new
  session (`started_after_loss`). An `ask` job's pack adds the asked step in full, the rest of its flow analysis
  and the step's questions answered before it (`ask::context_pack`), so answers given while it waited are in it.
  Session and pack are prepared before the installation token is minted; when they cannot be (stored results
  unreadable, pack not renderable) the claim is released (job `queued`, attempt unspent, no token minted) and
  the claim answers 500 `internal`.
- `jobs.github_access_granted` records whether the attempt's claim got an installation token; the executor
  reports it with the result (`JobUpdate::Finished { github_access }`), the pipeline stores it with the accepted
  PR context (`documents.github_access`), and the Tour's `related_work_checked` reads only that stored fact.
- Cancel and a re-analysis on a moved head request the drop of the analysis's session (`drop_sessions`).
- Claims skip jobs of cancelled analyses. At start-up, before claims are served, the server finishes cancellations
  a crash interrupted (jobs cancelled, documents deleted, session drop requested; idempotent).
- `access.memcan` is filled at claim from the server configuration (never stored, never logged).

## Pipeline recovery (server restart)
Durable state only; no in-memory handle is needed after a restart.
- **Stage link.** Every job carries its stage (`JobStage`: `plan:<batch>`, `pr_context`, `flow:<id>`,
  `ask:<question id>`; `jobs.stage`) and its **generation**: 0 for the first run, 1..2 for repair runs
  (`MAX_REPAIR_RUNS` = 2, brief §5 Pipeline 6). The database allows one live or succeeded job per (analysis,
  stage, generation, round) (`jobs_one_live_per_stage`); a failed or cancelled one may be submitted again. An
  explicit Retry increments `analyses.round` atomically with failed → planning and cancels the earlier round's
  unfinished jobs in the same transaction; jobs are enqueued in the current round and `submit` finds only jobs of
  that round, so a Retry calls the agent again while a restart (same round) re-uses stored outputs. Accepted
  results of an earlier round are re-used only as verified documents.
- **One job at a time.** The driver submits an analysis's pipeline jobs one at a time, in order: plan batches,
  PR context, then the flows in plan order. The next job is submitted only after the previous result was verified
  and its document stored (or the stage ended), so a claim never overtakes the previous result and every context
  pack holds all earlier results. A restart re-submits the first unresolved stage (an idempotent submit returns
  its live job).
- **Not sent.** A flow whose prompt is over budget is never submitted: its lines are Unexplained with reason
  `too_large_for_prompt` (an `unsent` document), and the analysis goes on. Hunks of a file too large for any plan
  prompt get the same reason, stored before the plan.
- **Repair.** When verification rejects the output of a succeeded job of generation g, the pipeline submits
  generation g + 1 with the repair prompt; with g = 2 repair is exhausted: a flow ends without output (its lines
  are Unexplained, the analysis goes on), a plan or PR context fails the analysis. The rejection
  needs no stored flag: verification is deterministic, so after a restart re-verifying the newest succeeded
  generation reproduces the decision.
- **Submit boundary.** `JobExecutor::submit` is idempotent per (stage, generation): an existing live or
  succeeded job's handle is returned (enqueue `Conflict` → `Jobs::list_for_analysis`). A crash between enqueue
  and recording progress therefore never duplicates a job, and a repair is never answered with the rejected
  result of an earlier generation.
- **End boundary.** When an analysis fails or is cancelled, the pipeline calls
  `JobExecutor::cancel_analysis(analysis_id)` once. It requests cancellation of every queued, claimed, running or
  paused job of the analysis, also jobs submitted before a restart (`QueueExecutor`: `Jobs::list_for_analysis`
  + `Jobs::request_cancel`); the pipeline itself never reads the jobs table.
- **Result boundary.** `Jobs::complete` stores the result and the state in one transaction, idempotently per
  attempt; a runner retry after a lost reply changes nothing.
- **Persist boundary.** Verification and `Documents::put` (upsert by analysis, kind and key) run from the
  stored `jobs.result`; repeating them yields the same documents. Analysis progress is derived from the
  stored documents, not from counters.
- **At start** the pipeline lists `Analyses::unfinished` and starts one driver per analysis
  (`pipeline::resume_unfinished`). The driver uses only `JobExecutor` and the stored documents, never the jobs
  table: a stage with a document is skipped; every other stage is submitted again, which the submit boundary
  answers with the existing live or succeeded job, followed with `JobExecutor::events`; a succeeded result whose
  document is missing is verified again and persisted, or repaired (next generation) if rejected. Because
  verification is deterministic, this replay reaches the newest generation. Expired leases are handled by
  `Jobs::expire_leases` as usual.
- **Installation-token revocation.** The encrypted token is stored per (job, attempt) at claim
  (`installation_tokens`). Revocation reads it without removing it (`Jobs::github_token`), calls
  `DELETE /installation/token`, then confirms (`Jobs::confirm_github_token_revoked`). One server worker sweeps
  at start, every minute and when a result or an expired lease nudges it: it first ends overdue leases
  (`Jobs::expire_leases`), then `Jobs::github_tokens_to_revoke` lists every token not used by a claimed or
  running attempt; expired ones are only confirmed. Failed revocations stay listed and are retried with
  exponential backoff (5 s up to 5 min), honouring GitHub's `retry-after` / `x-ratelimit-reset`.
  A crash between read and revocation therefore leaves the
  token listed, and a re-claim never overwrites the previous attempt's token.
  The server also masks the attempt's token in events, outputs and results (T10).

## Secrets and scrubbing
The runner scrubs every known secret value (installation token, MemCan token, job token, runner secret,
Codex access and refresh tokens) from events, ready outputs, results and the session files it stores, before
they leave the job directory. Summaries never carry raw command output. `Secret` values are redacted in
`Debug` and never logged by either side.

A missing required agent skill is `JobOutcome::Failed { kind: configuration, ... }`. The server preserves the
kind as the failed analysis's `reason`, presents a fixed administrator-action sentence, and the UI offers no
Retry for it. This failure occurs before Codex starts; no agent output schema changes are involved.
