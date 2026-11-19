//! The signed-in user.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::common::LlmLanguage;

/// Response of `GET /api/v1/me`; `user` is null when signed out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct MeResponse {
    /// The signed-in user.
    pub user: Option<Me>,
    /// Token for the `X-CSRF-Token` header of mutating requests; null when signed out.
    pub csrf_token: Option<String>,
}

/// A signed-in user.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Me {
    /// GitHub login.
    pub login: String,
    /// Display name.
    pub name: Option<String>,
    /// Avatar URL.
    pub avatar_url: Option<String>,
    /// Language of LLM texts.
    pub language: LlmLanguage,
}

/// Body of `POST /auth/logout`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct LogoutRequest {
    /// End every session of the user, not only this one.
    pub everywhere: bool,
}
