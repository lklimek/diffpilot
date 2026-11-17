//! User and repository settings, LLM status (brief §4.4). The LLM status is the outcome of the most recent LLM
//! call as the runners reported it (`llm_credentials`), never an on-demand check: `null` before the first call.

use std::borrow::Cow;

use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use contracts::api::error::ApiErrorCode;
use contracts::api::settings::{RepoSettings, SetLanguageRequest, Settings};
use contracts::common::RepoRef;
use contracts::job::result::{LlmCallOutcome, LlmCallStatus};
use contracts::routes::{GET_REPO_SETTINGS, GET_SETTINGS, SET_LANGUAGE};
use db::error::DbError;
use db::repo::settings::AnalysedRepo;

use crate::api::error::{ApiErrorSource, ApiFailure};
use crate::auth::{Session, SessionCsrf, UserTokenError, user_access_token};
use crate::github::rest::{self, GithubError, is_valid_repo};
use crate::state::AppState;

/// Shown while a credential the runner reported unusable is the newest LLM fact.
const CREDENTIAL_UNUSABLE: &str = "The runner's Codex sign-in can no longer be refreshed: sign in to Codex again in \
     the runner container (see the runbook).";

/// A settings request failed.
#[derive(Debug, thiserror::Error)]
pub enum SettingsApiError {
    /// Malformed JSON body, or a value outside the allowed list.
    #[error("the request body is not valid")]
    Json(#[from] JsonRejection),
    /// Malformed path.
    #[error("the path is not valid")]
    Path(#[from] PathRejection),
    /// The repository owner or name is not a valid GitHub name.
    #[error("the repository name is not valid")]
    InvalidRepository,
    /// No pre-pass of the repository is stored yet.
    #[error("the repository settings are not known yet")]
    NotKnownYet,
    /// The user's GitHub token is not available.
    #[error("the user's GitHub token is not available")]
    Token(#[from] UserTokenError),
    /// Listing the App installations failed.
    #[error("a GitHub call failed")]
    Github(#[from] GithubError),
    /// The database failed, or the item is not visible.
    #[error("the database failed")]
    Db(#[from] DbError),
}

impl ApiErrorSource for SettingsApiError {
    fn code(&self) -> ApiErrorCode {
        match self {
            Self::Json(rejection) => rejection.code(),
            Self::Path(rejection) => rejection.code(),
            Self::InvalidRepository => ApiErrorCode::InvalidRequest,
            Self::NotKnownYet => ApiErrorCode::AnalysisNotFinished,
            Self::Token(error) => error.code(),
            Self::Github(GithubError::Unauthorized) => ApiErrorCode::Unauthenticated,
            Self::Github(_) => ApiErrorCode::GithubUnavailable,
            Self::Db(error) => error.code(),
        }
    }

    fn public_message(&self) -> Cow<'static, str> {
        match self {
            Self::Json(rejection) => rejection.public_message(),
            Self::Path(rejection) => rejection.public_message(),
            Self::Token(error) => error.public_message(),
            Self::Db(error) => error.public_message(),
            Self::InvalidRepository => Cow::Borrowed("The repository name is not valid."),
            Self::NotKnownYet => Cow::Borrowed(
                "Not known yet: the repository settings are read during an analysis and appear once one has \
                 run.",
            ),
            Self::Github(GithubError::Unauthorized) => {
                Cow::Borrowed("Your GitHub sign-in has ended. Sign in again.")
            }
            Self::Github(_) => {
                Cow::Borrowed("GitHub did not list the App installations. Try again later.")
            }
        }
    }
}

type SettingsResult<T> = Result<T, ApiFailure<SettingsApiError>>;

fn failure(error: impl Into<SettingsApiError>) -> ApiFailure<SettingsApiError> {
    ApiFailure(error.into())
}

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(GET_SETTINGS.path, get(read))
        .route(SET_LANGUAGE.path, put(set_language))
        .route(GET_REPO_SETTINGS.path, get(read_repo))
}

async fn read(State(state): State<AppState>, session: Session) -> SettingsResult<Json<Settings>> {
    Ok(Json(settings(&state, &session).await.map_err(failure)?))
}

async fn set_language(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    body: Result<Json<SetLanguageRequest>, JsonRejection>,
) -> SettingsResult<StatusCode> {
    let Json(request) = body.map_err(failure)?;
    state
        .db
        .settings()
        .set_language(&session.user_id, request.language, Utc::now())
        .await
        .map_err(failure)?;
    tracing::info!(user_id = %session.user_id, language = ?request.language, "LLM text language set");
    Ok(StatusCode::NO_CONTENT)
}

async fn read_repo(
    State(state): State<AppState>,
    session: Session,
    path: Result<Path<(String, String)>, PathRejection>,
) -> SettingsResult<Json<RepoSettings>> {
    let Path((owner, name)) = path.map_err(failure)?;
    let repo = RepoRef { owner, name };
    if !is_valid_repo(&repo) {
        return Err(failure(SettingsApiError::InvalidRepository));
    }
    let analysed = state
        .db
        .settings()
        .analysed_repo(&session.tenant_id, &repo)
        .await
        .map_err(failure)?
        .ok_or_else(|| failure(DbError::NotFound))?;
    let settings = repo_facts(&state, analysed)
        .await
        .map_err(failure)?
        .ok_or_else(|| failure(SettingsApiError::NotKnownYet))?;
    Ok(Json(settings))
}

/// The signed-in user's settings: LLM status, language, App installations, the tenant's analysed repositories.
async fn settings(state: &AppState, session: &Session) -> Result<Settings, SettingsApiError> {
    let user = state.db.users().get(&session.user_id).await?;
    let last_call = state
        .db
        .llm_credentials()
        .last_call(&session.tenant_id)
        .await?;
    let unusable_since = state
        .db
        .settings()
        .credential_unusable_since(&session.tenant_id)
        .await?;
    let token = user_access_token(state, &session.user_id).await?;
    let app_installations = rest::user_installations(&state.config.github, &token).await?;
    Ok(Settings {
        llm_status: llm_status(last_call, unusable_since),
        language: user.language,
        app_installations,
        repos: state
            .db
            .settings()
            .analysed_repos(&session.tenant_id)
            .await?,
    })
}

/// The newest LLM fact: the last call's outcome, or a credential reported unusable after it (threat model T13).
fn llm_status(
    last_call: Option<LlmCallOutcome>,
    unusable_since: Option<DateTime<Utc>>,
) -> Option<LlmCallOutcome> {
    match (last_call, unusable_since) {
        (Some(call), Some(since)) if call.at > since => Some(call),
        (_, Some(since)) => Some(LlmCallOutcome {
            at: since,
            status: LlmCallStatus::NotWorking {
                message: CREDENTIAL_UNUSABLE.to_owned(),
            },
        }),
        (call, None) => call,
    }
}

/// Read-only settings of an analysed repository from its newest stored pre-pass; `None` = not known yet, never
/// invented values.
// TODO(WP3-PIPE): read `DocumentKind::Prepass` of `repo.prepass_analysis` as `analysis::prepass::Prepass`: rules
// from the `analysis::tech` rule list, `related_repos`, and `profile_status(memcan)` with the MemCan profile source
// the pipeline found; `repo.repo` is GitHub's spelling.
async fn repo_facts(
    _state: &AppState,
    _repo: AnalysedRepo,
) -> Result<Option<RepoSettings>, DbError> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use chrono::{DateTime, Utc};
    use contracts::common::LlmLanguage;
    use contracts::job::result::{LlmCallOutcome, LlmCallStatus};
    use contracts::job::spec::{CredentialKind, LlmCredentialRef};
    use contracts::routes::{
        GET_DIFF, GET_REPO_SETTINGS, GET_REVIEW_STATE, GET_SETTINGS, PUT_REVIEW_STATE, Route,
        RouteAuth, SET_LANGUAGE,
    };
    use db::repo::analyses::testing::{seed_analysis, seed_tenant};
    use db::repo::{DEFAULT_TENANT, Db};
    use http_body_util::BodyExt;
    use secrecy::SecretString;
    use serde_json::{Value, json};
    use tower::ServiceExt;
    use url::Url;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::api;
    use crate::auth::{lock_user, store_sign_in_tokens};
    use crate::config::tests::test_config;
    use crate::github::oauth::TokenGrant;
    use crate::jobs::queue::QueueExecutor;
    use crate::state::AppState;
    use crate::test_support::logs::LogCapture;
    use crate::test_support::session_cookie;

    const ACCESS: &str = "ghu_settings_access_canary";

    struct Harness {
        state: AppState,
        github: MockServer,
        cookie: String,
    }

    fn fixture(name: &str) -> Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/pr-482")
            .join(name);
        serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("JSON")
    }

    fn at(time: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(time)
            .expect("time")
            .with_timezone(&Utc)
    }

    /// User `u1` signed in to the default tenant with GitHub tokens, one analysis of `acme/node-client`, and a
    /// fake GitHub answering `/user/installations` with the recorded fixture.
    async fn harness() -> Harness {
        let github = MockServer::start().await;
        let mut config = test_config();
        config.github.api_url = Url::parse(&format!("{}/api", github.uri())).expect("url");
        let db = Db::in_memory().await.expect("db");
        let executor = std::sync::Arc::new(QueueExecutor::new(db.clone()));
        let state = AppState::new(config, db, executor);
        seed_analysis(&state.db).await.expect("analysis");
        let lock = lock_user("u1").await;
        let grant = TokenGrant {
            access_token: SecretString::from(ACCESS),
            expires_in: 8 * 3600,
            refresh_token: SecretString::from("ghr_settings_refresh_canary"),
            refresh_token_expires_in: 180 * 86_400,
        };
        store_sign_in_tokens(&state, &lock, DEFAULT_TENANT, &grant, Utc::now())
            .await
            .expect("tokens");
        drop(lock);
        let recorded = fixture("github/rest/user-installations.json");
        Mock::given(method("GET"))
            .and(path("/api/user/installations"))
            .and(header("authorization", format!("Bearer {ACCESS}").as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(&recorded["response"]))
            .mount(&github)
            .await;
        let cookie = session_cookie(&state, DEFAULT_TENANT).await;
        Harness {
            state,
            github,
            cookie,
        }
    }

    impl Harness {
        async fn send(
            &self,
            method: &str,
            uri: &str,
            cookie: Option<&str>,
            csrf: bool,
            body: Option<Value>,
        ) -> (StatusCode, Value) {
            let mut request = Request::builder().method(method).uri(uri);
            if let Some(cookie) = cookie {
                request = request.header("cookie", cookie);
            }
            if csrf {
                request = request.header("x-csrf-token", "csrf");
            }
            let request = match body {
                Some(body) => request
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string())),
                None => request.body(Body::empty()),
            }
            .expect("request");
            let response = api::router(self.state.clone())
                .oneshot(request)
                .await
                .expect("infallible");
            let status = response.status();
            let bytes = response
                .into_body()
                .collect()
                .await
                .expect("body")
                .to_bytes();
            let body = if bytes.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&bytes).expect("JSON body")
            };
            (status, body)
        }

        async fn get(&self, uri: &str) -> (StatusCode, Value) {
            self.send("GET", uri, Some(&self.cookie), false, None).await
        }

        async fn set_language(&self, body: Value) -> (StatusCode, Value) {
            self.send(
                "PUT",
                SET_LANGUAGE.path,
                Some(&self.cookie),
                true,
                Some(body),
            )
            .await
        }

        async fn llm_status(&self) -> Value {
            let (status, settings) = self.get(GET_SETTINGS.path).await;
            assert_eq!(status, StatusCode::OK, "{settings}");
            settings["llm_status"].clone()
        }
    }

    fn credential(id: &str) -> LlmCredentialRef {
        LlmCredentialRef {
            id: id.to_owned(),
            kind: CredentialKind::CodexChatgpt,
        }
    }

    #[tokio::test]
    async fn settings_match_the_seed() {
        let h = harness().await;
        let credentials = h.state.db.llm_credentials();
        credentials
            .register(DEFAULT_TENANT, &credential("codex-1"), true, Utc::now())
            .await
            .expect("credential");
        credentials
            .record_call(
                "codex-1",
                &LlmCallOutcome {
                    at: at("2026-11-16T09:31:00Z"),
                    status: LlmCallStatus::Working,
                },
            )
            .await
            .expect("call");
        let (status, settings) = h.get(GET_SETTINGS.path).await;
        assert_eq!(status, StatusCode::OK, "{settings}");
        assert_eq!(settings, fixture("api-seed/default/settings.json"));
    }

    #[tokio::test]
    async fn llm_status_reflects_last_call() {
        let h = harness().await;
        assert_eq!(h.llm_status().await, Value::Null, "nothing checked yet");
        let credentials = h.state.db.llm_credentials();
        credentials
            .register(DEFAULT_TENANT, &credential("codex-1"), true, Utc::now())
            .await
            .expect("credential");
        assert_eq!(
            h.llm_status().await,
            Value::Null,
            "a registered credential is not a call"
        );
        credentials
            .record_call(
                "codex-1",
                &LlmCallOutcome {
                    at: at("2026-11-17T08:00:00Z"),
                    status: LlmCallStatus::Working,
                },
            )
            .await
            .expect("call");
        assert_eq!(
            h.llm_status().await,
            json!({ "at": "2026-11-17T08:00:00Z", "status": { "state": "working" } })
        );
        let failed = json!({
            "at": "2026-11-17T08:05:00Z",
            "status": { "state": "not_working", "message": "Codex usage limit reached." },
        });
        credentials
            .record_call(
                "codex-1",
                &serde_json::from_value(failed.clone()).expect("outcome"),
            )
            .await
            .expect("call");
        assert_eq!(h.llm_status().await, failed);

        // The runner reports the credential unusable (refresh failed): the status says so from then on.
        credentials
            .register(
                DEFAULT_TENANT,
                &credential("codex-1"),
                false,
                at("2026-11-17T09:00:00Z"),
            )
            .await
            .expect("unusable");
        let status = h.llm_status().await;
        assert_eq!(status["at"], json!("2026-11-17T09:00:00Z"));
        assert_eq!(status["status"]["state"], json!("not_working"));
        let message = status["status"]["message"].as_str().expect("message");
        assert!(message.contains("sign in to Codex again"), "{message}");
        credentials
            .register(DEFAULT_TENANT, &credential("codex-1"), true, Utc::now())
            .await
            .expect("usable again");
        assert_eq!(h.llm_status().await, failed);
        credentials
            .record_call(
                "codex-1",
                &LlmCallOutcome {
                    at: at("2026-11-17T10:00:00Z"),
                    status: LlmCallStatus::Working,
                },
            )
            .await
            .expect("call");
        assert_eq!(
            h.llm_status().await["status"],
            json!({ "state": "working" })
        );
    }

    #[tokio::test]
    async fn set_language_accepts_only_supported_languages() {
        let h = harness().await;
        for body in [
            json!({ "language": "de" }),
            json!({ "language": "PL" }),
            json!({ "language": "" }),
            json!({ "language": 1 }),
            json!({}),
        ] {
            let (status, error) = h.set_language(body.clone()).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(error["code"], json!("invalid_request"));
        }
        let (_, settings) = h.get(GET_SETTINGS.path).await;
        assert_eq!(
            settings["language"],
            json!("en"),
            "rejected values change nothing"
        );
        for _ in 0..2 {
            let (status, body) = h.set_language(json!({ "language": "pl" })).await;
            assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
        }
        let (_, settings) = h.get(GET_SETTINGS.path).await;
        assert_eq!(settings["language"], json!("pl"));
        let user = h.state.db.users().get("u1").await.expect("user");
        assert_eq!(user.language, LlmLanguage::Pl, "later analyses use it");
    }

    #[tokio::test]
    async fn repo_settings_are_not_known_without_a_prepass() {
        let h = harness().await;
        for repo in ["acme/node-client", "Acme/Node-Client"] {
            let (status, error) = h.get(&format!("/api/v1/repos/{repo}/settings")).await;
            assert_eq!(status, StatusCode::CONFLICT, "{repo}: {error}");
            assert_eq!(error["code"], json!("analysis_not_finished"));
            let message = error["message"].as_str().expect("message");
            assert!(message.starts_with("Not known yet"), "{message}");
        }
        let (status, error) = h.get("/api/v1/repos/acme/never-analysed/settings").await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{error}");
        assert_eq!(error["code"], json!("not_found"));
        let (status, error) = h.get("/api/v1/repos/ac%24me/node-client/settings").await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
        assert_eq!(error["code"], json!("invalid_request"));
    }

    #[tokio::test]
    async fn settings_show_only_the_tenants_data() {
        let h = harness().await;
        let credentials = h.state.db.llm_credentials();
        credentials
            .register(DEFAULT_TENANT, &credential("codex-1"), true, Utc::now())
            .await
            .expect("credential");
        credentials
            .record_call(
                "codex-1",
                &LlmCallOutcome {
                    at: Utc::now(),
                    status: LlmCallStatus::Working,
                },
            )
            .await
            .expect("call");
        seed_tenant(&h.state.db, "other").await.expect("tenant");
        let other = session_cookie(&h.state, "other").await;
        let (status, settings) = h
            .send("GET", GET_SETTINGS.path, Some(&other), false, None)
            .await;
        assert_eq!(status, StatusCode::OK, "{settings}");
        assert_eq!(settings["llm_status"], Value::Null);
        assert_eq!(settings["repos"], json!([]));
        let (status, _) = h
            .send(
                "GET",
                "/api/v1/repos/acme/node-client/settings",
                Some(&other),
                false,
                None,
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "another tenant's repository");
    }

    #[tokio::test]
    async fn settings_report_github_failures_and_never_log_the_token() {
        let logs = LogCapture::start();
        let h = harness().await;
        let (status, settings) = h.get(GET_SETTINGS.path).await;
        assert_eq!(status, StatusCode::OK, "{settings}");
        h.github.reset().await;
        Mock::given(method("GET"))
            .and(path("/api/user/installations"))
            .respond_with(ResponseTemplate::new(502).set_body_string(ACCESS))
            .mount(&h.github)
            .await;
        let (status, error) = h.get(GET_SETTINGS.path).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY, "{error}");
        assert_eq!(error["code"], json!("github_unavailable"));
        assert!(!error.to_string().contains(ACCESS));
        h.github.reset().await;
        Mock::given(method("GET"))
            .and(path("/api/user/installations"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&h.github)
            .await;
        let (status, error) = h.get(GET_SETTINGS.path).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{error}");
        assert_eq!(error["code"], json!("unauthenticated"));
        let text = logs.text();
        assert!(text.contains("API request"), "errors are logged: {text}");
        assert!(!text.contains(ACCESS), "{text}");
        assert!(!text.contains("ghr_settings_refresh_canary"), "{text}");
    }

    /// Every route of this work package with its auth (http-api.md): no session → 401; a mutation without the
    /// CSRF header → 403.
    #[tokio::test]
    async fn route_auth_table() {
        let h = harness().await;
        let analysis = "00000000-0000-4000-8000-000000000000";
        let table: [(Route, RouteAuth, Option<Value>); 6] = [
            (GET_SETTINGS, RouteAuth::Session, None),
            (
                SET_LANGUAGE,
                RouteAuth::SessionCsrf,
                Some(json!({ "language": "pl" })),
            ),
            (GET_REPO_SETTINGS, RouteAuth::Session, None),
            (GET_REVIEW_STATE, RouteAuth::Session, None),
            (
                PUT_REVIEW_STATE,
                RouteAuth::SessionCsrf,
                Some(json!({ "ticked_steps": [], "last_viewed": null })),
            ),
            (GET_DIFF, RouteAuth::Session, None),
        ];
        for (route, auth, body) in table {
            assert_eq!(route.auth, auth, "{}", route.path);
            let uri = route
                .path
                .replace("{id}", analysis)
                .replace("{owner}", "acme")
                .replace("{name}", "node-client")
                + "?path=src/lib.rs&side=head&from=1&to=5";
            let method = route.method.as_str();
            let (status, error) = h.send(method, &uri, None, true, body.clone()).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
            assert_eq!(error["code"], json!("unauthenticated"));
            let (status, error) = h.send(method, &uri, Some(&h.cookie), false, body).await;
            if auth == RouteAuth::SessionCsrf {
                assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
                assert_eq!(error["code"], json!("csrf_rejected"));
            } else {
                assert_ne!(status, StatusCode::FORBIDDEN, "{method} {uri}");
                assert_ne!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
                assert_ne!(status, StatusCode::METHOD_NOT_ALLOWED, "{method} {uri}");
            }
        }
    }
}
