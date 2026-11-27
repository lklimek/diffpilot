# Server-sent events

`GET /api/v1/analyses/{id}/events` (session). Type: `AnalysisEvent` (`crates/contracts/src/api/events.rs`).

- A user holds at most 16 open event streams (this route and the dashboard's together, cookie sessions and access
  tokens alike, `MAX_STREAMS_PER_USER`); one more answers 429 `rate_limited` with `Retry-After` until one closes.
- SSE `event:` = the `event` tag (`analysis` | `question` | `heartbeat`), `data:` = the JSON of the variant. No `id:` and no
  replay. Every event is a full snapshot of one entity (the analysis or one question), applied by id.
- **(Re)connect = snapshot, then live events.** On every connection the server:
  1. subscribes to the analysis's in-process event channel (events are buffered from here on);
  2. reads from the database the analysis and **all** questions of the analysis, in every state (also
     `answered` and `failed`, so an answer finished while the client was disconnected arrives);
  3. sends one `analysis` event and one `question` event per question;
  4. forwards the buffered and later events in channel order.
  Invariant: updates of one entity (the analysis, or one question) are serialised: the database write and
  the publish of its event happen under one per-entity lock (or one task owning the entity), so events of an
  entity are published in the order of their writes and each after its write. Hence the last event a client
  receives for an entity is never older than the database; a buffered event older than the snapshot may
  arrive once after it and is followed by the newer one. "Write before publish" alone is not enough: two
  concurrent updates A → B could otherwise publish B then A. Test idea (WP3): two tasks update the same
  question concurrently with a forced interleaving (write A, write B, publish B, publish A must be
  impossible); a subscriber's last event equals the stored state. A buffer overflow closes the stream; the browser
  reconnects (default `EventSource` retry) and gets a fresh snapshot.
- `analysis {summary}`: full `AnalysisSummary` on connect and on every state or progress change (planning →
  running {done,total} → finished | failed | paused | cancelled). `paused {resumes_at, done, total}` keeps the
  progress (`total` null before the plan) while a job waits for the usage limit: its own (`resumes_at` of the
  job) or its credential's, paused by any job (another analysis, an earlier round; `paused_until` of the
  credential). When `resumes_at` passes, the state returns to planning/running. The stream stays open after a terminal state
  (re-analysis and Ask still emit) until the client closes it.
- **Stage** (`planning`, `running`, `paused`: optional `stage {kind, flow?, part?, started_at}`): the stage of the
  analysis's live job, read from the jobs table on every summary (REST and SSE alike, so a reconnect snapshot
  shows the same). Live job = the claimed or running pipeline job of the current round, else the next queued
  or paused one in claim order; `ask` jobs are no stage. `kind` = `plan` | `pr_context` | `flow`; `flow
  {index, title}` (1-based position in the stored plan, its title) only for `flow` and only when the stored plan
  lists it. `started_at` = claim time of the job; null while it waits for the runner (queued, e.g. behind another
  analysis, or paused). `stage` absent = no live job (e.g. between a job's end and the next submit). Facts only:
  no provider activity. Multi-batch planning also includes `part {index, total}` (1-based; total fixed).
  `summary.progress` carries the whole-run start, stage counters, nullable percentage and labelled estimate
  described in `http-api.md`; these measure analysis processing, never review completion or confidence.
  An `analysis` event is also sent whenever a job of the
  analysis changes state (submitted, claimed, ended, requeued after a lease expiry or a released claim, paused,
  resumed): the job's follower publishes the summary within its poll interval (250 ms), and the claim route at
  once. A summary equal to the one published last is not sent again.
- `question {question}`: full `AskQuestion` on every state change: `pending` → `answering` → `answered` |
  `failed`. `answered` is sent as soon as the job's output arrives before session archival (runner
  `OutputReady`, ADR-004); if that attempt is later requeued or cancelled, a new `question` event returns it to
  `answering` or `failed`. A usage limit returns it to `pending` with `resumes_at`. No token streaming in v1.
- Reader fields are projected after the shared broadcast: `question.asked_by` is the asker's GitHub login,
  `question.mine` is true after checking that the stored asker is the subscriber, and `summary.has_my_state` checks
  that subscriber's saved review state. Neither flag is reused across subscribers. Projection failure closes
  the stream for a fresh reconnect; it never emits an event with another reader's ownership.
- Activity events of jobs (commands, tool calls) are not forwarded to the browser (brief: no feed).
- `heartbeat {at}`: server time, every 15 s while the stream is open (first 15 s after connect), so the client
  can tell an open but silent stream from a dead one. It replaces the former keep-alive comment. Each tick
  checks the stream's session again (one indexed read: sign-out, expiry, idle timeout, allow-list removal), or its
  access token (revoked, expired, allow-list removal; ADR-019); the stream ends at the first tick after the
  session or token ended, and the client's reconnect gets 401.
- The server's stop signal ends every open stream, so a graceful shutdown does not wait for the browsers.
  Events carry no secrets and no raw agent output (summaries only).

Question snapshots are queried by user. Live foreign question events are skipped, without closing the stream
or emitting counts/author markers. The shared broadcast stays server-internal; no other user's Q&A leaves it.

Lazy review reuses `analysis` snapshots: `review_ready` becomes true after accepted plan and PR context,
and each accepted flow, flow failure, demand or background-switch change publishes a snapshot. The client
refetches the Tour to read `Flow.analysis_status` and new steps. Cancel sets `review_ready` false and also
invalidates the partial Tour. `state.idle` of a running analysis (`http-api.md`) is part of the snapshot: a
snapshot is sent whenever it changes (the driver ends without admitted flow work; a flow request or background
analysis on admits work again), so an open page needs no polling to show that nothing is being analysed.
No extra polling or replay protocol is added; reconnect snapshots resynchronise it.

## Dashboard refresh events (v1.1)

`GET /api/v1/dashboard/refreshes/{id}/events` (`read`: session or a `read` access token; plan
`docs/v1.1-dashboard-plan.md` §E.3). Type: `DashboardEvent` (`crates/contracts/src/api/dashboard.rs`). Only the
refresh's own user may open it; another user's refresh id answers `not_found` (threat model T16).

- `refresh {status}`: full `RefreshStatus` on connect and on every change of that refresh, including its ranking
  (`ranking.state`, `judged` of `total`, `resumes_at`). When the ranking stores judgements, the `revision` of
  `GET /api/v1/dashboard` changes; the client refetches the summary and the items on `refresh` events.
- `heartbeat {at}`: every 15 s, as for analyses; each tick re-checks the session or the access token (revoked,
  expired, user off the allow-list), so the stream ends within 15 s of a revocation.
- Reconnect = current status, then live events. Updates of a refresh are written before they are published, under
  one per-user task (the v1 invariant above). The stream stays open after the refresh ended until the client
  closes it. No provider activity, no item text and no secrets are sent.

## Review-loop events (v1.3)

Plan `docs/review-workflow-plan.md` §2.8, §5 (ADR-023). Types in `crates/contracts/src/api/review_loop.rs`.

- `GET /api/v1/events` (RW3, `read`): `UserStreamEvent` tagged by `event`, the SSE event name:
  `user_event {user_event}` for each of the reader's own `UserEvent`s (`questions_ready`, `round_executed`, `test`)
  stored after the stream opened, oldest first (the server looks for new ones every 5 s; a reconnect resends nothing
  older than the connection, so a browser never notifies twice), and `heartbeat {at}`. The browser shows user events
  as notifications when its per-browser toggle is on. Heartbeats and token re-checks as for dashboard streams; the
  stream ends when the server stops.
  The v1.4 `llm_activity {activity}` event carries the same reader-scoped snapshot as `GET /api/v1/llm/activity`.
  The stream starts with the current snapshot on every connection. It checks for visible changes on tenant job
  updates and at the heartbeat. Heartbeat checks also pick up the reader's slot changes and capacity used by
  their jobs in other tenants. Activity events are sent at most once a second per stream.
  Other jobs in the reader's tenant carry only their kind and a response-local opaque id. The reader's pause
  and last call remain private. [Phase B activity contract](../v1.4-phase-b-plan.md#8-llm-activity-c7)
- `GET /api/v1/decision-feed/events` (RF2, `read`, query `FeedQuery`): `FeedEvent` tagged by `event`:
  `decision_available {decision, pr, fetch}` when a decision within the subscriber's capabilities becomes
  claimable (also when its open effects shrink so that it newly fits them), `decision_cancelled {decision, pr}` when it leaves the feed (superseded or claimed by another agent),
  `heartbeat {at}`. Opening the stream records the token's subscription and capabilities; no comment text, reply
  text or secret is sent (fetch the decision with RF1b).
