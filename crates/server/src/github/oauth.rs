//! GitHub HTTP transport for user sign-in: authorisation URL (web flow, PKCE S256), code exchange, user-token
//! refresh and `GET /user` against the configured OAuth and API base URLs; orchestration lives in `auth`.
//! OAuth errors arrive as HTTP 200 with an `error` field (spike S4).

use std::time::Duration;

use reqwest::header::{ACCEPT, USER_AGENT};
use reqwest::{Client, StatusCode, redirect};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use url::Url;

use crate::config::GithubConfig;

const HTTP_TIMEOUT: Duration = Duration::from_secs(20);
const CLIENT_NAME: &str = "smart-review";
const GITHUB_API_VERSION: &str = "2022-11-28";

/// An OAuth or user-profile call to GitHub failed.
#[derive(Debug, thiserror::Error)]
pub enum OauthError {
    /// The HTTP client could not be built.
    #[error("the HTTP client for GitHub could not be created")]
    Client(#[source] reqwest::Error),
    /// The request did not complete or the body was not the expected JSON.
    #[error("the request to GitHub failed")]
    Http(#[source] reqwest::Error),
    /// GitHub answered with an unexpected HTTP status.
    #[error("GitHub answered with HTTP status {0}")]
    Status(StatusCode),
    /// GitHub rejected the grant (`error` field, e.g. `bad_verification_code`, `bad_refresh_token`).
    #[error("GitHub rejected the OAuth request with error '{error}'")]
    Rejected {
        /// The OAuth error code (never secret).
        error: String,
    },
    /// The token response lacks an expiring access token or a refresh token; the App must have expiring
    /// user tokens enabled (ADR-007).
    #[error(
        "GitHub returned no expiring token pair; enable expiring user tokens on the GitHub App"
    )]
    IncompleteGrant,
}

/// A fresh access and refresh token pair.
pub struct TokenGrant {
    /// User access token.
    pub access_token: SecretString,
    /// Seconds until the access token expires.
    pub expires_in: i64,
    /// Refresh token.
    pub refresh_token: SecretString,
    /// Seconds until the refresh token expires.
    pub refresh_token_expires_in: i64,
}

/// The signed-in GitHub user (`GET /user`).
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct GithubUser {
    /// Numeric user id.
    pub id: i64,
    /// Login.
    pub login: String,
    /// Display name.
    pub name: Option<String>,
    /// Avatar URL.
    pub avatar_url: Option<String>,
}

// No Debug: holds token values.
#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    expires_in: Option<i64>,
    refresh_token: Option<String>,
    refresh_token_expires_in: Option<i64>,
    error: Option<String>,
}

/// `<base>/<path>`, keeping any path prefix of `base`.
pub(crate) fn endpoint(base: &Url, path: &str) -> Url {
    let mut url = base.clone();
    let joined = format!("{}/{}", base.path().trim_end_matches('/'), path);
    url.set_path(&joined);
    url
}

/// The GitHub authorisation URL the browser is sent to.
pub fn authorize_url(
    config: &GithubConfig,
    redirect_uri: &str,
    state: &str,
    challenge: &str,
) -> Url {
    let mut url = endpoint(&config.oauth_url, "login/oauth/authorize");
    url.query_pairs_mut()
        .append_pair("client_id", &config.client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("state", state)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("allow_signup", "false");
    url
}

fn client() -> Result<Client, OauthError> {
    Client::builder()
        .redirect(redirect::Policy::none())
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(OauthError::Client)
}

/// Exchanges the authorisation `code` (with its PKCE verifier) for a token pair.
pub async fn exchange_code(
    config: &GithubConfig,
    code: &str,
    redirect_uri: &str,
    code_verifier: &SecretString,
) -> Result<TokenGrant, OauthError> {
    token_request(
        config,
        &[
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("code_verifier", code_verifier.expose_secret()),
        ],
    )
    .await
}

/// Exchanges a refresh token for a new pair; GitHub invalidates the old pair at once.
pub async fn refresh(
    config: &GithubConfig,
    refresh_token: &SecretString,
) -> Result<TokenGrant, OauthError> {
    token_request(
        config,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.expose_secret()),
        ],
    )
    .await
}

async fn token_request(
    config: &GithubConfig,
    params: &[(&str, &str)],
) -> Result<TokenGrant, OauthError> {
    let mut form = vec![
        ("client_id", config.client_id.as_str()),
        ("client_secret", config.client_secret.expose_secret()),
    ];
    form.extend_from_slice(params);
    let response = client()?
        .post(endpoint(&config.oauth_url, "login/oauth/access_token"))
        .header(ACCEPT, "application/json")
        .header(USER_AGENT, CLIENT_NAME)
        .form(&form)
        .send()
        .await
        .map_err(OauthError::Http)?;
    if response.status() != StatusCode::OK {
        return Err(OauthError::Status(response.status()));
    }
    let body: TokenResponse = response.json().await.map_err(OauthError::Http)?;
    if let Some(error) = body.error {
        return Err(OauthError::Rejected { error });
    }
    match body {
        TokenResponse {
            access_token: Some(access),
            expires_in: Some(expires_in),
            refresh_token: Some(refresh),
            refresh_token_expires_in: Some(refresh_expires_in),
            ..
        } if !access.is_empty() && !refresh.is_empty() => Ok(TokenGrant {
            access_token: SecretString::from(access),
            expires_in,
            refresh_token: SecretString::from(refresh),
            refresh_token_expires_in: refresh_expires_in,
        }),
        _ => Err(OauthError::IncompleteGrant),
    }
}

/// The user the access token belongs to.
pub async fn fetch_user(
    config: &GithubConfig,
    access_token: &SecretString,
) -> Result<GithubUser, OauthError> {
    let response = client()?
        .get(endpoint(&config.api_url, "user"))
        .bearer_auth(access_token.expose_secret())
        .header(ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
        .header(USER_AGENT, CLIENT_NAME)
        .send()
        .await
        .map_err(OauthError::Http)?;
    if response.status() != StatusCode::OK {
        return Err(OauthError::Status(response.status()));
    }
    response.json().await.map_err(OauthError::Http)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_keeps_the_base_path() {
        let base = Url::parse("http://127.0.0.1:9000/api").expect("url");
        assert_eq!(
            endpoint(&base, "user").as_str(),
            "http://127.0.0.1:9000/api/user"
        );
        let root = Url::parse("https://github.com/").expect("url");
        assert_eq!(
            endpoint(&root, "login/oauth/access_token").as_str(),
            "https://github.com/login/oauth/access_token"
        );
    }
}
