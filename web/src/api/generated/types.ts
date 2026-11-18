// This file is generated from crates/contracts by the test generated_artifacts_are_current (UPDATE_GENERATED=1 rewrites it). Do not edit.

/**
 * A token as listed in Settings (`GET /api/v1/settings/access-tokens`); never carries the secret or its hash.
 */
export type AccessToken = {
/**
 * Public id.
 */
id: AccessTokenId,
/**
 * The user's name for the token, 1–60 characters, unique among the user's tokens.
 */
name: string,
/**
 * Recognisable start of the token, `srp_<id>`.
 */
prefix: string,
/**
 * What the token may do.
 */
scopes: Array<TokenScope>,
/**
 * Creation time.
 */
created_at: string,
/**
 * The token stops working at this time.
 */
expires_at: string,
/**
 * Last accepted use (recorded at most once a minute); null before the first.
 */
last_used_at: string | null, };

/**
 * Public id of a token: 8 base62 characters, shown in Settings and part of the token (`srp_<id>_<secret>`).
 */
export type AccessTokenId = string;

/**
 * The whole token `srp_<id>_<secret>`: sent once in [`CreatedAccessToken`], never shown by `Debug`.
 */
export type AccessTokenSecret = string;

/**
 * A bounded estimate, not a deadline or a guarantee.
 */
export type AnalysisEstimate = {
/**
 * Remaining seconds, never more than seven days.
 */
remaining_seconds: number,
/**
 * Strongest observation used; before planning ends the flow component is only line-based.
 */
basis: EstimateBasis,
/**
 * True while the flow count is unknown and estimated from changed lines.
 */
rough: boolean, };

/**
 * One SSE event; the SSE `event` field is the `event` tag, `data` is this JSON.
 */
export type AnalysisEvent = { "event": "analysis",
/**
 * The analysis.
 */
summary: AnalysisSummary, } | { "event": "question",
/**
 * The question.
 */
question: AskQuestion, } | { "event": "heartbeat",
/**
 * Server time of sending.
 */
at: string, };

/**
 * Identifier of an analysis (one per pull request, head commit and LLM language).
 */
export type AnalysisId = string;

/**
 * Whole-analysis waiting progress, never review completion or confidence.
 */
export type AnalysisProgress = {
/**
 * Start of this run, preserved across parts and server restarts.
 */
started_at: string,
/**
 * Plan parts accepted.
 */
planning: RunStage,
/**
 * PR context accepted.
 */
pr_context: RunStage,
/**
 * Flows completed; total unknown until the plan is accepted.
 */
flows: RunStage,
/**
 * Final verification and projection.
 */
finishing: RunStage,
/**
 * Estimated remaining time; null without sufficient observations, or while paused/terminal. With background
 * analysis off it covers plan and PR context only and is null once the review is ready.
 */
estimate: AnalysisEstimate | null,
/**
 * Overall waiting progress, 0..=100; null until a denominator can be estimated or counted. With background
 * analysis off it covers plan and PR context only and is null once the review is ready (until finished).
 */
percent: number | null, };

/**
 * The stage of the analysis's live job (the queued, running or paused job of its current round); only facts the
 * server knows, no estimate.
 */
export type AnalysisStage = {
/**
 * Current planning part; present only for a multi-part plan.
 */
part?: PlanPart,
/**
 * What the job does.
 */
kind: StageKind,
/**
 * The flow analysed (`flow` only); absent when the stored plan does not list it.
 */
flow?: StageFlow,
/**
 * When the server handed the job to the runner; null while the job waits for the runner.
 */
started_at: string | null, };

/**
 * State of an analysis (brief §4.2).
 */
export type AnalysisState = { "state": "planning",
/**
 * The current stage; absent when no job of the analysis is queued, running or paused.
 */
stage?: AnalysisStage, } | { "state": "running",
/**
 * Flows analysed.
 */
done: number,
/**
 * Flows in total.
 */
total: number,
/**
 * True when flows remain but none is admitted (none requested, background analysis off or the rest
 * failed for good): nothing is analysed until a flow is requested or background analysis is switched on.
 */
idle?: boolean,
/**
 * The current stage; absent when no job of the analysis is queued, running or paused.
 */
stage?: AnalysisStage, } | { "state": "paused",
/**
 * User pause takes precedence over a provider usage limit.
 */
reason?: PauseReason,
/**
 * Login that requested the pause.
 */
paused_by?: string,
/**
 * When the user requested the pause.
 */
paused_at?: string,
/**
 * When it resumes, if known.
 */
resumes_at: string | null,
/**
 * Flows analysed before the pause.
 */
done: number,
/**
 * Flows in total; null while planning (the number of flows is not known yet).
 */
total: number | null,
/**
 * The stage that resumes; absent when no job of the analysis is queued, running or paused.
 */
stage?: AnalysisStage, } | { "state": "failed",
/**
 * Typed cause when known; absent on older failures.
 */
reason?: ApiErrorCode,
/**
 * The flow that failed, or null when it failed before any flow existed.
 */
flow: FlowRef | null,
/**
 * Plain description of what failed.
 */
message: string, } | { "state": "finished",
/**
 * Flows in total.
 */
total: number, } | { "state": "cancelled" };

/**
 * An analysis and its state.
 */
export type AnalysisSummary = {
/**
 * Plan and PR context are accepted; analysed flows can be reviewed while work continues.
 */
review_ready?: boolean,
/**
 * Whether unrequested flows are analysed in rank order.
 */
background_analysis?: boolean,
/**
 * Distinct model choices of jobs actually claimed, in first-use order.
 */
models_used?: Array<ModelChoice>,
/**
 * Provider-reported totals of finished jobs; null without reports.
 */
usage: UsageTotals | null,
/**
 * Whole-run progress derived from existing jobs and documents, absent on older servers.
 */
progress?: AnalysisProgress,
/**
 * Whether this reader has saved review state (including explicitly empty state).
 */
has_my_state?: boolean,
/**
 * Analysis id.
 */
id: AnalysisId,
/**
 * The pull request as analysed (its head is the analysis head).
 */
pr: PrSummary,
/**
 * Language of the LLM texts.
 */
language: LlmLanguage,
/**
 * State and progress.
 */
state: AnalysisState,
/**
 * When the analysis was requested.
 */
created_at: string,
/**
 * When it finished, failed or was cancelled.
 */
finished_at: string | null, };

/**
 * An annotation anchored to lines of a step (same side as the step); its range is a highlight.
 */
export type Annotation = {
/**
 * Annotation id.
 */
id: AnnotationId,
/**
 * Type.
 */
type: AnnotationType,
/**
 * Fact or inference.
 */
basis: Basis,
/**
 * Annotated lines.
 */
range: LineRange,
/**
 * Behaviour before the pull request; the hover tooltip shows only was → is.
 */
was: string,
/**
 * Behaviour after the pull request.
 */
is: string,
/**
 * The consequence, the flag's title or the note; null for a behaviour annotation.
 */
text: string | null,
/**
 * Evidence lines.
 */
evidence: Array<Evidence>,
/**
 * Present exactly on a flag: its level (`unknown` for a flag stored before levels), scenario and what was
 * not checked.
 */
flag?: Flag, };

/**
 * Identifier of an annotation, assigned by the server: `"<step>.a<n>"`, e.g. `f2.3.a1`.
 */
export type AnnotationId = string;

/**
 * Type of an annotation inside a step.
 */
export type AnnotationType = "behaviour" | "consequence" | "flag" | "note";

/**
 * Body of every API error response.
 */
export type ApiError = {
/**
 * Machine-readable code.
 */
code: ApiErrorCode,
/**
 * Plain message for the user.
 */
message: string, };

/**
 * Machine-readable API error code.
 */
export type ApiErrorCode = "configuration" | "unauthenticated" | "login_not_allowed" | "csrf_rejected" | "not_found" | "invalid_request" | "not_a_pr_url" | "private_repository" | "app_not_installed" | "analysis_not_finished" | "conflict" | "pr_head_changed" | "position_rejected" | "comment_published" | "range_too_large" | "not_rendered" | "github_unavailable" | "internal" | "unavailable" | "refresh_running" | "github_rate_limited" | "invalid_dashboard_settings" | "cursor_expired" | "insufficient_scope" | "token_not_accepted" | "token_limit_reached" | "rate_limited";

/**
 * A GitHub App installation.
 */
export type AppInstallation = {
/**
 * Account (user or organisation) login.
 */
account: string,
/**
 * Installed on all repositories of the account, or on selected ones.
 */
all_repositories: boolean, };

/**
 * An LLM answer to a question about a step.
 */
export type AskOutput = {
/**
 * The answer; a suspected defect is stated as a risk scenario saying what was not checked.
 */
answer: string,
/**
 * Lines read that support the answer.
 */
evidence: Array<Evidence>, };

/**
 * A private question and its LLM answer, pinned to the step; returned only to its asker.
 */
export type AskQuestion = {
/**
 * Question id.
 */
id: QuestionId,
/**
 * GitHub login of the asker; absent only in older stored payloads.
 */
asked_by?: string,
/**
 * Always true in current HTTP/SSE responses; absent only in older payloads.
 */
mine?: boolean,
/**
 * The step.
 */
step_id: StepId,
/**
 * The question.
 */
text: string,
/**
 * State.
 */
state: AskState,
/**
 * The LLM answer once answered.
 */
answer: AskOutput | null,
/**
 * Plain description when failed.
 */
failure: string | null,
/**
 * When it was asked.
 */
created_at: string,
/**
 * While `pending` and waiting for the LLM usage limit to reset: when its job is queued again; absent
 * otherwise.
 */
resumes_at?: string, };

/**
 * Body of `POST /api/v1/analyses/{id}/steps/{step_id}/questions`.
 */
export type AskRequest = {
/**
 * The question, at most `AskRequest::MAX_TEXT_CHARS` characters.
 */
text: string, };

/**
 * State of a question.
 */
export type AskState = "pending" | "answering" | "answered" | "failed";

/**
 * Assessment of a step at system scale (brief §3); evidence comes before it.
 */
export type Assessment = {
/**
 * What a production user or operator would notice.
 */
headline?: string,
/**
 * Risk band from the severity skill; never its numeric scores.
 */
risk: RiskBand,
/**
 * Reason for the band, also for low.
 */
reason: string,
/**
 * What the agent did: `Read: …` and, if it ran anything, `Ran: …`.
 */
basis_note?: string,
/**
 * What was not checked.
 */
not_checked?: string,
/**
 * Introduced by this pull request or pre-existing.
 */
origin?: Origin,
/**
 * Whether an action is proposed before merge, as a follow-up, or neither.
 */
blocking?: Blocking,
/**
 * The smallest correction, or null; the agent may omit it for low.
 */
smallest_correction: string | null, };

/**
 * A pull request of the user that waits on other people.
 */
export type AwaitingItem = {
/**
 * Display alias, e.g. `PLAT-1176`.
 */
id: string,
/**
 * Repository.
 */
repo: RepoRef,
/**
 * Pull request number.
 */
number: number,
/**
 * Title.
 */
title: string,
/**
 * GitHub link, built by the server.
 */
url: string,
/**
 * Logins and teams (`team:<slug>`) whose review is requested.
 */
waiting_on: Array<string>,
/**
 * Since when it waits (the oldest open request).
 */
waiting_since: string,
/**
 * Deterministic tags from scan facts (e.g. CI passing).
 */
tags: Array<FactTag>,
/**
 * Its score in tenths, when it is also ranked.
 */
total_tenths: number | null, };

/**
 * Change background work; an already running job is kept.
 */
export type BackgroundAnalysisRequest = {
/**
 * Analyse remaining flows automatically.
 */
enabled: boolean, };

/**
 * Whether a statement was read in the code or inferred.
 */
export type Basis = "fact" | "inference";

/**
 * A block of the Tour sequence or a menu row.
 */
export type BlockRef = { "block": "pr_context" } | { "block": "flow_intro",
/**
 * The flow.
 */
flow_id: FlowId, } | { "block": "step",
/**
 * The step.
 */
step_id: StepId, } | { "block": "quiet",
/**
 * The row.
 */
row: QuietRowRef, } | { "block": "pending_review" };

/**
 * Merge impact of an assessed step.
 */
export type Blocking = "no_action_proposed" | "blocks_this_pr" | "follow_up";

/**
 * Callers and callees of a step symbol.
 */
export type CallHierarchy = {
/**
 * Callers.
 */
callers: Array<CallSite>,
/**
 * Callees.
 */
callees: Array<CallSite>, };

/**
 * A caller or callee.
 */
export type CallSite = {
/**
 * Symbol.
 */
symbol: string,
/**
 * Repository-relative path.
 */
file: string,
/**
 * Line.
 */
line: number,
/**
 * GitHub link.
 */
github_url: string, };

/**
 * A failing check.
 */
export type CiCheck = {
/**
 * Check name.
 */
name: string,
/**
 * Link to the check, if any.
 */
url: string | null,
/**
 * Names of failing tests, when the check reports them.
 */
failing_tests: Array<string>, };

/**
 * Checks of one CI source by outcome.
 */
export type CiCounts = {
/**
 * Concluded `success`.
 */
passed: number,
/**
 * Failed: `failure`, `error`, `timed_out`, `cancelled`, `action_required` and any other conclusion.
 */
failed: number,
/**
 * Not concluded yet (queued, in progress, a `pending` status).
 */
pending: number,
/**
 * Concluded `skipped` (check runs only): did not run.
 */
skipped: number,
/**
 * Concluded `neutral` (check runs only).
 */
neutral: number, };

/**
 * Overall CI state.
 */
export type CiState = "passing" | "no_failures" | "failing" | "pending" | "none" | "not_checked";

/**
 * CI status of the head commit: its check runs and its commit statuses.
 */
export type CiStatus = {
/**
 * Overall state.
 */
state: CiState,
/**
 * Number of checks read: check runs plus commit statuses.
 */
total: number,
/**
 * Failing checks (check runs and commit statuses).
 */
failing: Array<CiCheck>,
/**
 * Check runs by outcome; absent when the check runs were not read.
 */
check_runs?: CiCounts,
/**
 * Commit statuses (latest per context) by outcome; absent when the statuses were not read.
 */
statuses?: CiCounts, };

/**
 * Classification of a flow (brief §3, closed list).
 */
export type Classification = "behavioural" | "api_contract" | "new_code" | "refactor_pure" | "move" | "rename" | "docs_comments" | "generated_mechanical" | "config_build" | "dependency" | "other";

/**
 * Where a pending comment sits on GitHub.
 */
export type CommentKind = "line" | "file";

/**
 * Commenting on displayed lines, decided by the server from the pull request diff (ADR-006).
 */
export type CommentScope = { "scope": "in_diff",
/**
 * Ranges, on the response's side, whose lines get a line thread.
 */
line_thread_ranges: Array<LineRange>, } | { "scope": "not_in_diff" };

/**
 * The displayed line(s) a comment is written on.
 */
export type CommentTarget = {
/**
 * Repository-relative path.
 */
path: string,
/**
 * `head` for head lines, `base` for removed lines.
 */
side: LineSide,
/**
 * First line of a multi-line target.
 */
start_line: number | null,
/**
 * Last line.
 */
line: number, };

/**
 * A full 40-character hexadecimal git commit id.
 */
export type CommitSha = string;

/**
 * Counters over material that is not analysed.
 */
export type Counters = {
/**
 * Added diff lines in deterministically classified test files; not an assertion or test-function count.
 */
lines_added_to_tests?: number,
/**
 * Removed diff lines from test files, including replacements; label as "changed or removed".
 */
lines_removed_from_tests: number,
/**
 * Names of changed dependencies.
 */
dependencies_changed: Array<string>,
/**
 * Changed dependency files whose dependencies were not checked (other ecosystems, unreadable or unparsable
 * files); their changes are not counted in `dependencies_changed`.
 */
dependency_files_not_checked: Array<string>, };

/**
 * Body of `POST /api/v1/settings/access-tokens`.
 */
export type CreateAccessToken = {
/**
 * Name, 1–60 characters, unique among the user's tokens.
 */
name: string,
/**
 * Requested scopes; v1.1 accepts only `read`.
 */
scopes: Array<TokenScope>,
/**
 * Lifetime in days: one of [`CreateAccessToken::EXPIRY_DAYS`].
 */
expires_in_days: number, };

/**
 * Body of `POST /api/v1/analyses`.
 */
export type CreateAnalysisRequest = {
/**
 * The pull request.
 */
pr: PrRef,
/**
 * The head the user saw; a different current head is a conflict.
 */
head_sha: CommitSha, };

/**
 * Body of `POST /api/v1/analyses/{id}/pending-review/comments`.
 */
export type CreateCommentRequest = {
/**
 * Client-generated key; a repeated request returns the first response.
 */
idempotency_key: string,
/**
 * Target lines.
 */
target: CommentTarget,
/**
 * Comment text.
 */
body: string, };

/**
 * Response of creating a comment.
 */
export type CreateCommentResponse = {
/**
 * The created comment.
 */
comment: PendingComment,
/**
 * The line was changed and had no annotation, so `missed_by_llm` feedback was recorded.
 */
missed_by_llm_recorded: boolean, };

/**
 * Response of `POST /api/v1/settings/access-tokens` (201): the only response that carries the secret.
 */
export type CreatedAccessToken = {
/**
 * The token; store it now, it cannot be shown again.
 */
token: AccessTokenSecret,
/**
 * The new token as listed.
 */
access_token: AccessToken, };

/**
 * Server-sent event of `GET /api/v1/dashboard/refreshes/{id}/events`; the SSE `event` field is the tag.
 */
export type DashboardEvent = { "event": "refresh",
/**
 * The refresh.
 */
status: RefreshStatus, } | { "event": "heartbeat",
/**
 * Server time of sending.
 */
at: string, };

/**
 * Per-user dashboard settings (body of `PUT /api/v1/settings/dashboard`); validated by the server (plan §F.6).
 */
export type DashboardSettings = {
/**
 * Other GitHub logins of the user, scanned with the own login.
 */
linked_logins: Array<string>,
/**
 * Repositories with their rules, one row each.
 */
repositories: Array<RepositoryRule>,
/**
 * Owners listed when GitHub search refuses a scanned login.
 */
enumerate_owners: Array<string>,
/**
 * Days an issue counts as new (1–30).
 */
lookback_days: number,
/**
 * Label that marks a pull request ready for a human.
 */
ready_label: string,
/**
 * Excluded authors: every pull request and issue they author is left out of the dashboard. Being listed
 * does not make a login a bot.
 */
skip_authors: Array<string>,
/**
 * Logins treated as bots besides GitHub's `Bot` accounts and `[bot]` logins: never a waiting human.
 */
known_bots: Array<string>,
/**
 * Show the user's MemCan TODOs.
 */
include_todos: boolean,
/**
 * MemCan projects read besides the repositories' short names.
 */
memcan_extra_projects: Array<string>,
/**
 * Daily automatic refresh on.
 */
schedule_enabled: boolean,
/**
 * Its local time, `HH:MM` (24 h, 15-minute steps).
 */
schedule_time: string,
/**
 * IANA time zone; null until the browser sends one.
 */
time_zone: string | null, };

/**
 * Response of `GET`/`PUT /api/v1/settings/dashboard`.
 */
export type DashboardSettingsView = {
/**
 * The user's settings (the defaults when never saved).
 */
settings: DashboardSettings,
/**
 * The defaults ("Reset to defaults").
 */
defaults: DashboardSettings,
/**
 * The signed-in login, always scanned.
 */
own_login: string,
/**
 * MemCan is configured on this server.
 */
memcan_configured: boolean,
/**
 * Effective MemCan projects: the repositories' short names plus the extras, deduplicated.
 */
memcan_projects: Array<string>,
/**
 * Repositories and owners of these settings the GitHub token could not see at the latest refresh.
 */
unreachable: Array<string>, };

/**
 * Response of `GET /api/v1/dashboard`: the user's dashboard without its items.
 */
export type DashboardSummary = {
/**
 * Opaque; changes whenever the stored snapshot or its ranking changes (poll this, then fetch the items).
 */
revision: string,
/**
 * The newest refresh of the user, live or ended; null before the first.
 */
latest_refresh: RefreshStatus | null,
/**
 * The snapshot the items come from; null before the first successful scan.
 */
snapshot: SnapshotMeta | null,
/**
 * The daily automatic refresh.
 */
schedule: ScheduleStatus,
/**
 * Items of the snapshot whose judgement came from an older ranking prompt (re-judged within the caps).
 */
older_prompt_judgements: number, };

/**
 * Removed code shown at its deletion marker.
 */
export type DeletedBlock = {
/**
 * Head line after which the code was removed; 0 = before the first line.
 */
after_line: number,
/**
 * First removed line at the base commit.
 */
base_start: number,
/**
 * Removed lines, from `base_start` on; fewer than `total_lines` when the response's line budget ran out
 * (read the rest with `side=base`).
 */
lines: Array<string>,
/**
 * Number of removed lines in the block.
 */
total_lines: number,
/**
 * Head range paired with these old lines; absent for pure deletions.
 */
replacement?: LineRange,
/**
 * Removed Rust comment and plain import lines of the block a deterministic line rule covers: the main view may
 * hide them.
 */
context_only?: Array<number>, };

/**
 * A diff hunk.
 */
export type DiffHunk = {
/**
 * The `@@ … @@` header.
 */
header: string,
/**
 * Lines.
 */
lines: Array<DiffLine>, };

/**
 * A line of a diff hunk.
 */
export type DiffLine = {
/**
 * Kind.
 */
kind: DiffLineKind,
/**
 * Line number at the base commit.
 */
old_line: number | null,
/**
 * Line number at the head commit.
 */
new_line: number | null,
/**
 * Text without the diff marker and line ending.
 */
text: string,
/**
 * Line ending when it is not LF: `crlf`, or `no_newline` for a last line without one; null for LF.
 */
eol: LineEnding | null, };

/**
 * Kind of a diff line.
 */
export type DiffLineKind = "context" | "added" | "removed";

/**
 * Classic diff of a range.
 */
export type DiffResponse = {
/**
 * Repository-relative path.
 */
path: string,
/**
 * Side the requested range refers to.
 */
side: LineSide,
/**
 * Hunks overlapping the range.
 */
hunks: Array<DiffHunk>, };

/**
 * Where waiting-time estimates came from.
 */
export type EstimateBasis = "this_analysis" | "recent_runs" | "lines_only";

/**
 * A line the agent actually read and cites as evidence.
 */
export type Evidence = {
/**
 * Repository-relative path on that side.
 */
file: string,
/**
 * head for the checked-out code; base only for removed code read from the base commit.
 */
side: LineSide,
/**
 * 1-based line number on that side.
 */
line: number, };

/**
 * A labelled scan fact.
 */
export type Fact = {
/**
 * Label.
 */
label: string,
/**
 * Value.
 */
value: string, };

/**
 * A deterministic tag from scan facts.
 */
export type FactTag = {
/**
 * Short text.
 */
text: string,
/**
 * Tooltip with the underlying fact.
 */
tip: string, };

/**
 * A stored feedback entry.
 */
export type Feedback = {
/**
 * Feedback id.
 */
id: FeedbackId,
/**
 * Whether this value is a severity calibration example; saved only when `memory` is `written`.
 */
calibration_example?: boolean,
/**
 * What it is about.
 */
target: FeedbackTarget,
/**
 * The value.
 */
value: FeedbackValue,
/**
 * Optional note.
 */
note: string | null,
/**
 * When it was recorded.
 */
created_at: string,
/**
 * The copy of this entry's current revision in MemCan (calibration memory for `overrated` /
 * `underrated`); only `written` means MemCan confirmed it.
 */
memory: FeedbackMemory, };

/**
 * Identifier of a feedback entry.
 */
export type FeedbackId = string;

/**
 * State of the MemCan copy of a feedback entry's current revision.
 */
export type FeedbackMemory = "pending" | "written" | "failed";

/**
 * Body of `POST /api/v1/analyses/{id}/feedback`: one entry per user, target and value, so several values may
 * coexist on a target; posting a value again replaces its note; `overrated` and `underrated` exclude each other
 * (posting one removes the other). `missed_by_llm` is recorded only by the server.
 */
export type FeedbackRequest = {
/**
 * What the feedback is about.
 */
target: FeedbackTarget,
/**
 * The value.
 */
value: UserFeedbackValue,
/**
 * Optional note, at most 2000 characters (`FeedbackRequest::MAX_NOTE_CHARS`).
 */
note: string | null, };

/**
 * What a feedback entry is about.
 */
export type FeedbackTarget = { "target": "step",
/**
 * The step.
 */
step_id: StepId, } | { "target": "annotation",
/**
 * The annotation.
 */
annotation_id: AnnotationId, } | { "target": "line",
/**
 * Repository-relative path on that side.
 */
path: string,
/**
 * `base` for a removed line.
 */
side: LineSide,
/**
 * Line on that side.
 */
line: number, };

/**
 * Every stored feedback value (brief §3).
 */
export type FeedbackValue = "correct" | "wrong" | "irrelevant" | "overrated" | "underrated" | "missed_by_llm";

/**
 * A line range of one file.
 */
export type FileRange = {
/**
 * Repository-relative path on that side.
 */
file: string,
/**
 * Commit the lines refer to.
 */
side: LineSide,
/**
 * The lines.
 */
range: LineRange, };

/**
 * Several line ranges of one file on one side.
 */
export type FileRanges = {
/**
 * Repository-relative path on that side.
 */
file: string,
/**
 * Commit the lines refer to.
 */
side: LineSide,
/**
 * The lines, ascending and disjoint.
 */
ranges: Array<LineRange>, };

/**
 * A question about the fit of the change at project scale.
 */
export type FitQuestion = {
/**
 * The question, phrased as a question.
 */
question: string,
/**
 * What the question is about.
 */
topic: FitTopic,
/**
 * Lines read that motivate the question.
 */
evidence: Array<Evidence>, };

/**
 * Topic of a fit question.
 */
export type FitTopic = "layer_or_repository" | "size_for_problem" | "duplicate_mechanism" | "other";

/**
 * The scenario of a flag; its title (the risk class, "Potential …") is the annotation's `text`.
 */
export type Flag = {
/**
 * red or yellow in agent output; unknown only on a flag stored before levels.
 */
level: FlagLevel,
/**
 * Ordered steps: who acts, the order of events, the outcome and its cost.
 */
scenario: Array<string>,
/**
 * One line: what was not reproduced or run.
 */
not_checked: string, };

/**
 * How strongly the LLM flags a risk: its own judgement, not checked.
 */
export type FlagLevel = "red" | "yellow" | "unknown";

/**
 * One flow.
 */
export type Flow = {
/**
 * Present on lazy Tours; absent on older finished Tours.
 */
analysis_status?: FlowAnalysisStatus,
/**
 * Plan metadata and deterministic ranges, available before step analysis.
 */
planned?: PlannedFlowDetails,
/**
 * Flow id.
 */
id: FlowId,
/**
 * 1-based position in the tour.
 */
position: number,
/**
 * Ordering group and menu label, derived from the classification.
 */
group: FlowGroup,
/**
 * Deterministic facts.
 */
facts: FlowFacts,
/**
 * LLM analysis.
 */
llm: FlowLlm,
/**
 * Flow graph; nodes derived by the server from the steps.
 */
graph: FlowGraph,
/**
 * Steps in order.
 */
steps: Array<Step>, };

/**
 * Progress of one planned flow.
 */
export type FlowAnalysisState = "not_analysed" | "queued" | "analysing" | "analysed" | "failed";

/**
 * Work status, separate from risk or the reviewer's ticks.
 */
export type FlowAnalysisStatus = {
/**
 * Where the work stands.
 */
state: FlowAnalysisState,
/**
 * Runner claim time for the current attempt, if any.
 */
started_at: string | null,
/**
 * Plain failure description; never provider output.
 */
message: string | null, };

/**
 * Deterministic facts of a flow.
 */
export type FlowFacts = {
/**
 * Files touched by the flow's steps.
 */
files: Array<string>,
/**
 * Number of steps.
 */
step_count: number, };

/**
 * A flow graph: nodes are steps.
 */
export type FlowGraph = {
/**
 * One node per step.
 */
nodes: Array<GraphNode>,
/**
 * Edges in execution order.
 */
edges: Array<GraphEdge>, };

/**
 * Ordering group of a flow (brief §4.3 flow order).
 */
export type FlowGroup = "behaviour" | "llm_classified_refactor" | "llm_classified_tests";

/**
 * Identifier of a flow, assigned by the `plan` job: `f1`, `f2`, ….
 */
export type FlowId = string;

/**
 * Introduction of a flow (brief §3).
 */
export type FlowIntroduction = {
/**
 * Problem before this pull request: the local trigger and what was wrong or missing.
 */
problem_before: string,
/**
 * The intended correction this pull request makes.
 */
change: string,
/**
 * A short concrete example of behaviour before and after.
 */
example_before_after: string, };

/**
 * LLM content of a flow.
 */
export type FlowLlm = {
/**
 * Title.
 */
title: string,
/**
 * Classification.
 */
classification: Classification,
/**
 * Highest band of the step assessments; null when no step is assessed.
 */
risk: RiskBand | null,
/**
 * Flow introduction.
 */
introduction: FlowIntroduction,
/**
 * Flows this flow depends on.
 */
depends_on: Array<FlowId>,
/**
 * Patterns explained once, applied elsewhere.
 */
pattern_repetitions: Array<PatternRepetition>, };

/**
 * A flow named in a state message.
 */
export type FlowRef = {
/**
 * Flow id.
 */
id: FlowId,
/**
 * Flow title.
 */
title: string, };

/**
 * Kind of a step (fragment) of a flow (brief §3).
 */
export type FragmentKind = "modified" | "added" | "moved" | "renamed" | "deleted_site" | "context";

/**
 * GitHub item kinds of the list filter `kind`.
 */
export type GithubKind = "pr" | "issue";

/**
 * A GitHub review thread of the pull request.
 */
export type GithubThread = {
/**
 * GitHub node id.
 */
id: string,
/**
 * Repository-relative path.
 */
path: string,
/**
 * Last line; null for file-level threads.
 */
line: number | null,
/**
 * First line of a multi-line thread.
 */
start_line: number | null,
/**
 * Diff side (GitHub LEFT = base, RIGHT = head); null for file-level threads.
 */
side: LineSide | null,
/**
 * Author login of the first comment.
 */
author: string,
/**
 * Start of the first comment.
 */
excerpt: string,
/**
 * Number of comments.
 */
comment_count: number,
/**
 * Link to the thread.
 */
url: string,
/**
 * Resolved on GitHub.
 */
is_resolved: boolean,
/**
 * Outdated on GitHub.
 */
is_outdated: boolean,
/**
 * The step the thread is located in, if any.
 */
step_id: StepId | null,
/**
 * LLM status for threads located in a flow's steps; null otherwise.
 */
llm_status: ThreadLlmStatus | null,
/**
 * LLM reason for the status.
 */
llm_reason: string | null, };

/**
 * A graph edge.
 */
export type GraphEdge = {
/**
 * Source step.
 */
from: StepId,
/**
 * Target step.
 */
to: StepId,
/**
 * Branch label (decision outcome, loop back-edge, error exit).
 */
label: string | null, };

/**
 * A graph node, the diagram twin of one step.
 */
export type GraphNode = {
/**
 * The step.
 */
step_id: StepId,
/**
 * Step number.
 */
number: number,
/**
 * Node kind.
 */
kind: NodeKind,
/**
 * Label (the step title).
 */
label: string, };

/**
 * A dated event for the charts.
 */
export type HistoryEvent = {
/**
 * What happened.
 */
kind: HistoryKind,
/**
 * When.
 */
at: string,
/**
 * The pull request, `owner/name#number`; one event per pull request and kind.
 */
item_id: string, };

/**
 * Kind of a history event; pull requests and reviews only (OD16).
 */
export type HistoryKind = "pr_opened" | "pr_merged" | "pr_closed_unmerged" | "review_given";

/**
 * A moved block whose text is identical after whitespace normalisation. The UI says "Body text identical"
 * and nothing more: identical text does not mean unchanged behaviour, and no call-site flow is linked in v1
 * (ADR-008).
 */
export type IdenticalMove = {
/**
 * Original block at the base commit.
 */
from: FileRange,
/**
 * New block at the head commit.
 */
to: FileRange,
/**
 * Always `text_identical`.
 */
badge: ProvenanceBadge, };

/**
 * Code outside the diff that a step may affect.
 */
export type ImpactRef = {
/**
 * Repository-relative path at the head commit.
 */
file: string,
/**
 * Lines.
 */
range: LineRange,
/**
 * Enclosing symbol.
 */
symbol: string | null,
/**
 * Why it may be affected.
 */
reason: string,
/**
 * Fact or inference.
 */
basis: Basis,
/**
 * GitHub link.
 */
github_url: string, };

/**
 * Ranked items by kind and tier.
 */
export type ItemCounts = {
/**
 * All ranked items.
 */
total: number,
/**
 * Pull requests.
 */
pr: number,
/**
 * Issues.
 */
issue: number,
/**
 * MemCan TODOs.
 */
todo: number,
/**
 * Tier P0 (alarms).
 */
p0: number,
/**
 * Tier P1.
 */
p1: number,
/**
 * Tier P2.
 */
p2: number,
/**
 * Tier P3.
 */
p3: number,
/**
 * Items in the group "Not judged yet" (score state `unjudged`; also counted in their tier).
 */
not_judged: number, };

/**
 * Stable key of a work item, the normative form (`docs/contracts/http-api.md` "Item keys"):
 * `<owner>~<repo>~pr~<number>`, `<owner>~<repo>~issue~<number>` or `memcan~<uuid>`. Built and parsed only here;
 * deserialisation rejects any other form.
 */
export type ItemKey = string;

/**
 * Kind of a work item.
 */
export type ItemKind = "pr" | "issue" | "todo";

/**
 * Response of `GET /api/v1/dashboard/items`: ranked items of the latest snapshot, in rank order.
 */
export type ItemPage = {
/**
 * The summary's `revision` the page belongs to.
 */
revision: string,
/**
 * Items of this page.
 */
items: Array<WorkItem>,
/**
 * Cursor of the next page; null on the last.
 */
next_cursor: string | null, };

/**
 * Query of `GET /api/v1/dashboard/items`; every filter is optional.
 */
export type ItemQuery = {
/**
 * Only this tier.
 */
tier?: Tier,
/**
 * Only pull requests or only issues; absent = both. TODO items are listed whatever this filter says (while the
 * user shows MemCan TODOs, plan §G) unless `todos` is false.
 */
kind?: GithubKind,
/**
 * `false` leaves TODO items out; absent = true. A TODOs-only list is not supported in v1.1.
 */
todos?: boolean,
/**
 * Only this repository, `owner/name`.
 */
repo?: string,
/**
 * Only this score state.
 */
state?: ScoreState,
/**
 * Only items changed at or after this time (TODOs: their change time).
 */
updated_since?: string,
/**
 * Page size, [`ItemQuery::DEFAULT_LIMIT`] when absent, at most [`ItemQuery::MAX_LIMIT`].
 */
limit?: number,
/**
 * `next_cursor` of the previous page.
 */
cursor?: string, };

/**
 * Score of an item: the sum of its parts, in tenths of a point.
 */
export type ItemScore = {
/**
 * Sum of the parts' points, in tenths.
 */
total_tenths: number,
/**
 * Whether every part is final.
 */
state: ScoreState,
/**
 * Tier, computed by the server (P0 = alarm; otherwise P1 ≥ 65, P2 35–<65, P3 < 35 points).
 */
tier: Tier,
/**
 * The components, in display order.
 */
parts: Array<ScorePart>, };

/**
 * LLM-written text of an item (shown with the "LLM" tag).
 */
export type ItemText = {
/**
 * Why it ranks where it does.
 */
why: string,
/**
 * The next action.
 */
next: string,
/**
 * Reason of a P0 alarm; null when none.
 */
alarm: string | null,
/**
 * When it was judged.
 */
judged_at: string,
/**
 * Carried over from before the item changed.
 */
inherited: boolean,
/**
 * Keys of items the LLM merged into this one (chosen only from server-suggested pairs).
 */
also_keys: Array<ItemKey>, };

/**
 * Counts by item kind.
 */
export type KindCounts = {
/**
 * Pull requests.
 */
pr: number,
/**
 * Issues.
 */
issue: number,
/**
 * MemCan TODOs.
 */
todo: number, };

/**
 * A line ending other than LF.
 */
export type LineEnding = "crlf" | "no_newline";

/**
 * Lines `start_line..=end_line`, 1-based, on the side given by the enclosing type.
 */
export type LineRange = {
/**
 * First line, 1-based.
 */
start_line: number,
/**
 * Last line, inclusive.
 */
end_line: number, };

/**
 * Commit a line number refers to. Head is the common case (final code); base only for code that no longer
 * exists at the head commit (a deleted file, an emptied file, removed lines without a head line to anchor to).
 */
export type LineSide = "head" | "base";

/**
 * Lines of a file with the deterministic gutter facts for them.
 */
export type LinesResponse = {
/**
 * Repository-relative path.
 */
path: string,
/**
 * Side of the returned lines.
 */
side: LineSide,
/**
 * First returned line.
 */
from: number,
/**
 * Last returned line (clamped to the file).
 */
to: number,
/**
 * Lines in the file on that side (0 for an emptied file).
 */
total_lines: number,
/**
 * Text of lines `from..=to`.
 */
lines: Array<string>,
/**
 * Changed lines in the range (neutral gutter mark): added lines on the head side, removed lines on the
 * base side; Rust lines whose only change is whitespace are left out (they read as unchanged code).
 */
changed: Array<number>,
/**
 * Changed Rust comment and plain import lines a deterministic line rule covers: the main view may hide them,
 * the classic diff always shows them.
 */
context_only?: Array<number>,
/**
 * Code removed by the pull request at markers in the range (head side only).
 */
deleted: Array<DeletedBlock>,
/**
 * Moved blocks in the range (head side only).
 */
moved: Array<MovedLines>,
/**
 * Whether and how these lines can be commented on (deterministic, ADR-006).
 */
comments: CommentScope, };

/**
 * Outcome and time of an LLM call.
 */
export type LlmCallOutcome = {
/**
 * When the call ended.
 */
at: string,
/**
 * Whether it worked.
 */
status: LlmCallStatus, };

/**
 * Whether the LLM worked on its most recent call (brief §4.4).
 */
export type LlmCallStatus = { "state": "working" } | { "state": "not_working",
/**
 * Plain error message.
 */
message: string, };

/**
 * Language of LLM-written texts.
 */
export type LlmLanguage = "en" | "pl";

/**
 * One model available in this deployment.
 */
export type LlmModelOption = {
/**
 * Provider id.
 */
id: string,
/**
 * Plain display name.
 */
label: string, };

/**
 * Allowed models and the installation-wide choice for newly submitted jobs.
 */
export type LlmSelection = {
/**
 * Fixed server-supported model ids and labels.
 */
models: Array<LlmModelOption>,
/**
 * Allowed reasoning efforts.
 */
efforts: Array<ReasoningEffort>,
/**
 * Current choice, or the server default before the first save.
 */
current: ModelChoice,
/**
 * Reasoning effort of the dashboard ranking (`judge_work`; default low). Optional on the wire; absent on
 * older servers.
 */
judge_effort?: ReasoningEffort,
/**
 * Reasoning effort of Ask jobs (default medium); every other job uses `current`. Optional on the wire;
 * absent on older servers.
 */
ask_effort?: ReasoningEffort,
/**
 * Login of the last editor of the model, effort, ranking effort or Ask effort; null for the default.
 */
changed_by: string | null,
/**
 * Time of the last save; null for the default.
 */
changed_at: string | null, };

/**
 * Why sign-in failed; passed to Home as `/?login_error=<code>` by the OAuth callback.
 */
export type LoginError = "login_not_allowed" | "oauth_failed" | "state_mismatch";

/**
 * Body of `POST /auth/logout`.
 */
export type LogoutRequest = {
/**
 * End every session of the user, not only this one.
 */
everywhere: boolean, };

/**
 * A signed-in user.
 */
export type Me = {
/**
 * GitHub login.
 */
login: string,
/**
 * Display name.
 */
name: string | null,
/**
 * Avatar URL.
 */
avatar_url: string | null,
/**
 * Language of LLM texts.
 */
language: LlmLanguage, };

/**
 * Response of `GET /api/v1/me`; `user` is null when signed out.
 */
export type MeResponse = {
/**
 * The signed-in user.
 */
user: Me | null,
/**
 * Token for the `X-CSRF-Token` header of mutating requests; null when signed out.
 */
csrf_token: string | null, };

/**
 * What the server knows about MemCan connectivity.
 */
export type MemcanState = "not_configured" | "not_checked" | "reachable" | "not_reachable";

/**
 * Most recent observed MemCan connectivity; never an on-demand health probe.
 */
export type MemcanStatus = {
/**
 * Outcome observed by the server's client.
 */
state: MemcanState,
/**
 * Last successful tool call in this server process, retained after a later failure.
 */
last_success_at: string | null, };

/**
 * Model and reasoning effort (plan §5.2, ADR-002).
 */
export type ModelChoice = {
/**
 * Provider model id, e.g. `gpt-6.1-sol`.
 */
model: string,
/**
 * Reasoning effort.
 */
reasoning_effort: ReasoningEffort, };

/**
 * Result of the deterministic move check.
 */
export type MoveFact = {
/**
 * The original block at the base commit.
 */
from: FileRange,
/**
 * `text_identical` when no line differs after whitespace normalisation, else null.
 */
badge: ProvenanceBadge | null,
/**
 * Number of lines that differ from the original.
 */
differing_lines: number, };

/**
 * Original location of a moved block at the base commit.
 */
export type MovedFrom = {
/**
 * Repository-relative path at the base commit.
 */
file: string,
/**
 * First base line.
 */
base_start: number,
/**
 * Last base line.
 */
base_end: number, };

/**
 * A moved block in the range.
 */
export type MovedLines = {
/**
 * Lines of the moved block.
 */
range: LineRange,
/**
 * Lines that differ from the original (`≠`).
 */
differing: Array<number>, };

/**
 * Kind of a flow-graph node, derived from the step kind.
 */
export type NodeKind = "added" | "changed" | "unchanged";

/**
 * Section of material that is not analysed.
 */
export type NotAnalysedCategory = "tests" | "docs_comments" | "generated_mechanical";

/**
 * One file (or line ranges of it) recognised by a rule; one item per file and side.
 */
export type NotAnalysedItem = {
/**
 * Whether the server can show code for this side and file; absent on older Tours.
 */
renderable?: boolean,
/**
 * Detector that actually matched; absent on older stored pre-passes.
 */
rule_kind?: RuleKind,
/**
 * Repository-relative path on that side.
 */
file: string,
/**
 * `base` for a deleted file or removed lines, else `head`.
 */
side: LineSide,
/**
 * Matched changed lines on that side; null for the whole file.
 */
ranges: Array<LineRange> | null,
/**
 * How it was recognised.
 */
badge: ProvenanceBadge,
/**
 * The matching rule or detector description.
 */
rule: string, };

/**
 * A built-in "not analysed" rule.
 */
export type NotAnalysedRule = {
/**
 * Detector used to classify the material.
 */
kind: RuleKind,
/**
 * The pattern or detector description.
 */
pattern: string,
/**
 * Section it fills.
 */
category: NotAnalysedCategory, };

/**
 * A Not analysed section.
 */
export type NotAnalysedSection = {
/**
 * Section.
 */
category: NotAnalysedCategory,
/**
 * Number of files in the section.
 */
file_count: number,
/**
 * Items.
 */
items: Array<NotAnalysedItem>, };

/**
 * Whether an issue was introduced by the pull request.
 */
export type Origin = "introduced" | "pre_existing" | "unclear";

/**
 * Who decides a part.
 */
export type PartSource = "computed" | "llm" | "not_applicable";

/**
 * How a part's value was reached.
 */
export type PartStatus = "computed" | "judged" | "overridden" | "inherited" | "estimated" | "pending";

/**
 * A pattern explained once and applied in several places ("same pattern × N").
 */
export type PatternRepetition = {
/**
 * The pattern.
 */
description: string,
/**
 * The step that explains it.
 */
explained_in: StepId,
/**
 * The other places.
 */
sites: Array<PatternSite>, };

/**
 * One place where a repeated pattern is applied.
 */
export type PatternSite = {
/**
 * Repository-relative path on that side.
 */
file: string,
/**
 * Commit the lines refer to.
 */
side: LineSide,
/**
 * Lines.
 */
range: LineRange,
/**
 * The step containing these lines, if any.
 */
step_id: StepId | null,
/**
 * GitHub link.
 */
github_url: string, };

/**
 * Why an analysis waits; absent reasons in older responses mean a usage limit.
 */
export type PauseReason = "user" | "usage_limit";

/**
 * A comment in the pending review.
 */
export type PendingComment = {
/**
 * GitHub comment node id.
 */
id: string,
/**
 * GitHub thread node id.
 */
thread_id: string,
/**
 * Repository-relative path.
 */
path: string,
/**
 * Line or file-level comment.
 */
kind: CommentKind,
/**
 * Diff side of a line comment (GitHub LEFT = base, RIGHT = head).
 */
side: LineSide | null,
/**
 * First line of a multi-line comment; for a file-level comment, of the linked range.
 */
start_line: number | null,
/**
 * Last line; for a file-level comment, of the linked range.
 */
line: number | null,
/**
 * Comment text.
 */
body: string, };

/**
 * The user's pending review on GitHub, merged over every pending review of the user.
 */
export type PendingReview = {
/**
 * False when the GitHub App is not installed on the repository.
 */
commenting_available: boolean,
/**
 * Pending comments; the count is their number.
 */
comments: Array<PendingComment>,
/**
 * The server stopped reading at its cap (more pending comments or reviews exist on GitHub than listed).
 */
incomplete: boolean, };

/**
 * Position within the fixed plan batches of this run.
 */
export type PlanPart = {
/**
 * One-based current part.
 */
index: number,
/**
 * Total parts, fixed before the first plan job is submitted.
 */
total: number, };

/**
 * Information from the accepted plan, before step analysis.
 */
export type PlannedFlowDetails = {
/**
 * LLM intent from planning.
 */
intent: string,
/**
 * Unique reading rank from planning.
 */
importance_rank: number,
/**
 * Changed ranges assigned by the verified plan.
 */
ranges: Array<FileRanges>, };

/**
 * An ownership policy file (JSON format of `stale_prs_are_bad`).
 */
export type PolicyRef = {
/**
 * Repository holding the file.
 */
repo: RepoRef,
/**
 * Relative path ending `.json`.
 */
path: string, };

/**
 * The PR context: deterministic facts and LLM content.
 */
export type PrContextBlock = {
/**
 * Deterministic facts.
 */
facts: PrContextFacts,
/**
 * LLM analysis.
 */
llm: PrContextOutput, };

/**
 * Deterministic facts of the PR context.
 */
export type PrContextFacts = {
/**
 * Structured GitHub targets resolved independently of the agent's prose; missing on older Tours.
 */
related_work_links?: Array<RelatedWorkLink>,
/**
 * Current observed MemCan connection state, separate from the analysis's profile lookup.
 */
memcan_status?: MemcanStatus,
/**
 * CI status of the head commit.
 */
ci: CiStatus,
/**
 * Whether `Tour.threads` holds every review thread of the pull request.
 */
threads_status: ThreadsStatus,
/**
 * Counters from the pre-pass.
 */
counters: Counters,
/**
 * Where the project profile came from.
 */
project_profile: ProjectProfileStatus,
/**
 * Paths in the pull request diff that look like agent instruction files (AGENTS.md, CLAUDE.md, `.agents/`,
 * `.codex/`, `.claude/`, `.cursor*`, `.github/copilot-instructions.md`); shown as a notice, empty = none.
 */
agent_instruction_files: Array<string>,
/**
 * False when related work could not be looked up (the GitHub App is not installed on the repository);
 * the related-work list then reads "not checked".
 */
related_work_checked: boolean,
/**
 * Number of flows.
 */
flow_count: number,
/**
 * Number of steps over all flows.
 */
step_count: number, };

/**
 * The pull request at project scale.
 */
export type PrContextOutput = {
/**
 * The problem the pull request solves.
 */
problem: string,
/**
 * Why the change is needed.
 */
why_needed: string,
/**
 * What the change gives users or developers.
 */
what_it_gives: string,
/**
 * Related pull requests, issues or mechanisms; may be empty.
 */
related_work: Array<RelatedWork>,
/**
 * Questions about the fit of the change, never verdicts.
 */
fit_questions: Array<FitQuestion>, };

/**
 * A pull request of a repository.
 */
export type PrRef = {
/**
 * The repository.
 */
repo: RepoRef,
/**
 * The pull request number.
 */
number: number, };

/**
 * Size of a pull request as GitHub reports it.
 */
export type PrSize = {
/**
 * Additions plus deletions.
 */
changed_lines: number,
/**
 * Changed files.
 */
changed_files: number, };

/**
 * A pull request as GitHub reports it.
 */
export type PrSummary = {
/**
 * Repository.
 */
repo: RepoRef,
/**
 * Number.
 */
number: number,
/**
 * Title.
 */
title: string,
/**
 * Author login.
 */
author: string,
/**
 * Pull request page on GitHub.
 */
html_url: string,
/**
 * Base commit.
 */
base_sha: CommitSha,
/**
 * Head commit.
 */
head_sha: CommitSha, };

/**
 * A source of the project profile.
 */
export type ProfileSource = "repository_file" | "memcan";

/**
 * Sources of the project profile; empty with MemCan `checked` means no profile (ratings are component-level).
 */
export type ProjectProfileStatus = {
/**
 * Where the profile came from.
 */
sources: Array<ProfileSource>,
/**
 * Whether MemCan was searched for a `type: project-profile` memory; `not_checked` means a MemCan profile
 * may exist that `sources` does not list.
 */
memcan: SourceCheck, };

/**
 * A deterministic check that a badge names (brief §1, §3). No badge means LLM analysis.
 */
export type ProvenanceBadge = "text_identical" | "classified_by_path" | "classified_by_line_rule" | "binary_content";

/**
 * Identifier of an Ask question.
 */
export type QuestionId = string;

/**
 * A quiet row of the menu.
 */
export type QuietRowRef = { "row": "unexplained" } | { "row": "moves" } | { "row": "not_analysed",
/**
 * The section.
 */
category: NotAnalysedCategory, };

/**
 * Quiet rows: never ranked above a flow.
 */
export type QuietRows = {
/**
 * Changed lines no flow explains ("Unexplained — read these yourself"); listed first. Removed lines
 * without a head location are base-side entries.
 */
unexplained: Array<UnexplainedRanges>,
/**
 * Moves whose text is deterministically identical.
 */
moves: Array<IdenticalMove>,
/**
 * Material recognised by path or line rules.
 */
not_analysed: Array<NotAnalysedSection>, };

/**
 * Query of the `lines` and `diff` endpoints: `path`, lines `from..=to` on `side`.
 */
export type RangeQuery = {
/**
 * Repository-relative path on that side.
 */
path: string,
/**
 * `head` for final code; `base` for removed code of a deleted or emptied file.
 */
side: LineSide,
/**
 * First line, 1-based.
 */
from: number,
/**
 * Last line, inclusive.
 */
to: number, };

/**
 * State of the LLM ranking (plan §B.3); the last four end it and leave the refresh `finished`.
 */
export type RankingState = "waiting_for_analysis" | "running" | "paused" | "failed" | "interrupted" | "cancelled" | "finished";

/**
 * The LLM ranking of a stored snapshot.
 */
export type RankingStatus = {
/**
 * State.
 */
state: RankingState,
/**
 * Items judged so far.
 */
judged: number,
/**
 * Items queued for judgement in this refresh.
 */
total: number,
/**
 * When a usage-limit pause ends; null unless `paused`.
 */
resumes_at: string | null,
/**
 * Plain text of a failure; null unless `failed` or `interrupted`.
 */
message: string | null, };

/**
 * Reasoning effort of the model.
 */
export type ReasoningEffort = "low" | "medium" | "high";

/**
 * Why a refresh failed.
 */
export type RefreshFailure = {
/**
 * Cause.
 */
code: RefreshFailureCode,
/**
 * Plain message for the user.
 */
message: string,
/**
 * Earliest time a new refresh can succeed (GitHub rate-limit reset); null when not limited.
 */
retry_at: string | null, };

/**
 * Cause of a failed refresh.
 */
export type RefreshFailureCode = "github_rate_limited" | "github_unavailable" | "sign_in_required" | "timed_out" | "interrupted" | "internal";

/**
 * Identifier of a dashboard refresh (one scan and its ranking).
 */
export type RefreshId = string;

/**
 * State of a refresh; transitions in `docs/contracts/http-api.md` "Refresh states". At most one refresh per user
 * is `scanning` or `ranking`; only `ranking` and `finished` refreshes hold a snapshot.
 */
export type RefreshState = "scanning" | "ranking" | "finished" | "failed" | "cancelled";

/**
 * One refresh: scan progress, ranking and outcome (`GET /api/v1/dashboard/refreshes/{id}`, SSE `refresh`).
 */
export type RefreshStatus = {
/**
 * Id.
 */
id: RefreshId,
/**
 * What started it.
 */
trigger: RefreshTrigger,
/**
 * State.
 */
state: RefreshState,
/**
 * Items the LLM judged in this refresh.
 */
judged: number,
/**
 * Items whose stored judgement was reused at no cost (judged inputs unchanged).
 */
reused: number,
/**
 * Items left provisional by the per-refresh or daily ranking limit.
 */
left_provisional: number,
/**
 * Scan steps in order.
 */
steps: Array<ScanStep>,
/**
 * Scan progress, 0–100.
 */
percent: number,
/**
 * Start time.
 */
started_at: string,
/**
 * When the scan finished and the snapshot was stored; null while scanning or when it failed.
 */
scanned_at: string | null,
/**
 * The LLM ranking of the snapshot; null before the scan finished.
 */
ranking: RankingStatus | null,
/**
 * Why the scan failed; null unless `failed`.
 */
failure: RefreshFailure | null,
/**
 * Plain log lines of the scan (at most 200, oldest first); no tokens, no query strings.
 */
log: Array<string>, };

/**
 * What started a refresh.
 */
export type RefreshTrigger = "manual" | "scheduled";

/**
 * One piece of related work.
 */
export type RelatedWork = {
/**
 * URL or `owner/repo#number` of the related item.
 */
reference: string,
/**
 * Kind of relation.
 */
kind: RelatedWorkKind,
/**
 * One line on the overlap with this pull request.
 */
overlap: string, };

/**
 * Kind of related work in the PR context.
 */
export type RelatedWorkKind = "duplicate" | "competing_solution" | "stacked_pr" | "linked_issue" | "companion_repository";

/**
 * A validated target syntax and the result of checking it against GitHub.
 */
export type RelatedWorkLink = {
/**
 * Exact reference in the agent output, used only to associate the item.
 */
reference: string,
/**
 * Repository in owner/name form, validated by the server.
 */
repo: string,
/**
 * Positive GitHub issue or pull request number.
 */
number: number,
/**
 * Resource kind, from GitHub when verified, otherwise inferred from the reference.
 */
kind: RelatedWorkTargetKind,
/**
 * GitHub confirmed the resource exists and its kind; false if it could not be read.
 */
verified: boolean, };

/**
 * GitHub resource linked from related work (distinct from the LLM's relationship kind).
 */
export type RelatedWorkTargetKind = "pull_request" | "issue";

/**
 * A GitHub repository.
 */
export type RepoRef = {
/**
 * Owner (user or organisation) login.
 */
owner: string,
/**
 * Repository name.
 */
name: string, };

/**
 * Response of `GET /api/v1/repos/{owner}/{name}/settings` (read-only).
 */
export type RepoSettings = {
/**
 * Repository.
 */
repo: RepoRef,
/**
 * Built-in "not analysed" rules.
 */
rules: Array<NotAnalysedRule>,
/**
 * Related repositories, auto-detected from Cargo files on the base ref.
 */
related_repos: Array<RepoRef>,
/**
 * Project profile source.
 */
project_profile: ProjectProfileStatus, };

/**
 * Settings of one repository row.
 */
export type RepositoryRule = {
/**
 * The repository.
 */
repo: RepoRef,
/**
 * Display alias prefix (`[A-Z0-9][A-Z0-9-]{0,11}`); null = the bare repository name.
 */
alias: string | null,
/**
 * Include its open issues.
 */
watch_issues: boolean,
/**
 * File naming the owned paths; null = no ownership policy.
 */
ownership_policy: PolicyRef | null,
/**
 * Release and long-lived branch patterns; empty = the built-ins.
 */
branch_patterns: Array<string>, };

/**
 * Body of `POST /api/v1/pr/resolve`.
 */
export type ResolvePrRequest = {
/**
 * GitHub pull request URL.
 */
url: string, };

/**
 * Response of `POST /api/v1/pr/resolve`.
 */
export type ResolvePrResponse = {
/**
 * Non-blocking notice for a large PR, using this same GitHub metadata response.
 */
size_warning: SizeWarning | null,
/**
 * The pull request's size from this same GitHub metadata response, whatever its size; absent when GitHub
 * did not report it.
 */
size?: PrSize,
/**
 * Rough estimate of what starting the analysis runs: plan and PR context (flows are analysed on request).
 * From recent local runs; absent without the size or enough history.
 */
planning_estimate?: AnalysisEstimate,
/**
 * The pull request now.
 */
pr: PrSummary,
/**
 * False when the GitHub App is not installed on the repository: analysis works, comments do not.
 */
commenting_available: boolean,
/**
 * The latest analysis of this pull request, if any: a not cancelled one first, then the one in the user's
 * language, else in another language (its `language` says which).
 */
analysis: AnalysisSummary | null,
/**
 * The analysis is for an older head than the pull request now has.
 */
head_moved: boolean, };

/**
 * The user's smart-review analysis of a pull request item.
 */
export type ReviewLink = {
/**
 * The newest analysis of the pull request.
 */
analysis_id: AnalysisId,
/**
 * Head commit it analysed.
 */
analysed_head: CommitSha,
/**
 * The pull request head moved since.
 */
head_moved: boolean,
/**
 * The user has saved review state on it.
 */
has_my_state: boolean, };

/**
 * Review state of one user for one analysis; a flow is reviewed when all its steps are ticked.
 */
export type ReviewState = {
/**
 * Ticked steps.
 */
ticked_steps: Array<StepId>,
/**
 * Last viewed block; null before the first visit (starts at the PR context).
 */
last_viewed: BlockRef | null, };

/**
 * Risk at system scale, rated by the LLM (no numbers, brief §1).
 */
export type RiskBand = "low" | "medium" | "high";

/**
 * Kind of rule.
 */
export type RuleKind = "path_glob" | "line_regex" | "lexer" | "header" | "binary";

/**
 * One stage in the short whole-run list.
 */
export type RunStage = {
/**
 * Accepted units, not attempts (repairs do not count twice).
 */
done: number,
/**
 * Unit count; null until known.
 */
total: number | null,
/**
 * Position relative to the currently executing stage.
 */
state: RunStageState, };

/**
 * State of a whole-run stage; pause/failure remain on AnalysisState.
 */
export type RunStageState = "pending" | "current" | "done";

/**
 * Something the scan missed or approximated (plain text).
 */
export type ScanNote = {
/**
 * The note.
 */
text: string,
/**
 * The item it concerns, if one.
 */
item: ItemKey | null, };

/**
 * A step of the scan, shown as a list with progress.
 */
export type ScanStep = {
/**
 * Which step.
 */
key: ScanStepKey,
/**
 * Its state.
 */
state: StepState,
/**
 * Units done (e.g. PR details read); null when the step has no count.
 */
done: number | null,
/**
 * Units in total; null when not known yet or not counted.
 */
total: number | null,
/**
 * Plain detail, e.g. the item being read or why the step failed.
 */
detail: string | null, };

/**
 * The scan steps, in order (plan §A.4).
 */
export type ScanStepKey = "search_prs" | "issues" | "pr_details" | "context" | "history" | "todos" | "ranking";

/**
 * The daily automatic refresh (ADR-020).
 */
export type ScheduleStatus = {
/**
 * On or off.
 */
enabled: boolean,
/**
 * Local time of day, `HH:MM`.
 */
time: string,
/**
 * IANA time zone; null until known (the schedule does not run without it).
 */
time_zone: string | null,
/**
 * Next scheduled instant; null when off or without a time zone.
 */
next_run_at: string | null,
/**
 * The most recent scheduled run; null before the first.
 */
last_run: ScheduledRun | null, };

/**
 * Outcome of a scheduled run.
 */
export type ScheduledOutcome = "started" | "skipped" | "needs_sign_in" | "failed";

/**
 * One scheduled run.
 */
export type ScheduledRun = {
/**
 * Its scheduled instant (the configured local time on that local date).
 */
at: string,
/**
 * What happened.
 */
outcome: ScheduledOutcome,
/**
 * Plain reason for `skipped`, `needs_sign_in` and `failed`.
 */
reason: string | null, };

/**
 * The twelve score components (daily-priorities `scoreParts`).
 */
export type ScoreComponent = "advancement" | "defect_severity" | "unblocks" | "human_waiting" | "recency" | "change_impact" | "completion" | "change_size" | "urgency_bonus" | "blocked_penalty" | "default_branch_bonus" | "milestone_bonus";

/**
 * One score component.
 */
export type ScorePart = {
/**
 * Which component.
 */
component: ScoreComponent,
/**
 * Points in tenths; null while pending.
 */
points_tenths: number | null,
/**
 * Who decides it.
 */
source: PartSource,
/**
 * How the value was reached.
 */
status: PartStatus,
/**
 * Plain basis of the value (an estimate names its rule).
 */
basis: string, };

/**
 * Whether a score is final.
 */
export type ScoreState = "final" | "provisional" | "unjudged";

/**
 * Body of `PUT /api/v1/settings/llm/ask-effort`.
 */
export type SetAskEffortRequest = {
/**
 * Reasoning effort of Ask jobs.
 */
reasoning_effort: ReasoningEffort, };

/**
 * Body of `PUT /api/v1/settings/llm/judge-effort`.
 */
export type SetJudgeEffortRequest = {
/**
 * Reasoning effort of the dashboard ranking.
 */
reasoning_effort: ReasoningEffort, };

/**
 * Body of `PUT /api/v1/settings/language`.
 */
export type SetLanguageRequest = {
/**
 * Language of LLM texts.
 */
language: LlmLanguage, };

/**
 * Body of `PUT /api/v1/settings/dashboard/time-zone`.
 */
export type SetTimeZoneRequest = {
/**
 * IANA time zone name.
 */
time_zone: string, };

/**
 * Response of `GET /api/v1/settings`.
 */
export type Settings = {
/**
 * Installation-wide model selection; absent on older servers.
 */
llm_selection?: LlmSelection,
/**
 * Tenant usage in the server's local calendar; absent on older responses.
 */
llm_usage?: UsageWindows,
/**
 * Outcome of the most recent LLM call; null before the first call. No on-demand check.
 */
llm_status: LlmCallOutcome | null,
/**
 * Observed MemCan connection state, without an on-demand probe.
 */
memcan_status?: MemcanStatus,
/**
 * Language of LLM texts.
 */
language: LlmLanguage,
/**
 * GitHub App installations the user can access; null = not checked (GitHub could not be reached or the
 * user's GitHub sign-in is not usable), never a stand-in for "none installed".
 */
app_installations: Array<AppInstallation> | null,
/**
 * The tenant's known repositories (any pull request opened); each one's settings can be opened.
 */
repos: Array<RepoRef>, };

/**
 * Large-PR facts and a rough estimate, never a prerequisite for starting.
 */
export type SizeWarning = {
/**
 * Additions plus deletions reported by GitHub.
 */
changed_lines: number,
/**
 * Changed files reported by GitHub.
 */
changed_files: number,
/**
 * Rough estimate of plan and PR context from recent local runs (the same as
 * `ResolvePrResponse.planning_estimate`); null without enough history.
 */
estimate: AnalysisEstimate | null, };

/**
 * Facts of the stored snapshot besides its items.
 */
export type SnapshotMeta = {
/**
 * The refresh that produced it.
 */
refresh_id: RefreshId,
/**
 * When its scan finished.
 */
scanned_at: string,
/**
 * When its ranking finished; null while ranking or when it did not finish.
 */
ranked_at: string | null,
/**
 * Ranked items by kind and tier.
 */
item_counts: ItemCounts,
/**
 * Items left out of the ranking (drafts of others, skipped bot authors, no owned change).
 */
excluded: number,
/**
 * The user's pull requests waiting on other people.
 */
awaiting: Array<AwaitingItem>,
/**
 * Dated pull request and review events of the chart window (12 weeks), across the scanned logins.
 */
history: Array<HistoryEvent>,
/**
 * State of the MemCan TODOs.
 */
todos: TodosState,
/**
 * What the scan missed or approximated.
 */
notes: Array<ScanNote>,
/**
 * Configured repositories and owners the user's GitHub token cannot see (`owner/name` or `owner`).
 */
unreachable: Array<string>,
/**
 * What the scan read before exclusion.
 */
checked: KindCounts, };

/**
 * Whether a source was checked.
 */
export type SourceCheck = "checked" | "not_checked";

/**
 * The flow of a `flow` stage.
 */
export type StageFlow = {
/**
 * 1-based position of the flow in the stored plan (the order flows are analysed in).
 */
index: number,
/**
 * Flow title.
 */
title: string, };

/**
 * Kind of an analysis stage.
 */
export type StageKind = "plan" | "pr_context" | "flow";

/**
 * One step of a flow.
 */
export type Step = {
/**
 * Step id.
 */
id: StepId,
/**
 * 1-based number within the flow.
 */
number: number,
/**
 * Deterministic facts.
 */
facts: StepFacts,
/**
 * LLM analysis.
 */
llm: StepLlm, };

/**
 * Deterministic facts of a step.
 */
export type StepFacts = {
/**
 * Repository-relative path on the step's side.
 */
file: string,
/**
 * `head` normally; `base` for removed code without a head location (deleted or emptied file).
 */
side: LineSide,
/**
 * Step lines on that side.
 */
range: LineRange,
/**
 * GitHub link to the lines at the head commit (base commit for a base-side step).
 */
github_url: string,
/**
 * Changed lines inside the step: added lines of a head step, removed lines of a base step.
 */
changed_lines: Array<LineRange>,
/**
 * Move check result, for steps whose code was moved.
 */
moved: MoveFact | null,
/**
 * Callers and callees from the tech plugin; null in v1 ("n/a — needs a language plugin").
 */
call_hierarchy: CallHierarchy | null,
/**
 * Ids of the GitHub threads located in this step.
 */
thread_ids: Array<string>, };

/**
 * Identifier of a step, assigned by the server in step order: `"<flow>.<n>"`, e.g. `f2.3`.
 */
export type StepId = string;

/**
 * LLM content of a step.
 */
export type StepLlm = {
/**
 * Title.
 */
title: string,
/**
 * Step kind.
 */
kind: FragmentKind,
/**
 * Enclosing symbol.
 */
symbol: string | null,
/**
 * Who reaches this step.
 */
called_by: string,
/**
 * What it decides or changes.
 */
decides: string,
/**
 * Where execution goes next.
 */
next: string,
/**
 * Why it is needed.
 */
why: string,
/**
 * Annotations.
 */
annotations: Array<Annotation>,
/**
 * Assessment, collapsed by default; null when the LLM gave none.
 */
assessment: Assessment | null,
/**
 * Original location the LLM claims for a moved block.
 */
moved_from: MovedFrom | null,
/**
 * Code outside the diff this step may affect, shown inline as real code.
 */
impact: Array<ImpactRef>, };

/**
 * State of a scan step.
 */
export type StepState = "waiting" | "running" | "done" | "failed" | "skipped";

/**
 * LLM status of an existing GitHub review thread.
 */
export type ThreadLlmStatus = "still_valid" | "stale" | "already_addressed";

/**
 * How completely the pull request's review threads were read.
 */
export type ThreadsStatus = "checked" | "partial" | "not_checked";

/**
 * Priority tier.
 */
export type Tier = "p0" | "p1" | "p2" | "p3";

/**
 * State of the MemCan TODOs in the snapshot.
 */
export type TodosState = {
/**
 * State.
 */
status: TodosStatus,
/**
 * When the TODO rows shown were read; null when none were read.
 */
read_at: string | null,
/**
 * When MemCan could not be reached; null unless `not_refreshed`.
 */
failed_at: string | null, };

/**
 * Whether the TODO rows are current.
 */
export type TodosStatus = "read" | "not_refreshed" | "off" | "not_configured";

/**
 * What a token may do; a list on each token. Open enum: clients treat unknown values as opaque.
 */
export type TokenScope = "read";

/**
 * Response of `GET /api/v1/analyses/{id}/tour`.
 */
export type Tour = {
/**
 * The analysis.
 */
analysis: AnalysisSummary,
/**
 * The PR context block.
 */
pr_context: PrContextBlock,
/**
 * Flows in tour order: behaviour by importance, LLM-classified refactors, LLM-classified tests.
 */
flows: Array<Flow>,
/**
 * Quiet rows, always after the flows.
 */
quiet: QuietRows,
/**
 * Every GitHub review thread of the pull request.
 */
threads: Array<GithubThread>, };

/**
 * Unexplained lines of one file on one side, with why the server left them out when it decided so.
 */
export type UnexplainedRanges = {
/**
 * Repository-relative path on that side.
 */
file: string,
/**
 * Commit the lines refer to.
 */
side: LineSide,
/**
 * The lines, ascending and disjoint.
 */
ranges: Array<LineRange>,
/**
 * Why the server did not send these lines to the LLM (deterministic); absent when the LLM did not
 * explain them.
 */
reason?: UnexplainedReason, };

/**
 * Why the server left Unexplained lines out of every LLM request.
 */
export type UnexplainedReason = "too_large_for_prompt";

/**
 * Body of `PATCH /api/v1/analyses/{id}/pending-review/comments/{comment_id}`.
 */
export type UpdateCommentRequest = {
/**
 * New text.
 */
body: string, };

/**
 * Token sums; cached input is included in input, not additional to it.
 */
export type UsageTotals = {
/**
 * Input tokens, including cached input.
 */
input_tokens: number,
/**
 * Cached input tokens.
 */
cached_input_tokens: number,
/**
 * Output tokens.
 */
output_tokens: number,
/**
 * Unknown: the retained job-result contract does not store reasoning tokens separately.
 */
reasoning_output_tokens: number | null,
/**
 * Finished jobs with a provider usage report, including Ask and all analysis rounds.
 */
jobs: number, };

/**
 * Usage windows in the server's local calendar, scoped to the current tenant.
 */
export type UsageWindows = {
/**
 * From local midnight through now; null without reports.
 */
today: UsageTotals | null,
/**
 * Today and the previous six local calendar days; null without reports.
 */
last_7_days: UsageTotals | null, };

/**
 * Feedback values a reviewer can choose.
 */
export type UserFeedbackValue = "correct" | "wrong" | "irrelevant" | "overrated" | "underrated";

/**
 * One ranked item.
 */
export type WorkItem = {
/**
 * Display alias, e.g. `PLAT-1187` or `TODO-3fa91c2e`.
 */
id: string,
/**
 * Stable, unique, URL-safe key.
 */
key: ItemKey,
/**
 * Kind.
 */
kind: ItemKind,
/**
 * Repository; null for TODOs (their MemCan project is a tag).
 */
repo: RepoRef | null,
/**
 * Pull request or issue number; null for TODOs.
 */
number: number | null,
/**
 * Title.
 */
title: string,
/**
 * Link built by the server (GitHub, or the MemCan UI); null for a TODO without a configured MemCan UI.
 */
url: string | null,
/**
 * Author login; null for TODOs.
 */
author: string | null,
/**
 * From a private repository (shown only to this user).
 */
private: boolean,
/**
 * Pull requests and issues: GitHub `updatedAt`. TODOs: when the dashboard first saw the current values of the
 * fields of plan §G (MemCan has no `updated_at`).
 */
updated_at: string,
/**
 * Deterministic tags from scan facts.
 */
tags: Array<FactTag>,
/**
 * Deterministic facts from the scan.
 */
facts: Array<Fact>,
/**
 * Score with its parts.
 */
score: ItemScore,
/**
 * LLM-written text; null before the first judgement.
 */
llm: ItemText | null,
/**
 * The user's smart-review analysis of this pull request, read at request time; null when none.
 */
review: ReviewLink | null, };
