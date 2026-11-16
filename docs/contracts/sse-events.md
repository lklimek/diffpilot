# Server-sent events

`GET /api/v1/analyses/{id}/events` (session). Type: `AnalysisEvent` (`crates/contracts/src/api/events.rs`).

- SSE `event:` = the `event` tag (`analysis` | `question`), `data:` = the JSON of the variant. No `id:` and no
  replay: on (re)connect the server first sends one `analysis` snapshot and a `question` event per question in
  `pending` or `answering`, so a client that missed events is current again. The browser reconnects with the
  default `EventSource` retry.
- `analysis {summary}`: full `AnalysisSummary` on connect and on every state or progress change (planning →
  running {done,total} → finished | failed | paused | cancelled). The stream stays open after a terminal state
  (re-analysis and Ask still emit) until the client closes it.
- `question {question}`: full `AskQuestion` on every state change: `pending` → `answering` → `answered` |
  `failed`. `answered` is sent as soon as the job's output arrives before compaction (runner
  `OutputReady`, ADR-004); if that attempt is later requeued or cancelled, a new `question` event returns it to
  `answering` or `failed`. No token streaming in v1.
- Activity events of jobs (commands, tool calls) are not forwarded to the browser (brief: no feed).
- Keep-alive: an SSE comment every 15 s. Events carry no secrets and no raw agent output (summaries only).
