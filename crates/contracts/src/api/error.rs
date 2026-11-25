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
    /// Server configuration requires administrator action.
    Configuration,
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
    /// The route is reserved for a feature that is not available yet.
    NotImplemented,
    /// GitHub did not answer as expected.
    GithubUnavailable,
    /// An internal error.
    Internal,
    /// The server is busy (e.g. its database is locked); retry after `Retry-After`.
    Unavailable,
    /// A dashboard refresh of the user is already scanning (R2), or live (R4).
    RefreshRunning,
    /// GitHub's rate limit for the user's token is reached; the message names the reset time.
    GithubRateLimited,
    /// Dashboard settings break a rule; the message names the field and the rule.
    InvalidDashboardSettings,
    /// The list changed since the cursor was issued; read it again from the start.
    CursorExpired,
    /// The access token lacks the scope the route needs.
    InsufficientScope,
    /// The route does not accept access tokens.
    TokenNotAccepted,
    /// The user has the maximum number of live access tokens.
    TokenLimitReached,
    /// Too many requests; retry after `Retry-After`.
    RateLimited,
    /// GitHub refused a change made as the user (e.g. the Viewed mark); nothing was changed.
    GithubRefused,
    /// The review-loop item changed since the decision's revision; read it again (ADR-023).
    ItemChanged,
    /// Another agent holds the decision's claim.
    Claimed,
    /// The claim's lease ended; the decision is back in the feed.
    LeaseExpired,
    /// An open effect of the decision is outside the claimer's capabilities.
    CapabilityMissing,
    /// The user watches the maximum number of pull requests.
    WatchLimitReached,
    /// A notification URL must use `http` or `https`.
    SchemeNotAllowed,
    /// A changed kind or URL origin of a notification target needs its secret replaced or removed.
    SecretRequired,
}

impl ApiErrorCode {
    /// Every code.
    pub const ALL: [ApiErrorCode; 36] = [
        Self::Configuration,
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
        Self::NotImplemented,
        Self::Unavailable,
        Self::RefreshRunning,
        Self::GithubRateLimited,
        Self::InvalidDashboardSettings,
        Self::CursorExpired,
        Self::InsufficientScope,
        Self::TokenNotAccepted,
        Self::TokenLimitReached,
        Self::RateLimited,
        Self::GithubRefused,
        Self::ItemChanged,
        Self::Claimed,
        Self::LeaseExpired,
        Self::CapabilityMissing,
        Self::WatchLimitReached,
        Self::SchemeNotAllowed,
        Self::SecretRequired,
    ];

    /// The HTTP status code of a response with this code.
    pub fn http_status(self) -> u16 {
        match self {
            Self::InvalidRequest | Self::NotAPrUrl | Self::RangeTooLarge => 400,
            Self::Unauthenticated => 401,
            Self::LoginNotAllowed
            | Self::CsrfRejected
            | Self::InsufficientScope
            | Self::TokenNotAccepted => 403,
            Self::NotFound => 404,
            Self::Conflict
            | Self::AnalysisNotFinished
            | Self::PrHeadChanged
            | Self::CommentPublished
            | Self::RefreshRunning
            | Self::CursorExpired
            | Self::TokenLimitReached
            | Self::ItemChanged
            | Self::Claimed
            | Self::LeaseExpired
            | Self::WatchLimitReached => 409,
            Self::PrivateRepository
            | Self::AppNotInstalled
            | Self::PositionRejected
            | Self::NotRendered
            | Self::InvalidDashboardSettings
            | Self::GithubRefused
            | Self::CapabilityMissing
            | Self::SchemeNotAllowed
            | Self::SecretRequired => 422,
            Self::GithubRateLimited | Self::RateLimited => 429,
            Self::Internal | Self::Configuration => 500,
            Self::NotImplemented => 501,
            Self::GithubUnavailable => 502,
            Self::Unavailable => 503,
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
