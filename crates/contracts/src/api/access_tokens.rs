//! Personal access tokens (ADR-019): read-only API access for the user's agents, managed in Settings.

use crate::time::Timestamp;
use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// What a token may do; a list on each token. Open enum: clients treat unknown values as opaque.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum TokenScope {
    /// Every safe read route marked `read` (`RouteAuth::SessionOrToken`), as the token's user.
    Read,
    /// Claim and report decisions of the review-loop feed, and register the token's push target (ADR-023;
    /// `RouteAuth::TokenOnly`); never decides.
    ReviewLoopExecute,
}

impl TokenScope {
    /// snake_case name, as on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::ReviewLoopExecute => "review_loop_execute",
        }
    }
}

/// Public id of a token: 8 base62 characters, shown in Settings and part of the token (`srp_<id>_<secret>`).
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS,
)]
pub struct AccessTokenId(pub String);

impl fmt::Display for AccessTokenId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A token as listed in Settings (`GET /api/v1/settings/access-tokens`); never carries the secret or its hash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AccessToken {
    /// Public id.
    pub id: AccessTokenId,
    /// The user's name for the token, 1–60 characters, unique among the user's tokens.
    pub name: String,
    /// Recognisable start of the token, `srp_<id>`.
    pub prefix: String,
    /// What the token may do.
    pub scopes: Vec<TokenScope>,
    /// Creation time.
    pub created_at: Timestamp,
    /// The token stops working at this time.
    pub expires_at: Timestamp,
    /// Last accepted use (recorded at most once a minute); null before the first.
    pub last_used_at: Option<Timestamp>,
}

/// Body of `POST /api/v1/settings/access-tokens`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CreateAccessToken {
    /// Name, 1–60 characters, unique among the user's tokens.
    pub name: String,
    /// Requested scopes; v1.1 accepts only `read`.
    pub scopes: Vec<TokenScope>,
    /// Lifetime in days: one of [`CreateAccessToken::EXPIRY_DAYS`].
    pub expires_in_days: u32,
}

impl CreateAccessToken {
    /// Allowed lifetimes in days; there are no unlimited tokens.
    pub const EXPIRY_DAYS: [u32; 3] = [30, 90, 365];
    /// Lifetime preselected in Settings.
    pub const DEFAULT_EXPIRY_DAYS: u32 = 90;
    /// Most live (not revoked, not expired) tokens per user.
    pub const MAX_LIVE_PER_USER: u32 = 10;
}

/// The whole token `srp_<id>_<secret>`: sent once in [`CreatedAccessToken`], never shown by `Debug`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AccessTokenSecret(pub String);

impl fmt::Debug for AccessTokenSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AccessTokenSecret([redacted])")
    }
}

/// Response of `POST /api/v1/settings/access-tokens` (201): the only response that carries the secret.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CreatedAccessToken {
    /// The token; store it now, it cannot be shown again.
    pub token: AccessTokenSecret,
    /// The new token as listed.
    pub access_token: AccessToken,
}
