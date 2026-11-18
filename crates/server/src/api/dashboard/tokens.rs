//! Personal access tokens (R8–R10, ADR-019): list, create (the secret is returned exactly once) and revoke, by the
//! signed-in user only; a token can neither list nor mint tokens.

use std::borrow::Cow;

use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get};
use axum::{Json, Router};
use chrono::{TimeDelta, Utc};
use contracts::api::access_tokens::{
    AccessToken, AccessTokenId, CreateAccessToken, CreatedAccessToken, TokenScope,
};
use contracts::api::error::ApiErrorCode;
use contracts::routes::{CREATE_ACCESS_TOKEN, LIST_ACCESS_TOKENS, REVOKE_ACCESS_TOKEN};
use db::error::DbError;
use db::repo::access_tokens::{AccessTokenRecord, CreateOutcome, NewAccessToken};

use crate::api::error::{ApiErrorSource, ApiFailure};
use crate::auth::access_tokens::{TOKEN_PREFIX, is_token_id, issue};
use crate::auth::{Session, SessionCsrf};
use crate::secrets::SecretError;
use crate::state::AppState;

/// Longest token name, in characters.
const MAX_NAME_CHARS: usize = 60;
/// Attempts at a fresh id when a random id or hash collides with a stored one.
const ISSUE_ATTEMPTS: u32 = 3;

/// A token request failed.
#[derive(Debug, thiserror::Error)]
pub enum TokensApiError {
    /// Malformed JSON body or an unknown scope.
    #[error("the request body is not valid")]
    Json(#[from] JsonRejection),
    /// Malformed path.
    #[error("the path is not valid")]
    Path(#[from] PathRejection),
    /// A name, scope list or expiry outside the rules.
    #[error("the token request breaks a rule: {0}")]
    Invalid(&'static str),
    /// The user has a token of this name that is not revoked.
    #[error("the user already has a token of this name")]
    NameTaken,
    /// The user has the maximum number of live tokens.
    #[error("the user has the maximum number of live tokens")]
    LimitReached,
    /// The request authenticated with an access token.
    #[error("access tokens cannot manage access tokens")]
    TokenNotAccepted,
    /// No random token could be produced.
    #[error("a token could not be generated")]
    Secret(#[from] SecretError),
    /// The database failed, or the token is unknown.
    #[error("the token store failed")]
    Db(#[from] DbError),
}

impl ApiErrorSource for TokensApiError {
    fn code(&self) -> ApiErrorCode {
        match self {
            Self::Json(rejection) => rejection.code(),
            Self::Path(rejection) => rejection.code(),
            Self::Invalid(_) => ApiErrorCode::InvalidRequest,
            Self::NameTaken => ApiErrorCode::Conflict,
            Self::LimitReached => ApiErrorCode::TokenLimitReached,
            Self::TokenNotAccepted => ApiErrorCode::TokenNotAccepted,
            Self::Secret(_) => ApiErrorCode::Internal,
            Self::Db(error) => error.code(),
        }
    }

    fn public_message(&self) -> Cow<'static, str> {
        match self {
            Self::Json(rejection) => rejection.public_message(),
            Self::Path(rejection) => rejection.public_message(),
            Self::Invalid(rule) => Cow::Borrowed(rule),
            Self::NameTaken => Cow::Borrowed("You already have a token with this name."),
            Self::LimitReached => Cow::Borrowed(
                "You have the maximum of 10 active tokens. Revoke one to create another.",
            ),
            Self::TokenNotAccepted => {
                Cow::Borrowed("Access tokens are managed in Settings while signed in.")
            }
            Self::Secret(_) => {
                Cow::Borrowed("Something went wrong on the server. Try again later.")
            }
            Self::Db(DbError::NotFound) => Cow::Borrowed("The token was not found."),
            Self::Db(error) => error.public_message(),
        }
    }
}

type TokensResult<T> = Result<T, ApiFailure<TokensApiError>>;

fn failure(error: impl Into<TokensApiError>) -> ApiFailure<TokensApiError> {
    ApiFailure(error.into())
}

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(LIST_ACCESS_TOKENS.path, get(list))
        .route(CREATE_ACCESS_TOKEN.path, axum::routing::post(create))
        .route(REVOKE_ACCESS_TOKEN.path, delete(revoke))
}

fn listed(record: AccessTokenRecord) -> AccessToken {
    AccessToken {
        prefix: format!("{TOKEN_PREFIX}{}", record.id),
        id: record.id,
        name: record.name,
        scopes: record.scopes,
        created_at: record.created_at,
        expires_at: record.expires_at,
        last_used_at: record.last_used_at,
    }
}

/// R8: the user's tokens that are not revoked, newest first; never their secrets or hashes.
async fn list(
    State(state): State<AppState>,
    session: Session,
) -> TokensResult<Json<Vec<AccessToken>>> {
    // The token check refuses tokens on this `session` route; this keeps it so if routing ever changes.
    if session.csrf_token().is_none() {
        return Err(failure(TokensApiError::TokenNotAccepted));
    }
    let records = state
        .db
        .access_tokens()
        .list(&session.tenant_id, &session.user_id)
        .await
        .map_err(failure)?;
    Ok(Json(records.into_iter().map(listed).collect()))
}

/// The request's rule violation, if any (http-api.md "Access tokens").
fn validate(request: &CreateAccessToken) -> Result<(), TokensApiError> {
    let chars = request.name.chars().count();
    if chars == 0 || chars > MAX_NAME_CHARS {
        return Err(TokensApiError::Invalid(
            "The name must be 1 to 60 characters long.",
        ));
    }
    if request.scopes != [TokenScope::Read] {
        return Err(TokensApiError::Invalid("The only scope is \"read\"."));
    }
    if !CreateAccessToken::EXPIRY_DAYS.contains(&request.expires_in_days) {
        return Err(TokensApiError::Invalid(
            "The token must expire after 30, 90 or 365 days.",
        ));
    }
    Ok(())
}

/// R9: creates a token; the 201 body is the only place its secret ever appears.
async fn create(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    body: Result<Json<CreateAccessToken>, JsonRejection>,
) -> TokensResult<(StatusCode, Json<CreatedAccessToken>)> {
    let Json(request) = body.map_err(failure)?;
    validate(&request).map_err(failure)?;
    let now = Utc::now();
    let expires_at = now + TimeDelta::days(i64::from(request.expires_in_days));
    let mut attempt = 1;
    let (issued, record) = loop {
        let issued = issue().map_err(failure)?;
        let new = NewAccessToken {
            id: issued.id.clone(),
            name: request.name.clone(),
            token_hash: issued.hash,
            scopes: request.scopes.clone(),
            expires_at,
        };
        match state
            .db
            .access_tokens()
            .create(&session.tenant_id, &session.user_id, &new, now)
            .await
        {
            Ok(CreateOutcome::Created(record)) => break (issued, record),
            Ok(CreateOutcome::NameTaken) => return Err(failure(TokensApiError::NameTaken)),
            Ok(CreateOutcome::LimitReached) => return Err(failure(TokensApiError::LimitReached)),
            Err(DbError::Conflict) if attempt < ISSUE_ATTEMPTS => {
                tracing::debug!(
                    attempt,
                    "a new access token id collided with a stored one; drawing again"
                );
                attempt += 1;
            }
            Err(error) => return Err(failure(error)),
        }
    };
    tracing::info!(
        target: "audit",
        token_id = %record.id,
        user_id = %session.user_id,
        expires_at = %record.expires_at,
        "access token created"
    );
    Ok((
        StatusCode::CREATED,
        Json(CreatedAccessToken {
            token: issued.token,
            access_token: listed(record),
        }),
    ))
}

/// R10: revokes one of the user's tokens; 404 for an unknown, revoked or another user's token.
async fn revoke(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    path: Result<Path<String>, PathRejection>,
) -> TokensResult<StatusCode> {
    let Path(id) = path.map_err(failure)?;
    if !is_token_id(&id) {
        return Err(failure(DbError::NotFound));
    }
    let id = AccessTokenId(id);
    state
        .db
        .access_tokens()
        .revoke(&session.tenant_id, &session.user_id, &id, Utc::now())
        .await
        .map_err(failure)?;
    tracing::info!(target: "audit", token_id = %id, user_id = %session.user_id, "access token revoked");
    Ok(StatusCode::NO_CONTENT)
}
