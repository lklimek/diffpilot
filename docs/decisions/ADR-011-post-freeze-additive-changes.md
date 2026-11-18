# ADR-011: Additive changes after the contract freeze

- Status: accepted (coordinator, 2026-11-16, stage-2 change requests of WP2-INGEST and WP2-JOBS)
- Source: ADR-010 (freeze; every contract change is a coordinator-approved change request, plan §4.3).

## Rule: the analysis base is the merge base
The analysed diff is GitHub's three-dot comparison: from the merge base of the PR's base tip and head to the
head. The pipeline fetches both commits, computes the merge base and stores it as the analysis base before any
job is submitted (`pipeline::prepare`). Preparing again is a no-op (the merge base of a merge base and the head
is that merge base). An analysis whose stored base is not the merge base is not ready: `lines` and `diff` answer
409 `analysis_not_finished` (`docs/contracts/http-api.md` "Analysis base").

## Changes
All changes are additive: a new field, variant, route or column, or a stricter rule on a value no client could send before. Existing fields keep their meaning.

| Change | Reason | Effect |
|---|---|---|
| `DeletedBlock.total_lines: u32` (`contracts::api::code`, `72d8147`) | A deleted block longer than the line budget (`RangeQuery::MAX_LINES` = 2000 removed lines per response, shared by the blocks in order) was cut without a sign. | The truncation is visible: `lines.len() < total_lines` marks a cut block, and a client reads the rest with `side=base` from `base_start + lines.len()`. The Rust type, the generated TS types and the seed fixtures carry the field; the generated JSON Schemas (agent output only) are unchanged. |
| `Analyses::set_base_sha(id, sha)` (`crates/db`, `173e5cb`) | The pull request row holds the base tip; the pipeline needs to store the merge base (rule above). | Replaces the base while the analysis has no job and no document; afterwards `Conflict` (jobs and documents were made against the stored base); unknown analysis `NotFound`. |
| `JobExecutor::cancel_analysis(analysis)` (`server::jobs`, `135f245`) | Rule A3: the pipeline talks only to `JobExecutor`, yet an ended analysis must cancel jobs submitted before a server restart, which no in-memory handle knows. | Requests cancellation of every queued, claimed, running or paused job of the analysis (`QueueExecutor`: `Jobs::list_for_analysis` + `Jobs::request_cancel`). The pipeline calls it once when an analysis fails or is cancelled and never reads the jobs table (`runner-protocol.md` "End boundary"). |

### Wire types (`crates/contracts`)
| Change | Reason |
|---|---|
| `CiState::NotChecked` (`api::tour`) | Check runs that could not be read (no token, GitHub error, rate limit) are not reported as "no checks"; nothing is claimed. |
| `PrContextFacts.threads_status: ThreadsStatus` (`checked`, `partial`, `not_checked`) | A thread list cut at the server's read cap or not read is visible, so an empty list is never read as "no threads". |
| `PendingReview.incomplete` (`api::comments`) | The pending-review list stopped at the server's cap; absence from it is not proof that a comment does not exist. |
| `ApiErrorCode::CommentPublished` (409, `api::error`) | An edit or delete that raced a publish of the same review is reported, not silently accepted (ADR-006). |
| Feedback rules (`api::feedback`): one entry per user, analysis, target and value; `overrated` and `underrated` exclude each other; `missed_by_llm` recorded only by the server; `DELETE …/feedback/{feedback_id}`; `Feedback.memory: FeedbackMemory` (`pending`, `written`, `failed`) | Several values may coexist on a target, a user can retract one, and the state of its MemCan copy is visible (`db-schema.md` "Feedback → MemCan"). |
| `AnalysisState::Paused { resumes_at, done, total }` (`api::analyses`; `total` null while planning) | A usage-limit pause shows progress and the resume time. |
| `Counters.dependency_files_not_checked` (`api::tour`) | Changed dependency files that were not parsed are listed instead of being counted as "no dependency change". |
| `ProjectProfileStatus.memcan: SourceCheck` (`checked`, `not_checked`) | An empty profile source list with MemCan `not_checked` is not read as "no profile". |
| `ProvenanceBadge::BinaryContent` (`common`) | Files git treats as binary are marked by a deterministic content check; they are neither shown nor analysed. |
| `UnexplainedRanges.reason: Option<UnexplainedReason>` (`too_large_for_prompt`; `api::tour`) | Lines the server did not send to the LLM say why; absent when the LLM did not explain them. |
| `DiffLine.eol: Option<LineEnding>` (`crlf`, `no_newline`; `api::code`) | The diff keeps line endings that are not LF. |
| `Settings.app_installations` is `Option`; null = not checked (`api::settings`) | "GitHub unreachable" is not shown as "no installation". |
| `AskQuestion.resumes_at` (optional) and `AskRequest::MAX_TEXT_CHARS` = 2000 (`api::questions`) | A pending question shows when the usage limit resets; the question length is bounded. |
| `ApiErrorCode::Unavailable` (503, `Retry-After: 1`, `api::error`) | A database busy or locked past its wait (`DbError::is_busy`, SQLite 5/6) is a retryable condition, not an internal error; the web client retries idempotent requests once. |
| `AnalysisState::{Planning, Running, Paused}.stage: Option<AnalysisStage>` (`kind`, `flow {index, title}`, `started_at`; `api::analyses`) | The progress screen shows the live job's stage (plan, PR context, flow k with title) and whether the runner has it (`started_at` = claim time, null = waiting for the runner), derived from the jobs table; an `analysis` event is sent on every claim. |
| `AnalysisEvent::Heartbeat { at }` (SSE `heartbeat`, every 15 s; `api::events`) | The client tells an open but silent stream from a dead one; replaces the keep-alive comment. |

### Trust and ownership observations (coordinator-approved, 2026-11-17)

| Change | Reason |
|---|---|
| `CiStatus.check_runs`, `statuses`: optional `CiCounts {passed, failed, pending, skipped, neutral}`; `CiState::NoFailures` | Skipped/neutral runs are not successful runs; commit statuses count too. A missing source means not checked. `passing` requires both complete sources and only success, with at least one result. Existing `total` and `failing` include both sources. |
| `AskQuestion.asked_by`, `mine`; `AnalysisSummary.has_my_state` | Questions remain shared tenant knowledge; each HTTP/SSE response identifies the asker and computes personal flags for its reader. Old stored payloads can omit the fields. |
| `Counters.lines_added_to_tests` | Existing removed-line count includes replacements. Show diff churn as “N test lines changed or removed, M added”, not proof of weakened tests. |
| `Settings.memcan_status`, `PrContextFacts.memcan_status`: `MemcanStatus {state, last_success_at}` | Client observations are separate from the stored profile lookup. States: `reachable`, `not_reachable`, `not_configured`, `not_checked` (before the first completed attempt). No status read makes a network call. |
| `RuleKind::{Lexer, Header, Binary}` | The label names the actual detector, including Rust lexer comments and git binary detection. |
| `Feedback.calibration_example` | Only `overrated`/`underrated` are calibration examples; all values are recorded as feedback memories. A calibration example is saved only when `memory == written`. |

Ownership follows brief §3: “Fragment 1─* Question (Ask thread, LLM answers)” and “User 1─* ReviewState
(per analysis): ticks on fragments, last viewed block”. There is no question edit/delete/retry endpoint;
posting another question creates a new question owned by its sender. Shared broadcasts carry no `mine` or
`has_my_state`; the SSE handler projects them per subscriber.

Settings installations remain user-scoped, as the existing HTTP contract requires: “Installations are read
from GitHub with the user token (`GET /user/installations`, suspended ones left out)”. ADR-007's user-token
lifetime and refresh rules apply. The installation account can differ from the logged-in account; no rule
limits visible installations to those personally owned by the user. The server does not use `/app/installations`
for Settings and does not widen the tenant boundary.

MemCan observations live in the server's shared connection configuration, with no lock held across I/O.
They reset on restart, retain the last successful tool-call time after failure, and reflect the most recently
completed attempt. Current ADR-001 requires MemCan URL/token at startup: `not_configured` is reserved for a
configuration without a token; normal startup cannot produce it. Reachable means the client reached MemCan,
not that every tool or profile lookup succeeded.

### Analysis and verification (`crates/analysis`)
| Change | Reason |
|---|---|
| `VerifyIssueKind::FileTooLarge`, `ImpactInsideDiff`, `MarkupOrLink` (`analysis::verify`) | A file over the read limit, an impact reference on changed lines, and markup or a non-GitHub link in an Ask answer are rejected with a repair message. |
| `AnalysisVerdict.moves_incomplete` | An incomplete move detection is passed through the verdict so the pipeline records it. |

### Jobs and database
| Change | Reason |
|---|---|
| `JobUpdate::Finished { result, github_access }` (`server::jobs`) | The pipeline learns whether the attempt's agent could read GitHub (`runner-protocol.md`). |
| `jobs.github_access_granted` and `documents.github_access` (`0001_init.sql`) | Store that fact per claim and with each accepted agent output. |
| `jobs.round`, `analyses.round`, unique index `jobs_one_live_per_stage` over (analysis, stage, generation, round) | An explicit Retry calls the agent again, while a restart re-uses stored outputs (`runner-protocol.md`). |
| Document kinds `unsent` and `memcan_profile` (`documents.kind`) | Lines left out of every LLM request with their reason; the analysis's MemCan profile lookup. |
| `JobSpec.context_pack` built at claim | A lost agent session falls back to a context pack; a claim whose pack cannot be built is released. |

### Coordinator-approved follow-up (2026-11-17)
| Change | Reason |
|---|---|
| `PrContextFacts.related_work_links?: RelatedWorkLink[]` with `reference`, `repo`, `number`, `kind`, `verified` | Safe GitHub links from strictly parsed server fields, with independent resource verification; agent output schemas unchanged. |
| `NotAnalysedItem.renderable?: bool`, `rule_kind?: RuleKind` | Open mechanical text when available and name the detector actually used; do not offer generated/binary content. |
| `JobFailureKind::Configuration`, `ApiErrorCode::Configuration`, `AnalysisState::Failed.reason?: ApiErrorCode` | Required skills fail closed with an administrator-action message and no UI Retry. |
| `analyses.failure_reason`, `llm_credentials.last_call_failure` accepts `configuration` (`0001_init.sql`) | Preserve the typed cause through stored analysis state and Settings; follows the pre-deployment single-schema convention. |

### Required step assessments (2026-11-17 regression follow-up)
| Change | Reason |
|---|---|
| `AgentStep.assessment` required and non-null in the agent schema; verifier rejects missing assessments | Brief §3 requires a per-step rating with reason and basis, also for LOW. The flow band is derived from the highest step band. Older documents remain deserializable for verification and repair. |
| Flow schema rejections enter the existing two-repair budget | A missing or null required assessment requests correction before acceptance; exhausted repairs leave the flow Unexplained. |
| `Blocking::NoActionProposed` (`no_action_proposed`) | Rate bounded exposure without inventing a defect, blocking action or follow-up; never suppress the assessment to represent absence of an action. |

### Owner-approved PR session lifecycle (2026-11-17)

| Change | Reason |
|---|---|
| `JobSpec.pull_request_id`; `SessionRef { pull_request_id, thread_id }`; numeric PR keys in `drop_sessions` / `dropped_sessions` | One analytic session across every head, language and Retry of a PR; Ask uses private forks as specified below (ADR-004). |
| Move `agent_thread_id`, `session_state` to `pull_requests`; AUTOINCREMENT PR ids; running-job exclusion per PR | Stable, non-reused archive key; serialize shared-session jobs even with different credentials. Single rewritable migration, no compatibility. |
| Codex auto-compaction at 400000; archive each successful useful turn | Remove the extra compaction turn and its latency; usage remains the useful turn's delta. |

## Waiting progress (owner approval, 2026-11-17)

| Addition | Reason |
|---|---|
| Optional `AnalysisStage.part {index, total}` | Fixed count of plan batches, including jobs already stored before this change. |
| Optional `AnalysisSummary.progress` with `started_at`, four stage counters, `percent` and nullable `estimate` | Processing progress and ETA on the waiting screen; never a review score. Derived from existing jobs and documents without a migration or stored-document change. |
| `AnalysisEstimate {remaining_seconds, basis, rough}`; basis `this_analysis`, `recent_runs`, `lines_only` | Show the source and uncertainty of an estimate; null when history is insufficient. |

## Usage and pre-start size warning (owner approval, 2026-11-17)

| Addition | Reason |
|---|---|
| Nullable `AnalysisSummary.usage: UsageTotals` | Sum existing finished job reports across all rounds, including Ask; no new storage or runner protocol. |
| `UsageTotals {input_tokens, cached_input_tokens, output_tokens, reasoning_output_tokens, jobs}` | Provider-reported counts. Reasoning is null because the stored job contract has no separate reasoning count. |
| Optional `Settings.llm_usage {today, last_7_days}` | One tenant-scoped aggregate query over existing results, using the server's local calendar days. Each window is nullable without reports. |
| Nullable `ResolvePrResponse.size_warning {changed_lines, changed_files, estimate}` | Non-blocking warning at 3,000 changed lines; metadata from the existing GitHub PR read and the waiting screen's estimator. |

## Installation settings and user pause (owner approval, 2026-11-17)

| Addition | Reason |
|---|---|
| Optional `Settings.llm_selection {models, efforts, current, changed_by, changed_at}`; `PUT /api/v1/settings/llm` with `ModelChoice` | One installation-wide choice, CSRF protected, fixed server catalog, auditable editor and time. |
| Optional `AnalysisSummary.models_used: ModelChoice[]` | Facts from distinct claimed job specs across all rounds, including Ask. |
| Optional paused-state `reason: user \| usage_limit`, `paused_by`, `paused_at`; POST `/api/v1/analyses/{id}/pause` and `/resume` | Persistent user pause, independent of provider limits; current job may finish, further submissions and claims wait. Missing reason means usage limit for older clients. |
| Singleton `llm_selection` table; `analyses.user_paused_by`, `analyses.user_paused_at` | Owner-approved rewrite of the single pre-deployment migration; local database must be recreated by the coordinator. Stored documents and runner protocol unchanged. |

## Consequences
- The generated TypeScript types and JSON Schemas follow these types; the drift test guards them.
- Further contract changes still need a change request (ADR-010).

## Known deviations from the brief
These remaining statements in `docs/design-brief.md` are superseded:

| Brief statement | Now | Reference |
|---|---|---|
| §5 "GitHub access for the agent": token "restricted to the PR's repository and its related repositories" | `repositories` = PR repository only; related repositories are read as public data | ADR-005, threat model T3a |
| §5 "Job execution interface": `JobExecutor` = `submit`, `events`, `cancel` | adds `cancel_analysis` | this ADR, plan rule A3 |
| §2 "Agent access": "Read-only checkout, no code execution"; §5 "Security": "only the LLM credential and the job's read-only installation token in the runner", §5: "the agent holds only the job's short-lived read-only installation token" | Codex sandbox off: no code execution is a prompt rule only; the agent uid can read its per-job Codex credential, installation token; MemCan stays server-side under ADR-004 | ADR-002, threat model AR-4, AR-6, §9 |
| §5 Agent sessions: one per analysis, compact at each job end, new session on moved head; session files persist across one analysis only | One analytic session per PR across analyses, private working forks per PR/user for Ask; automatic compaction at 400k only; Cancel/Re-analyze preserve archives, retention defaults to 14 days | owner decision 2026-11-17, ADR-004 |

## Private Ask session forks (owner approval, 2026-11-17)

| Change | Reason |
|---|---|
| `JobSpec.working_session?: {user_key, analytic_generation, thread_id?}` | Self-contained archive routing: Ask restores its valid working fork, else analytic; saves only its private fork (ADR-004). |
| `pull_requests.analytic_generation`; `working_sessions` keyed by PR/user | Re-fork after analytic advancement without allowing user instructions into analysis. |
| Questions HTTP/SSE filtered to asker; private quotas/history | Shared analyses never expose foreign Q&A, counts or ownership markers. Existing `asked_by`/`mine` retained for compatibility, always caller-owned. |
| `AgentAccess.memcan.allow_write?: bool` (default false) | The agent has MemCan again: analytic jobs get search and `add_memory`, Ask jobs search only (ADR-004). |
| MemCan feedback `private_text_excluded: true`; Ask has no MemCan write tool | Prevent private feedback notes or Ask-written memories from reaching shared analytical context. Legacy calibration text is not read. |

## Lazy review and concise low assessments (owner approval, 2026-11-17)

| Addition | Reason |
|---|---|
| Optional `AnalysisSummary.review_ready`, `background_analysis` | Review opens after accepted plan/context; background work is a durable per-analysis preference, default on. |
| Optional `Flow.analysis_status {state, started_at, message}`, `planned {intent, importance_rank, ranges}` | All planned flows appear before steps exist; pending work cannot be mistaken for accepted assessment. |
| `POST /api/v1/analyses/{id}/flows/{flow}/request`; `PUT /api/v1/analyses/{id}/background` (`{enabled}`) | CSRF, tenant and moved-head checks; newest demand precedes rank-ordered background work, never interrupts a claimed job. |
| `analyses.background_analysis`; `flow_work` requests/failures | Single rewritten pre-deployment migration; lazy scheduling adds no stored plan/flow fields or runner protocol change. Coordinator recreates the local DB. |
| Nullable/optional assessment `headline`, `basis_note`, `not_checked`, `origin`, `blocking`; concise closed shape `{risk:"low", reason}` | Low assessments contain one sentence; full details and action requirements remain mandatory above low. Existing full assessments still parse. |

| `flow_work.auto_retries` (migration `0002_flow_auto_retries.sql`, in-place, data kept) | A failed flow is retried automatically at most twice (owner decision, 2026-11-18), then stays failed until requested. |
| No new state: an analysis without admitted flow work stays `running {done,total}` without `stage`, with no driver task | Unrequested or failed-for-good flows no longer hold a task and the diff in memory; `finished` keeps meaning "every flow analysed". `missed_by_llm` is recorded for comments in analysed flows before `finished`. |
| `Flow.analysis_status.message` of a failed flow job: `The flow analysis failed. <cause sentence> Retry this flow.` | The typed cause of the failed call is kept (same fixed sentences as Settings); an unusable credential fails the analysis with the sign-in sentence. |
| Question failure sentences for a cancelled `ask` job name the cause (analysis cancelled, PR session removed, other) | A failed pipeline or Retry no longer cancels Ask jobs; the "analysed again" sentence is used only when the PR session was removed. |
| Unexplained rows keep the planned lines of failed flows (with `too_large_for_prompt` when not sent) | Only flows still waiting are left out of Unexplained; failed ones stay listed as not explained. |

Existing `analysis` SSE events publish availability and scheduling changes; clients refetch the Tour.
A failed flow stays explicitly failed until requested again or retried automatically (twice at most); accepted siblings remain usable. Retry starts
another analysis job round at a serial boundary, preserving accepted documents and PR/private sessions.

## Flow file output (owner approval, 2026-11-17)

| Addition | Reason |
|---|---|
| `JobSpec.flow_file?: {context, previous?}` | Self-contained, versioned no-secret flow checker inputs; rejected text restored for edit-only repairs (ADR-014). |
| Flow `JobOutcome::Succeeded.output` carries a JSON string | Transport tagged file contents through the existing output channel; server parses/checks into unchanged canonical documents. Other job kinds keep JSON objects. |

### 2026-11-17 — compact review and deterministic code context (owner approved)

| Addition | Purpose |
|---|---|
| `LinesResponse.context_only?: number[]` | Rust comment/import changed lines covered by deterministic rules; the user's main-view preference may hide them. Classic diff is unchanged. |
| `DeletedBlock.replacement?: LineRange` | Head lines paired with old lines from the same zero-context diff hunk; absent for pure deletion. |
| `DeletedBlock.context_only?: number[]` | Rule-covered base lines within removed/replaced blocks; applies the same main-view preference. |

No stored document or database schema changes. Step ids and numbers remain unchanged; the top-to-bottom diagram follows the persisted walk order at every viewport width.
