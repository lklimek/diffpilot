//! The API error model: a closed code plus a plain user-facing message, never internal error text.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Body of every API error response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ApiError {
    /// Machine-readable code.
    pub code: ApiErrorCode,
    /// Plain message for the user.
    pub message: String,
}

/// Machine-readable API error code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ApiErrorCode {
    /// Not signed in.
    Unauthenticated,
    /// The GitHub login is not allowed to sign in.
    LoginNotAllowed,
    /// Missing or wrong CSRF token.
    CsrfRejected,
    /// The resource does not exist or is not visible.
    NotFound,
    /// Malformed request.
    InvalidRequest,
    /// The URL is not a GitHub pull request URL.
    NotAPrUrl,
    /// The repository is private (v1 supports public repositories only).
    PrivateRepository,
    /// The GitHub App is not installed where it is needed.
    AppNotInstalled,
    /// The analysis has not finished yet.
    AnalysisNotFinished,
    /// The request conflicts with the current state.
    Conflict,
    /// The pull request head moved since the analysis.
    PrHeadChanged,
    /// GitHub rejected the comment position.
    PositionRejected,
    /// The pending comment was published on GitHub meanwhile; the edit or deletion applied to the published one.
    CommentPublished,
    /// The requested line range is too large.
    RangeTooLarge,
    /// The file is not rendered (generated material).
    NotRendered,
    /// GitHub did not answer as expected.
    GithubUnavailable,
    /// An internal error.
    Internal,
}

impl ApiErrorCode {
    /// Every code.
    pub const ALL: [ApiErrorCode; 17] = [
        Self::Unauthenticated,
        Self::LoginNotAllowed,
        Self::CsrfRejected,
        Self::NotFound,
        Self::InvalidRequest,
        Self::NotAPrUrl,
        Self::PrivateRepository,
        Self::AppNotInstalled,
        Self::AnalysisNotFinished,
        Self::Conflict,
        Self::PrHeadChanged,
        Self::PositionRejected,
        Self::CommentPublished,
        Self::RangeTooLarge,
        Self::NotRendered,
        Self::GithubUnavailable,
        Self::Internal,
    ];

    /// The HTTP status code of a response with this code.
    pub fn http_status(self) -> u16 {
        match self {
            Self::InvalidRequest | Self::NotAPrUrl | Self::RangeTooLarge => 400,
            Self::Unauthenticated => 401,
            Self::LoginNotAllowed | Self::CsrfRejected => 403,
            Self::NotFound => 404,
            Self::Conflict
            | Self::AnalysisNotFinished
            | Self::PrHeadChanged
            | Self::CommentPublished => 409,
            Self::PrivateRepository
            | Self::AppNotInstalled
            | Self::PositionRejected
            | Self::NotRendered => 422,
            Self::Internal => 500,
            Self::GithubUnavailable => 502,
        }
    }
}

/// Why sign-in failed; passed to Home as `/?login_error=<code>` by the OAuth callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum LoginError {
    /// The tenant does not allow this GitHub login.
    LoginNotAllowed,
    /// The OAuth exchange with GitHub failed.
    OauthFailed,
    /// The OAuth `state` did not match.
    StateMismatch,
}
