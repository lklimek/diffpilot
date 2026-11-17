//! Every HTTP route: method, path (axum syntax), authentication and listener. The server router and the generated
//! `web/src/api/generated/routes.ts` are built from [`ROUTES`].

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
/// An analysis.
pub const GET_ANALYSIS: Route = route("getAnalysis", Get, "/api/v1/analyses/{id}", Session);
/// SSE stream of an analysis.
pub const ANALYSIS_EVENTS: Route = route(
    "analysisEvents",
    Get,
    "/api/v1/analyses/{id}/events",
    Session,
);
/// Cancels an analysis.
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
pub const GET_TOUR: Route = route("getTour", Get, "/api/v1/analyses/{id}/tour", Session);
/// Head lines by range.
pub const GET_LINES: Route = route("getLines", Get, "/api/v1/analyses/{id}/lines", Session);
/// Classic diff by range.
pub const GET_DIFF: Route = route("getDiff", Get, "/api/v1/analyses/{id}/diff", Session);
/// The user's review state.
pub const GET_REVIEW_STATE: Route = route(
    "getReviewState",
    Get,
    "/api/v1/analyses/{id}/review-state",
    Session,
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
    Session,
);
/// Asks a question about a step.
pub const ASK_QUESTION: Route = route(
    "askQuestion",
    Post,
    "/api/v1/analyses/{id}/steps/{step_id}/questions",
    SessionCsrf,
);
/// The user's feedback on the analysis.
pub const LIST_FEEDBACK: Route = route(
    "listFeedback",
    Get,
    "/api/v1/analyses/{id}/feedback",
    Session,
);
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
    Session,
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
/// Settings.
pub const GET_SETTINGS: Route = route("getSettings", Get, "/api/v1/settings", Session);
/// Sets the language of LLM texts.
pub const SET_LANGUAGE: Route = route("setLanguage", Put, "/api/v1/settings/language", SessionCsrf);
/// Read-only settings of a repository.
pub const GET_REPO_SETTINGS: Route = route(
    "getRepoSettings",
    Get,
    "/api/v1/repos/{owner}/{name}/settings",
    Session,
);
/// Runner: claim a job.
pub const RUNNER_CLAIM: Route = runner_route("runnerClaim", "/runner/v1/claim", RunnerSecret);
/// Runner: post events (heartbeat).
pub const RUNNER_EVENTS: Route =
    runner_route("runnerEvents", "/runner/v1/jobs/{id}/events", JobToken);
/// Runner: post the final output before compaction (ARCH-005).
pub const RUNNER_OUTPUT: Route =
    runner_route("runnerOutput", "/runner/v1/jobs/{id}/output", JobToken);
/// Runner: post the result.
pub const RUNNER_RESULT: Route =
    runner_route("runnerResult", "/runner/v1/jobs/{id}/result", JobToken);

/// Every route.
pub const ROUTES: &[Route] = &[
    HEALTHZ,
    AUTH_LOGIN,
    AUTH_CALLBACK,
    AUTH_LOGOUT,
    GET_ME,
    RESOLVE_PR,
    CREATE_ANALYSIS,
    GET_ANALYSIS,
    ANALYSIS_EVENTS,
    CANCEL_ANALYSIS,
    RETRY_ANALYSIS,
    GET_TOUR,
    GET_LINES,
    GET_DIFF,
    GET_REVIEW_STATE,
    PUT_REVIEW_STATE,
    LIST_QUESTIONS,
    ASK_QUESTION,
    LIST_FEEDBACK,
    POST_FEEDBACK,
    DELETE_FEEDBACK,
    GET_PENDING_REVIEW,
    CREATE_COMMENT,
    UPDATE_COMMENT,
    DELETE_COMMENT,
    GET_SETTINGS,
    SET_LANGUAGE,
    GET_REPO_SETTINGS,
    RUNNER_CLAIM,
    RUNNER_EVENTS,
    RUNNER_OUTPUT,
    RUNNER_RESULT,
];
