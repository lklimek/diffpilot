//! GitHub App JWT and per-job read-only installation tokens (ADR-005, threat model T3a). The App private key is
//! read only here; neither it nor a token appears in a log, an error or `Debug` output.

use contracts::common::RepoRef;
use contracts::job::access::{InstallationToken, Secret};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use reqwest::header::{ACCEPT, HeaderMap, RETRY_AFTER, USER_AGENT};
use reqwest::{Client, Method, RequestBuilder, StatusCode, redirect};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;

use contracts::time::Timestamp;
use jiff::SignedDuration;

use crate::config::GithubConfig;
use crate::github::oauth::endpoint;
use crate::github::rest::is_valid_repo;

/// Per call; lookup and mint at claim stay below the runner's 10 s request timeout.
const HTTP_TIMEOUT: Duration = Duration::from_secs(4);
const CLIENT_NAME: &str = "diffpilot";
const GITHUB_API_VERSION: &str = "2022-11-28";

/// Permissions of every job token, always sent explicitly: without them GitHub grants the App's write access.
pub const READ_ONLY_PERMISSIONS: [(&str, &str); 4] = [
    ("contents", "read"),
    ("issues", "read"),
    ("metadata", "read"),
    ("pull_requests", "read"),
];

/// `iat` is set this far in the past against clock drift (GitHub docs).
const JWT_BACKDATE_SECS: i64 = 60;
/// JWT lifetime from `iat`; GitHub allows at most 10 minutes.
const JWT_LIFETIME_SECS: i64 = 540;
/// Installation token lifetime fixed by GitHub.
const TOKEN_LIFETIME: SignedDuration = SignedDuration::from_hours(1);
/// Clock skew and latency allowed beyond [`TOKEN_LIFETIME`] in a mint answer.
const EXPIRY_TOLERANCE: SignedDuration = SignedDuration::from_mins(5);

/// The GitHub App could not provide or revoke an installation token.
#[derive(Debug, thiserror::Error)]
pub enum AppTokenError {
    /// The private key file is missing or unreadable.
    #[error(
        "the GitHub App private key file could not be read; check SR_GITHUB_APP_PRIVATE_KEY_FILE"
    )]
    KeyFile(#[source] std::io::Error),
    /// The file holds no usable RSA private key, or signing with it failed.
    #[error(
        "the GitHub App private key is not a valid RSA key in PEM form; check SR_GITHUB_APP_PRIVATE_KEY_FILE"
    )]
    Key(#[source] jsonwebtoken::errors::Error),
    /// GitHub rejected the App JWT (wrong App id or key).
    #[error(
        "GitHub rejected the GitHub App credentials; check SR_GITHUB_APP_ID and the private key"
    )]
    Unauthorized,
    /// The repository name is not safe to put into an API path.
    #[error("the repository name is not valid")]
    InvalidRepository,
    /// The HTTP client could not be built.
    #[error("the HTTP client for GitHub could not be created")]
    Client(#[source] reqwest::Error),
    /// The request did not complete.
    #[error("the request to GitHub failed")]
    Http(#[source] reqwest::Error),
    /// GitHub answered with an unexpected status.
    #[error("GitHub answered with HTTP status {0}")]
    Status(StatusCode),
    /// The answer was not the expected JSON (no detail kept: it may quote a token).
    #[error("GitHub answered with an unreadable body")]
    Malformed,
    /// GitHub limited the request rate; `retry_after` is the wait GitHub asked for, if it said.
    #[error("GitHub limited the request rate")]
    RateLimited {
        /// Wait from `retry-after` or `x-ratelimit-reset`.
        retry_after: Option<Duration>,
    },
}

impl AppTokenError {
    /// The App is not usable as configured (key file, key or App id): jobs then run without GitHub access.
    pub fn is_configuration(&self) -> bool {
        matches!(self, Self::KeyFile(_) | Self::Key(_) | Self::Unauthorized)
    }
}

/// Why a minted token was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Rejection {
    /// Permissions other than [`READ_ONLY_PERMISSIONS`].
    #[error("permissions other than read-only")]
    Permissions,
    /// Repositories other than exactly the PR repository.
    #[error("repositories other than the pull request repository")]
    Repositories,
    /// The repository is private (v1 supports public repositories only).
    #[error("the repository is private")]
    PrivateRepository,
    /// The token expires before the job's runtime cap, or later than GitHub's one hour.
    #[error("the token expiry is outside the job's runtime and GitHub's one hour")]
    Expiry,
    /// The response lacks a field the check needs.
    #[error("the response lacks expected fields")]
    Shape,
}

/// Outcome of a mint for one job.
#[derive(Debug, PartialEq, Eq)]
pub enum Minted {
    /// The App is not installed on the repository: no token.
    NotInstalled,
    /// A token exactly as requested.
    Accepted(InstallationToken),
    /// A token that differs from the request: never handed out; the caller stores it until it is revoked.
    /// `expires_at` is the revocation deadline (at least one hour from the mint).
    Rejected {
        /// The rejected token.
        token: InstallationToken,
        /// What differs.
        rejection: Rejection,
    },
}

/// Whether the GitHub App is installed on a repository.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Installation {
    /// Installed (and not suspended) with this installation id.
    Installed {
        /// GitHub installation id.
        id: u64,
    },
    /// Not installed or suspended: no token, no commenting, related work not checked.
    NotInstalled,
}

#[derive(Serialize)]
struct JwtClaims {
    iat: i64,
    exp: i64,
    iss: String,
}

/// GitHub App installation details for one repository.
#[derive(Deserialize)]
pub(crate) struct InstallationResponse {
    /// Installation ID from GitHub.
    pub id: u64,
    #[serde(default)]
    suspended_at: Option<String>,
}

impl InstallationResponse {
    /// Whether GitHub suspended the installation.
    pub(crate) fn suspended(&self) -> bool {
        self.suspended_at.is_some()
    }
}

/// Reuses the App JWT and HTTP client across a batch of repository lookups.
pub(crate) struct InstallationLookup<'a> {
    config: &'a GithubConfig,
    client: Client,
    jwt: Secret,
}

impl<'a> InstallationLookup<'a> {
    /// Signs the App JWT used for the batch.
    pub(crate) async fn new(
        config: &'a GithubConfig,
        now: Timestamp,
    ) -> Result<Self, AppTokenError> {
        Ok(Self {
            config,
            client: client()?,
            jwt: app_jwt(config, now).await?,
        })
    }

    /// Looks up the installation even when it is suspended.
    pub(crate) async fn get(
        &self,
        repo: &RepoRef,
    ) -> Result<Option<InstallationResponse>, AppTokenError> {
        lookup_details(&self.client, self.config, repo, &self.jwt).await
    }
}

#[derive(Serialize)]
struct MintRequest<'a> {
    repositories: [&'a str; 1],
    permissions: BTreeMap<&'static str, &'static str>,
}

/// The parts of a mint answer that are checked against the request; the token is read separately.
#[derive(Deserialize)]
struct MintFacts {
    expires_at: Timestamp,
    permissions: BTreeMap<String, String>,
    repository_selection: String,
    #[serde(default)]
    repositories: Vec<MintedRepository>,
}

#[derive(Deserialize)]
struct MintedRepository {
    name: String,
    owner: Owner,
    private: bool,
}

#[derive(Deserialize)]
struct Owner {
    login: String,
}

fn client() -> Result<Client, AppTokenError> {
    Client::builder()
        .redirect(redirect::Policy::none())
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(AppTokenError::Client)
}

fn request(
    client: &Client,
    method: Method,
    config: &GithubConfig,
    path: &str,
    bearer: &str,
) -> RequestBuilder {
    client
        .request(method, endpoint(&config.api_url, path))
        .bearer_auth(bearer)
        .header(ACCEPT, "application/vnd.github+json")
        .header(USER_AGENT, CLIENT_NAME)
        .header("x-github-api-version", GITHUB_API_VERSION)
}

/// An App JWT (RS256) signed with the key file, valid from 60 s before `now` for 9 minutes.
async fn app_jwt(config: &GithubConfig, now: Timestamp) -> Result<Secret, AppTokenError> {
    let pem = secrecy::zeroize::Zeroizing::new(
        tokio::fs::read(&config.private_key_file)
            .await
            .map_err(AppTokenError::KeyFile)?,
    );
    let key = EncodingKey::from_rsa_pem(&pem).map_err(AppTokenError::Key)?;
    let iat = now.as_second() - JWT_BACKDATE_SECS;
    let claims = JwtClaims {
        iat,
        exp: iat + JWT_LIFETIME_SECS,
        iss: config.app_id.to_string(),
    };
    jsonwebtoken::encode(&Header::new(Algorithm::RS256), &claims, &key)
        .map(Secret::new)
        .map_err(AppTokenError::Key)
}

/// Looks up the App installation covering `repo` (`GET /repos/{owner}/{name}/installation`, App JWT). The
/// pipeline uses it for `commenting_available` and the related-work fact; claims use it before minting.
pub async fn installation(
    config: &GithubConfig,
    repo: &RepoRef,
    now: Timestamp,
) -> Result<Installation, AppTokenError> {
    let jwt = app_jwt(config, now).await?;
    lookup(&client()?, config, repo, &jwt).await
}

async fn lookup(
    client: &Client,
    config: &GithubConfig,
    repo: &RepoRef,
    jwt: &Secret,
) -> Result<Installation, AppTokenError> {
    Ok(match lookup_details(client, config, repo, jwt).await? {
        Some(found) if !found.suspended() => Installation::Installed { id: found.id },
        _ => Installation::NotInstalled,
    })
}

async fn lookup_details(
    client: &Client,
    config: &GithubConfig,
    repo: &RepoRef,
    jwt: &Secret,
) -> Result<Option<InstallationResponse>, AppTokenError> {
    if !is_valid_repo(repo) {
        return Err(AppTokenError::InvalidRepository);
    }
    let path = format!("repos/{}/{}/installation", repo.owner, repo.name);
    let response = request(client, Method::GET, config, &path, jwt.expose())
        .send()
        .await
        .map_err(AppTokenError::Http)?;
    match response.status() {
        StatusCode::OK => {}
        StatusCode::NOT_FOUND => return Ok(None),
        StatusCode::UNAUTHORIZED => return Err(AppTokenError::Unauthorized),
        status => return Err(AppTokenError::Status(status)),
    }
    let bytes = response.bytes().await.map_err(AppTokenError::Http)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| AppTokenError::Malformed)
}

/// Mints the read-only token for one job: `repositories` = `repo` only, [`READ_ONLY_PERMISSIONS`], valid at
/// least `runtime` from `now` and at most GitHub's hour. Never a token from another installation. A token that
/// differs from the request comes back as [`Minted::Rejected`] for durable revocation by the caller.
pub async fn mint_for_job(
    config: &GithubConfig,
    repo: &RepoRef,
    now: Timestamp,
    runtime: SignedDuration,
) -> Result<Minted, AppTokenError> {
    let jwt = app_jwt(config, now).await?;
    let client = client()?;
    let Installation::Installed { id } = lookup(&client, config, repo, &jwt).await? else {
        return Ok(Minted::NotInstalled);
    };
    let body = MintRequest {
        repositories: [repo.name.as_str()],
        permissions: READ_ONLY_PERMISSIONS.into_iter().collect(),
    };
    let path = format!("app/installations/{id}/access_tokens");
    let response = request(&client, Method::POST, config, &path, jwt.expose())
        .json(&body)
        .send()
        .await
        .map_err(AppTokenError::Http)?;
    match response.status() {
        StatusCode::CREATED => {}
        StatusCode::UNAUTHORIZED => return Err(AppTokenError::Unauthorized),
        status => return Err(AppTokenError::Status(status)),
    }
    let bytes = response.bytes().await.map_err(AppTokenError::Http)?;
    let answer: Value = serde_json::from_slice(&bytes).map_err(|_| AppTokenError::Malformed)?;
    let token = answer
        .get("token")
        .and_then(Value::as_str)
        .map(Secret::new)
        .ok_or(AppTokenError::Malformed)?;
    Ok(match check_minted(&answer, repo, now, runtime) {
        Ok(expires_at) => Minted::Accepted(InstallationToken {
            token,
            expires_at,
            repository: repo.clone(),
        }),
        Err(rejection) => {
            let claimed = answer
                .get("expires_at")
                .and_then(Value::as_str)
                .and_then(|at| at.parse::<Timestamp>().ok());
            Minted::Rejected {
                token: InstallationToken {
                    token,
                    expires_at: claimed
                        .map_or(now + TOKEN_LIFETIME, |at| at.max(now + TOKEN_LIFETIME)),
                    repository: repo.clone(),
                },
                rejection,
            }
        }
    })
}

/// Compares a mint answer with the request; returns the token's expiry.
fn check_minted(
    answer: &Value,
    repo: &RepoRef,
    now: Timestamp,
    runtime: SignedDuration,
) -> Result<Timestamp, Rejection> {
    let facts = MintFacts::deserialize(answer).map_err(|_| Rejection::Shape)?;
    let requested: BTreeMap<String, String> = READ_ONLY_PERMISSIONS
        .iter()
        .map(|(name, level)| ((*name).to_owned(), (*level).to_owned()))
        .collect();
    if facts.permissions != requested {
        return Err(Rejection::Permissions);
    }
    let [only] = facts.repositories.as_slice() else {
        return Err(Rejection::Repositories);
    };
    if facts.repository_selection != "selected"
        || !only.name.eq_ignore_ascii_case(&repo.name)
        || !only.owner.login.eq_ignore_ascii_case(&repo.owner)
    {
        return Err(Rejection::Repositories);
    }
    if only.private {
        return Err(Rejection::PrivateRepository);
    }
    if facts.expires_at < now + runtime
        || facts.expires_at > now + TOKEN_LIFETIME + EXPIRY_TOLERANCE
    {
        return Err(Rejection::Expiry);
    }
    Ok(facts.expires_at)
}

/// Revokes an installation token (`DELETE /installation/token`); a token GitHub already rejects (revoked or
/// expired) counts as revoked.
pub async fn revoke(config: &GithubConfig, token: &str) -> Result<(), AppTokenError> {
    let response = request(
        &client()?,
        Method::DELETE,
        config,
        "installation/token",
        token,
    )
    .send()
    .await
    .map_err(AppTokenError::Http)?;
    match response.status() {
        StatusCode::NO_CONTENT | StatusCode::UNAUTHORIZED => Ok(()),
        StatusCode::TOO_MANY_REQUESTS => Err(rate_limited(response.headers(), Timestamp::now())),
        StatusCode::FORBIDDEN
            if response.headers().contains_key(RETRY_AFTER)
                || response
                    .headers()
                    .get("x-ratelimit-remaining")
                    .is_some_and(|remaining| remaining == "0") =>
        {
            Err(rate_limited(response.headers(), Timestamp::now()))
        }
        status => Err(AppTokenError::Status(status)),
    }
}

/// The wait GitHub asks for: `retry-after` seconds, else until `x-ratelimit-reset` (epoch seconds).
fn rate_limited(headers: &HeaderMap, now: Timestamp) -> AppTokenError {
    let number = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<i64>().ok())
    };
    let seconds = number(RETRY_AFTER.as_str())
        .or_else(|| number("x-ratelimit-reset").map(|reset| reset.saturating_sub(now.as_second())));
    AppTokenError::RateLimited {
        retry_after: seconds
            .and_then(|seconds| u64::try_from(seconds).ok())
            .map(Duration::from_secs),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::LazyLock;

    use aws_lc_rs::encoding::{AsDer, Pkcs8V1Der};
    use aws_lc_rs::rsa::{KeyPair, KeySize};
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use jsonwebtoken::{DecodingKey, Validation};
    use serde_json::json;
    use tempfile::TempDir;
    use url::Url;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    use super::*;
    use crate::config::tests::test_config;
    use crate::test_support::logs::LogCapture;

    /// Installation id the fake GitHub reports for `acme/node-client`.
    pub(crate) const INSTALLATION_ID: u64 = 4242;
    /// A minted token in GitHub's format (opaque, long).
    pub(crate) const MINTED_TOKEN: &str = "ghs_minted.Canary-Installation-Token.0123456789abcdef";

    /// A throwaway RSA key, generated once per test binary: (PKCS#8 PEM, PKCS#1 public key DER).
    static KEY: LazyLock<(String, Vec<u8>)> = LazyLock::new(|| {
        let pair = KeyPair::generate(KeySize::Rsa2048).expect("RSA key generation");
        let der: Pkcs8V1Der<'static> = pair.as_der().expect("PKCS#8");
        let body = STANDARD.encode(der.as_ref());
        let lines: Vec<&str> = body
            .as_bytes()
            .chunks(64)
            .map(|chunk| std::str::from_utf8(chunk).expect("base64 is ASCII"))
            .collect();
        let pem = format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
            lines.join("\n")
        );
        let public = aws_lc_rs::signature::KeyPair::public_key(&pair)
            .as_ref()
            .to_vec();
        (pem, public)
    });

    /// The throwaway private key in PEM form.
    pub(crate) fn key_pem() -> &'static str {
        &KEY.0
    }

    /// Writes the throwaway key into `dir` and returns its path.
    pub(crate) fn write_key(dir: &Path) -> PathBuf {
        let file = dir.join("app.pem");
        std::fs::write(&file, key_pem()).expect("write key");
        file
    }

    /// GitHub settings pointing at `server` (under `/api`) with `key_file`.
    pub(crate) fn github_config(server: &MockServer, key_file: PathBuf) -> GithubConfig {
        let mut config = test_config().github;
        config.api_url = Url::parse(&format!("{}/api", server.uri())).expect("url");
        config.private_key_file = key_file;
        config
    }

    /// The PR repository of the db test seed.
    pub(crate) fn repo() -> RepoRef {
        RepoRef {
            owner: "acme".to_owned(),
            name: "node-client".to_owned(),
        }
    }

    fn runtime() -> SignedDuration {
        SignedDuration::from_mins(45)
    }

    /// GitHub's mint answer for `acme/node-client`, valid one hour from `now`.
    pub(crate) fn minted(now: Timestamp) -> Value {
        json!({
            "token": MINTED_TOKEN,
            "expires_at": (now + SignedDuration::from_hours(1)).to_rfc3339_offset(),
            "permissions": {"contents": "read", "issues": "read", "metadata": "read", "pull_requests": "read"},
            "repository_selection": "selected",
            "repositories": [{
                "id": 1, "name": "node-client", "full_name": "acme/node-client", "private": false,
                "owner": {"login": "acme"}
            }]
        })
    }

    /// Mounts the installation lookup for `acme/node-client`.
    pub(crate) async fn mount_installation(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/api/repos/acme/node-client/installation"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": INSTALLATION_ID, "app_id": 123_456, "suspended_at": null
            })))
            .mount(server)
            .await;
    }

    /// Mounts a mint answer; `expected` mint calls are verified when the server drops.
    pub(crate) async fn mount_mint(server: &MockServer, answer: Value, expected: u64) {
        Mock::given(method("POST"))
            .and(path(format!(
                "/api/app/installations/{INSTALLATION_ID}/access_tokens"
            )))
            .respond_with(ResponseTemplate::new(201).set_body_json(answer))
            .expect(expected)
            .mount(server)
            .await;
    }

    /// Mounts the revocation of [`MINTED_TOKEN`]; `expected` calls are verified when the server drops.
    pub(crate) async fn mount_revoke(server: &MockServer, status: u16, expected: u64) {
        Mock::given(method("DELETE"))
            .and(path("/api/installation/token"))
            .and(header(
                "authorization",
                format!("Bearer {MINTED_TOKEN}").as_str(),
            ))
            .respond_with(ResponseTemplate::new(status))
            .expect(expected)
            .mount(server)
            .await;
    }

    struct Fixture {
        server: MockServer,
        config: GithubConfig,
        _dir: TempDir,
    }

    async fn fixture() -> Fixture {
        let dir = tempfile::tempdir().expect("temp dir");
        let server = MockServer::start().await;
        let config = github_config(&server, write_key(dir.path()));
        Fixture {
            server,
            config,
            _dir: dir,
        }
    }

    fn requests_to(requests: &[Request], method_name: &str) -> Vec<Request> {
        requests
            .iter()
            .filter(|request| request.method.as_str() == method_name)
            .cloned()
            .collect()
    }

    fn bearer(request: &Request) -> String {
        request
            .headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .unwrap_or_default()
            .to_owned()
    }

    /// The rejection of a mint, with the rejected token handed back (never revoked or dropped here).
    fn rejection(minted: Minted) -> Rejection {
        match minted {
            Minted::Rejected { token, rejection } => {
                assert_eq!(
                    token.token.expose(),
                    MINTED_TOKEN,
                    "handed back for revocation"
                );
                rejection
            }
            other => panic!("expected a rejection, got {other:?}"),
        }
    }

    fn accepted(minted: Minted) -> InstallationToken {
        match minted {
            Minted::Accepted(token) => token,
            other => panic!("expected a token, got {other:?}"),
        }
    }

    #[derive(Debug, Deserialize)]
    struct Claims {
        iat: i64,
        exp: i64,
        iss: String,
    }

    #[tokio::test]
    async fn app_jwt_is_rs256_signed_by_the_key_file() {
        let f = fixture().await;
        mount_installation(&f.server).await;
        let now = Timestamp::now();
        installation(&f.config, &repo(), now).await.expect("lookup");
        let requests = f.server.received_requests().await.expect("recorded");
        let jwt = bearer(&requests[0]);
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_required_spec_claims(&["iat", "exp", "iss"]);
        validation.validate_exp = false;
        let claims =
            jsonwebtoken::decode::<Claims>(&jwt, &DecodingKey::from_rsa_der(&KEY.1), &validation)
                .expect("JWT signed by the App key")
                .claims;
        assert_eq!(claims.iss, "123456", "issuer is the App id");
        assert_eq!(claims.iat, now.as_second() - 60, "issued 60 s back");
        assert_eq!(claims.exp - claims.iat, 540, "below GitHub's 10 minutes");
    }

    #[tokio::test]
    async fn installation_lookup_reports_installed_suspended_and_missing() {
        let f = fixture().await;
        mount_installation(&f.server).await;
        Mock::given(method("GET"))
            .and(path("/api/repos/acme/suspended/installation"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": 7, "suspended_at": "2026-11-01T00:00:00Z"
            })))
            .mount(&f.server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/repos/acme/other/installation"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&f.server)
            .await;
        let now = Timestamp::now();
        let of = |name: &str| RepoRef {
            owner: "acme".to_owned(),
            name: name.to_owned(),
        };
        assert_eq!(
            installation(&f.config, &repo(), now).await.expect("lookup"),
            Installation::Installed {
                id: INSTALLATION_ID
            }
        );
        assert_eq!(
            installation(&f.config, &of("suspended"), now)
                .await
                .expect("lookup"),
            Installation::NotInstalled
        );
        assert_eq!(
            installation(&f.config, &of("other"), now)
                .await
                .expect("lookup"),
            Installation::NotInstalled
        );
        assert!(matches!(
            installation(&f.config, &of(".."), now).await,
            Err(AppTokenError::InvalidRepository)
        ));
    }

    #[tokio::test]
    async fn installation_token_read_only_permissions() {
        let f = fixture().await;
        mount_installation(&f.server).await;
        let now = Timestamp::now();
        Mock::given(method("POST"))
            .and(path(format!(
                "/api/app/installations/{INSTALLATION_ID}/access_tokens"
            )))
            .and(body_json(json!({
                "repositories": ["node-client"],
                "permissions": {"contents": "read", "issues": "read", "metadata": "read", "pull_requests": "read"}
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(minted(now)))
            .expect(1)
            .mount(&f.server)
            .await;
        let token = mint_for_job(&f.config, &repo(), now, runtime())
            .await
            .map(accepted)
            .expect("mint");
        assert_eq!(token.token.expose(), MINTED_TOKEN);
        let requests = f.server.received_requests().await.expect("recorded");
        let mint = &requests_to(&requests, "POST")[0];
        assert!(bearer(mint).starts_with("eyJ"), "minted with the App JWT");
        let body: Value = serde_json::from_slice(&mint.body).expect("JSON body");
        assert_eq!(
            body["permissions"],
            json!({"contents": "read", "issues": "read", "metadata": "read", "pull_requests": "read"}),
            "permissions are explicit and read-only"
        );
    }

    #[tokio::test]
    async fn installation_token_repositories_scoped() {
        let f = fixture().await;
        mount_installation(&f.server).await;
        let now = Timestamp::now();
        mount_mint(&f.server, minted(now), 1).await;
        let token = mint_for_job(&f.config, &repo(), now, runtime())
            .await
            .map(accepted)
            .expect("mint");
        assert_eq!(token.repository, repo());
        let requests = f.server.received_requests().await.expect("recorded");
        let body: Value =
            serde_json::from_slice(&requests_to(&requests, "POST")[0].body).expect("JSON body");
        assert_eq!(
            body["repositories"],
            json!(["node-client"]),
            "only the PR repository"
        );
        assert!(body.get("repository_ids").is_none());
    }

    /// A response covering more or other repositories, or all of them, is rejected and revoked.
    #[tokio::test]
    async fn installation_token_repositories_pr_repo_only() {
        let now = Timestamp::now();
        let mut extra = minted(now);
        extra["repositories"]
            .as_array_mut()
            .expect("array")
            .push(json!({"name": "secret", "owner": {"login": "acme"}, "private": true}));
        let mut other = minted(now);
        other["repositories"][0]["name"] = json!("other");
        let mut foreign_owner = minted(now);
        foreign_owner["repositories"][0]["owner"]["login"] = json!("evil");
        let mut all = minted(now);
        all["repository_selection"] = json!("all");
        let mut none = minted(now);
        none.as_object_mut().expect("object").remove("repositories");
        for answer in [extra, other, foreign_owner, all, none] {
            let f = fixture().await;
            mount_installation(&f.server).await;
            mount_mint(&f.server, answer.clone(), 1).await;
            mount_revoke(&f.server, 204, 0).await;
            let minted = mint_for_job(&f.config, &repo(), now, runtime())
                .await
                .expect("answered");
            assert_eq!(rejection(minted), Rejection::Repositories, "{answer}");
        }
    }

    #[tokio::test]
    async fn installation_token_expiry_one_hour() {
        let f = fixture().await;
        mount_installation(&f.server).await;
        let now = Timestamp::now();
        mount_mint(&f.server, minted(now), 1).await;
        let token = mint_for_job(&f.config, &repo(), now, runtime())
            .await
            .map(accepted)
            .expect("mint");
        let expected: Timestamp = minted(now)["expires_at"]
            .as_str()
            .and_then(|at| at.parse().ok())
            .expect("expiry");
        assert_eq!(
            token.expires_at, expected,
            "GitHub's expiry, not a local guess"
        );
        assert_eq!(token.expires_at - now, SignedDuration::from_hours(1));

        let f = fixture().await;
        mount_installation(&f.server).await;
        let mut short = minted(now);
        short["expires_at"] = json!((now + SignedDuration::from_mins(30)).to_rfc3339_offset());
        mount_mint(&f.server, short, 1).await;
        assert_eq!(
            rejection(
                mint_for_job(&f.config, &repo(), now, runtime())
                    .await
                    .expect("answered")
            ),
            Rejection::Expiry,
            "shorter than the job's runtime cap"
        );
    }

    /// An expiry later than GitHub's hour (plus tolerance) is rejected; the revocation deadline keeps it.
    #[tokio::test]
    async fn installation_token_expiry_beyond_one_hour_rejected() {
        let now = Timestamp::now();
        let f = fixture().await;
        mount_installation(&f.server).await;
        let mut skewed = minted(now);
        skewed["expires_at"] = json!((now + SignedDuration::from_mins(64)).to_rfc3339_offset());
        mount_mint(&f.server, skewed, 1).await;
        mint_for_job(&f.config, &repo(), now, runtime())
            .await
            .map(accepted)
            .expect("within the tolerance");

        let f = fixture().await;
        mount_installation(&f.server).await;
        let late = now + SignedDuration::from_hours(24);
        let mut long = minted(now);
        long["expires_at"] = json!(late.to_rfc3339_offset());
        mount_mint(&f.server, long, 1).await;
        match mint_for_job(&f.config, &repo(), now, runtime())
            .await
            .expect("answered")
        {
            Minted::Rejected { token, rejection } => {
                assert_eq!(rejection, Rejection::Expiry);
                assert_eq!(
                    token.expires_at.as_second(),
                    late.as_second(),
                    "revoked until its claimed expiry"
                );
            }
            other => panic!("expected a rejection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn mint_response_with_write_permission_rejected() {
        let now = Timestamp::now();
        let mut write = minted(now);
        write["permissions"]["issues"] = json!("write");
        let mut extra = minted(now);
        extra["permissions"]["checks"] = json!("read");
        let mut missing = minted(now);
        missing["permissions"]
            .as_object_mut()
            .expect("object")
            .remove("contents");
        for answer in [write, extra, missing] {
            let f = fixture().await;
            mount_installation(&f.server).await;
            mount_mint(&f.server, answer.clone(), 1).await;
            mount_revoke(&f.server, 204, 0).await;
            let minted = mint_for_job(&f.config, &repo(), now, runtime())
                .await
                .expect("answered");
            assert_eq!(rejection(minted), Rejection::Permissions, "{answer}");
        }
    }

    #[tokio::test]
    async fn mint_rejects_private_repo() {
        let f = fixture().await;
        mount_installation(&f.server).await;
        let now = Timestamp::now();
        let mut private = minted(now);
        private["repositories"][0]["private"] = json!(true);
        mount_mint(&f.server, private, 1).await;
        assert_eq!(
            rejection(
                mint_for_job(&f.config, &repo(), now, runtime())
                    .await
                    .expect("answered")
            ),
            Rejection::PrivateRepository
        );
    }

    #[tokio::test]
    async fn uninstalled_repo_gets_no_token_and_no_mint_call() {
        let f = fixture().await;
        Mock::given(method("GET"))
            .and(path("/api/repos/acme/node-client/installation"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&f.server)
            .await;
        mount_mint(&f.server, minted(Timestamp::now()), 0).await;
        assert_eq!(
            mint_for_job(&f.config, &repo(), Timestamp::now(), runtime())
                .await
                .expect("not installed is not an error"),
            Minted::NotInstalled
        );
    }

    #[tokio::test]
    async fn revoke_accepts_204_and_an_already_invalid_token() {
        for (status, ok) in [(204, true), (401, true), (500, false)] {
            let f = fixture().await;
            mount_revoke(&f.server, status, 1).await;
            let result = revoke(&f.config, MINTED_TOKEN).await;
            assert_eq!(result.is_ok(), ok, "{status}: {result:?}");
        }
    }

    #[tokio::test]
    async fn revoke_reports_rate_limits() {
        let reset = Timestamp::now().as_second() + 90;
        for (status, headers, expected) in [
            (429, vec![("retry-after", "30".to_owned())], Some(30)),
            (
                403,
                vec![
                    ("x-ratelimit-remaining", "0".to_owned()),
                    ("x-ratelimit-reset", reset.to_string()),
                ],
                Some(90),
            ),
            (429, Vec::new(), None),
        ] {
            let f = fixture().await;
            let mut answer = ResponseTemplate::new(status);
            for (name, value) in headers {
                answer = answer.insert_header(name, value.as_str());
            }
            Mock::given(method("DELETE"))
                .respond_with(answer)
                .mount(&f.server)
                .await;
            match revoke(&f.config, MINTED_TOKEN).await {
                Err(AppTokenError::RateLimited { retry_after }) => {
                    let seconds = retry_after.map(|wait| wait.as_secs());
                    match expected {
                        Some(90) => assert!(matches!(seconds, Some(88..=90)), "{seconds:?}"),
                        other => assert_eq!(seconds, other),
                    }
                }
                other => panic!("{status}: expected a rate limit, got {other:?}"),
            }
        }
        let f = fixture().await;
        mount_revoke(&f.server, 403, 1).await;
        assert!(matches!(
            revoke(&f.config, MINTED_TOKEN).await,
            Err(AppTokenError::Status(StatusCode::FORBIDDEN))
        ));
    }

    #[tokio::test]
    async fn key_file_problems_are_configuration_errors() {
        let dir = tempfile::tempdir().expect("temp dir");
        let server = MockServer::start().await;
        let placeholder = dir.path().join("placeholder.pem");
        std::fs::write(&placeholder, "placeholder, not a key").expect("write");
        for file in [dir.path().join("missing.pem"), placeholder] {
            let config = github_config(&server, file);
            let error = installation(&config, &repo(), Timestamp::now())
                .await
                .expect_err("no usable key");
            assert!(error.is_configuration(), "{error:?}");
        }
        assert!(
            server
                .received_requests()
                .await
                .expect("recorded")
                .is_empty(),
            "nothing is sent without a key"
        );
        let f = fixture().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&f.server)
            .await;
        let error = installation(&f.config, &repo(), Timestamp::now())
            .await
            .expect_err("JWT rejected");
        assert!(error.is_configuration(), "{error:?}");
        assert!(!AppTokenError::Status(StatusCode::BAD_GATEWAY).is_configuration());
    }

    /// Every message, `Debug` rendering and log line of the failure paths is free of the key and the token.
    #[tokio::test]
    async fn key_and_tokens_absent_from_logs_and_errors() {
        let capture = LogCapture::start();
        let now = Timestamp::now();
        let mut rejected = minted(now);
        rejected["permissions"]["issues"] = json!("write");
        let mut rendered = String::new();
        let f = fixture().await;
        mount_installation(&f.server).await;
        mount_mint(&f.server, rejected, 1).await;
        let minted_rejected = mint_for_job(&f.config, &repo(), now, runtime())
            .await
            .expect("answered");
        rendered.push_str(&format!("{minted_rejected:?}"));
        let mut errors = Vec::new();
        let f = fixture().await;
        mount_installation(&f.server).await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500).set_body_json(minted(now)))
            .mount(&f.server)
            .await;
        errors.push(
            mint_for_job(&f.config, &repo(), now, runtime())
                .await
                .expect_err("fails"),
        );
        let f = fixture().await;
        mount_revoke(&f.server, 502, 1).await;
        errors.push(revoke(&f.config, MINTED_TOKEN).await.expect_err("fails"));
        let dir = tempfile::tempdir().expect("temp dir");
        let placeholder = dir.path().join("placeholder.pem");
        std::fs::write(&placeholder, "placeholder, not a key").expect("write");
        errors.push(
            installation(&github_config(&f.server, placeholder), &repo(), now)
                .await
                .expect_err("no key"),
        );
        let ok = fixture().await;
        mount_installation(&ok.server).await;
        mount_mint(&ok.server, minted(now), 1).await;
        let token = mint_for_job(&ok.config, &repo(), now, runtime())
            .await
            .map(accepted)
            .expect("mint");

        let key_body: String = key_pem()
            .lines()
            .filter(|line| !line.starts_with("-----"))
            .collect();
        rendered.push_str(&capture.text());
        rendered.push_str(&format!("{token:?} {:?}", ok.config));
        for error in &errors {
            rendered.push_str(&format!("{error} {error:?}"));
            let mut source = std::error::Error::source(error);
            while let Some(cause) = source {
                rendered.push_str(&format!("{cause} {cause:?}"));
                source = cause.source();
            }
        }
        assert!(!rendered.contains(MINTED_TOKEN), "{rendered}");
        assert!(!rendered.contains(&key_body[100..164]), "{rendered}");
        assert!(!rendered.contains("eyJ"), "no App JWT: {rendered}");
    }
}
