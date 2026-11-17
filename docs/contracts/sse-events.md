# Server-sent events

`GET /api/v1/analyses/{id}/events` (session). Type: `AnalysisEvent` (`crates/contracts/src/api/events.rs`).

- SSE `event:` = the `event` tag (`analysis` | `question`), `data:` = the JSON of the variant. No `id:` and no
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
  progress (`total` null before the plan) while a job waits for the usage limit. The stream stays open after a terminal state
  (re-analysis and Ask still emit) until the client closes it.
- `question {question}`: full `AskQuestion` on every state change: `pending` → `answering` → `answered` |
  `failed`. `answered` is sent as soon as the job's output arrives before compaction (runner
  `OutputReady`, ADR-004); if that attempt is later requeued or cancelled, a new `question` event returns it to
  `answering` or `failed`. No token streaming in v1.
- Activity events of jobs (commands, tool calls) are not forwarded to the browser (brief: no feed).
- Keep-alive: an SSE comment every 15 s. Events carry no secrets and no raw agent output (summaries only).
