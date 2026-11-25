//! Every HTTP route: method, path (axum syntax), authentication and listener. The server router and the generated
//! `web/src/api/generated/routes.ts` are built from [`ROUTES`].

use crate::api::access_tokens::TokenScope;

/// HTTP method.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HttpMethod {
    /// GET.
    Get,
    /// POST.
    Post,
    /// PUT.
    Put,
    /// PATCH.
    Patch,
    /// DELETE.
    Delete,
}

impl HttpMethod {
    /// Upper-case method name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }
}

/// Authentication a route requires.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RouteAuth {
    /// None.
    Public,
    /// Session cookie.
    Session,
    /// Session cookie and the `X-CSRF-Token` header.
    SessionCsrf,
    /// GitHub App webhook signature.
    GithubSignature,
    /// `Authorization: Bearer <runner secret>`.
    RunnerSecret,
    /// `Authorization: Bearer <job token>`.
    JobToken,
    /// Session cookie, or a personal access token with this scope (`Authorization: Bearer srp_…`, ADR-019);
    /// safe methods only.
    SessionOrToken(TokenScope),
    /// A personal access token with this scope only, never a session (ADR-023: the decision feed's claim, report
    /// and push-target routes); any method.
    TokenOnly(TokenScope),
}

impl RouteAuth {
    /// snake_case name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Session => "session",
            Self::SessionCsrf => "session_csrf",
            Self::GithubSignature => "github_signature",
            Self::RunnerSecret => "runner_secret",
            Self::JobToken => "job_token",
            Self::SessionOrToken(scope) => scope.as_str(),
            Self::TokenOnly(TokenScope::Read) => "token_read",
            Self::TokenOnly(TokenScope::ReviewLoopExecute) => "token_review_loop_execute",
        }
    }
}

/// The listener a route is served on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Listener {
    /// The public listener behind the reverse proxy (`SR_LISTEN_ADDR`).
    Public,
    /// The runner-only listener (`SR_RUNNER_LISTEN_ADDR`); never exposed through the reverse proxy.
    Runner,
    /// The GitHub webhook-only listener (`SR_WEBHOOK_LISTEN_ADDR`).
    Webhook,
}

/// One route.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Route {
    /// camelCase name, the key in `routes.ts`.
    pub name: &'static str,
    /// Method.
    pub method: HttpMethod,
    /// Path with `{param}` placeholders.
    pub path: &'static str,
    /// Authentication.
    pub auth: RouteAuth,
    /// Listener.
    pub listener: Listener,
}

const fn route(
    name: &'static str,
    method: HttpMethod,
    path: &'static str,
    auth: RouteAuth,
) -> Route {
    Route {
        name,
        method,
        path,
        auth,
        listener: Listener::Public,
    }
}

const fn runner_route(name: &'static str, path: &'static str, auth: RouteAuth) -> Route {
    Route {
        name,
        method: HttpMethod::Post,
        path,
        auth,
        listener: Listener::Runner,
    }
}

use HttpMethod::{Delete, Get, Patch, Post, Put};
use RouteAuth::{JobToken, Public, RunnerSecret, Session, SessionCsrf};

/// Session or a `read` personal access token.
const READ: RouteAuth = RouteAuth::SessionOrToken(TokenScope::Read);
/// A personal access token with `review_loop_execute` only (ADR-023).
const EXECUTE: RouteAuth = RouteAuth::TokenOnly(TokenScope::ReviewLoopExecute);

/// GitHub App delivery on the dedicated webhook listener.
pub const GITHUB_WEBHOOK: Route = Route {
    name: "githubWebhook",
    method: Post,
    path: "/webhooks/github",
    auth: RouteAuth::GithubSignature,
    listener: Listener::Webhook,
};
/// Repository coverage for the reader's dashboard.
pub const GET_REPOSITORY_INSTALLATIONS: Route = route(
    "getRepositoryInstallations",
    Get,
    "/api/v1/settings/repository-installations",
    READ,
);
/// GitHub webhook configuration and delivery status.
pub const GET_GITHUB_WEBHOOK: Route = route(
    "getGithubWebhook",
    Get,
    "/api/v1/settings/github-webhook",
    READ,
);
/// The reader's current LLM activity.
pub const GET_LLM_ACTIVITY: Route = route("getLlmActivity", Get, "/api/v1/llm/activity", READ);
/// The reader's ranking cap usage.
pub const GET_RANKING_USAGE: Route = route(
    "getRankingUsage",
    Get,
    "/api/v1/settings/ranking-usage",
    READ,
);
/// Resets the reader's daily ranking usage.
pub const RESET_RANKING_USAGE: Route = route(
    "resetRankingUsage",
    Post,
    "/api/v1/settings/ranking-usage/reset",
    SessionCsrf,
);
/// Request a new judgement for an outdated dashboard item.
pub const REJUDGE_DASHBOARD_ITEM: Route = route(
    "rejudgeDashboardItem",
    Post,
    "/api/v1/dashboard/items/{key}/rejudge",
    SessionCsrf,
);
/// Pull requests the reader has analysed, watched or opened.
pub const LIST_MY_PRS: Route = route("listMyPrs", Get, "/api/v1/me/prs", READ);

/// Liveness for compose health checks.
pub const HEALTHZ: Route = route("healthz", Get, "/healthz", Public);
/// Starts the GitHub sign-in (redirect).
pub const AUTH_LOGIN: Route = route("authLogin", Get, "/auth/github/login", Public);
/// GitHub OAuth callback (redirect).
pub const AUTH_CALLBACK: Route = route("authCallback", Get, "/auth/github/callback", Public);
/// Signs out.
pub const AUTH_LOGOUT: Route = route("authLogout", Post, "/auth/logout", SessionCsrf);
/// The signed-in user and CSRF token.
pub const GET_ME: Route = route("getMe", Get, "/api/v1/me", Public);
/// Resolves a pull request URL.
pub const RESOLVE_PR: Route = route("resolvePr", Post, "/api/v1/pr/resolve", SessionCsrf);
/// Starts (or returns) an analysis.
pub const CREATE_ANALYSIS: Route = route("createAnalysis", Post, "/api/v1/analyses", SessionCsrf);
/// The reader's own analyses, the latest per pull request, newest first.
pub const LIST_MY_ANALYSES: Route = route("listMyAnalyses", Get, "/api/v1/analyses", READ);
/// An analysis.
pub const GET_ANALYSIS: Route = route("getAnalysis", Get, "/api/v1/analyses/{id}", READ);
/// SSE stream of an analysis.
pub const ANALYSIS_EVENTS: Route =
    route("analysisEvents", Get, "/api/v1/analyses/{id}/events", READ);
/// Pauses future jobs of an analysis.
pub const PAUSE_ANALYSIS: Route = route(
    "pauseAnalysis",
    Post,
    "/api/v1/analyses/{id}/pause",
    SessionCsrf,
);
/// Resume a user-paused analysis.
pub const RESUME_ANALYSIS: Route = route(
    "resumeAnalysis",
    Post,
    "/api/v1/analyses/{id}/resume",
    SessionCsrf,
);
/// Cancel an analysis.
pub const CANCEL_ANALYSIS: Route = route(
    "cancelAnalysis",
    Post,
    "/api/v1/analyses/{id}/cancel",
    SessionCsrf,
);
/// Retries a failed analysis.
pub const RETRY_ANALYSIS: Route = route(
    "retryAnalysis",
    Post,
    "/api/v1/analyses/{id}/retry",
    SessionCsrf,
);
/// The Tour of a finished analysis.
pub const GET_TOUR: Route = route("getTour", Get, "/api/v1/analyses/{id}/tour", READ);
/// Head lines by range.
pub const GET_LINES: Route = route("getLines", Get, "/api/v1/analyses/{id}/lines", READ);
/// Classic diff by range.
pub const GET_DIFF: Route = route("getDiff", Get, "/api/v1/analyses/{id}/diff", READ);
/// The user's review state.
pub const GET_REVIEW_STATE: Route = route(
    "getReviewState",
    Get,
    "/api/v1/analyses/{id}/review-state",
    READ,
);
/// Replaces the user's review state.
pub const PUT_REVIEW_STATE: Route = route(
    "putReviewState",
    Put,
    "/api/v1/analyses/{id}/review-state",
    SessionCsrf,
);
/// Every question of the analysis.
pub const LIST_QUESTIONS: Route = route(
    "listQuestions",
    Get,
    "/api/v1/analyses/{id}/questions",
    READ,
);
/// Asks a question about a step.
pub const ASK_QUESTION: Route = route(
    "askQuestion",
    Post,
    "/api/v1/analyses/{id}/steps/{step_id}/questions",
    SessionCsrf,
);
/// The user's feedback on the analysis.
pub const LIST_FEEDBACK: Route = route("listFeedback", Get, "/api/v1/analyses/{id}/feedback", READ);
/// Records feedback.
pub const POST_FEEDBACK: Route = route(
    "postFeedback",
    Post,
    "/api/v1/analyses/{id}/feedback",
    SessionCsrf,
);
/// Removes one of the user's feedback entries (one value on one target).
pub const DELETE_FEEDBACK: Route = route(
    "deleteFeedback",
    Delete,
    "/api/v1/analyses/{id}/feedback/{feedback_id}",
    SessionCsrf,
);
/// The user's pending review on GitHub.
pub const GET_PENDING_REVIEW: Route = route(
    "getPendingReview",
    Get,
    "/api/v1/analyses/{id}/pending-review",
    READ,
);
/// Adds a comment to the pending review.
pub const CREATE_COMMENT: Route = route(
    "createComment",
    Post,
    "/api/v1/analyses/{id}/pending-review/comments",
    SessionCsrf,
);
/// Edits a pending comment.
pub const UPDATE_COMMENT: Route = route(
    "updateComment",
    Patch,
    "/api/v1/analyses/{id}/pending-review/comments/{comment_id}",
    SessionCsrf,
);
/// Deletes a pending comment.
pub const DELETE_COMMENT: Route = route(
    "deleteComment",
    Delete,
    "/api/v1/analyses/{id}/pending-review/comments/{comment_id}",
    SessionCsrf,
);
/// Saves the installation model and reasoning effort.
pub const SET_LLM: Route = route("setLlm", Put, "/api/v1/settings/llm", SessionCsrf);
/// Read settings.
pub const GET_SETTINGS: Route = route("getSettings", Get, "/api/v1/settings", READ);
/// Frozen assessment method configuration and UTC quota.
pub const GET_ASSESSMENT_SETTINGS: Route = route(
    "getAssessmentSettings",
    Get,
    "/api/v1/settings/comment-assessment",
    READ,
);
/// Configure methods without scheduling work.
pub const SET_ASSESSMENT_SETTINGS: Route = route(
    "setAssessmentSettings",
    Put,
    "/api/v1/settings/comment-assessment",
    SessionCsrf,
);
/// Latest payer-owned paired comparison; never fetches GitHub.
pub const GET_ASSESSMENT_COMPARISON: Route = route(
    "getAssessmentComparison",
    Get,
    "/api/v1/prs/{owner}/{name}/{number}/comments/assessment-comparison",
    READ,
);
/// All cleaned source versions, including zero-point sources.
pub const GET_COMMENT_SOURCES: Route = route(
    "getCommentSources",
    Get,
    "/api/v1/prs/{owner}/{name}/{number}/comments/sources",
    READ,
);
/// The signed-in payer's gateway-observed plan status.
pub const GET_CODEX: Route = route(
    "getCodex",
    Get,
    "/api/v1/settings/codex",
    RouteAuth::Session,
);
/// Explicitly unlink the signed-in payer's plan.
pub const DELETE_CODEX: Route = route(
    "deleteCodex",
    Delete,
    "/api/v1/settings/codex",
    RouteAuth::SessionCsrf,
);
/// Begin plan-only device linking.
pub const LINK_CODEX: Route = route(
    "linkCodex",
    Post,
    "/api/v1/settings/codex/link",
    RouteAuth::SessionCsrf,
);
/// Poll the signed-in payer's staged plan link.
pub const GET_CODEX_LINK: Route = route(
    "getCodexLink",
    Get,
    "/api/v1/settings/codex/link",
    RouteAuth::Session,
);

/// Sets the language of LLM texts.
pub const SET_LANGUAGE: Route = route("setLanguage", Put, "/api/v1/settings/language", SessionCsrf);
/// Read-only settings of a repository.
pub const GET_REPO_SETTINGS: Route = route(
    "getRepoSettings",
    Get,
    "/api/v1/repos/{owner}/{name}/settings",
    READ,
);
/// Runner: claim a job.
pub const RUNNER_CLAIM: Route = runner_route("runnerClaim", "/runner/v1/claim", RunnerSecret);
/// Runner: post events (heartbeat).
pub const RUNNER_EVENTS: Route =
    runner_route("runnerEvents", "/runner/v1/jobs/{id}/events", JobToken);
/// Runner: post the final output before session archival and runner cleanup.
pub const RUNNER_OUTPUT: Route =
    runner_route("runnerOutput", "/runner/v1/jobs/{id}/output", JobToken);
/// Runner: post the result.
pub const RUNNER_RESULT: Route =
    runner_route("runnerResult", "/runner/v1/jobs/{id}/result", JobToken);

/// Prioritise one planned flow, retrying it when needed.
pub const REQUEST_FLOW: Route = route(
    "requestFlow",
    Post,
    "/api/v1/analyses/{id}/flows/{flow}/request",
    SessionCsrf,
);
/// Enable or disable automatic work on remaining flows.
pub const SET_BACKGROUND_ANALYSIS: Route = route(
    "setBackgroundAnalysis",
    Put,
    "/api/v1/analyses/{id}/background",
    SessionCsrf,
);

/// R1: the user's dashboard summary.
pub const GET_DASHBOARD: Route = route("getDashboard", Get, "/api/v1/dashboard", READ);
/// R1b: ranked items of the latest snapshot (`ItemQuery`).
pub const LIST_DASHBOARD_ITEMS: Route =
    route("listDashboardItems", Get, "/api/v1/dashboard/items", READ);
/// R11: hides one mention for the signed-in browser user.
pub const HIDE_DASHBOARD_MENTION: Route = route(
    "hideDashboardMention",
    Post,
    "/api/v1/dashboard/mentions/{id}/hide",
    SessionCsrf,
);

/// R12: snoozes one ranked item of the signed-in browser user for a number of days (`SnoozeRequest`).
pub const SNOOZE_DASHBOARD_ITEM: Route = route(
    "snoozeDashboardItem",
    Put,
    "/api/v1/dashboard/items/{key}/snooze",
    SessionCsrf,
);
/// R13: ends the snooze of one item now ("Wake now").
pub const WAKE_DASHBOARD_ITEM: Route = route(
    "wakeDashboardItem",
    Delete,
    "/api/v1/dashboard/items/{key}/snooze",
    SessionCsrf,
);

/// R2: starts a refresh.
pub const START_DASHBOARD_REFRESH: Route = route(
    "startDashboardRefresh",
    Post,
    "/api/v1/dashboard/refreshes",
    SessionCsrf,
);
/// R2b: one refresh.
pub const GET_DASHBOARD_REFRESH: Route = route(
    "getDashboardRefresh",
    Get,
    "/api/v1/dashboard/refreshes/{id}",
    READ,
);
/// R3: cancels a refresh.
pub const CANCEL_DASHBOARD_REFRESH: Route = route(
    "cancelDashboardRefresh",
    Post,
    "/api/v1/dashboard/refreshes/{id}/cancel",
    SessionCsrf,
);
/// R4: ranks the refresh's snapshot again (Retry ranking).
pub const RANK_DASHBOARD_REFRESH: Route = route(
    "rankDashboardRefresh",
    Post,
    "/api/v1/dashboard/refreshes/{id}/rank",
    SessionCsrf,
);
/// R5: SSE stream of a refresh.
pub const DASHBOARD_REFRESH_EVENTS: Route = route(
    "dashboardRefreshEvents",
    Get,
    "/api/v1/dashboard/refreshes/{id}/events",
    READ,
);
/// R6: the user's dashboard settings.
pub const GET_DASHBOARD_SETTINGS: Route = route(
    "getDashboardSettings",
    Get,
    "/api/v1/settings/dashboard",
    READ,
);
/// R6: saves the user's dashboard settings.
pub const SET_DASHBOARD_SETTINGS: Route = route(
    "setDashboardSettings",
    Put,
    "/api/v1/settings/dashboard",
    SessionCsrf,
);
/// R6b: stores the browser's time zone.
pub const SET_DASHBOARD_TIME_ZONE: Route = route(
    "setDashboardTimeZone",
    Put,
    "/api/v1/settings/dashboard/time-zone",
    SessionCsrf,
);
/// Saves the installation-wide reasoning effort of Ask jobs.
pub const SET_ASK_EFFORT: Route = route(
    "setAskEffort",
    Put,
    "/api/v1/settings/llm/ask-effort",
    SessionCsrf,
);
/// R7: saves the installation-wide reasoning effort of the dashboard ranking.
pub const SET_JUDGE_EFFORT: Route = route(
    "setJudgeEffort",
    Put,
    "/api/v1/settings/llm/judge-effort",
    SessionCsrf,
);
/// R8: the user's personal access tokens (session only: a token never lists tokens).
pub const LIST_ACCESS_TOKENS: Route = route(
    "listAccessTokens",
    Get,
    "/api/v1/settings/access-tokens",
    Session,
);
/// R9: creates a personal access token; the response carries the secret once.
pub const CREATE_ACCESS_TOKEN: Route = route(
    "createAccessToken",
    Post,
    "/api/v1/settings/access-tokens",
    SessionCsrf,
);
/// R10: revokes a personal access token.
pub const REVOKE_ACCESS_TOKEN: Route = route(
    "revokeAccessToken",
    Delete,
    "/api/v1/settings/access-tokens/{id}",
    SessionCsrf,
);

/// R12: the files of the pull request with their hunks, owners and the user's GitHub Viewed marks.
pub const GET_FILES: Route = route("getFiles", Get, "/api/v1/analyses/{id}/files", READ);
/// R13: marks a file as viewed on GitHub, or removes the mark, as the user.
pub const SET_FILE_VIEWED: Route = route(
    "setFileViewed",
    Put,
    "/api/v1/analyses/{id}/files/viewed",
    SessionCsrf,
);
/// R14: asks a question about a step or about lines of a file.
pub const ASK_ANCHORED: Route = route(
    "askAnchored",
    Post,
    "/api/v1/analyses/{id}/questions",
    SessionCsrf,
);

/// R15: the Cargo map of the head commit with conversion facts and crate notes (ADR-021).
pub const GET_DEPENDENCIES: Route = route(
    "getDependencies",
    Get,
    "/api/v1/analyses/{id}/dependencies",
    READ,
);
/// R16: replaces the installation-wide list of controlled GitHub owners.
pub const SET_CONTROLLED_OWNERS: Route = route(
    "setControlledOwners",
    Put,
    "/api/v1/settings/controlled-owners",
    SessionCsrf,
);

/// RC1: the PR page (ADR-023): analysis summary, comments summary, plug-in notes, watch.
pub const GET_PR_PAGE: Route = route(
    "getPrPage",
    Get,
    "/api/v1/prs/{owner}/{name}/{number}",
    READ,
);
/// RC2: the Comments overview: read state, counts, rounds, assessment state, queue order.
pub const GET_COMMENTS: Route = route(
    "getComments",
    Get,
    "/api/v1/prs/{owner}/{name}/{number}/comments",
    READ,
);
/// RC3: reads the PR's threads, reviews and comments from GitHub again (202; 429 under 10 s).
pub const READ_COMMENTS: Route = route(
    "readComments",
    Post,
    "/api/v1/prs/{owner}/{name}/{number}/comments/read",
    SessionCsrf,
);
/// RC4: the PR's items with assessments, decisions and effects (`ItemsQuery`).
pub const LIST_COMMENT_ITEMS: Route = route(
    "listCommentItems",
    Get,
    "/api/v1/prs/{owner}/{name}/{number}/comments/items",
    READ,
);
/// RC5: code context of an item (`ContextQuery`); `{item}` is `C<no>`.
pub const GET_COMMENT_CONTEXT: Route = route(
    "getCommentContext",
    Get,
    "/api/v1/prs/{owner}/{name}/{number}/comments/items/{item}/context",
    READ,
);
/// RC6: assesses an item again (202).
pub const ASSESS_COMMENT: Route = route(
    "assessComment",
    Post,
    "/api/v1/prs/{owner}/{name}/{number}/comments/items/{item}/assess",
    SessionCsrf,
);
/// RC8: decides an item (201 `Decision`; 409 `item_changed`).
pub const DECIDE_COMMENT: Route = route(
    "decideComment",
    Post,
    "/api/v1/prs/{owner}/{name}/{number}/comments/items/{item}/decisions",
    SessionCsrf,
);
/// RC9: decides several items at once (group, bulk, table; 201 `Decision[]`).
pub const DECIDE_COMMENT_BATCH: Route = route(
    "decideCommentBatch",
    Post,
    "/api/v1/prs/{owner}/{name}/{number}/comments/decision-batches",
    SessionCsrf,
);
/// RC12: undoes the item's current decision while none of its effects is claimed (`ItemActionRequest`; 200
/// `CommentItem`; 409 `conflict` once an effect is claimed).
pub const UNDO_COMMENT_DECISION: Route = route(
    "undoCommentDecision",
    Post,
    "/api/v1/prs/{owner}/{name}/{number}/comments/items/{item}/undo",
    SessionCsrf,
);
/// RC13: asks a decided or carried item again (`ItemActionRequest`; 200 `CommentItem`).
pub const REOPEN_COMMENT_ITEM: Route = route(
    "reopenCommentItem",
    Post,
    "/api/v1/prs/{owner}/{name}/{number}/comments/items/{item}/reopen",
    SessionCsrf,
);
/// RC14: "Skip for now": the reader's queue shows the item last until it changes (`ItemActionRequest`; 204).
pub const SKIP_COMMENT_ITEM: Route = route(
    "skipCommentItem",
    Post,
    "/api/v1/prs/{owner}/{name}/{number}/comments/items/{item}/skip",
    SessionCsrf,
);
/// RC15: the owner marks or clears a gate on an item (`GateMarkRequest`; 200 `CommentItem`).
pub const MARK_COMMENT_GATE: Route = route(
    "markCommentGate",
    Post,
    "/api/v1/prs/{owner}/{name}/{number}/comments/items/{item}/gates",
    SessionCsrf,
);
/// RC16: the effects decisions would get, without storing them (`EffectsPreviewRequest`; 200 `EffectsPreview[]`).
pub const PREVIEW_COMMENT_EFFECTS: Route = route(
    "previewCommentEffects",
    Post,
    "/api/v1/prs/{owner}/{name}/{number}/comments/effects-preview",
    SessionCsrf,
);
/// RC17: the "Decide the rest per recommendation" confirmation list (`DecideRest`).
pub const GET_DECIDE_REST: Route = route(
    "getDecideRest",
    Get,
    "/api/v1/prs/{owner}/{name}/{number}/comments/decide-rest",
    READ,
);
/// RC10: the PR's decision log with effects, claims, reports and carry events.
pub const GET_COMMENT_LOG: Route = route(
    "getCommentLog",
    Get,
    "/api/v1/prs/{owner}/{name}/{number}/comments/log",
    READ,
);
/// RC11: carry-over events and counts, one PR or all (`CarryOverQuery`).
pub const LIST_CARRY_OVERS: Route =
    route("listCarryOvers", Get, "/api/v1/comments/carry-overs", READ);
/// RC7: asks about a review-comment item (`AnchoredAskRequest` with a `comment_item` anchor; 202 `AskQuestion`):
/// answered in the Ask session of a finished analysis of the head, else by a `verify_comments` question pass.
pub const ASK_PR_QUESTION: Route = route(
    "askPrQuestion",
    Post,
    "/api/v1/prs/{owner}/{name}/{number}/questions",
    SessionCsrf,
);
/// RC7: the reader's own questions about the PR's review-comment items, oldest first (`AskQuestion[]`).
pub const LIST_PR_QUESTIONS: Route = route(
    "listPrQuestions",
    Get,
    "/api/v1/prs/{owner}/{name}/{number}/questions",
    READ,
);
/// RW1: watches a PR (204; 409 `watch_limit_reached`).
pub const WATCH_PR: Route = route(
    "watchPr",
    Put,
    "/api/v1/prs/{owner}/{name}/{number}/watch",
    SessionCsrf,
);
/// RW1: stops watching a PR (204).
pub const UNWATCH_PR: Route = route(
    "unwatchPr",
    Delete,
    "/api/v1/prs/{owner}/{name}/{number}/watch",
    SessionCsrf,
);
/// RW2: the reader's watched PRs.
pub const LIST_WATCHES: Route = route("listWatches", Get, "/api/v1/watches", READ);
/// RW3: SSE stream of the reader's events (`UserStreamEvent`).
pub const USER_EVENTS: Route = route("userEvents", Get, "/api/v1/events", READ);
/// RN1: the user's notification channels (secrets masked; session only).
pub const GET_NOTIFICATION_SETTINGS: Route = route(
    "getNotificationSettings",
    Get,
    "/api/v1/settings/notifications",
    Session,
);
/// RN2: saves a notification channel (`{kind}` = `ntfy` | `webhook`).
pub const PUT_NOTIFICATION_CHANNEL: Route = route(
    "putNotificationChannel",
    Put,
    "/api/v1/settings/notifications/channels/{kind}",
    SessionCsrf,
);
/// RN2: removes a notification channel.
pub const DELETE_NOTIFICATION_CHANNEL: Route = route(
    "deleteNotificationChannel",
    Delete,
    "/api/v1/settings/notifications/channels/{kind}",
    SessionCsrf,
);
/// RN3: sends a test message through a channel (one attempt).
pub const TEST_NOTIFICATION_CHANNEL: Route = route(
    "testNotificationChannel",
    Post,
    "/api/v1/settings/notifications/channels/{kind}/test",
    SessionCsrf,
);
/// RF1: claimable decisions within the capabilities (`FeedQuery`; `ETag`).
pub const GET_DECISION_FEED: Route = route("getDecisionFeed", Get, "/api/v1/decision-feed", READ);
/// RF1b: one feed decision (the `fetch` link of `decision_available` and of push messages).
pub const GET_FEED_DECISION: Route = route(
    "getFeedDecision",
    Get,
    "/api/v1/decision-feed/decisions/{id}",
    READ,
);
/// RF2: SSE stream of `FeedEvent`; registers the subscription.
pub const DECISION_FEED_EVENTS: Route = route(
    "decisionFeedEvents",
    Get,
    "/api/v1/decision-feed/events",
    READ,
);
/// RF3: claims a decision under a lease (201 `Claim`; 409 `claimed`).
pub const CLAIM_DECISION: Route = route(
    "claimDecision",
    Post,
    "/api/v1/decision-feed/decisions/{id}/claim",
    EXECUTE,
);
/// RF4: reports one effect of a claimed decision (201).
pub const REPORT_EFFECT: Route = route(
    "reportEffect",
    Post,
    "/api/v1/decision-feed/effects/{id}/reports",
    EXECUTE,
);
/// RF5: registers the calling token's push target.
pub const PUT_PUSH_TARGET: Route = route(
    "putPushTarget",
    Put,
    "/api/v1/decision-feed/push-target",
    EXECUTE,
);
/// RF5: removes the calling token's push target.
pub const DELETE_PUSH_TARGET: Route = route(
    "deletePushTarget",
    Delete,
    "/api/v1/decision-feed/push-target",
    EXECUTE,
);

/// Every route.
pub const ROUTES: &[Route] = &[
    REQUEST_FLOW,
    SET_BACKGROUND_ANALYSIS,
    HEALTHZ,
    GITHUB_WEBHOOK,
    AUTH_LOGIN,
    AUTH_CALLBACK,
    AUTH_LOGOUT,
    GET_ME,
    LIST_MY_PRS,
    RESOLVE_PR,
    CREATE_ANALYSIS,
    LIST_MY_ANALYSES,
    GET_ANALYSIS,
    ANALYSIS_EVENTS,
    CANCEL_ANALYSIS,
    PAUSE_ANALYSIS,
    RESUME_ANALYSIS,
    RETRY_ANALYSIS,
    GET_TOUR,
    GET_LINES,
    GET_DIFF,
    GET_REVIEW_STATE,
    PUT_REVIEW_STATE,
    LIST_QUESTIONS,
    ASK_QUESTION,
    ASK_ANCHORED,
    GET_FILES,
    SET_FILE_VIEWED,
    GET_DEPENDENCIES,
    LIST_FEEDBACK,
    POST_FEEDBACK,
    DELETE_FEEDBACK,
    GET_PENDING_REVIEW,
    CREATE_COMMENT,
    UPDATE_COMMENT,
    DELETE_COMMENT,
    GET_SETTINGS,
    GET_REPOSITORY_INSTALLATIONS,
    GET_GITHUB_WEBHOOK,
    GET_LLM_ACTIVITY,
    GET_RANKING_USAGE,
    RESET_RANKING_USAGE,
    GET_ASSESSMENT_SETTINGS,
    SET_ASSESSMENT_SETTINGS,
    GET_ASSESSMENT_COMPARISON,
    GET_COMMENT_SOURCES,
    GET_CODEX,
    DELETE_CODEX,
    LINK_CODEX,
    GET_CODEX_LINK,
    SET_LANGUAGE,
    SET_CONTROLLED_OWNERS,
    SET_LLM,
    SET_ASK_EFFORT,
    GET_REPO_SETTINGS,
    GET_DASHBOARD,
    LIST_DASHBOARD_ITEMS,
    HIDE_DASHBOARD_MENTION,
    SNOOZE_DASHBOARD_ITEM,
    WAKE_DASHBOARD_ITEM,
    START_DASHBOARD_REFRESH,
    REJUDGE_DASHBOARD_ITEM,
    GET_DASHBOARD_REFRESH,
    CANCEL_DASHBOARD_REFRESH,
    RANK_DASHBOARD_REFRESH,
    DASHBOARD_REFRESH_EVENTS,
    GET_DASHBOARD_SETTINGS,
    SET_DASHBOARD_SETTINGS,
    SET_DASHBOARD_TIME_ZONE,
    SET_JUDGE_EFFORT,
    LIST_ACCESS_TOKENS,
    CREATE_ACCESS_TOKEN,
    REVOKE_ACCESS_TOKEN,
    GET_PR_PAGE,
    GET_COMMENTS,
    READ_COMMENTS,
    LIST_COMMENT_ITEMS,
    GET_COMMENT_CONTEXT,
    ASSESS_COMMENT,
    DECIDE_COMMENT,
    DECIDE_COMMENT_BATCH,
    UNDO_COMMENT_DECISION,
    REOPEN_COMMENT_ITEM,
    SKIP_COMMENT_ITEM,
    MARK_COMMENT_GATE,
    PREVIEW_COMMENT_EFFECTS,
    GET_DECIDE_REST,
    GET_COMMENT_LOG,
    LIST_CARRY_OVERS,
    ASK_PR_QUESTION,
    LIST_PR_QUESTIONS,
    WATCH_PR,
    UNWATCH_PR,
    LIST_WATCHES,
    USER_EVENTS,
    GET_NOTIFICATION_SETTINGS,
    PUT_NOTIFICATION_CHANNEL,
    DELETE_NOTIFICATION_CHANNEL,
    TEST_NOTIFICATION_CHANNEL,
    GET_DECISION_FEED,
    GET_FEED_DECISION,
    DECISION_FEED_EVENTS,
    CLAIM_DECISION,
    REPORT_EFFECT,
    PUT_PUSH_TARGET,
    DELETE_PUSH_TARGET,
    RUNNER_CLAIM,
    RUNNER_EVENTS,
    RUNNER_OUTPUT,
    RUNNER_RESULT,
];
