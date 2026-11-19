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
    /// `Authorization: Bearer <runner secret>`.
    RunnerSecret,
    /// `Authorization: Bearer <job token>`.
    JobToken,
    /// Session cookie, or a personal access token with this scope (`Authorization: Bearer srp_…`, ADR-019);
    /// safe methods only.
    SessionOrToken(TokenScope),
}

impl RouteAuth {
    /// snake_case name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Session => "session",
            Self::SessionCsrf => "session_csrf",
            Self::RunnerSecret => "runner_secret",
            Self::JobToken => "job_token",
            Self::SessionOrToken(scope) => scope.as_str(),
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

/// Every route.
pub const ROUTES: &[Route] = &[
    REQUEST_FLOW,
    SET_BACKGROUND_ANALYSIS,
    HEALTHZ,
    AUTH_LOGIN,
    AUTH_CALLBACK,
    AUTH_LOGOUT,
    GET_ME,
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
    LIST_FEEDBACK,
    POST_FEEDBACK,
    DELETE_FEEDBACK,
    GET_PENDING_REVIEW,
    CREATE_COMMENT,
    UPDATE_COMMENT,
    DELETE_COMMENT,
    GET_SETTINGS,
    SET_LANGUAGE,
    SET_LLM,
    SET_ASK_EFFORT,
    GET_REPO_SETTINGS,
    GET_DASHBOARD,
    LIST_DASHBOARD_ITEMS,
    START_DASHBOARD_REFRESH,
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
    RUNNER_CLAIM,
    RUNNER_EVENTS,
    RUNNER_OUTPUT,
    RUNNER_RESULT,
];
