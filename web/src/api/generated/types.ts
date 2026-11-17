// This file is generated from crates/contracts by the test generated_artifacts_are_current (UPDATE_GENERATED=1 rewrites it). Do not edit.

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
question: AskQuestion, };

/**
 * Identifier of an analysis (one per pull request, head commit and LLM language).
 */
export type AnalysisId = string;

/**
 * State of an analysis (brief §4.2).
 */
export type AnalysisState = { "state": "planning" } | { "state": "running",
/**
 * Flows analysed.
 */
done: number,
/**
 * Flows in total.
 */
total: number, } | { "state": "paused",
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
total: number | null, } | { "state": "failed",
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
 * The consequence, question or note; null for a behaviour annotation.
 */
text: string | null,
/**
 * Evidence lines.
 */
evidence: Array<Evidence>, };

/**
 * Identifier of an annotation, assigned by the server: `"<step>.a<n>"`, e.g. `f2.3.a1`.
 */
export type AnnotationId = string;

/**
 * Type of an annotation inside a step.
 */
export type AnnotationType = "behaviour" | "consequence" | "question" | "note";

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
export type ApiErrorCode = "unauthenticated" | "login_not_allowed" | "csrf_rejected" | "not_found" | "invalid_request" | "not_a_pr_url" | "private_repository" | "app_not_installed" | "analysis_not_finished" | "conflict" | "pr_head_changed" | "position_rejected" | "comment_published" | "range_too_large" | "not_rendered" | "github_unavailable" | "internal" | "unavailable";

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
 * The answer; suspicions phrased as questions to check.
 */
answer: string,
/**
 * Lines read that support the answer.
 */
evidence: Array<Evidence>, };

/**
 * A question and its LLM answer, pinned to the step.
 */
export type AskQuestion = {
/**
 * Question id.
 */
id: QuestionId,
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
headline: string,
/**
 * Risk band from the severity skill; never its numeric scores.
 */
risk: RiskBand,
/**
 * Reason for the band, also for low.
 */
reason: string,
/**
 * What was read and that nothing was executed.
 */
basis_note: string,
/**
 * What was not checked.
 */
not_checked: string,
/**
 * Introduced by this pull request or pre-existing.
 */
origin: Origin,
/**
 * Whether it blocks this pull request or is a follow-up.
 */
blocking: Blocking,
/**
 * The smallest correction, or null.
 */
smallest_correction: string | null, };

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
export type Blocking = "blocks_this_pr" | "follow_up";

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
 * Overall CI state.
 */
export type CiState = "passing" | "failing" | "pending" | "none" | "not_checked";

/**
 * CI status of the head commit.
 */
export type CiStatus = {
/**
 * Overall state.
 */
state: CiState,
/**
 * Number of checks.
 */
total: number,
/**
 * Failing checks.
 */
failing: Array<CiCheck>, };

/**
 * Classification of a flow (brief §3, closed list).
 */
export type Classification = "behavioural" | "api_contract" | "new_code" | "refactor_pure" | "move" | "rename" | "test" | "docs_comments" | "generated_mechanical" | "config_build" | "dependency" | "other";

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
 * Lines removed from tests.
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
total_lines: number, };

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
 * A stored feedback entry.
 */
export type Feedback = {
/**
 * Feedback id.
 */
id: FeedbackId,
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
 * One flow.
 */
export type Flow = {
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
 * base side.
 */
changed: Array<number>,
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
 * The rule (path glob or line regex).
 */
rule: string, };

/**
 * A built-in "not analysed" rule.
 */
export type NotAnalysedRule = {
/**
 * Path glob or line regex.
 */
kind: RuleKind,
/**
 * The glob or regex.
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
 * The pull request now.
 */
pr: PrSummary,
/**
 * False when the GitHub App is not installed on the repository: analysis works, comments do not.
 */
commenting_available: boolean,
/**
 * The latest analysis of this pull request in the user's language, if any.
 */
analysis: AnalysisSummary | null,
/**
 * The analysis is for an older head than the pull request now has.
 */
head_moved: boolean, };

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
export type RuleKind = "path_glob" | "line_regex";

/**
 * Body of `PUT /api/v1/settings/language`.
 */
export type SetLanguageRequest = {
/**
 * Language of LLM texts.
 */
language: LlmLanguage, };

/**
 * Response of `GET /api/v1/settings`.
 */
export type Settings = {
/**
 * Outcome of the most recent LLM call; null before the first call. No on-demand check.
 */
llm_status: LlmCallOutcome | null,
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
 * Whether a source was checked.
 */
export type SourceCheck = "checked" | "not_checked";

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
 * LLM status of an existing GitHub review thread.
 */
export type ThreadLlmStatus = "still_valid" | "stale" | "already_addressed";

/**
 * How completely the pull request's review threads were read.
 */
export type ThreadsStatus = "checked" | "partial" | "not_checked";

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
 * Feedback values a reviewer can choose.
 */
export type UserFeedbackValue = "correct" | "wrong" | "irrelevant" | "overrated" | "underrated";
