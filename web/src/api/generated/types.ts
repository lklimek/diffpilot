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
 * Event of an agent push message.
 */
export type AgentPushEvent = "decision_available";

/**
 * JSON body of an agent push-target webhook delivery (ntfy: the same fields as the message): ids and a fetch
 * link only, never reply text, reasons, comment bodies or secrets (T28).
 */
export type AgentPushPayload = {
/**
 * Payload version, 1.
 */
v: number,
/**
 * Event.
 */
event: AgentPushEvent,
/**
 * The decision.
 */
decision_id: DecisionId,
/**
 * Pull request, `owner/name#number`.
 */
pr: string,
/**
 * API path to fetch the decision (RF1b).
 */
fetch: string, };

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
 * The stored status of an analysis (the `state` of [`AnalysisState`] without its details).
 */
export type AnalysisStatus = "planning" | "running" | "paused" | "failed" | "finished" | "cancelled";

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
 * Body of `POST /api/v1/analyses/{id}/questions`: a question with what it is about.
 */
export type AnchoredAskRequest = {
/**
 * The question, at most `AskRequest::MAX_TEXT_CHARS` characters.
 */
text: string,
/**
 * What the question is about; a follow-up repeats its thread's anchor.
 */
anchor: AskAnchor,
/**
 * A follow-up: a question of the thread it continues (the thread's first question is stored); absent for a new
 * thread.
 */
parent_id?: QuestionId, };

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
export type ApiErrorCode = "configuration" | "unauthenticated" | "login_not_allowed" | "csrf_rejected" | "not_found" | "invalid_request" | "not_a_pr_url" | "private_repository" | "app_not_installed" | "analysis_not_finished" | "conflict" | "pr_head_changed" | "position_rejected" | "comment_published" | "range_too_large" | "not_rendered" | "not_implemented" | "github_unavailable" | "internal" | "unavailable" | "refresh_running" | "github_rate_limited" | "invalid_dashboard_settings" | "cursor_expired" | "insufficient_scope" | "token_not_accepted" | "token_limit_reached" | "rate_limited" | "github_refused" | "item_changed" | "claimed" | "lease_expired" | "capability_missing" | "watch_limit_reached" | "scheme_not_allowed" | "secret_required";

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
 * What a question is about.
 */
export type AskAnchor = { "kind": "step",
/**
 * The step.
 */
step_id: StepId,
/**
 * Lines inside the step, on its side and file; null for the whole step.
 */
lines: LineRange | null, } | { "kind": "lines",
/**
 * Repository-relative path on `side`.
 */
path: string,
/**
 * Commit the lines refer to.
 */
side: LineSide,
/**
 * The lines, at most `AskAnchor::MAX_LINES`; null for the whole file.
 */
range: LineRange | null, } | { "kind": "flow",
/**
 * The flow.
 */
flow_id: FlowId, } | { "kind": "pr" } | { "kind": "comment_item",
/**
 * The item.
 */
item: CommentItemId, };

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
 * First-Ask preflight bound to the working thread's seed and analytic generation.
 */
export type AskPreflight = {
/**
 * Working-thread seed.
 */
seed: SessionSeed,
/**
 * Analytic generation at seeding.
 */
generation: number,
/**
 * Confirmed preflight state.
 */
state: PreflightState, };

/**
 * A private question and its LLM answer, pinned to its anchor; returned only to its asker.
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
 * What the question is about.
 */
anchor: AskAnchor,
/**
 * The step of a step anchor; absent for other anchors.
 */
step_id?: StepId,
/**
 * A follow-up: the first question of its thread; absent for a first question.
 */
parent_id?: QuestionId,
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
 * When the answer became final; null for an open question, a ready output or a failure.
 */
answered_at?: string | null,
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
 * Pre-admission rejection consumes no slot.
 */
export type AssessmentAdmissionFailure = { "code": "assessment_daily_limit",
/**
 * Next UTC midnight.
 */
resets_at: string, } | { "code": "credential_missing" };

/**
 * GET …/comments/assessment-comparison, scoped to payer and PR.
 */
export type AssessmentComparison = {
/**
 * Exact comparison counts with explicit denominators.
 */
summary: ComparisonSummary,
/**
 * Frozen logical pass.
 */
pass: AssessmentPass,
/**
 * Owner revisions are selected as of this cutoff.
 */
cutoff: string,
/**
 * Independent method state and metering.
 */
runs: Array<AssessmentMethodRun>,
/**
 * Includes unmatched, missing, duplicate and failed rows explicitly.
 */
items: Array<AssessmentComparisonItem>, };

/**
 * Common method-local comparison row; secondary IDs never allocate primary items.
 */
export type AssessmentComparisonItem = {
/**
 * Model-reported reply-chain evidence, separate from actual diffpilot owner truth.
 */
recorded_decision?: RecordedDecision,
/**
 * Stable comparison identity inside this pass.
 */
comparison_key: string,
/**
 * Adapter-local ID, distinct from a primary item ID.
 */
local_id: string,
/**
 * Adapter and pass.
 */
run: AssessmentRunRef,
/**
 * Adapter version.
 */
method_version: string,
/**
 * Frozen source identity.
 */
source: AssessmentSource,
/**
 * Existing primary item, only for an unambiguous alignment.
 */
primary_item_id: CommentItemId | null,
/**
 * Frozen normalized primary point hash, distinct from the full source version.
 */
primary_item_hash: string | null,
/**
 * Alignment/completeness state.
 */
status: ComparisonItemStatus,
/**
 * Canonical comparison key for a duplicate.
 */
duplicate_of: string | null,
/**
 * Normalized verdict, absent when unavailable.
 */
verdict: AssessmentVerdict | null,
/**
 * Native verdict preserved exactly.
 */
native_verdict: string | null,
/**
 * Machine proposal; absence is distinct from a missing row or verdict.
 */
proposal: ProposedDecision | null,
/**
 * Actual diffpilot owner decision reference as of the report cutoff.
 */
owner_decision: AssessmentOwnerDecision | null, };

/**
 * Per-user settings; changing them alone never schedules work.
 */
export type AssessmentConfig = {
/**
 * Only this method can change visible items or effects.
 */
primary_method: AssessmentMethod,
/**
 * Explicit owner/repo#number selections, empty by default.
 */
ab_prs: Array<string>,
/**
 * Optional UTC deadline, absent by default.
 */
ab_until: string | null, };

/**
 * A line read that supports the validity reading.
 */
export type AssessmentEvidence = {
/**
 * Repository-relative path at the head.
 */
path: string,
/**
 * 1-based line at the head.
 */
line: number,
/**
 * What the line shows.
 */
note: string, };

/**
 * Non-secret immutable snapshot reference; includes source versions, scope and standing decisions.
 */
export type AssessmentInput = {
/**
 * owner/repo#number.
 */
pr: string,
/**
 * Stable PR database key.
 */
pull_request_id: number,
/**
 * Assessed head.
 */
head: CommitSha,
/**
 * Base used by both adapters.
 */
base: CommitSha,
/**
 * SHA-256 of the frozen input, including source versions and standing decisions.
 */
input_hash: string,
/**
 * SHA-256 of the frozen scope.
 */
scope_hash: string,
/**
 * Immutable host-owned non-secret snapshot reference, never model-supplied.
 */
snapshot_ref: string,
/**
 * Identical model and effort for both adapters.
 */
model: ModelChoice, };

/**
 * An assessment adapter; native outputs retain their own validation contracts.
 */
export type AssessmentMethod = "claudius_host" | "prompt_v4";

/**
 * Persisted method summary; usage and duration include failed attempts once each.
 */
export type AssessmentMethodRun = {
/**
 * False when any started attempt has unavailable metering; usage then reports only known totals.
 */
usage_complete: boolean,
/**
 * Commit actually reported by the adapter; a mismatch stays comparison-only.
 */
reported_head: CommitSha | null,
/**
 * Frozen method identity.
 */
reference: AssessmentRunRef,
/**
 * One job whose attempts execute this method.
 */
job_id: JobId,
/**
 * Version of the adapter.
 */
version: string,
/**
 * Current state.
 */
status: AssessmentRunStatus,
/**
 * Current attempt fence.
 */
attempt: number,
/**
 * The only accepted successful attempt, if any.
 */
accepted_attempt: number | null,
/**
 * Billable usage from all attempts, deduplicated by job/attempt.
 */
usage: Usage,
/**
 * Total duration in milliseconds across attempts.
 */
duration_ms: number,
/**
 * Scrubbed failure kind, never upstream response text.
 */
failure: JobFailureKind | null, };

/**
 * Actual owner choice, separate from inferred/relayed report evidence.
 */
export type AssessmentOwnerDecision = {
/**
 * Durable diffpilot decision ID.
 */
id: DecisionId,
/**
 * Revision retained in append-only history.
 */
revision: number,
/**
 * Actual deciding user.
 */
owner: UserId,
/**
 * Actual decision time.
 */
at: string,
/**
 * Actual chosen kind.
 */
kind: ReviewDecisionKind,
/**
 * Actual target.
 */
target: DecisionTarget | null, };

/**
 * One admitted logical pass; retries keep its original day and configuration.
 */
export type AssessmentPass = {
/**
 * Pass identity.
 */
id: PassId,
/**
 * Payer and quota owner.
 */
payer: UserId,
/**
 * Frozen input.
 */
input: AssessmentInput,
/**
 * Frozen user configuration.
 */
config: AssessmentConfig,
/**
 * Whether both adapters are scheduled.
 */
paired: boolean,
/**
 * Original UTC admission day, YYYY-MM-DD.
 */
day_utc: string,
/**
 * Original admission time.
 */
admitted_at: string, };

/**
 * Assessment progress of a PR ("LLM assessment running: 4 of 9 comments").
 */
export type AssessmentProgress = {
/**
 * Items in the live pass input.
 */
in_pass?: number,
/**
 * Items waiting for the next pass.
 */
waiting?: number,
/**
 * When the live pass started.
 */
since?: string,
/**
 * A pass is queued or running.
 */
running: boolean,
/**
 * Items assessed at the current head.
 */
assessed: number,
/**
 * Items needing an assessment.
 */
total: number,
/**
 * Items left out by the daily limit.
 */
limited: number,
/**
 * Head of the latest pass.
 */
head: CommitSha | null,
/**
 * End of the latest pass.
 */
at: string | null, };

/**
 * Safe quota status; unrelated jobs are unaffected.
 */
export type AssessmentQuota = {
/**
 * Logical passes still available today, out of 80.
 */
remaining: number,
/**
 * Next UTC midnight.
 */
resets_at: string, };

/**
 * Durable method identity carried by both job input and result; attempt is the enclosing job's fence.
 */
export type AssessmentRunRef = {
/**
 * Logical pass shared by an A/B pair and all its retries.
 */
pass_id: PassId,
/**
 * Independent adapter.
 */
method: AssessmentMethod, };

/**
 * Method execution status, independent of per-row completeness.
 */
export type AssessmentRunStatus = "queued" | "running" | "succeeded" | "failed" | "cancelled";

/**
 * GET /settings/comment-assessment; PUT uses AssessmentSettingsUpdate and never schedules work itself.
 */
export type AssessmentSettings = {
/**
 * Opaque content revision required for an atomic conditional write.
 */
revision: string,
/**
 * Effective per-user configuration.
 */
config: AssessmentConfig,
/**
 * Safe remaining/reset status.
 */
quota: AssessmentQuota, };

/**
 * Conditional settings write; stale revisions fail with HTTP 409 and never schedule work.
 */
export type AssessmentSettingsUpdate = {
/**
 * Configuration the caller wants to save.
 */
config: AssessmentConfig,
/**
 * Revision obtained with the configuration the caller edited.
 */
revision: string, };

/**
 * Frozen source identity; quote and version are required for body-point alignment.
 */
export type AssessmentSource = { "kind": "inline",
/**
 * GitHub node ID.
 */
thread_id: string,
/**
 * Frozen thread body/version hash, preventing decision reuse after edits.
 */
version: string, } | { "kind": "body",
/**
 * Original report source kind.
 */
source_kind: CommentSourceKind,
/**
 * Source URL.
 */
url: string,
/**
 * Frozen body SHA-256.
 */
version: string,
/**
 * Verbatim grounded quote.
 */
quote: string, };

/**
 * Whether an item has an LLM assessment.
 */
export type AssessmentState = { "state": "queued" } | { "state": "running" } | { "state": "assessed" } | { "state": "not_assessed",
/**
 * Why.
 */
reason: NotAssessedReason, };

/**
 * A nullable common verdict never inferred from prose.
 */
export type AssessmentVerdict = "valid" | "invalid" | "already_fixed" | "out_of_scope";

/**
 * Who wrote the item: the bot rule (`Bot` type, `[bot]` login or Settings `known_bots`, RQ17).
 */
export type AuthorKind = "human" | "bot" | "diffpilot";

/**
 * The latest automatic assessment.
 */
export type AutoPass = {
/**
 * When it ended.
 */
at: string,
/**
 * Questions waiting for a decision when it ended.
 */
questions: number, };

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
 * Whom to ping: logins and teams (`team:<slug>`) whose review is requested, or those a plug-in says owe a
 * review; empty when nobody is named.
 */
waiting_on: Array<string>,
/**
 * Why it waits on others.
 */
reason: AwaitingReason,
/**
 * Since when it waits (the oldest open request; else when the pull request was opened).
 */
waiting_since: string,
/**
 * Deterministic tags from scan facts (e.g. CI passing).
 */
tags: Array<FactTag>,
/**
 * Size mark of its full diff (display only); absent when the totals are unknown.
 */
change_size?: ChangeSizeMark,
/**
 * Always null: a pull request waiting only on others is not ranked (kept for compatibility).
 */
total_tenths: number | null, };

/**
 * Why a pull request of the user waits on other people (plan K15 W3), the first that applies.
 */
export type AwaitingReason = { "kind": "ready_label",
/**
 * The label as on GitHub.
 */
label: string, } | { "kind": "plugin",
/**
 * Plug-in id.
 */
plugin: string,
/**
 * Name of the plug-in, shown before the reason.
 */
source: string,
/**
 * The plug-in's reason, plain text.
 */
reason: string,
/**
 * Evidence: the source's record of the pull request.
 */
url: string, } | { "kind": "review_requested" };

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
 * How a batch was decided.
 */
export type BatchVia = "group" | "bulk" | "table";

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
 * thepastaclaw's own status of a thread, read from its status block (spike S13-1 M4); a GitHub fact, not a
 * decision. Open enum.
 */
export type BotThreadStatus = "resolved" | "deferred" | "withdrawn" | "no_longer_applies" | "disputed";

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
 * Carry-over counts.
 */
export type CarryCounts = {
/**
 * Carried.
 */
carried: number,
/**
 * Asked again: bot edited.
 */
asked_again_bot_edited: number,
/**
 * Asked again: code changed.
 */
asked_again_code_changed: number,
/**
 * Carried decisions whose effects are pending.
 */
effects_pending: number, };

/**
 * A carry-over event (V5).
 */
export type CarryEvent = {
/**
 * Id.
 */
id: CarryEventId,
/**
 * Pull request.
 */
pr: PrRef,
/**
 * The repeat, `C<no>`; may equal the source item when the bot re-raised it in place (spike S13-1 E2).
 */
item_no: number,
/**
 * The item of the source decision, `C<no>`.
 */
source_item_no: number,
/**
 * The decision carried or re-asked.
 */
source_decision: DecisionId,
/**
 * Its kind.
 */
source_kind: ReviewDecisionKind,
/**
 * Kind.
 */
kind: CarryKind,
/**
 * Why asked again.
 */
reason: ReaskReason | null,
/**
 * Evidence.
 */
evidence: CarryEvidence,
/**
 * Effects of the carried decision not yet done.
 */
effects_pending: number,
/**
 * When.
 */
at: string, };

/**
 * Identifier of a carry-over event (a decision carried to a repeat, or a repeat asked again).
 */
export type CarryEventId = string;

/**
 * Evidence of a carry-over event, read by the server.
 */
export type CarryEvidence = {
/**
 * Verbatim grounded quote of a restated point, when supplied.
 */
quote?: string,
/**
 * GitHub link of the repeat.
 */
url: string | null,
/**
 * Normalised text hash at decision time (hex).
 */
body_hash_before: string | null,
/**
 * Normalised text hash now (hex).
 */
body_hash_after: string | null,
/**
 * The commit that changed the lines.
 */
commit: CommitSha | null,
/**
 * The decided lines mapped forward to the head.
 */
lines: LineRange | null, };

/**
 * Carry-over event kind.
 */
export type CarryKind = "carried" | "asked_again";

/**
 * Response of RC11: events newest first, counts per PR and in total.
 */
export type CarryOverPage = {
/**
 * Totals over the scope.
 */
totals: CarryCounts,
/**
 * Per pull request (all-PR scope; one row for a PR scope).
 */
per_pr: Array<PrCarryCounts>,
/**
 * Events, newest first.
 */
events: Array<CarryEvent>,
/**
 * Cursor of the next page.
 */
next_cursor: string | null, };

/**
 * Query of RC11.
 */
export type CarryOverQuery = {
/**
 * One pull request, `owner/name/number`; absent for all.
 */
pr: string | null,
/**
 * Opaque cursor of the next page.
 */
cursor: string | null, };

/**
 * A pull request's size mark with its evidence (a scan fact, not a score input).
 */
export type ChangeSizeMark = {
/**
 * The mark.
 */
size: SizeMark,
/**
 * Added lines of the full diff.
 */
additions: number,
/**
 * Removed lines of the full diff.
 */
deletions: number, };

/**
 * Push channel kind.
 */
export type ChannelKind = "ntfy" | "webhook";

/**
 * Response of RN3 (one attempt, no retry).
 */
export type ChannelTestResult = {
/**
 * Outcome.
 */
outcome: DeliveryOutcome,
/**
 * HTTP status when one was received.
 */
http_status: number | null,
/**
 * Error in plain words.
 */
error: string | null, };

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
 * Response of RF3: 201, the claimed decision and its lease (409 `claimed`).
 */
export type Claim = {
/**
 * The decision.
 */
decision: FeedDecision,
/**
 * Lease end (30 minutes; renewed by each report).
 */
lease_until: string, };

/**
 * The claim on an effect's decision.
 */
export type ClaimInfo = {
/**
 * Name of the claiming access token.
 */
token_name: string,
/**
 * Lease end; renewed by each report.
 */
lease_until: string, };

/**
 * Body of RF3 (`POST …/decisions/{id}/claim`).
 */
export type ClaimRequest = {
/**
 * Capabilities; every open effect of the decision must be within them (422 `capability_missing`).
 */
capabilities: Array<EffectKind>, };

/**
 * Classification of a flow (brief §3, closed list).
 */
export type Classification = "behavioural" | "api_contract" | "new_code" | "refactor_pure" | "move" | "rename" | "docs_comments" | "generated_mechanical" | "config_build" | "dependency" | "other";

/**
 * Response of RC5.
 */
export type CodeContext = {
/**
 * Repository-relative path.
 */
path: string,
/**
 * Head the lines come from.
 */
head: CommitSha,
/**
 * Lines served.
 */
lines: Array<CodeLine>,
/**
 * Lines in the file.
 */
total_lines: number,
/**
 * The commented lines.
 */
marked: LineRange | null,
/**
 * Enclosing range and description (LLM), when assessed.
 */
code: CodeSummary | null,
/**
 * The analysis the flow steps come from; null without one.
 */
analysis: AnalysisId | null,
/**
 * Head of that analysis (labelled when older than the PR head).
 */
analysis_head: CommitSha | null,
/**
 * Flow steps containing the lines.
 */
in_flows: Array<FlowStepRef>,
/**
 * Notes elsewhere.
 */
notes: Array<NoteElsewhere>, };

/**
 * One line of code.
 */
export type CodeLine = {
/**
 * 1-based line at the head.
 */
line: number,
/**
 * Text.
 */
text: string, };

/**
 * The enclosing code of the discussed lines and what it does.
 */
export type CodeSummary = {
/**
 * Repository-relative path at the head.
 */
path: string,
/**
 * Enclosing range (function or block).
 */
enclosing: LineRange,
/**
 * One paragraph on what the code does (LLM).
 */
description: string, };

/**
 * The LLM assessment of one item at one head (`verify_comments`).
 */
export type CommentAssessment = {
/**
 * Original host proposal, preserving targets beyond the legacy action form.
 */
proposed_decision?: ProposedDecision,
/**
 * Evidence from the report, labelled LLM text.
 */
evidence_text?: string,
/**
 * Attributed prior decision; authority and grounding are verified by server ingest.
 */
recorded_decision?: RecordedDecision,
/**
 * The job that wrote it.
 */
job: JobId,
/**
 * Head it was made at.
 */
head: CommitSha,
/**
 * When it was stored.
 */
assessed_at: string,
/**
 * Reused from this head: comment text, code at the lines and PR purpose unchanged since (fingerprint).
 */
reused_since: CommitSha | null,
/**
 * Validity.
 */
validity: Validity,
/**
 * Lines that support it.
 */
evidence: Array<AssessmentEvidence>,
/**
 * The problem exists on the base already.
 */
pre_existing: boolean | null,
/**
 * Category slug (claudius #118 catalog).
 */
category: string,
/**
 * Necessity.
 */
necessity: Necessity,
/**
 * Gates the LLM names.
 */
gates: Array<Gate>,
/**
 * Where else it is handled.
 */
elsewhere: Array<Elsewhere>,
/**
 * Fix sketch.
 */
fix_sketch: string | null,
/**
 * Fix size.
 */
fix_size: FixSize | null,
/**
 * Reply drafts (English, "per maintainer decision").
 */
reply_drafts: Array<string>,
/**
 * Recommendation.
 */
recommendation: Recommendation,
/**
 * Enclosing code and its description.
 */
code: CodeSummary | null,
/**
 * Prompt version of the job.
 */
prompt_version: number,
/**
 * Commit of the claudius checkout whose skills the job read ("Assessed with claudius <short sha>"); null when
 * the runner gave none or the assessment predates it. A reused assessment keeps the commit it was made with.
 */
skills_commit: string | null, };

/**
 * Item counts of a PR.
 */
export type CommentCounts = {
/**
 * Items classified as not a review point.
 */
not_a_review_point: number,
/**
 * Items in the Needs you queue.
 */
needs_you: number,
/**
 * Gated items in the Needs you queue.
 */
needs_you_gates: number,
/**
 * Items with a decision imported since this reader last visited Comments.
 */
imported_unseen?: number,
/**
 * Unresolved threads on GitHub (fact).
 */
unresolved_threads: number,
/**
 * Undecided questions.
 */
undecided: number,
/**
 * Repeats asked again.
 */
asked_again: number,
/**
 * Decisions with open or claimed effects.
 */
waiting_for_agents: number,
/**
 * Failed effects.
 */
failed_effects: number,
/**
 * Items resolved outside diffpilot.
 */
outside: number,
/**
 * All items.
 */
total: number, };

/**
 * A review-loop item with its assessment, decisions and effects (RC4).
 */
export type CommentItem = {
/**
 * Active internal disposition.
 */
not_a_review_point?: ItemMark,
/**
 * Whether the item is a bot status comment.
 */
status_comment?: boolean,
/**
 * Evidence of the current decision imported from GitHub.
 */
imported_decision?: ImportedGithubDecision,
/**
 * A recorded GitHub decision awaiting confirmation.
 */
import_proposal?: ImportedGithubDecision,
/**
 * First sentence of the primary assessment description, cut at a word within 90 characters.
 */
llm_title?: string,
/**
 * Whether this item follows the decision on its primary.
 */
decision_follows_primary?: boolean,
/**
 * Grounded point read from a versioned source; absent on inline threads and PR questions.
 */
point?: CommentPoint,
/**
 * Per-PR number of the linked point; each inline thread keeps its own item.
 */
duplicate_of?: number,
/**
 * Database id.
 */
id: CommentItemId,
/**
 * Per-PR number shown as `C<no>`, assigned at first sight, never reused.
 */
no: number,
/**
 * Source.
 */
kind: CommentItemKind,
/**
 * Stable key: the bot's `dedupe=` marker, else the thread node id; review id + ordinal for body items;
 * `pr:<round>:merge` / `pr:<round>:dismiss` for PR-level questions.
 */
stable_key: string,
/**
 * Author's GitHub login (`diffpilot` for PR-level questions).
 */
author: string,
/**
 * Author kind.
 */
author_kind: AuthorKind,
/**
 * GitHub link (thread, review or comment); null for PR-level questions.
 */
url: string | null,
/**
 * Discussed lines.
 */
location: ItemLocation | null,
/**
 * Title: the bot's finding heading or the first line; plain text.
 */
title: string,
/**
 * Comment text (GitHub): plain text, rendered as text; links only to `https://github.com/…`.
 */
body: string,
/**
 * The bot's own severity words (e.g. `Blocking`), shown with the GitHub tag; a claim, not a fact.
 */
bot_severity: string | null,
/**
 * Gates from the assessment or the owner.
 */
gates: Array<GateMark>,
/**
 * State.
 */
state: ItemState,
/**
 * Revision; a decision names the revision it saw (409 `item_changed` otherwise).
 */
revision: number,
/**
 * Head when diffpilot first read it (not the head the comment was written on).
 */
first_seen_head: CommitSha,
/**
 * Bot round when first read; null before the first round.
 */
first_round: number | null,
/**
 * Where GitHub says the comment was written, from the latest read; absent when that read gives no time (an
 * item it no longer produces, a read stored before this was kept).
 */
written?: Written,
/**
 * GitHub facts.
 */
github: ItemGithubFacts,
/**
 * Set when resolved outside diffpilot.
 */
outside: OutsideResolution | null,
/**
 * When a complete read of the pull request first no longer produced it (deleted on GitHub, or no longer an
 * item by the read rules, such as a person's earlier review); absent while reads produce it. Such an item
 * leaves the queue, "Decide the rest" and bulk decisions; its decisions stay.
 */
gone_at?: string,
/**
 * The item whose decision this repeat carried or re-asks (`C<no>`).
 */
repeat_of: number | null,
/**
 * Assessment state.
 */
assessment_state: AssessmentState,
/**
 * The latest LLM assessment.
 */
assessment: CommentAssessment | null,
/**
 * Decisions, oldest first; the last is current.
 */
decisions: Array<Decision<DecisionKind>>,
/**
 * Answers in the Ask chat about this item.
 */
ask_answers: number, };

/**
 * Identifier of a review-loop item (a PR review comment, review-body item or PR-level question).
 */
export type CommentItemId = string;

/**
 * Where an item comes from (plan §2.1 I1–I4). Open enum.
 */
export type CommentItemKind = "thread" | "review_body" | "pr_comment" | "pr_question";

/**
 * Where a pending comment sits on GitHub.
 */
export type CommentKind = "line" | "file";

/**
 * Response of RC10: decisions, marks, carry events and feed state.
 */
export type CommentLog = {
/**
 * Marks and their undo history, oldest first.
 */
marks?: Array<MarkLogEntry>,
/**
 * Agents subscribed now.
 */
subscribers: Array<FeedSubscriber>,
/**
 * Open or claimed effects.
 */
open_effects: number,
/**
 * Failed effects.
 */
failed_effects: number,
/**
 * Decisions, oldest first.
 */
decisions: Array<Decision<DecisionKind>>,
/**
 * Carry events of this PR, oldest first.
 */
carry_events: Array<CarryEvent>, };

/**
 * Point provenance, grounded in the source's cleaned GitHub text.
 */
export type CommentPoint = {
/**
 * Server-validated source URL.
 */
source_url: string,
/**
 * Submitted review body or PR comment.
 */
source_kind: CommentSourceKind,
/**
 * Distinct verbatim span: 12–600 characters, or the whole shorter source.
 */
quote: string, };

/**
 * Host report envelope; rows are decoded individually so a malformed row can be dropped.
 */
export type CommentReport = {
/**
 * Accepted versions are 4.2.x.
 */
schema_version: string,
/**
 * Host provenance, matched against the server's input.
 */
metadata: ReportMetadata,
/**
 * Independently parsed assessment rows; unknown report fields are ignored.
 */
comment_assessments: unknown[], };

/**
 * Commenting on displayed lines, decided by the server from the pull request diff (ADR-006).
 */
export type CommentScope = { "scope": "in_diff",
/**
 * Ranges, on the response's side, whose lines get a line thread.
 */
line_thread_ranges: Array<LineRange>, } | { "scope": "not_in_diff" };

/**
 * Read-only source version, including sources from which no points were read.
 */
export type CommentSource = {
/**
 * Source URL.
 */
url: string,
/**
 * Source kind.
 */
kind: CommentSourceKind,
/**
 * GitHub author login.
 */
author: string,
/**
 * Cleaned source text, retaining details content.
 */
body: string,
/**
 * SHA-256 of the cleaned text identifies this version.
 */
body_hash: string,
/**
 * Host-assigned source role.
 */
role: SourceRole,
/**
 * First successful assessment read of this source version.
 */
read_at: string | null, };

/**
 * Stored source kind; inline threads remain independent items.
 */
export type CommentSourceKind = "review_body" | "pr_comment";

/**
 * Counts of a pull request's review comments in Comments (plan docs/review-workflow-plan.md §2.1): threads,
 * review-body and PR-comment items of the last read, without PR-level questions and items the read no longer
 * produces.
 */
export type CommentTally = {
/**
 * Comments in any state.
 */
total: number,
/**
 * Those waiting for a decision: the Needs you list of Comments.
 */
needs_you: number, };

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
 * Response of RC2.
 */
export type CommentsOverview = {
/**
 * The reader’s watch for this pull request.
 */
watch?: WatchedPr,
/**
 * Pull request.
 */
pr: PrRef,
/**
 * Title (GitHub).
 */
title: string,
/**
 * GitHub link.
 */
url: string,
/**
 * Head.
 */
head: CommitSha,
/**
 * PR purpose in one sentence (LLM), or the analysis's PR context.
 */
purpose: string | null,
/**
 * The last read.
 */
read: GithubRead,
/**
 * Counts.
 */
counts: CommentCounts,
/**
 * Rounds, oldest first.
 */
rounds: Array<Round>,
/**
 * Assessment progress.
 */
assessment: AssessmentProgress,
/**
 * Questions in order.
 */
queue: Array<QueueEntry>,
/**
 * State badge.
 */
badge: PrBadgeView,
/**
 * Latest analysis of this PR, if any (the view menu needs one for Flows, Files and Dependencies).
 */
analysis: AnalysisId | null,
/**
 * Review-ready analysis of the current head used by Ask, absent if only standalone questions are available.
 */
ask_analysis?: AnalysisId, };

/**
 * Comments summary on the PR page.
 */
export type CommentsSummary = {
/**
 * Counts.
 */
counts: CommentCounts,
/**
 * Bot rounds so far.
 */
rounds: number,
/**
 * Assessment progress.
 */
assessment: AssessmentProgress,
/**
 * The last read.
 */
read: GithubRead, };

/**
 * A full 40-character hexadecimal git commit id.
 */
export type CommitSha = string;

/**
 * Distinct completeness/alignment states; null verdict and proposal remain independent.
 */
export type ComparisonItemStatus = "context" | "matched" | "missing_row" | "duplicate" | "unmatched" | "method_failed" | "head_mismatch";

/**
 * Counts exclude duplicates, mismatched heads and unaligned secondary points from scoring.
 */
export type ComparisonSummary = {
/**
 * Rows whose two available normalized verdicts can be compared.
 */
verdict_compared: number,
/**
 * Different normalized verdicts among verdict_compared.
 */
verdict_disagreements: number,
/**
 * Primary points waiting for an actual owner choice.
 */
pending_owner: number,
/**
 * Per-method coverage and exact proposal-kind/target agreement.
 */
methods: Array<MethodComparisonScore>, };

/**
 * A worker's identity; each identity owns at most one running job.
 */
export type ContainerId = { "shape": "pr",
/**
 * Database PR key.
 */
pull_request_id: number,
/**
 * User paying for this worker's turns.
 */
payer: UserId, } | { "shape": "judge",
/**
 * Dashboard user paying for the turn.
 */
payer: UserId, };

/**
 * Query of RC5: lines `from..=to` (defaults: the enclosing range, at most 40 lines).
 */
export type ContextQuery = {
/**
 * First line.
 */
from: number | null,
/**
 * Last line.
 */
to: number | null, };

/**
 * The installation-wide list of controlled GitHub owners: their repositories are "yours" in the Dependencies
 * view, besides the pull request's owner, which always is.
 */
export type ControlledOwners = {
/**
 * Owner logins, sorted case-insensitively, unique.
 */
owners: Array<string>,
/**
 * Who saved the list last; null before the first save.
 */
changed_by: string | null,
/**
 * When it was saved last.
 */
changed_at: string | null, };

/**
 * A conversion added for a type of another crate: a deterministic fact (Rust tokens), never a verdict.
 */
export type ConversionFact = {
/**
 * What the line adds.
 */
kind: ConversionKind,
/**
 * The `impl` or `fn` line.
 */
at: HeadLine,
/**
 * The type's name.
 */
type_name: string,
/**
 * The type's path as written.
 */
written: string,
/**
 * The crate the type comes from.
 */
type_crate: string,
/**
 * The `use` line importing it, if any.
 */
imported_at: HeadLine | null,
/**
 * Where the type is defined.
 */
definition: TypeDefinition, };

/**
 * What a conversion fact's line adds.
 */
export type ConversionKind = "from_impl" | "try_from_impl" | "impl_on_foreign_type" | "conversion_fn";

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
 * What the pull request changes in a crate.
 */
export type CrateChanges = {
/**
 * Changed lines (added and removed) in its files.
 */
changed_lines: number,
/**
 * Changed files.
 */
files: number,
/**
 * Flows with lines in it, in Tour order.
 */
flows: Array<CrateFlow>,
/**
 * Conversions added for types of workspace or controlled crates (deterministic), at most 50.
 */
facts: Array<ConversionFact>,
/**
 * Facts beyond those listed.
 */
facts_hidden: number,
/**
 * Facts about types of other crates (counted, not listed).
 */
other_facts: number,
/**
 * The LLM's notes, at most 3, most changed lines first.
 */
notes: Array<CrateNote>,
/**
 * Notes beyond those listed.
 */
notes_hidden: number,
/**
 * Direct dependencies not listed (over the view's cap of crates outside this repository and controlled ones).
 */
other_dependencies: number, };

/**
 * A flow with lines in a crate.
 */
export type CrateFlow = {
/**
 * Flow id.
 */
id: FlowId,
/**
 * Its title (LLM).
 */
title: string, };

/**
 * A keyword note of the LLM on what changed in a crate.
 */
export type CrateNote = {
/**
 * The note (LLM).
 */
text: string,
/**
 * The changed lines of the hunks it describes, in this crate (head side when present).
 */
lines: Array<FileRange>,
/**
 * Changed lines of those hunks: the ranking key.
 */
changed_lines: number, };

/**
 * Body of `POST /api/v1/settings/access-tokens`.
 */
export type CreateAccessToken = {
/**
 * Name, 1–60 characters, unique among the user's tokens.
 */
name: string,
/**
 * Requested scopes: `read`, optionally with `review_loop_execute` (ADR-023).
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
 * The only supported credential mode; no paid fallback.
 */
export type CredentialMode = "chatgpt_plan";

/**
 * ChatGPT-plan connection state.
 */
export type CredentialState = "linking" | "linked" | "relink_needed";

/**
 * Non-secret status shared by gateway, server and Settings.
 */
export type CredentialStatus = {
/**
 * Safe observations, without tokens, key IDs or upstream response bodies.
 */
telemetry: CredentialTelemetry,
/**
 * Current device challenge on the staged-link GET only; reading never starts a flow.
 */
challenge: DeviceLink | null,
/**
 * Only ChatGPT-plan linking is supported.
 */
mode: CredentialMode,
/**
 * Common connection state.
 */
state: CredentialState,
/**
 * Present only when relink is needed.
 */
relink_reason: RelinkReason | null,
/**
 * Human-readable connection label.
 */
label: string | null,
/**
 * Provider plan label, when supplied.
 */
plan: string | null,
/**
 * Payer pause after a usage limit.
 */
paused_until: string | null,
/**
 * Most recent model call.
 */
last_call: LlmCallOutcome | null, };

/**
 * Safe account observations; gateway fills plan data and server fills payer job data.
 */
export type CredentialTelemetry = {
/**
 * Gateway commit time of the current account link; refresh does not change it.
 */
linked_at: string | null,
/**
 * Last observed primary plan window.
 */
primary: PlanUsage | null,
/**
 * Last observed secondary plan window.
 */
secondary: PlanUsage | null,
/**
 * Retained provider reports finished today in UTC, null without reports.
 */
tokens_today: UsageTotals | null,
/**
 * Retained provider reports finished this calendar month in UTC, null without reports.
 */
tokens_month: UsageTotals | null,
/**
 * Server-known containers with claimed/running jobs; null until read from server state.
 */
live_containers: Array<ContainerId> | null, };

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
time_zone: string | null,
/**
 * GitHub logins with the Slack name "Copy for Slack" writes instead of `@login` (at most 100 rows).
 */
slack_names: Array<SlackName>, };

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
 * Counts of distinct judgement and sync states.
 */
status_counts: StatusCounts,
/**
 * Repositories updated by GitHub App webhooks.
 */
live_repositories: number,
/**
 * Repositories refreshed by manual and scheduled scans.
 */
not_live_repositories: number,
/**
 * Latest GitHub event applied to this dashboard.
 */
last_event_at?: string,
/**
 * Unanswered, unhidden mentions, oldest first; independent of the ranking.
 */
mentions: Array<Mention>,
/**
 * Mention reads omitted in this snapshot, independent of item filters and pagination.
 */
mention_coverage: MentionCoverage,
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
 * Items judged against an older prompt. Superseded by `status_counts`.
 */
older_prompt_judgements: number,
/**
 * Configured dashboard plug-ins (ADR-022); absent when none is configured. While one is listed, the browser
 * reads the summary again every minute.
 */
plugins?: Array<PluginStatus>, };

/**
 * Body of RC8 (`POST …/comments/items/{item}/decisions`).
 */
export type DecideRequest<K = DecisionKind> = {
/**
 * Decision.
 */
kind: K,
/**
 * Reason (required for D3 and D8).
 */
reason: string | null,
/**
 * Target (D4, D6, D7; optional title for D5).
 */
target: DecisionTarget | null,
/**
 * Override flag.
 */
override: Override | null,
/**
 * "Resolve this thread" as submitted.
 */
resolve_ticked: boolean,
/**
 * Reply text to post (edited preview); null for the server's default.
 */
reply_text: string | null,
/**
 * The owner's comment for the executor; never posted.
 */
comment: string | null,
/**
 * The item revision the decision is based on.
 */
item_revision: number,
/**
 * The `plan_digest` of the RC16 preview the owner confirmed (PR-level decisions); when the plan computed at
 * storing differs, the decision is refused (409 `conflict`). Absent: stored as planned then.
 */
plan_digest?: string, };

/**
 * The "Decide the rest per recommendation" confirmation list; nothing is stored until RC9 confirms it.
 */
export type DecideRest = {
/**
 * Rows to decide; their count is the number shown.
 */
rows: Array<DecideRestRow>,
/**
 * Undecided items left out, gate items included.
 */
left_out: Array<LeftOutItem>, };

/**
 * One row of the "Decide the rest" confirmation list.
 */
export type DecideRestRow = {
/**
 * The decision submitted on confirmation (RC9 `via: bulk`).
 */
row: DecisionBatchRow<ReviewDecisionKind>,
/**
 * The recommendation it follows.
 */
recommendation: Recommendation,
/**
 * Its effects.
 */
effects: Array<PlannedEffect>, };

/**
 * A stored decision (append-only) with its effects.
 */
export type Decision<K = DecisionKind> = {
/**
 * Evidence and authority for a decision imported from GitHub.
 */
imported?: ImportedDecision,
/**
 * Item number of the primary whose decision this follows.
 */
follows?: number,
/**
 * Id.
 */
id: DecisionId,
/**
 * The item (`C<no>`).
 */
item_no: number,
/**
 * Decision.
 */
kind: K,
/**
 * Reason (D3, D8; posted as the reply).
 */
reason: string | null,
/**
 * Target (D4–D7).
 */
target: DecisionTarget | null,
/**
 * Override flag.
 */
override: Override | null,
/**
 * "Resolve this thread" as submitted: the explicit resolve decision.
 */
resolve_ticked: boolean,
/**
 * The reply text that gets posted (English).
 */
reply_text: string | null,
/**
 * The owner's comment: stored, sent to the executing agent, never posted to GitHub.
 */
comment: string | null,
/**
 * The recommendation shown at decision time.
 */
recommendation: Recommendation | null,
/**
 * Where it was made.
 */
via: DecisionVia,
/**
 * The decision this one was carried from.
 */
carried_from: DecisionId | null,
/**
 * The decision this one supersedes.
 */
supersedes: DecisionId | null,
/**
 * Head at decision time.
 */
head: CommitSha,
/**
 * GitHub login of the deciding user (`carried` decisions: of the source decision).
 */
decided_by: string,
/**
 * When.
 */
decided_at: string,
/**
 * When the owner undid it (before any effect was claimed); its effects are cancelled and the item is asked
 * again. Absent while it stands.
 */
undone_at?: string,
/**
 * Effects in `seq` order.
 */
effects: Array<Effect>, };

/**
 * Body of RC9 (`POST …/comments/decision-batches`); 201 `Decision[]`.
 */
export type DecisionBatchRequest = {
/**
 * Where.
 */
via: BatchVia,
/**
 * One row per item.
 */
rows: Array<DecisionBatchRow<DecisionKind>>, };

/**
 * One row of a decision batch.
 */
export type DecisionBatchRow<K = DecisionKind> = {
/**
 * The item, `C<no>`.
 */
item_no: number,
/**
 * Decision.
 */
kind: K,
/**
 * Reason (required for D3 and D8).
 */
reason: string | null,
/**
 * Target (D4, D6, D7; optional title for D5).
 */
target: DecisionTarget | null,
/**
 * Override flag.
 */
override: Override | null,
/**
 * "Resolve this thread" as submitted.
 */
resolve_ticked: boolean,
/**
 * Reply text to post (edited preview); null for the server's default.
 */
reply_text: string | null,
/**
 * The owner's comment for the executor; never posted.
 */
comment: string | null,
/**
 * The item revision the decision is based on.
 */
item_revision: number,
/**
 * The `plan_digest` of the RC16 preview the owner confirmed (PR-level decisions); when the plan computed at
 * storing differs, the decision is refused (409 `conflict`). Absent: stored as planned then.
 */
plan_digest?: string, };

/**
 * Response of RF1: claimable decisions within the capabilities, oldest first.
 */
export type DecisionFeed = {
/**
 * Decisions.
 */
decisions: Array<FeedDecision>,
/**
 * Cursor of the next page.
 */
next_cursor: string | null, };

/**
 * Identifier of a decision on a review-loop item (append-only).
 */
export type DecisionId = string;

/**
 * A review decision or an internal mark accepted by the decide routes.
 */
export type DecisionKind = "resolve" | "fix_here" | "reject" | "handled_in_pr" | "follow_up_pr" | "follow_up_issue" | "upstream_issue" | "todo" | "merge_with_follow_ups" | "another_round" | "dismiss_stale_reviews" | "keep_reviews" | "not_a_review_point";

/**
 * The target a decision needs.
 */
export type DecisionTarget = { "type": "existing_pr",
/**
 * Its number in the same repository.
 */
number: number, } | { "type": "follow_up_pr",
/**
 * Title.
 */
title: string | null, } | { "type": "follow_up_issue",
/**
 * Title.
 */
title: string, } | { "type": "upstream_issue",
/**
 * Issue URL.
 */
url: string | null, } | { "type": "todo",
/**
 * Title.
 */
title: string, };

/**
 * Where a decision was made. Open enum.
 */
export type DecisionVia = "imported_github" | "follows_primary" | "question" | "group" | "bulk" | "table" | "carried";

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
 * Outcome of a delivery.
 */
export type DeliveryOutcome = "delivered" | "failed";

/**
 * Delivery state of a channel or push target.
 */
export type DeliveryStatus = {
/**
 * Latest delivery.
 */
last: LastDelivery | null,
/**
 * Latest error in plain words ("connection refused (3 attempts)").
 */
last_error: string | null,
/**
 * Failing since; null while healthy.
 */
failing_since: string | null, };

/**
 * A crate.
 */
export type DependencyCrate = {
/**
 * Package name.
 */
name: string,
/**
 * Version, if known.
 */
version: string | null,
/**
 * Manifest path of a crate of this repository.
 */
manifest: string | null,
/**
 * The `Cargo.lock` record or manifest declaration naming its source (head commit).
 */
source_at: HeadLine | null,
/**
 * Locked git commit, if any.
 */
revision: string | null,
/**
 * Changed crates depending on it directly.
 */
used_by: Array<string>,
/**
 * Conversion facts naming its types.
 */
facts_about: number,
/**
 * What the pull request changes in it; null for an unchanged crate.
 */
changes: CrateChanges | null, };

/**
 * Something the map does not cover.
 */
export type DependencyLimit = { "kind": "manifests_cut",
/**
 * Manifests at the head commit.
 */
total: number,
/**
 * Manifests read.
 */
read: number, } | { "kind": "manifest_unreadable",
/**
 * Its path.
 */
path: string, } | { "kind": "lock_unreadable",
/**
 * Its path.
 */
path: string, } | { "kind": "no_lock_file",
/**
 * Directory of its root manifest (`""` = repository root).
 */
workspace_dir: string, } | { "kind": "source_unread",
/**
 * Its path.
 */
path: string, } | { "kind": "facts_cut",
/**
 * Facts kept.
 */
kept: number, } | { "kind": "workspace_unresolved",
/**
 * Its manifest.
 */
manifest: string, } | { "kind": "workspace_entries_cut",
/**
 * The root manifest.
 */
manifest: string, } | { "kind": "target_specific",
/**
 * The manifest.
 */
manifest: string, } | { "kind": "source_cut",
/**
 * Its path.
 */
path: string, } | { "kind": "definitions_unread",
/**
 * The crate's directory (`""` = repository root).
 */
dir: string,
/**
 * Files not read.
 */
files: number, };

/**
 * A repository or other source of crates.
 */
export type DependencyRepository = {
/**
 * What kind of source it is.
 */
kind: RepositoryKind,
/**
 * The GitHub repository, when known.
 */
repo: RepoRef | null,
/**
 * The registry name, git URL or path as Cargo names it; null for this repository.
 */
location: string | null,
/**
 * The repository's owner is controlled ("yours").
 */
controlled: boolean,
/**
 * Its crates: changed crates first (by changed lines), then by name.
 */
crates: Array<DependencyCrate>, };

/**
 * Whether the map exists.
 */
export type DependencyStatus = "available" | "not_rust";

/**
 * Response of `GET /api/v1/analyses/{id}/dependencies`.
 */
export type DependencyView = {
/**
 * Whether the head commit has Cargo manifests.
 */
status: DependencyStatus,
/**
 * Owners whose repositories are "yours": the pull request's owner and the Settings list, sorted.
 */
controlled_owners: Array<string>,
/**
 * What the map does not cover.
 */
limits: Array<DependencyLimit>,
/**
 * Changed files in no crate (not checked).
 */
files_outside_crates: number,
/**
 * Whether the LLM's notes exist for this analysis.
 */
notes: NotesState,
/**
 * This repository first, then controlled GitHub repositories, other GitHub repositories, other git URLs,
 * registries, paths and unknown sources, holding the direct dependencies of the changed crates.
 */
repositories: Array<DependencyRepository>, };

/**
 * Public device-code challenge; upstream tokens never cross this boundary.
 */
export type DeviceLink = {
/**
 * Short code the owner enters at the provider.
 */
user_code: string,
/**
 * Fixed provider verification page.
 */
verification_url: string,
/**
 * Deadline of the staged attempt.
 */
expires_at: string, };

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
 * One effect of a decision.
 */
export type Effect = {
/**
 * Id.
 */
id: EffectId,
/**
 * Order within the decision, 1-based.
 */
seq: number,
/**
 * `seq` of the effect this one waits for (a reply after a fix waits for its commit).
 */
after: number | null,
/**
 * Kind.
 */
kind: EffectKind,
/**
 * Executor.
 */
executor: Executor,
/**
 * Parameters.
 */
params: EffectParams,
/**
 * State.
 */
state: EffectState,
/**
 * Current claim.
 */
claim: ClaimInfo | null,
/**
 * Reports, oldest first.
 */
reports: Array<EffectReport>,
/**
 * When a server read found the effect on GitHub (fact badge).
 */
seen_on_github_at: string | null,
/**
 * Last change.
 */
updated_at: string, };

/**
 * Identifier of one effect of a decision (reply, resolve, fix, …).
 */
export type EffectId = string;

/**
 * Kind of an effect; also an executor capability. Open enum.
 */
export type EffectKind = "fix" | "reply" | "resolve" | "pr_comment" | "follow_up_pr" | "upstream_issue" | "todo" | "dismiss_review";

/**
 * Parameters of an effect, set by the server's effect function.
 */
export type EffectParams = {
/**
 * Text to post (reply, PR-level comment, dismissal message).
 */
text: string | null,
/**
 * GitHub thread node id (reply, resolve).
 */
thread_id: string | null,
/**
 * GitHub review node id (dismiss; PR-level replies to a review's items).
 */
review_id: string | null,
/**
 * Title (follow-up PR, issue, TODO).
 */
title: string | null,
/**
 * Existing issue URL (D6 with a link).
 */
url: string | null,
/**
 * Items listed in a follow-up PR (P1), `C<no>`.
 */
item_nos: Array<number>, };

/**
 * A stored agent report ("Agent <token name> reports: …").
 */
export type EffectReport = {
/**
 * Outcome.
 */
outcome: ReportOutcome,
/**
 * Reference (commit SHA, URL, TODO id).
 */
reference: string | null,
/**
 * Message (failure reason).
 */
message: string | null,
/**
 * Reporting token's name.
 */
token_name: string,
/**
 * When.
 */
at: string, };

/**
 * Body of RF4 (`POST …/effects/{id}/reports`); a repeated idempotency key is a no-op.
 */
export type EffectReportRequest = {
/**
 * Outcome.
 */
outcome: ReportOutcome,
/**
 * Reference (required for `done`).
 */
reference: string | null,
/**
 * Message (required for `failed`).
 */
message: string | null,
/**
 * Client-generated key.
 */
idempotency_key: string, };

/**
 * Effect state. Open enum.
 */
export type EffectState = "waiting" | "open" | "claimed" | "done" | "failed" | "cancelled";

/**
 * The effects one decision would get.
 */
export type EffectsPreview = {
/**
 * The item, `C<no>`.
 */
item_no: number,
/**
 * Effects in order.
 */
effects: Array<PlannedEffect>,
/**
 * For a PR-level decision ("Merge now", dismissing stale reviews): a digest of what its plan rests on (the
 * items it follows up and their revisions, or the reviews it dismisses); send it back as the decision's
 * `plan_digest`.
 */
plan_digest?: string, };

/**
 * Body of the effects preview: the decisions the reader is about to submit (the revisions are not checked).
 */
export type EffectsPreviewRequest = {
/**
 * One row per item.
 */
rows: Array<DecisionBatchRow<DecisionKind>>, };

/**
 * A place where the matter is handled already.
 */
export type Elsewhere = {
/**
 * Kind.
 */
kind: ElsewhereKind,
/**
 * URL or `owner/repo#n`.
 */
reference: string,
/**
 * One line on the overlap.
 */
note: string, };

/**
 * Where else the matter is handled. Open enum.
 */
export type ElsewhereKind = "open_pr" | "issue" | "upstream";

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
 * One item the rules left out.
 */
export type ExcludedItem = {
/**
 * Stable key.
 */
key: ItemKey,
/**
 * Kind.
 */
kind: ItemKind,
/**
 * Display id (`PLAT-1203`, `TODO-3fa91c2e`).
 */
id: string,
/**
 * Repository; null for TODOs.
 */
repo: RepoRef | null,
/**
 * Number; null for TODOs.
 */
number: number | null,
/**
 * Link: GitHub, or the MemCan UI for a TODO when configured.
 */
url: string | null,
/**
 * Title, plain text.
 */
title: string,
/**
 * Author login; null for TODOs.
 */
author: string | null,
/**
 * The rule.
 */
reason: ExclusionReason,
/**
 * Display id of the item it was merged into (`merged_into_another` only).
 */
merged_into: string | null,
/**
 * When the snooze ends (`snoozed` only).
 */
snoozed_until?: string, };

/**
 * How many items one rule left out.
 */
export type ExclusionCount = {
/**
 * The rule.
 */
reason: ExclusionReason,
/**
 * Items it left out.
 */
count: number, };

/**
 * The rule that left an item out of the ranking. Clients show an unknown value as a plain "other rule".
 */
export type ExclusionReason = "excluded_author" | "outside_owned_areas" | "draft" | "judged_not_eligible" | "merged_into_another" | "snoozed";

/**
 * The items left out of the ranking, counted per reason. No LLM text; `judged_not_eligible` and
 * `merged_into_another` are LLM decisions, the other reasons deterministic rules.
 */
export type Exclusions = {
/**
 * All left-out items.
 */
total: number,
/**
 * Non-zero counts, in the order of [`ExclusionReason`].
 */
counts: Array<ExclusionCount>,
/**
 * With `?excluded=true` only: the items by reason, then display id, at most [`EXCLUDED_ITEMS_LIMIT`].
 */
items?: Array<ExcludedItem>, };

/**
 * Who executes an effect.
 */
export type Executor = "agent" | "diffpilot_bot";

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
 * Selects items with a deterministic fact.
 */
export type FactFilter = {
/**
 * Stable key for the fact kind and its value where applicable.
 */
key: string,
/**
 * Label shown on the filter chip.
 */
label: string, };

/**
 * A deterministic tag from scan facts.
 */
export type FactTag = {
/**
 * Filter for items with this fact.
 */
filter: FactFilter,
/**
 * Short text.
 */
text: string,
/**
 * Tooltip with the underlying fact.
 */
tip: string, };

/**
 * A decision as an agent reads it (RF1 entries, RF1b).
 */
export type FeedDecision = {
/**
 * Pull request.
 */
pr: PrRef,
/**
 * The item.
 */
item: FeedItem,
/**
 * The decision with its effects; `comment` carries the owner's instructions for the executor.
 */
decision: Decision<ReviewDecisionKind>, };

/**
 * SSE data of RF2.
 */
export type FeedEvent = { "event": "decision_available",
/**
 * Decision.
 */
decision: DecisionId,
/**
 * Pull request.
 */
pr: PrRef,
/**
 * API path to fetch it (RF1b).
 */
fetch: string, } | { "event": "decision_cancelled",
/**
 * Decision.
 */
decision: DecisionId,
/**
 * Pull request.
 */
pr: PrRef, } | { "event": "heartbeat",
/**
 * Server time.
 */
at: string, };

/**
 * The item a feed decision is about.
 */
export type FeedItem = {
/**
 * `C<no>`.
 */
no: number,
/**
 * Source.
 */
kind: CommentItemKind,
/**
 * GitHub link.
 */
url: string | null,
/**
 * Discussed lines.
 */
location: ItemLocation | null,
/**
 * Title.
 */
title: string,
/**
 * Comment text (GitHub; untrusted data).
 */
body: string, };

/**
 * Query of RF1 and RF2.
 */
export type FeedQuery = {
/**
 * Comma-separated `EffectKind` names the agent can execute.
 */
capabilities: string,
/**
 * Only this repository, `owner/name`.
 */
repo: string | null,
/**
 * Opaque cursor of the next page (RF1).
 */
cursor: string | null, };

/**
 * An agent subscribed to the feed (V3 header).
 */
export type FeedSubscriber = {
/**
 * Token name.
 */
token_name: string,
/**
 * Capabilities it declared.
 */
capabilities: Array<EffectKind>,
/**
 * Holds a live stream now.
 */
connected: boolean,
/**
 * Last request or stream activity.
 */
last_seen_at: string,
/**
 * Its push target, if registered.
 */
push: PushTarget | null, };

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
 * How a file changed.
 */
export type FileChangeStatus = "added" | "modified" | "deleted" | "renamed";

/**
 * Group of a file in the file tree.
 */
export type FileGroup = { "kind": "code" } | { "kind": "not_analysed",
/**
 * Section.
 */
category: NotAnalysedCategory,
/**
 * How it was recognised.
 */
badge: ProvenanceBadge,
/**
 * The matching rule or detector description.
 */
rule: string, };

/**
 * One hunk: a run of contiguous changed lines, removed and added together, without context.
 */
export type FileHunk = {
/**
 * Hunk id of the pre-pass.
 */
id: HunkId,
/**
 * Removed lines (base side); null when the hunk removes nothing.
 */
base: LineRange | null,
/**
 * Added lines (head side); null when the hunk adds nothing.
 */
head: LineRange | null,
/**
 * Head line after which the removed lines were (0 = before line 1); set when `head` is null and the file
 * exists at head.
 */
after_line: number | null,
/**
 * The hunk's changed lines by owner, base parts first; together they hold every changed line once.
 */
parts: Array<HunkPart>, };

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
 * The user's GitHub Viewed mark of a file (GraphQL `FileViewedState`).
 */
export type FileViewedState = "viewed" | "unviewed" | "dismissed";

/**
 * Response of `GET /api/v1/analyses/{id}/files`.
 */
export type FilesResponse = {
/**
 * Every file of the diff, in diff (path) order.
 */
files: Array<PrFile>,
/**
 * Whether `PrFile::viewed` was read from GitHub for every file.
 */
viewed_status: ViewedStatus,
/**
 * The pull request's head on GitHub when the Viewed state was read; null when not read. Marking Viewed is
 * refused while it differs from the analysed head.
 */
current_head: CommitSha | null, };

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
 * Size of a fix.
 */
export type FixSize = "XS" | "S" | "M";

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
 * A flow step whose range contains the commented lines.
 */
export type FlowStepRef = {
/**
 * Flow.
 */
flow: FlowId,
/**
 * 1-based position of the flow in the tour.
 */
flow_position: number,
/**
 * Flow title (LLM).
 */
flow_title: string,
/**
 * Step.
 */
step: StepId,
/**
 * 1-based number of the step within its flow.
 */
step_number: number, };

/**
 * Kind of a step (fragment) of a flow (brief §3).
 */
export type FragmentKind = "modified" | "added" | "moved" | "renamed" | "deleted_site" | "context";

/**
 * A blocker gate of claudius `severity` §2. Gate items are never grouped or bulk-decided (RQ4, RQ5). Open enum.
 */
export type Gate = "G-FUNDS" | "G-SECRET" | "G-CRYPTO" | "G-DATA";

/**
 * A gate on an item and who set it.
 */
export type GateMark = {
/**
 * The gate.
 */
gate: Gate,
/**
 * Who set it.
 */
source: GateSource, };

/**
 * Body of the gate mark route: the owner marks or clears a gate (clearing removes the owner's mark only; a gate
 * the assessment names stays).
 */
export type GateMarkRequest = {
/**
 * The gate.
 */
gate: Gate,
/**
 * Mark (true) or clear (false).
 */
marked: boolean,
/**
 * The item revision.
 */
item_revision: number, };

/**
 * Who marked a gate on an item.
 */
export type GateSource = "assessment" | "owner";

/**
 * GitHub item kinds of the list filter `kind`.
 */
export type GithubKind = "pr" | "issue";

/**
 * The last read of the PR's threads, reviews and comments.
 */
export type GithubRead = {
/**
 * Last successful read; null before the first.
 */
read_at: string | null,
/**
 * Threads read.
 */
threads: number,
/**
 * Every page was read (no cap hit).
 */
complete: boolean,
/**
 * Reviews read.
 */
reviews: number,
/**
 * PR-level comments read.
 */
comments: number,
/**
 * The latest attempt failed.
 */
failure: ReadFailure | null,
/**
 * Next timed GitHub read, when polling is the fallback.
 */
next_read_at: string | null, };

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
 * One submitted GitHub review comment.
 */
export type GithubThreadComment = {
/**
 * GitHub login, or `ghost` when the account was deleted.
 */
author: string,
/**
 * The original body, with whitespace and markup.
 */
body: string,
/**
 * The time the comment was created.
 */
created_at: string,
/**
 * Link to the comment on GitHub.
 */
url: string | null, };

/**
 * A submitted GitHub conversation and its exact review card, when it has one.
 */
export type GithubThreadDetail = {
/**
 * GitHub pull request title.
 */
title: string,
/**
 * Repository-relative path.
 */
path: string,
/**
 * Number of the review card for this exact GitHub thread, when known.
 */
item_no: number | null,
/**
 * Whether GitHub returned every page.
 */
complete: boolean,
/**
 * The first comment followed by the submitted replies.
 */
comments: Array<GithubThreadComment>, };

/**
 * GitHub App webhook configuration and delivery status.
 */
export type GithubWebhook = {
/**
 * Whether the webhook secret is configured.
 */
configured: boolean,
/**
 * Configured public HTTPS URL.
 */
webhook_url: string | null,
/**
 * When the last verified delivery was received.
 */
last_delivery_at: string | null,
/**
 * Verified deliveries dropped for untracked repositories, installation-wide, since the last restart, at most 14 days.
 */
dropped_deliveries: number,
/**
 * TCP connections refused at the webhook listener's capacity limit since the last restart.
 */
refused_connections: number, };

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
 * A line of a file at the head commit.
 */
export type HeadLine = {
/**
 * Repository-relative path.
 */
file: string,
/**
 * 1-based line.
 */
line: number, };

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
 * Identifier of a diff hunk from the server's pre-pass: `h1`, `h2`, ….
 */
export type HunkId = string;

/**
 * Changed lines of a hunk with one owner.
 */
export type HunkPart = {
/**
 * Side of the lines.
 */
side: LineSide,
/**
 * The lines.
 */
range: LineRange,
/**
 * Where the lines are explained, or why they are in no flow.
 */
owner: PartOwner,
/**
 * Other steps (of any flow) whose lines include these lines, in Tour order.
 */
also_in: Array<StepId>, };

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
 * An imported decision's source and grounded evidence.
 */
export type ImportedDecision = {
/**
 * GitHub login of the decision's author.
 */
by: string,
/**
 * Numeric GitHub user id when available.
 */
by_id: number | null,
/**
 * When the statement was made on GitHub.
 */
at: string,
/**
 * GitHub URL of the evidence.
 */
evidence_url: string,
/**
 * Verbatim excerpt supporting the decision.
 */
quote: string,
/**
 * GitHub login of the linked account that relayed the member's decision.
 */
relay: string | null, };

/**
 * Decision evidence shown on a comment item.
 */
export type ImportedGithubDecision = {
/**
 * Kind of the decision.
 */
kind: ReviewDecisionKind,
/**
 * GitHub login of the author.
 */
by: string,
/**
 * When the statement was made.
 */
at: string,
/**
 * GitHub URL of the statement.
 */
evidence_url: string, };

/**
 * The current durable reservation and whether this call won the right to create.
 */
export type IssueAttempt = {
/**
 * The server-generated attempt, absent if no reservation exists.
 */
attempt_id: string | null,
/**
 * Only true on the single successful transition to a new reservation.
 */
reserved: boolean, };

/**
 * Reserves the GitHub write for a claimed follow-up issue.
 */
export type IssueAttemptRequest = {
/**
 * The exact previous attempt, only after verifying its writer stopped and GitHub created no issue.
 */
retry_of: string | null, };

/**
 * Body of the item actions Undo, Reopen and Skip: the item revision the reader saw (409 `item_changed`
 * otherwise).
 */
export type ItemActionRequest = {
/**
 * The item revision.
 */
item_revision: number, };

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
 * Facts diffpilot read from GitHub about an item (badges name the check).
 */
export type ItemGithubFacts = {
/**
 * Thread resolved on GitHub; null for items without a thread.
 */
resolved: boolean | null,
/**
 * Who resolved the thread.
 */
resolved_by: string | null,
/**
 * Thread outdated (its lines changed since it was written); null without a thread.
 */
outdated: boolean | null,
/**
 * The bot's own thread status, when it writes one.
 */
bot_status: BotThreadStatus | null,
/**
 * The comment text changed after the latest decision (normalised text hash, spike S13-1 K1).
 */
edited_after_decision: boolean,
/**
 * Replies in the thread after the first comment.
 */
replies: number,
/**
 * The bot's thepastaclaw `dedupe=` marker, when present.
 */
marker: string | null, };

/**
 * Durable CMT number allocation and previous item identities.
 */
export type ItemIds = {
/**
 * Next unused number.
 */
next_id: number,
/**
 * Known point identities.
 */
items: Array<PrDataItem>, };

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
 * The lines an item discusses: at the head for a thread; for a body item naming `path:range`, at the review's
 * commit, which may predate the head.
 */
export type ItemLocation = {
/**
 * Repository-relative path.
 */
path: string,
/**
 * The lines; null for a file-level or outdated thread without a head line.
 */
lines: LineRange | null, };

/**
 * Who marked an item as not a review point and when.
 */
export type ItemMark = {
/**
 * GitHub login of the mark's author.
 */
by: string,
/**
 * When the mark was recorded.
 */
at: string,
/**
 * Number of the primary item, if this mark follows it.
 */
follows?: number, };

/**
 * Response of `GET /api/v1/dashboard/items`: ranked items of the latest snapshot, in rank order.
 */
export type ItemPage = {
/**
 * Unanswered, unhidden mentions on the first page only; empty on cursor pages.
 */
mentions: Array<Mention>,
/**
 * Mention reads omitted in this snapshot, independent of item filters and pagination.
 */
mention_coverage: MentionCoverage,
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
 * Item state (plan §2.7). Open enum.
 */
export type ItemState = "assessing" | "undecided" | "asked_back" | "decided" | "executing" | "done" | "repeat" | "reopened" | "outside";

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
 * Superseded by `WorkItem.judgement`.
 */
inherited: boolean,
/**
 * Keys of items the LLM merged into this one (chosen only from server-suggested pairs).
 */
also_keys: Array<ItemKey>, };

/**
 * Query of RC4.
 */
export type ItemsQuery = {
/**
 * Limit the list to items with an import since the entry preceding this visit.
 */
imported_unseen?: boolean,
/**
 * Only items in this state.
 */
state: ItemState | null,
/**
 * Only items of this kind.
 */
kind: CommentItemKind | null, };

/**
 * Kind of a failed attempt.
 */
export type JobFailureKind = "configuration" | "provider_failed" | "output_not_json" | "credential_unusable" | "checkout_failed" | "timeout" | "internal" | "worker_unavailable" | "worker_lost" | "credential_missing" | "credential_relink" | "model_unavailable" | "upstream_rejected" | "report_version_unsupported" | "invalid_json_schema" | "ask_compaction_unconfirmed";

/**
 * Identifier of a job (one agent run).
 */
export type JobId = string;

/**
 * Subject of a job paid for by the reader.
 */
export type JobSubject = {
/**
 * User-facing subject.
 */
label: string,
/**
 * App-relative URL built by the server.
 */
href: string, };

/**
 * What a job works on (ADR-018); exhaustive matches mark every pull-request-only site.
 */
export type JobTarget = { "type": "pull_request" } & PullRequestTarget | { "type": "dashboard",
/**
 * The refresh whose items are judged.
 */
refresh_id: RefreshId, } | { "type": "pull_request_comments" } & PullRequestCommentsTarget | { "type": "pull_request_ask" } & PullRequestAskTarget;

/**
 * Judgement state and its evidence.
 */
export type Judgement = {
/**
 * State of the item's judgement.
 */
state: JudgementState,
/**
 * When the existing judgement was made.
 */
judged_at: string | null,
/**
 * Which input to the existing judgement changed.
 */
outdated_because: OutdatedBecause | null, };

/**
 * Whether an item has a current judgement or is waiting for one.
 */
export type JudgementState = "current" | "outdated" | "queued" | "judging" | "not_judged" | "capped";

/**
 * Reasons to revoke a local virtual key; none authorizes upstream revoke.
 */
export type KeyRevokeReason = "rotation" | "idle_stop" | "retirement";

/**
 * Decisions of one kind.
 */
export type KindCount = {
/**
 * Kind.
 */
kind: DecisionKind,
/**
 * Count.
 */
count: number, };

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
 * Last-context estimate, separate from cumulative usage and billable job deltas.
 */
export type LastContext = {
/**
 * Input and output context tokens reported for the last completed turn.
 */
tokens: number,
/**
 * Thread generation this estimate describes; missing/stale usage needs recovery.
 */
generation: number, };

/**
 * The latest delivery of a channel.
 */
export type LastDelivery = {
/**
 * Event kind (`decision_available` deliveries of push targets show as null).
 */
event: UserEventKind | null,
/**
 * Outcome.
 */
outcome: DeliveryOutcome,
/**
 * When.
 */
at: string, };

/**
 * An undecided item left out of "Decide the rest".
 */
export type LeftOutItem = {
/**
 * The item, `C<no>`.
 */
item_no: number,
/**
 * Why.
 */
reason: LeftOutReason, };

/**
 * Why an undecided item is not part of "Decide the rest per recommendation". Open enum.
 */
export type LeftOutReason = "gate" | "not_assessed" | "asked_again" | "incomplete";

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
 * Starts device linking without accepting upstream secrets or endpoints.
 */
export type LinkCredential = {
/**
 * ChatGPT-plan linking only.
 */
mode: CredentialMode, };

/**
 * Running and queued jobs of the reader's tenant.
 */
export type LlmActivity = {
/**
 * Jobs currently running.
 */
running: Array<LlmJob>,
/**
 * Jobs awaiting execution or paused.
 */
queued: Array<LlmJob>,
/**
 * End of the reader's own provider pause, if known.
 */
paused_until: string | null,
/**
 * The reader's latest provider call.
 */
last_call: LlmCallOutcome | null, };

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
 * A job as visible to the reader.
 */
export type LlmJob = { "owner": "self",
/**
 * The job's id.
 */
id: JobId,
/**
 * The job kind.
 */
kind: LlmJobKind,
/**
 * The subject and app-relative link, when available.
 */
subject: JobSubject | null,
/**
 * When the job started running.
 */
since: string | null,
/**
 * When the job was queued.
 */
queued_at: string | null,
/**
 * Why the job is waiting.
 */
waits_for: WaitReason | null, } | { "owner": "other",
/**
 * Opaque index local to this response.
 */
id: string,
/**
 * The job kind.
 */
kind: LlmJobKind, };

/**
 * Kind of LLM work as shown in the activity list.
 */
export type LlmJobKind = "analysis" | "flow_analysis" | "ranking" | "comment_assessment" | "ask";

/**
 * Request and response of `PUT /api/v1/settings/llm/job-slots`.
 */
export type LlmJobSlots = {
/**
 * Maximum concurrent jobs using the authenticated payer's plan.
 */
slots: number, };

/**
 * Language of LLM-written texts.
 */
export type LlmLanguage = "en" | "pl" | "ru";

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
 * Allowed models and this payer's effective choice for newly submitted jobs.
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
 * Current choice, falling back to the installation choice or built-in default.
 */
current: ModelChoice,
/**
 * Reasoning effort of the dashboard ranking (`judge_work`; default low). Optional on the wire; absent on
 * older servers.
 */
judge_effort?: ReasoningEffort,
/**
 * Reasoning effort of Ask jobs (default medium). Optional on the wire;
 * absent on older servers.
 */
ask_effort?: ReasoningEffort,
/**
 * Login of the last editor of the model, effort, ranking effort or Ask effort; null without a personal save.
 */
changed_by: string | null,
/**
 * Time of the last save; null without a personal save.
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
 * Host provenance and author facts.
 */
export type Manifest = {
/**
 * owner/repo.
 */
repo: string,
/**
 * PR number.
 */
pr: number,
/**
 * Assessed head.
 */
head: string,
/**
 * Base branch name.
 */
base_ref: string,
/**
 * Frozen base commit for pre-existing checks.
 */
base_sha: string,
/**
 * Author-stated PR intent.
 */
title: string,
/**
 * Author-stated PR description.
 */
body: string,
/**
 * Linked GitHub logins that can relay proposals for member confirmation.
 */
delegates?: Array<string>,
/**
 * PR author's login; only their own text is author context.
 */
pr_author: string,
/**
 * Tenant members' logins, independent of source roles.
 */
maintainers: Array<string>,
/**
 * Known bot logins.
 */
bots: Array<string>, };

/**
 * An internal mark and any subsequent undo, as shown in the decision log.
 */
export type MarkLogEntry = {
/**
 * Mark ID.
 */
id: DecisionId,
/**
 * Item number within the pull request.
 */
item_no: number,
/**
 * Reason given by the owner, if any.
 */
reason: string | null,
/**
 * When the mark ended, if it has ended.
 */
undone_at: string | null,
/**
 * GitHub login of the user who ended it.
 */
undone_by: string | null,
/**
 * GitHub login of the mark's author.
 */
by: string,
/**
 * When the mark was recorded.
 */
at: string,
/**
 * Number of the primary item, if this mark follows it.
 */
follows?: number, };

/**
 * Facts about an actively marked item for classifier review.
 */
export type MarkedItem = {
/**
 * Pull request containing the item.
 */
pr: PrRef,
/**
 * Item number within the pull request.
 */
no: number,
/**
 * Source kind.
 */
kind: CommentItemKind,
/**
 * GitHub login of the comment's author.
 */
author: string,
/**
 * Whether the author is a human or a bot.
 */
author_kind: AuthorKind,
/**
 * Plain-text item title.
 */
title: string,
/**
 * Link to the original item on GitHub, if any.
 */
url: string | null,
/**
 * GitHub login of the mark's author.
 */
by: string,
/**
 * When the mark was recorded.
 */
at: string,
/**
 * Whether the classifier recognised a status comment.
 */
status_comment: boolean, };

/**
 * Tenant-wide list of active marks, newest first.
 */
export type MarkedItems = {
/**
 * Up to 100 marked items.
 */
items: Array<MarkedItem>,
/**
 * Cursor of the next page.
 */
next_cursor: string | null, };

/**
 * Pagination query for the tenant's marked items.
 */
export type MarkedItemsQuery = {
/**
 * Cursor from the previous page.
 */
cursor?: string, };

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
 * A request addressed to a scanned login, outside the ranked list.
 */
export type Mention = {
/**
 * Only a bounded part of this conversation was read.
 */
partly_read: boolean,
/**
 * Stable opaque identity of this post on this item (used by Hide).
 */
id: string,
/**
 * The containing item.
 */
item_key: ItemKey,
/**
 * Pull request or issue.
 */
kind: GithubKind,
/**
 * Repository.
 */
repo: RepoRef,
/**
 * Item number.
 */
number: number,
/**
 * Item title, plain text.
 */
title: string,
/**
 * Mentioning author; a deleted account is shown as `ghost`.
 */
author: string,
/**
 * GitHub Bot type, [bot] suffix or the user's known bots.
 */
is_bot: boolean,
/**
 * When the mentioning post was created.
 */
created_at: string,
/**
 * One line of the mentioning text, plain text only.
 */
text: string,
/**
 * Server-built GitHub link to the mentioning post.
 */
url: string, };

/**
 * Missing mention reads; zero items does not imply that every login was searched.
 */
export type MentionCoverage = {
/**
 * Real candidates not checked because of the read budget, deadline or read failure.
 */
unchecked_items: number,
/**
 * Logins whose mention search was unavailable; fallback covers only mention-only issues.
 */
unchecked_logins: Array<string>,
/**
 * Logins whose PR mention search exceeded its request budget.
 */
timed_out_logins: Array<string>, };

/**
 * Explicit per-method denominators; native verdict and proposal remain separately nullable.
 */
export type MethodComparisonScore = {
/**
 * Assessment adapter.
 */
method: AssessmentMethod,
/**
 * Method rows with an available normalized verdict.
 */
assessed: number,
/**
 * Unaligned points retained in the report.
 */
unmatched: number,
/**
 * Expected but absent rows, including failed methods.
 */
missing: number,
/**
 * Available rows without a normalized verdict.
 */
missing_verdict: number,
/**
 * Available rows without a machine proposal.
 */
missing_proposal: number,
/**
 * Eligible actual owner choices, whether a proposal is present or not.
 */
owner_decided: number,
/**
 * Exact kind and target matches among owner_decided.
 */
owner_agreed: number,
/**
 * Whether this method failed or was cancelled.
 */
failed: boolean, };

/**
 * Mint a fresh, expiring key and revoke the previous key for this container.
 */
export type MintKey = {
/**
 * Worker identity and payer.
 */
container: ContainerId, };

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
 * Response of `GET /api/v1/analyses`: the reader's own analyses (requested by them), the latest per pull
 * request, newest first.
 */
export type MyAnalyses = {
/**
 * The analyses, newest first.
 */
analyses: Array<MyAnalysis>, };

/**
 * Query of `GET /api/v1/analyses`.
 */
export type MyAnalysesQuery = {
/**
 * Most rows returned, 1 to 100; 20 when absent.
 */
limit?: number, };

/**
 * One analysis of the reader's list, as stored.
 */
export type MyAnalysis = {
/**
 * Analysis id.
 */
id: AnalysisId,
/**
 * The pull request.
 */
pr: PrRef,
/**
 * The pull request title as last fetched (untrusted text).
 */
title: string,
/**
 * The stored status.
 */
status: AnalysisStatus,
/**
 * When the analysis was requested.
 */
created_at: string, };

/**
 * Necessity verdict with its reason.
 */
export type Necessity = {
/**
 * Verdict.
 */
verdict: NecessityVerdict,
/**
 * Evidence (in) or reason (out).
 */
reason: string,
/**
 * Better-fitting pull request (`owner/repo#n` or URL) for `out`.
 */
better_pr: string | null, };

/**
 * Whether the fix belongs in this pull request.
 */
export type NecessityVerdict = "in" | "out";

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
 * Why an item has no assessment. Open enum.
 */
export type NotAssessedReason = "daily_limit" | "failed" | "missing" | "not_run";

/**
 * Why a file is never rendered.
 */
export type NotRenderedReason = "too_large" | "generated" | "binary";

/**
 * A note about this code elsewhere (an icon opening its place in a new tab).
 */
export type NoteElsewhere = {
/**
 * Source.
 */
source: NoteSource,
/**
 * Accessible name and tooltip.
 */
label: string,
/**
 * Count shown on the icon.
 */
count: number | null,
/**
 * Where it opens (app route or plug-in home).
 */
url: string, };

/**
 * A source of notes about code elsewhere in diffpilot. Open enum.
 */
export type NoteSource = "flows" | "files" | "dependencies" | "pr_hygiene" | "thepastaclaw";

/**
 * Whether the LLM's crate notes exist.
 */
export type NotesState = "produced" | "pending" | "not_produced";

/**
 * A user's channel; the secret is never returned.
 */
export type NotificationChannel = {
/**
 * Kind.
 */
kind: ChannelKind,
/**
 * ntfy server URL or webhook URL.
 */
url: string,
/**
 * ntfy topic.
 */
topic: string | null,
/**
 * Last 4 characters of the stored secret, or null without one.
 */
secret_last4: string | null,
/**
 * Events it receives.
 */
events: Array<UserEventKind>,
/**
 * On.
 */
enabled: boolean,
/**
 * Delivery state.
 */
delivery: DeliveryStatus, };

/**
 * Response of RN1. The browser notification toggle stays per browser (local storage).
 */
export type NotificationSettings = {
/**
 * Channels, at most one per kind.
 */
channels: Array<NotificationChannel>, };

/**
 * Whether an issue was introduced by the pull request.
 */
export type Origin = "introduced" | "pre_existing" | "unclear";

/**
 * Input that made an existing judgement outdated.
 */
export type OutdatedBecause = "facts_changed" | "prompt_changed";

/**
 * An item resolved or answered on GitHub outside diffpilot (US17).
 */
export type OutsideResolution = {
/**
 * GitHub login that resolved it.
 */
by: string,
/**
 * When diffpilot first read it resolved.
 */
seen_at: string, };

/**
 * A decision against the assessment.
 */
export type Override = { "type": "necessity_gate" } | { "type": "blocker_gate",
/**
 * The gate.
 */
gate: Gate, };

/**
 * Where changed lines are explained, decided by the server from the same coverage that yields Unexplained.
 */
export type PartOwner = { "kind": "step",
/**
 * The flow.
 */
flow_id: FlowId,
/**
 * The step.
 */
step_id: StepId, } | { "kind": "flow",
/**
 * The flow.
 */
flow_id: FlowId, } | { "kind": "incidental",
/**
 * The flow.
 */
flow_id: FlowId,
/**
 * What changed there, as the LLM wrote it.
 */
text: string, } | { "kind": "unexplained",
/**
 * Why the server did not send them to the LLM; absent when the LLM did not explain them.
 */
reason?: UnexplainedReason, } | { "kind": "not_analysed",
/**
 * Section.
 */
category: NotAnalysedCategory,
/**
 * How it was recognised.
 */
badge: ProvenanceBadge,
/**
 * The matching rule.
 */
rule: string, } | { "kind": "context_rule",
/**
 * How it was recognised.
 */
badge: ProvenanceBadge,
/**
 * The matching rule.
 */
rule: string, } | { "kind": "identical_move",
/**
 * Original block at the base commit.
 */
from: FileRange,
/**
 * New block at the head commit.
 */
to: FileRange, } | { "kind": "whitespace_only" };

/**
 * Who decides a part.
 */
export type PartSource = "computed" | "llm" | "not_applicable";

/**
 * How a part's value was reached.
 */
export type PartStatus = "computed" | "judged" | "overridden" | "inherited" | "estimated" | "pending";

/**
 * Identifier of a `verify_comments` pass over the items of one pull request.
 */
export type PassId = string;

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
 * Observed usage of one plan window; absent fields have not been reported upstream.
 */
export type PlanUsage = {
/**
 * Percentage supplied by x-codex-*-used-percent; never estimated from tokens.
 */
used_percent: number | null,
/**
 * Absolute reset time supplied by the backend.
 */
resets_at: string | null, };

/**
 * An effect a decision would get ("What happens next", the confirmation list); not stored.
 */
export type PlannedEffect = {
/**
 * Order, 1-based.
 */
seq: number,
/**
 * `seq` it waits for.
 */
after: number | null,
/**
 * Kind.
 */
kind: EffectKind,
/**
 * Executor.
 */
executor: Executor,
/**
 * Parameters (the reply text that gets posted is in `text`).
 */
params: EffectParams, };

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
 * An external statement of a dashboard plug-in about a pull request (ADR-022): attributed to its source, plain
 * text, never a diffpilot fact or verdict.
 */
export type PluginNote = {
/**
 * Plug-in id, e.g. `pr-hygiene`.
 */
plugin: string,
/**
 * Name shown before the statement, e.g. `PR Hygiene`.
 */
source: string,
/**
 * Evidence: the source's record of the pull request, built by the server.
 */
url: string,
/**
 * When the source produced the data.
 */
as_of: string,
/**
 * Headline (PR Hygiene: the next action).
 */
summary: string | null,
/**
 * Further points (PR Hygiene: the blockers).
 */
details: Array<string>,
/**
 * Labelled values (PR Hygiene: stage, whose move, lateness).
 */
facts: Array<Fact>,
/**
 * Ordered actionable statements for the PR Hygiene disclosure in Comments.
 */
actions?: Array<string>, };

/**
 * Whether a plug-in's statements are shown.
 */
export type PluginState = "waiting" | "current" | "stale";

/**
 * A configured dashboard plug-in: an external source of statements about pull requests (ADR-022).
 */
export type PluginStatus = {
/**
 * Stable id, e.g. `pr-hygiene`.
 */
plugin: string,
/**
 * Name shown before its statements, e.g. `PR Hygiene`.
 */
source: string,
/**
 * The source's home page.
 */
url: string,
/**
 * Whether its statements are shown now.
 */
state: PluginState,
/**
 * When the source produced the data last read; null before the first successful read.
 */
as_of: string | null,
/**
 * The plug-in can hold a pull request back from Ready to merge (PR Hygiene); absent when it never does.
 */
holds_ready_to_merge?: boolean, };

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
 * PR state badge (plan §3.9 B1–B5); first match wins.
 */
export type PrBadge = "blocked_failing" | "github_outdated" | "assessment_needed" | "assessing" | "questions_waiting" | "executing" | "waiting_for_bot_round" | "ready_to_merge";

/**
 * The badge with the facts behind it (tooltip).
 */
export type PrBadgeView = {
/**
 * Badge; null when none applies or inputs are unknown.
 */
badge: PrBadge | null,
/**
 * Facts diffpilot read that select it.
 */
facts: Array<string>,
/**
 * Inputs not known (no B5 while any is listed).
 */
missing: Array<string>, };

/**
 * Counts of one pull request.
 */
export type PrCarryCounts = {
/**
 * Pull request.
 */
pr: PrRef,
/**
 * Counts.
 */
counts: CarryCounts, };

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
 * Self-contained host-supplied review files; GitHub shapes are opaque to contracts.
 */
export type PrData = {
/**
 * manifest.json.
 */
manifest: Manifest,
/**
 * threads.json in the gh script shape.
 */
threads: unknown,
/**
 * review-comments.json in the gh script shape.
 */
review_comments: unknown[],
/**
 * reviews.json in the gh script shape.
 */
reviews: unknown[],
/**
 * comments.json in the gh script shape.
 */
comments: unknown[],
/**
 * Base-branch convention files, relative paths below conventions/.
 */
conventions: { [key in string]: string },
/**
 * ids.json, preserving durable point identities and standing decisions.
 */
ids: ItemIds,
/**
 * scope.json; all other handed-in material is context.
 */
scope: ReviewScope, };

/**
 * One stable point in ids.json.
 */
export type PrDataItem = {
/**
 * CMT identifier.
 */
id: string,
/**
 * Thread node id, review:id:k or comment:id:k.
 */
key: string,
/**
 * Original source kind, inferred from the key when absent.
 */
source_kind: SourceKind | null,
/**
 * Quote only for open and decided items.
 */
quote: string | null,
/**
 * Standing maintainer decision.
 */
decision: StandingDecision | null, };

/**
 * One file of the pull request.
 */
export type PrFile = {
/**
 * Path at the head commit (at the base commit for a deleted file); the path GitHub's Viewed mark uses.
 */
path: string,
/**
 * Path at the base commit of a renamed file.
 */
old_path: string | null,
/**
 * How the file changed.
 */
status: FileChangeStatus,
/**
 * Git treats the file as binary: no hunks.
 */
binary: boolean,
/**
 * Added lines in the diff.
 */
added: number,
/**
 * Removed lines in the diff.
 */
removed: number,
/**
 * Group of the file tree: code, or recognised by a path rule.
 */
group: FileGroup,
/**
 * Why the file is never rendered; null when it is.
 */
not_rendered: NotRenderedReason | null,
/**
 * The user's GitHub Viewed mark; null when it was not read.
 */
viewed: FileViewedState | null,
/**
 * Hunks (runs of changed lines without context) in file order.
 */
hunks: Array<FileHunk>, };

/**
 * Page of pull requests the reader analysed, watched, or opened.
 */
export type PrHistory = {
/**
 * One entry per pull request, newest activity first.
 */
prs: Array<PrHistoryItem>,
/**
 * Cursor of the next page, or null at the end.
 */
next_cursor: string | null, };

/**
 * The reader's most recent analysis of a pull request.
 */
export type PrHistoryAnalysis = {
/**
 * Analysis id.
 */
id: AnalysisId,
/**
 * Stored analysis status.
 */
status: AnalysisStatus,
/**
 * Head the analysis was requested for.
 */
head_sha: CommitSha,
/**
 * When the reader requested the analysis.
 */
requested_at: string,
/**
 * Whether the pull request has moved to a different head.
 */
head_moved: boolean, };

/**
 * The reader's activity on one pull request.
 */
export type PrHistoryItem = {
/**
 * Pull request.
 */
pr: PrRef,
/**
 * Latest stored pull request title.
 */
title: string,
/**
 * Latest activity across all included sources.
 */
last_activity_at: string,
/**
 * When the reader last opened the pull request page.
 */
opened_at?: string,
/**
 * Latest analysis requested by the reader.
 */
analysis?: PrHistoryAnalysis,
/**
 * The reader's watch, including an ended watch.
 */
watch?: PrHistoryWatch, };

/**
 * Query of `GET /api/v1/me/prs`.
 */
export type PrHistoryQuery = {
/**
 * Most rows to return.
 */
limit?: number,
/**
 * Opaque cursor issued by the previous page.
 */
cursor?: string, };

/**
 * The reader's watch of a pull request.
 */
export type PrHistoryWatch = {
/**
 * Current or ended watch state.
 */
state: WatchState,
/**
 * When the watch began.
 */
since: string,
/**
 * When the watch ended, if it did.
 */
ended_at: string | null, };

/**
 * Response of RC1: the PR page.
 */
export type PrPage = {
/**
 * Pull request.
 */
pr: PrRef,
/**
 * Title.
 */
title: string,
/**
 * Author's GitHub login (`ghost` for a deleted account).
 */
author: string,
/**
 * GitHub link.
 */
url: string,
/**
 * GitHub state (`OPEN`, `CLOSED`, `MERGED`); empty while GitHub was never read (`comments.read.failure`).
 */
state: string,
/**
 * Draft.
 */
draft: boolean,
/**
 * Base branch; empty while GitHub was never read.
 */
base: string,
/**
 * The PR whose branch is the base (stacked PR).
 */
stack_parent: PrRef | null,
/**
 * Head.
 */
head: CommitSha,
/**
 * CI of the head; null when not read.
 */
ci: CiStatus | null,
/**
 * Plug-in statements (PR Hygiene, thepastaclaw), attributed.
 */
plugin_notes: Array<PluginNote>,
/**
 * The reader's watch; null when not watching.
 */
watch: WatchedPr | null,
/**
 * State badge.
 */
badge: PrBadgeView,
/**
 * The latest analysis (the progress content); null without one.
 */
analysis: AnalysisSummary | null,
/**
 * Process PR comments.
 */
comments: CommentsSummary, };

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
 * Durable first-Ask preflight state; pending is never interpreted as completed.
 */
export type PreflightState = "not_needed" | "pending" | "done";

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
 * Machine-readable proposed action; target semantics are validated by the server.
 */
export type ProposedDecision = {
/**
 * Decision kind.
 */
kind: ProposedDecisionKind,
/**
 * Fix commit, if supplied.
 */
fixed_in: string | null,
/**
 * Target PR number.
 */
target_pr: number | null,
/**
 * Target issue or PR URL.
 */
target_url: string | null,
/**
 * Follow-up title.
 */
title: string | null, };

/**
 * Proposed decision kind; independent of PR-level decisions.
 */
export type ProposedDecisionKind = "resolve" | "fix_here" | "reject" | "handled_in_pr" | "follow_up_pr" | "upstream_issue" | "todo";

/**
 * A deterministic check that a badge names (brief §1, §3). No badge means LLM analysis.
 */
export type ProvenanceBadge = "text_identical" | "classified_by_path" | "classified_by_line_rule" | "binary_content";

/**
 * Private comment Ask, scoped by `JobSpec::payer`; no assessment quota or analytic archive.
 */
export type PullRequestAskTarget = {
/**
 * Stable database key of the pull request.
 */
pull_request_id: number,
/**
 * Frozen checkout for the question and its evidence.
 */
repo: RepoCheckout, };

/**
 * What a `verify_comments` job works on (ADR-023, plan §4.2): the head checkout and the read-only PR-scoped token,
 * no session, no MemCan.
 */
export type PullRequestCommentsTarget = {
/**
 * Stable database key of the pull request.
 */
pull_request_id: number,
/**
 * The repository to check out (head).
 */
repo: RepoCheckout,
/**
 * The assessment pass.
 */
pass_id: PassId, };

/**
 * The pull request an analysis job works on.
 */
export type PullRequestTarget = {
/**
 * The analysis the job belongs to.
 */
analysis_id: AnalysisId,
/**
 * Stable database key of the pull request; root of its analytic and private working archives.
 */
pull_request_id: number,
/**
 * The repository to check out.
 */
repo: RepoCheckout, };

/**
 * An agent token's push target; the secret is never returned.
 */
export type PushTarget = {
/**
 * Kind.
 */
kind: ChannelKind,
/**
 * ntfy server URL or webhook URL; null in the decision log for another user's token.
 */
url: string | null,
/**
 * ntfy topic; null in the decision log for another user's token.
 */
topic: string | null,
/**
 * Last 4 characters of the stored secret; null in the decision log for another user's token.
 */
secret_last4: string | null,
/**
 * Delivery state (in the decision log for another user's token without `last_error`).
 */
delivery: DeliveryStatus, };

/**
 * Body of RN2 (`PUT …/channels/{kind}`); 200 `NotificationChannel`.
 */
export type PutChannelRequest = {
/**
 * URL (`http`/`https` only; 422 `scheme_not_allowed`).
 */
url: string,
/**
 * ntfy topic.
 */
topic: string | null,
/**
 * The secret: ntfy access token or webhook `Authorization` value.
 */
secret: SecretChange,
/**
 * Events.
 */
events: Array<UserEventKind>,
/**
 * On.
 */
enabled: boolean, };

/**
 * Body of RF5 (`PUT /api/v1/decision-feed/push-target`); 200 `PushTarget`.
 */
export type PutPushTargetRequest = {
/**
 * Kind.
 */
kind: ChannelKind,
/**
 * URL (`http`/`https` only).
 */
url: string,
/**
 * ntfy topic.
 */
topic: string | null,
/**
 * The secret: ntfy access token or webhook `Authorization` value.
 */
secret: SecretChange, };

/**
 * Identifier of an Ask question.
 */
export type QuestionId = string;

/**
 * One question in the queue: a single item or a group.
 */
export type QueueEntry = {
/**
 * Items, `C<no>`; two or more for a group.
 */
item_nos: Array<number>,
/**
 * Reason.
 */
reason: QueueReason,
/**
 * The reader skipped it at its current revision and it belongs in Skipped. Absent when active.
 */
skipped?: boolean, };

/**
 * Why an entry is at its place in the queue (plan §2.4 order). Open enum.
 */
export type QueueReason = "pr_question" | "asked_again" | "answered" | "gate" | "blocking" | "single" | "group";

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
 * A limit on how many items can be sent for ranking.
 */
export type RankingLimit = "per_refresh" | "daily";

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
 * Usage of the reader's dashboard ranking caps.
 */
export type RankingUsage = {
/**
 * Items sent today after the last reset.
 */
sent_today: number,
/**
 * Items allowed per local calendar day.
 */
daily_cap: number,
/**
 * Items allowed per scan ranking.
 */
per_refresh_cap: number,
/**
 * Next local midnight in the reader's configured time zone.
 */
resets_at: string,
/**
 * Items in the current snapshot left unjudged at a cap.
 */
left_provisional: number, };

/**
 * Why the last GitHub read failed. Open enum.
 */
export type ReadFailure = "rate_limited" | "unavailable" | "sign_in_required";

/**
 * A pull request of the user ready to merge: into the default branch or a release branch, not a draft, write
 * access, every check passed except ignored ones, nothing left to answer and no ready label (plan K17).
 */
export type ReadyToMergeItem = {
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
 * Deterministic tags from scan facts (CI state; checks not passing that are not required).
 */
tags: Array<FactTag>,
/**
 * Size mark of its full diff (display only); absent when the totals are unknown.
 */
change_size?: ChangeSizeMark, };

/**
 * Why a repeat was asked again (RQ3). Open enum.
 */
export type ReaskReason = "bot_edited" | "code_changed";

/**
 * Reasoning effort of the model.
 */
export type ReasoningEffort = "low" | "medium" | "high";

/**
 * The LLM's recommendation; on the card under the evidence, the recommended option first (RQ7).
 */
export type Recommendation = {
/**
 * Recommended decision.
 */
kind: ReviewDecisionKind,
/**
 * Why.
 */
reason: string,
/**
 * Target the decision needs (D4–D7).
 */
target: DecisionTarget | null,
/**
 * Short class of the reason; groups share kind and class or target (plan §2.4).
 */
reason_class: string, };

/**
 * Attributed prior decision; server verifies authority and grounding before treating it as a fact.
 */
export type RecordedDecision = {
/**
 * Recorded decision.
 */
kind: RecordedDecisionKind,
/**
 * GitHub author login.
 */
by: string,
/**
 * Named maintainer for an explicit trusted relay; never inferred.
 */
on_behalf_of: string | null,
/**
 * Link to the decision's comment.
 */
comment_url: string,
/**
 * Verbatim decision quote, at most 600 characters.
 */
quote: string,
/**
 * PR target, when supplied.
 */
target_pr: number | null,
/**
 * URL target, when supplied.
 */
target_url: string | null, };

/**
 * Recorded decision kind, including deferral and withdrawal.
 */
export type RecordedDecisionKind = "deferred" | "withdrawn" | ProposedDecisionKind;

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
 * The cap that left items provisional, when a cap was reached.
 */
left_provisional_by?: RankingLimit,
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
export type RefreshTrigger = "manual" | "scheduled" | "automatic";

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
 * Safe relink reason; never contains a token, upstream response or account identifier.
 */
export type RelinkReason = "terminal" | "quarantined";

/**
 * Where the runner fetches the repository from.
 */
export type RepoCheckout = {
/**
 * Clone URL (any scheme; `file://` in development).
 */
clone_url: string,
/**
 * Base commit, fetched for diffs.
 */
base_sha: CommitSha,
/**
 * Head commit, checked out.
 */
head_sha: CommitSha, };

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
 * The upstream assessment, before mapping into the API's validity and necessity.
 */
export type ReportAssessment = "valid" | "already_fixed" | "out_of_scope" | "wrong";

/**
 * An upstream gate; only the four blocker gates map into stored gate items.
 */
export type ReportGate = {
/**
 * Upstream gate name, allowing gates beyond the four stored ones.
 */
gate: string,
/**
 * Gate evidence, labelled LLM.
 */
evidence: string, };

/**
 * Required host-supplied provenance; other metadata is ignored.
 */
export type ReportMetadata = {
/**
 * Must be comment_check.
 */
report_type: string,
/**
 * Host-supplied is the only accepted mode.
 */
input_mode: string,
/**
 * GitHub PR number.
 */
pr_number: number,
/**
 * Commit assessed; a differing head is labelled rather than discarded.
 */
commit: string, };

/**
 * Outcome an agent reports for an effect.
 */
export type ReportOutcome = "done" | "failed" | "released";

/**
 * Fields consumed from one upstream assessment; unknown fields, including severity floats, are discarded.
 */
export type ReportRow = {
/**
 * Direct canonical row ID for a duplicate from another source.
 */
merged_into: string | null,
/**
 * Upstream CMT identifier.
 */
id: string,
/**
 * Source author.
 */
reviewer: string,
/**
 * Source role.
 */
source_kind: SourceKind,
/**
 * Source URL, checked against the host scope.
 */
comment_url: string,
/**
 * Required for an inline row.
 */
thread_id: string | null,
/**
 * Required for a body or PR-comment point; server checks a distinct verbatim span.
 */
source_quote: string | null,
/**
 * Problem description.
 */
description: string,
/**
 * Validity assessment; absent on context rows.
 */
assessment: ReportAssessment | null,
/**
 * Reasoning text.
 */
reasoning: string,
/**
 * Evidence text, labelled LLM.
 */
evidence: string | null,
/**
 * Human-readable proposed action.
 */
proposed_action: string,
/**
 * Machine decision, required on assessed host rows.
 */
proposed_decision: ProposedDecision | null,
/**
 * Earlier attributed decision.
 */
recorded_decision: RecordedDecision | null,
/**
 * Optional location in file:lines notation.
 */
location: string | null,
/**
 * Draft reply, passed through the server's wording guard.
 */
proposed_reply: string | null,
/**
 * Whether the problem exists on the base.
 */
pre_existing: boolean | null,
/**
 * Category slug.
 */
category: string | null,
/**
 * Fix size.
 */
fix_size: FixSize | null,
/**
 * Named gates.
 */
gates: Array<ReportGate>,
/**
 * Threads grouped with this row, intersected with host scope.
 */
merged_thread_ids: Array<string>,
/**
 * Additional source links, subject to the same-source and author rules.
 */
merged_urls: Array<string>, };

/**
 * Whether the GitHub App updates this tracked repository.
 */
export type RepositoryInstallation = {
/**
 * Repository.
 */
repo: RepoRef,
/**
 * Whether it gets live webhook updates.
 */
live: boolean, };

/**
 * Live webhook coverage of repositories tracked by the reader's tenant.
 */
export type RepositoryInstallations = {
/**
 * Repositories tracked through owners to list, repository rules, watches, analyses or current snapshots.
 */
repositories: Array<RepositoryInstallation>, };

/**
 * Kind of a crate source.
 */
export type RepositoryKind = "this" | "github" | "other_git" | "registry" | "path" | "unknown";

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
branch_patterns: Array<string>,
/**
 * Checks and statuses that never make CI red or pending (whole name, case-insensitive, `*` any text); empty =
 * the built-ins.
 */
ignored_checks: Array<string>, };

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
 * A review decision (D1–D7 and P1–P4) eligible for external use.
 */
export type ReviewDecisionKind = "resolve" | "fix_here" | "reject" | "handled_in_pr" | "follow_up_pr" | "follow_up_issue" | "upstream_issue" | "todo" | "merge_with_follow_ups" | "another_round" | "dismiss_stale_reviews" | "keep_reviews";

/**
 * The user's diffpilot analysis of a pull request item.
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
 * What a pass may assess (10 points/threads and 5 new sources, with continuations).
 */
export type ReviewScope = {
/**
 * Open thread node ids.
 */
threads: Array<string>,
/**
 * Versioned source URLs.
 */
sources: Array<string>, };

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
 * Revoke local keys without logging out or revoking an upstream credential.
 */
export type RevokeKeys = {
/**
 * Worker whose keys become unusable.
 */
container: ContainerId,
/**
 * Local-only revocation reason.
 */
reason: KeyRevokeReason, };

/**
 * Risk at system scale, rated by the LLM (no numbers, brief §1).
 */
export type RiskBand = "low" | "medium" | "high";

/**
 * A bot round: a thepastaclaw review with `phase=final sha=<head>` (spike S13-1 R1).
 */
export type Round = {
/**
 * 1-based.
 */
number: number,
/**
 * The round's review (GitHub link).
 */
url: string,
/**
 * Head reviewed.
 */
head: CommitSha,
/**
 * Submitted.
 */
finished_at: string,
/**
 * The bot's review state as GitHub reports it (`CHANGES_REQUESTED`, `APPROVED`, `DISMISSED`, …).
 */
verdict: string,
/**
 * Items first seen in this round.
 */
new: number,
/**
 * Items still open at its end.
 */
still_open: number,
/**
 * Repeats carried.
 */
carried: number,
/**
 * Repeats asked again.
 */
asked_again: number,
/**
 * Undecided items.
 */
undecided: number,
/**
 * Decisions by kind.
 */
decisions: Array<KindCount>, };

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
 * What a save does with a channel's or push target's stored secret (an absent field keeps it).
 */
export type SecretChange = { "action": "keep" } | { "action": "replace",
/**
 * The new secret.
 */
value: string, } | { "action": "remove" };

/**
 * How a private working thread was seeded.
 */
export type SessionSeed = "fork" | "context_pack";

/**
 * Body of `PUT /api/v1/settings/llm/ask-effort`.
 */
export type SetAskEffortRequest = {
/**
 * Reasoning effort of Ask jobs.
 */
reasoning_effort: ReasoningEffort, };

/**
 * Body of `PUT /api/v1/settings/controlled-owners`.
 */
export type SetControlledOwnersRequest = {
/**
 * GitHub owner logins (users or organisations), at most [`SetControlledOwnersRequest::MAX_OWNERS`].
 */
owners: Array<string>, };

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
 * Body of `PUT /api/v1/analyses/{id}/files/viewed`.
 */
export type SetViewedRequest = {
/**
 * `PrFile::path` of a file of the analysis.
 */
path: string,
/**
 * True marks the file as viewed on GitHub, false removes the mark.
 */
viewed: boolean, };

/**
 * Response of `GET /api/v1/settings`.
 */
export type Settings = {
/**
 * The payer's effective model and effort selection; absent on older servers.
 */
llm_selection?: LlmSelection,
/**
 * The payer's concurrent job limit; absent on older servers.
 */
llm_job_slots?: number,
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
repos: Array<RepoRef>,
/**
 * Installation-wide controlled GitHub owners (Dependencies view, ADR-021); absent on older servers.
 */
controlled_owners?: ControlledOwners, };

/**
 * Size mark of a pull request: a fixed table of changed lines (additions + deletions), plan §C.2 K16.
 */
export type SizeMark = "tiny" | "small" | "medium" | "large" | "huge";

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
 * One row of the Slack names table: a GitHub login and the text copied for it.
 */
export type SlackName = {
/**
 * GitHub login (case-insensitive, unique in the table).
 */
login: string,
/**
 * Text written instead of `@login`, as typed (1–80 characters, no control characters), e.g. `@jan.kowalski`.
 */
slack: string, };

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
 * Items left out of the ranking (`exclusions.total`).
 */
excluded: number,
/**
 * The user's pull requests waiting on other people.
 */
awaiting: Array<AwaitingItem>,
/**
 * The user's pull requests ready to merge at the scan (plan K17), by display id; also ranked. A pull request a
 * current plug-in statement holds back (ADR-022) is left out and stays ranked with its note.
 */
ready_to_merge: Array<ReadyToMergeItem>,
/**
 * The user's pull requests Ready to merge could list (into the default branch or a release branch) whose merge
 * state GitHub had not computed by the scan (read again by the next refresh); null when the snapshot did not
 * record merge states (stored before they were).
 */
merge_state_pending: number | null,
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
checked: KindCounts,
/**
 * Why the rules left items out, computed with the ranking; absent only from a response without it (a server
 * older than this field), which the web shows as not recorded.
 */
exclusions?: Exclusions, };

/**
 * Body of `PUT /api/v1/dashboard/items/{key}/snooze` (R12).
 */
export type SnoozeRequest = {
/**
 * Days from now, 1 to [`MAX_SNOOZE_DAYS`].
 */
days: number, };

/**
 * Whether a source was checked.
 */
export type SourceCheck = "checked" | "not_checked";

/**
 * A row's source role in the upstream report.
 */
export type SourceKind = "inline" | "review_body" | "pr_comment" | "reply" | "status";

/**
 * Host-assigned source role, independent of author membership.
 */
export type SourceRole = "context" | "ignored" | "source";

/**
 * Position and dependent pull requests of an item in a stack.
 */
export type Stack = {
/**
 * Position of the pull request.
 */
position: StackPosition,
/**
 * Number of pull requests in the stack.
 */
size: number,
/**
 * Numbers of open pull requests above this one, in stack order.
 */
dependents: Array<number>, };

/**
 * Position of a pull request in a stack.
 */
export type StackPosition = "bottom" | "middle" | "top";

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
 * Standing choice offered to the skill, without invented reply-chain provenance.
 */
export type StandingDecision = {
/**
 * Stored decision kind.
 */
kind: RecordedDecisionKind,
/**
 * Existing PR target.
 */
target_pr: number | null,
/**
 * Existing issue or PR target.
 */
target_url: string | null,
/**
 * Follow-up or TODO title.
 */
title: string | null, };

/**
 * Optional body of `POST /api/v1/dashboard/refreshes`.
 */
export type StartRefreshRequest = {
/**
 * Request new judgements for outdated items within the refresh's caps.
 */
rejudge_outdated: boolean, };

/**
 * Dashboard item counts for the judgement and repository filters.
 */
export type StatusCounts = {
/**
 * Items judged against older facts or an older prompt, including those being re-judged.
 */
outdated: number,
/**
 * Items never judged, including queued and capped items.
 */
not_judged: number,
/**
 * Items in repositories without live webhook updates.
 */
not_live: number,
/**
 * Items excluded by the reader's rules.
 */
excluded: number, };

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
 * Doc comments and attributes directly above the step’s first item, as found by the technology plugin.
 */
leading_context?: LineRange,
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
 * Query of `GET /api/v1/dashboard`.
 */
export type SummaryQuery = {
/**
 * List the excluded items (`snapshot.exclusions.items`); without it only their counts are sent.
 */
excluded?: boolean, };

/**
 * Immutable thread policy; resend its full configuration on start, fork and resume.
 */
export type ThreadKind = "analytic" | "ask" | "verify" | "judge";

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
export type TokenScope = "read" | "review_loop_execute";

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
 * Where a fact's type is defined.
 */
export type TypeDefinition = { "kind": "located",
/**
 * The definition line.
 */
at: HeadLine, } | { "kind": "not_located" } | { "kind": "not_searched" } | { "kind": "other_repository" };

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
 * Explicit user-authorized upstream unlink; distinct from retiring a local virtual key.
 */
export type UnlinkCredential = {
/**
 * User whose credential and local keys are unlinked.
 */
user: UserId, };

/**
 * Body of `PATCH /api/v1/analyses/{id}/pending-review/comments/{comment_id}`.
 */
export type UpdateCommentRequest = {
/**
 * New text.
 */
body: string, };

/**
 * Token usage.
 */
export type Usage = {
/**
 * Input tokens, cached included.
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
 * Model requests of the call, counted from the provider's events (one per batch of tool calls, and the
 * answer); null when not counted.
 */
requests: number | null, };

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
last_7_days: UsageTotals | null,
/**
 * From local midnight on the first of the month through now; omitted without reports.
 */
this_month?: UsageTotals, };

/**
 * A per-user event (RW3 SSE data, push channels).
 */
export type UserEvent = {
/**
 * Id.
 */
id: UserEventId,
/**
 * Kind.
 */
kind: UserEventKind,
/**
 * Pull request; null for `test`.
 */
pr: PrRef | null,
/**
 * Questions (questions_ready) or decisions (round_executed).
 */
count: number,
/**
 * Plain text, e.g. "acme/node-client #482: 5 questions ready".
 */
text: string,
/**
 * The path of the PR's Comments page on this instance (`/pr/<owner>/<name>/<number>/comments`).
 */
url: string | null,
/**
 * When.
 */
at: string, };

/**
 * Identifier of a per-user event (questions ready, round executed, test).
 */
export type UserEventId = string;

/**
 * Kind of a per-user event; also what a channel subscribes to.
 */
export type UserEventKind = "questions_ready" | "round_executed" | "test";

/**
 * Feedback values a reviewer can choose.
 */
export type UserFeedbackValue = "correct" | "wrong" | "irrelevant" | "overrated" | "underrated";

/**
 * Stable authenticated user id, also the payer key.
 */
export type UserId = string;

/**
 * SSE data of RW3; the SSE `event` field is the `event` tag.
 */
export type UserStreamEvent = { "event": "user_event",
/**
 * The event.
 */
user_event: UserEvent, } | { "event": "llm_activity",
/**
 * Running and queued jobs visible to the reader.
 */
activity: LlmActivity, } | { "event": "heartbeat",
/**
 * Server time of sending.
 */
at: string, };

/**
 * The LLM's validity reading ("The LLM says: …"). Open enum.
 */
export type Validity = "valid" | "already_fixed" | "partly_valid" | "invalid";

/**
 * How completely the Viewed state was read.
 */
export type ViewedStatus = "checked" | "partial" | "not_checked";

/**
 * What must happen before the reader's job can run.
 */
export type WaitReason = { "kind": "runner" } | { "kind": "usage_limit",
/**
 * When execution can resume.
 */
until: string, } | { "kind": "analysis_running" };

/**
 * Response of RW2.
 */
export type WatchList = {
/**
 * Watches, newest first.
 */
watches: Array<WatchedPr>,
/**
 * Most watched PRs per user.
 */
limit: number,
/**
 * Assessment passes charged to the reader in the last 24 hours (automatic and question passes).
 */
passes_today: number,
/**
 * Passes per user and 24 hours; automatic passes stop at it.
 */
daily_limit: number, };

/**
 * State of a watch. Open enum.
 */
export type WatchState = "active" | "waiting_for_bot_round" | "job_running" | "limit_reached" | "paused" | "ended";

/**
 * A watched pull request (RW2, V6).
 */
export type WatchedPr = {
/**
 * Pull request.
 */
pr: PrRef,
/**
 * Title.
 */
title: string,
/**
 * Watching since.
 */
since: string,
/**
 * Last probe.
 */
last_probe_at: string | null,
/**
 * Latest automatic assessment.
 */
last_auto_pass: AutoPass | null,
/**
 * State.
 */
state: WatchState,
/**
 * When the watch ended (merged or closed).
 */
ended_at: string | null,
/**
 * State badge.
 */
badge: PrBadgeView, };

/**
 * JSON body of a generic webhook delivery (`POST`; plan §5.5).
 */
export type WebhookPayload = {
/**
 * Payload version, 1.
 */
v: number,
/**
 * Event kind.
 */
event: UserEventKind,
/**
 * Plain text, e.g. "acme/node-client #482: 5 questions ready".
 */
text: string,
/**
 * The PR's Comments page.
 */
url: string | null,
/**
 * Pull request, `owner/name#number`.
 */
pr: string | null,
/**
 * When.
 */
at: string, };

/**
 * One ranked item.
 */
export type WorkItem = {
/**
 * Judgement state when this item can be judged.
 */
judgement?: Judgement,
/**
 * Whether GitHub App webhooks update this repository.
 */
live?: boolean,
/**
 * Position of this pull request within a stack.
 */
stack?: Stack,
/**
 * The pull request's base branch when it matches a repository release pattern, configured or otherwise built-in.
 */
release_base?: string,
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
 * Size mark of a pull request's full diff (display only); absent for issues, TODOs and unknown totals.
 */
change_size?: ChangeSizeMark,
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
 * The user's diffpilot analysis of this pull request, read at request time; null when none.
 */
review: ReviewLink | null,
/**
 * Review comments diffpilot read for this pull request (Comments), counted at request time; absent before
 * diffpilot's first successful read of them.
 */
comments?: CommentTally,
/**
 * GitHub decisions imported since the reader last entered Comments on this PR.
 */
imported_unseen?: number,
/**
 * The user watches this pull request (RW1), read at request time; with `comments` absent its comments were
 * not read yet.
 */
watched?: boolean,
/**
 * Current statements of dashboard plug-ins about this pull request (ADR-022); absent when none.
 */
plugin_notes?: Array<PluginNote>, };

/**
 * Managed worker identity and current lifecycle state.
 */
export type WorkerStatus = {
/**
 * Worker identity derived from managed labels.
 */
container: ContainerId,
/**
 * Whether it is currently running.
 */
running: boolean, };

/**
 * Fixed workspace shape; paths never come from model output.
 */
export type Workspace = "pr" | "judge";

/**
 * The commit and time a comment was written on, as GitHub records them.
 */
export type Written = {
/**
 * The bot round whose own review raised it (then `commit` and `at` are that round's head and end); absent for
 * any other comment.
 */
round?: number,
/**
 * The commit its review was made on; absent for a PR comment or a review GitHub did not list.
 */
commit?: CommitSha,
/**
 * When it was written (the review's submission, the comment's creation).
 */
at: string, };
