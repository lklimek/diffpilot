//! User and repository settings, LLM status (brief §4.4). The LLM status is the outcome of the most recent LLM
//! call as the runners reported it (`llm_credentials`), never an on-demand check: `null` before the first call. A
//! failure is told with a fixed plain sentence chosen by its typed cause; the provider's raw text stays in the
//! database and the logs.

use std::borrow::Cow;

use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use contracts::api::error::ApiErrorCode;
use contracts::api::settings::AppInstallation;
use contracts::api::settings::{
    ControlledOwners, RepoSettings, SetAskEffortRequest, SetControlledOwnersRequest,
    SetJudgeEffortRequest, SetLanguageRequest, Settings,
};
use contracts::common::RepoRef;
use contracts::job::result::{JobFailureKind, LlmCallOutcome, LlmCallStatus};
use contracts::routes::{
    GET_REPO_SETTINGS, GET_SETTINGS, SET_ASK_EFFORT, SET_CONTROLLED_OWNERS, SET_JUDGE_EFFORT,
    SET_LANGUAGE, SET_LLM,
};
use contracts::time::Timestamp;
use db::error::DbError;
use db::repo::settings::{AnalysedRepo, CallFailure, LastCall};

use crate::api::error::{ApiErrorSource, ApiFailure};
use crate::auth::access_tokens::github_read;
use crate::auth::{Session, SessionCsrf, user_access_token, user_token_rejected};
use crate::github::rest::{self, GithubError, is_valid_repo};
use crate::memcan::profile;
use crate::pipeline;
use crate::state::AppState;

/// Plain sentences of a failed LLM call, by typed cause.
const USAGE_LIMIT: &str = "The LLM provider's usage limit was reached.";
const SIGN_IN_NEEDED: &str = "Link your Codex account in Settings";
const PROVIDER_REJECTED: &str = "The LLM provider rejected the request or could not answer it.";
const ANSWER_FORMAT: &str = "The LLM answer was not in the expected format.";
const TIMED_OUT: &str = "The LLM call took too long and was stopped.";
const OTHER_FAILURE: &str =
    "The LLM call failed for another reason; the server log has the details.";

/// A settings request failed.
#[derive(Debug, thiserror::Error)]
pub enum SettingsApiError {
    /// The deployment has no credentials administration connection.
    #[error("the Codex gateway is not configured")]
    GatewayNotConfigured,
    /// The gateway did not return a common status or challenge.
    #[error("the Codex gateway is unavailable")]
    Gateway(#[from] crate::gateway::client::Error),
    /// Malformed JSON body, or a value outside the allowed list.
    #[error("the request body is not valid")]
    Json(#[from] JsonRejection),
    /// Malformed path.
    #[error("the path is not valid")]
    Path(#[from] PathRejection),
    /// The repository owner or name is not a valid GitHub name.
    #[error("the repository name is not valid")]
    InvalidRepository,
    /// Model outside the deployment catalog.
    #[error("the model is not available")]
    InvalidModel,
    /// A controlled owner is not a GitHub login, or the list is too long.
    #[error("the controlled owners are not valid")]
    InvalidOwners,
    /// No pre-pass of the repository is stored yet.
    #[error("the repository settings are not known yet")]
    NotKnownYet,
    /// The database failed, or the item is not visible.
    #[error("the database failed")]
    Db(#[from] DbError),
}

impl ApiErrorSource for SettingsApiError {
    fn code(&self) -> ApiErrorCode {
        match self {
            Self::GatewayNotConfigured | Self::Gateway(_) => ApiErrorCode::Unavailable,
            Self::Json(rejection) => rejection.code(),
            Self::Path(rejection) => rejection.code(),
            Self::InvalidRepository | Self::InvalidModel | Self::InvalidOwners => {
                ApiErrorCode::InvalidRequest
            }
            Self::NotKnownYet => ApiErrorCode::AnalysisNotFinished,
            Self::Db(error) => error.code(),
        }
    }

    fn public_message(&self) -> Cow<'static, str> {
        match self {
            Self::GatewayNotConfigured | Self::Gateway(_) => {
                Cow::Borrowed("The Codex connection is unavailable. Try again later.")
            }
            Self::Json(rejection) => rejection.public_message(),
            Self::Path(rejection) => rejection.public_message(),
            Self::Db(error) => error.public_message(),
            Self::InvalidModel => Cow::Borrowed("Choose a model from the available list."),
            Self::InvalidRepository => Cow::Borrowed("The repository name is not valid."),
            Self::InvalidOwners => Cow::Borrowed(
                "Each organisation must be a GitHub login (letters, digits and -, up to 39 characters); at most \
                 50.",
            ),
            Self::NotKnownYet => Cow::Borrowed(
                "Not known yet: the repository settings are read during an analysis and appear once one has \
                 run.",
            ),
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
        .route(SET_CONTROLLED_OWNERS.path, put(set_controlled_owners))
        .route(SET_LLM.path, put(set_llm))
        .route(SET_ASK_EFFORT.path, put(set_ask_effort))
        .route(SET_JUDGE_EFFORT.path, put(set_judge_effort))
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
        .set_language(&session.user_id, request.language, Timestamp::now())
        .await
        .map_err(failure)?;
    tracing::info!(user_id = %session.user_id, language = ?request.language, "LLM text language set");
    Ok(StatusCode::NO_CONTENT)
}

/// Replaces the installation-wide controlled owners (R16).
async fn set_controlled_owners(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    body: Result<Json<SetControlledOwnersRequest>, JsonRejection>,
) -> SettingsResult<Json<ControlledOwners>> {
    let Json(request) = body.map_err(failure)?;
    let owners: Vec<String> = request.owners.iter().map(|o| o.trim().to_owned()).collect();
    if owners.len() > SetControlledOwnersRequest::MAX_OWNERS || !owners.iter().all(|o| is_login(o))
    {
        return Err(failure(SettingsApiError::InvalidOwners));
    }
    let user = state
        .db
        .users()
        .get(&session.user_id)
        .await
        .map_err(failure)?;
    let (old, saved) = state
        .db
        .settings()
        .set_controlled_owners(
            &session.tenant_id,
            &owners,
            &user.profile.login,
            Timestamp::now(),
        )
        .await
        .map_err(failure)?;
    tracing::info!(login = %user.profile.login, old_owners = ?old.owners, new_owners = ?saved.owners,
        "installation controlled owners changed");
    Ok(Json(saved))
}

/// A GitHub user or organisation login: letters, digits and `-`, 1 to 39 characters, not starting with `-`.
fn is_login(owner: &str) -> bool {
    (1..=39).contains(&owner.len())
        && !owner.starts_with('-')
        && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

async fn set_llm(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    body: Result<Json<contracts::job::spec::ModelChoice>, JsonRejection>,
) -> SettingsResult<Json<contracts::api::settings::LlmSelection>> {
    let Json(choice) = body.map_err(failure)?;
    if !crate::model::allowed(&choice) {
        return Err(failure(SettingsApiError::InvalidModel));
    }
    let user = state
        .db
        .users()
        .get(&session.user_id)
        .await
        .map_err(failure)?;
    let (old, saved) = state
        .db
        .settings()
        .set_llm_selection(&choice, &user.profile.login, Timestamp::now())
        .await
        .map_err(failure)?;
    let old = crate::model::selection(old).current;
    tracing::info!(login = %user.profile.login, old_model = %old.model, old_effort = ?old.reasoning_effort, new_model = %choice.model, new_effort = ?choice.reasoning_effort, "installation LLM selection changed");
    Ok(Json(crate::model::selection(Some(saved))))
}

async fn set_ask_effort(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    body: Result<Json<SetAskEffortRequest>, JsonRejection>,
) -> SettingsResult<Json<contracts::api::settings::LlmSelection>> {
    let Json(request) = body.map_err(failure)?;
    let user = state
        .db
        .users()
        .get(&session.user_id)
        .await
        .map_err(failure)?;
    let default_choice = crate::model::selection(None).current;
    let (old, saved) = state
        .db
        .settings()
        .set_ask_effort(
            request.reasoning_effort,
            &default_choice,
            &user.profile.login,
            Timestamp::now(),
        )
        .await
        .map_err(failure)?;
    let old = crate::model::selection(old).ask_effort;
    tracing::info!(login = %user.profile.login, ?old, new = ?request.reasoning_effort, "installation Ask effort changed");
    Ok(Json(crate::model::selection(Some(saved))))
}

/// R7: the installation-wide dashboard ranking effort; the model choice and other efforts are kept.
async fn set_judge_effort(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    body: Result<Json<SetJudgeEffortRequest>, JsonRejection>,
) -> SettingsResult<Json<contracts::api::settings::LlmSelection>> {
    let Json(request) = body.map_err(failure)?;
    let user = state
        .db
        .users()
        .get(&session.user_id)
        .await
        .map_err(failure)?;
    let default_choice = crate::model::selection(None).current;
    let (old, saved) = state
        .db
        .settings()
        .set_judge_effort(
            request.reasoning_effort,
            &default_choice,
            &user.profile.login,
            Timestamp::now(),
        )
        .await
        .map_err(failure)?;
    let old = crate::model::selection(old).judge_effort;
    tracing::info!(login = %user.profile.login, ?old, new = ?request.reasoning_effort, "installation dashboard ranking effort changed");
    Ok(Json(crate::model::selection(Some(saved))))
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
        .settings()
        .last_call(&session.tenant_id, &session.user_id)
        .await?;
    let unusable_since = state
        .db
        .settings()
        .credential_unusable_since(&session.tenant_id, &session.user_id)
        .await?;
    Ok(Settings {
        llm_selection: Some(crate::model::selection(
            state.db.settings().llm_selection().await?,
        )),
        llm_usage: Some(
            state
                .db
                .jobs()
                .usage_windows(&session.tenant_id, Timestamp::now())
                .await?,
        ),
        memcan_status: Some(state.config.memcan.status()),
        llm_status: llm_status(last_call, unusable_since),
        language: user.language,
        app_installations: github_read(session, "installations".to_owned(), async {
            installations(state, session).await.ok_or(())
        })
        .await
        .ok(),
        repos: state
            .db
            .settings()
            .analysed_repos(&session.tenant_id)
            .await?,
        controlled_owners: Some(
            state
                .db
                .settings()
                .controlled_owners(&session.tenant_id)
                .await?,
        ),
    })
}

/// The App installations the user can access; `None` (not checked) when the user's GitHub sign-in is not usable
/// or GitHub fails: never an empty list standing in for an unknown one. A GitHub 401 makes the next use refresh
/// the token pair.
async fn installations(state: &AppState, session: &Session) -> Option<Vec<AppInstallation>> {
    let listed = match user_access_token(state, &session.user_id).await {
        Ok(token) => {
            let listed = rest::user_installations(&state.config.github, &token).await;
            if matches!(listed, Err(GithubError::Unauthorized)) {
                user_token_rejected(state, &session.user_id, &token).await;
            }
            listed.map_err(|error| format!("{error:?}"))
        }
        Err(error) => Err(format!("{error:?}")),
    };
    listed
        .inspect_err(|error| {
            tracing::warn!(
                user_id = %session.user_id,
                %error,
                "GitHub App installations could not be listed; Settings shows them as not checked"
            );
        })
        .ok()
}

/// The newest LLM fact: the last call, or a credential reported unusable after it (threat model T13); a failure
/// carries the plain sentence of its typed cause, never the provider's text.
fn llm_status(
    last_call: Option<LastCall>,
    unusable_since: Option<Timestamp>,
) -> Option<LlmCallOutcome> {
    let not_working = |at, message: &str| LlmCallOutcome {
        at,
        status: LlmCallStatus::NotWorking {
            message: message.to_owned(),
        },
    };
    match (last_call, unusable_since) {
        (Some(call), Some(since)) if call.outcome.at <= since => {
            Some(not_working(since, SIGN_IN_NEEDED))
        }
        (None, Some(since)) => Some(not_working(since, SIGN_IN_NEEDED)),
        (None, None) => None,
        (Some(LastCall { outcome, failure }), _) => Some(match outcome.status {
            LlmCallStatus::Working => outcome,
            LlmCallStatus::NotWorking { .. } => not_working(outcome.at, plain_failure(failure)),
        }),
    }
}

/// The plain sentence for a failed call's typed cause (also the analysis's failed state, `pipeline`).
pub(crate) fn plain_failure(failure: Option<CallFailure>) -> &'static str {
    match failure {
        Some(CallFailure::UsageLimited) => USAGE_LIMIT,
        Some(CallFailure::Failed(kind)) => match kind {
            JobFailureKind::CredentialUnusable
            | JobFailureKind::CredentialMissing
            | JobFailureKind::CredentialRelink => SIGN_IN_NEEDED,
            JobFailureKind::Configuration => {
                "The server is misconfigured: a required agent skill is missing. Ask the administrator."
            }
            JobFailureKind::ProviderFailed => PROVIDER_REJECTED,
            JobFailureKind::OutputNotJson => ANSWER_FORMAT,
            JobFailureKind::Timeout => TIMED_OUT,
            JobFailureKind::InvalidJsonSchema => ANSWER_FORMAT,
            JobFailureKind::AskCompactionUnconfirmed => {
                "The answer was discarded because the required context shortening was not confirmed."
            }
            JobFailureKind::WorkerUnavailable
            | JobFailureKind::WorkerLost
            | JobFailureKind::ModelUnavailable
            | JobFailureKind::UpstreamRejected
            | JobFailureKind::ReportVersionUnsupported
            | JobFailureKind::CheckoutFailed
            | JobFailureKind::Internal => OTHER_FAILURE,
        },
        None => OTHER_FAILURE,
    }
}

/// Read-only settings of an analysed repository from its newest stored pre-pass; `None` = not known yet, never
/// invented values.
async fn repo_facts(state: &AppState, repo: AnalysedRepo) -> Result<Option<RepoSettings>, DbError> {
    let Some(id) = repo.prepass_analysis else {
        return Ok(None);
    };
    let Some(prepass) = pipeline::prepass::stored(state, id).await? else {
        return Ok(None);
    };
    Ok(Some(RepoSettings {
        repo: repo.repo,
        rules: analysis::tech::rules(),
        related_repos: prepass.related_repos.clone(),
        project_profile: prepass.profile_status(profile::stored(&state.db, id).await?.found()),
    }))
}

#[cfg(test)]
pub(super) mod tests {
    use std::path::Path;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use contracts::agent::JobKind;
    use contracts::api::tour::SourceCheck;
    use contracts::common::LlmLanguage;
    use contracts::ids::UserId;
    use contracts::job::protocol::{ClaimRequest, ClaimResponse};
    use contracts::job::result::{JobOutcome, JobResult};
    use contracts::job::spec::JobPriority;
    use contracts::routes::{
        GET_DIFF, GET_REPO_SETTINGS, GET_REVIEW_STATE, GET_SETTINGS, PUT_REVIEW_STATE,
        RUNNER_CLAIM, RUNNER_RESULT, Route, RouteAuth, SET_LANGUAGE,
    };
    use db::repo::analyses::testing::{job_spec, seed_analysis, seed_tenant};
    use db::repo::documents::DocumentKind;
    use db::repo::jobs::{JobStage, NewJob};
    use db::repo::{DEFAULT_TENANT, Db};
    use http_body_util::BodyExt;
    use secrecy::{ExposeSecret, SecretString};
    use serde_json::{Value, json};
    use tower::ServiceExt;
    use url::Url;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::api;
    use crate::auth::{lock_user, store_sign_in_tokens};
    use crate::config::tests::test_config;
    use crate::github::oauth::TokenGrant;
    use crate::state::AppState;
    use crate::test_support::logs::LogCapture;
    use crate::test_support::session_cookie;

    use super::*;

    const ACCESS: &str = "ghu_settings_access_canary";

    pub(super) struct Harness {
        pub(super) state: AppState,
        github: MockServer,
        pub(super) cookie: String,
    }

    fn fixture(name: &str) -> Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/pr-482")
            .join(name);
        serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("JSON")
    }

    fn at(time: &str) -> Timestamp {
        time.parse::<Timestamp>().expect("time")
    }

    /// User `u1` signed in to the default tenant with GitHub tokens, one analysis of `acme/node-client`, and a
    /// fake GitHub answering `/user/installations` with the recorded fixture.
    pub(super) async fn harness() -> Harness {
        let github = MockServer::start().await;
        let mut config = test_config();
        config.github.api_url = Url::parse(&format!("{}/api", github.uri())).expect("url");
        let db = Db::in_memory().await.expect("db");

        let state = AppState::with_queue(config, db);
        seed_analysis(&state.db).await.expect("analysis");
        db::repo::analyses::testing::seed_payer(&state.db, "codex-1")
            .await
            .unwrap();
        let lock = lock_user("u1").await;
        let grant = TokenGrant {
            access_token: SecretString::from(ACCESS),
            expires_in: 8 * 3600,
            refresh_token: SecretString::from("ghr_settings_refresh_canary"),
            refresh_token_expires_in: 180 * 86_400,
        };
        store_sign_in_tokens(&state, &lock, DEFAULT_TENANT, &grant, Timestamp::now())
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
        pub(super) async fn send(
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

        pub(super) async fn get(&self, uri: &str) -> (StatusCode, Value) {
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

        pub(super) async fn llm_status(&self) -> Value {
            let (status, settings) = self.get(GET_SETTINGS.path).await;
            assert_eq!(status, StatusCode::OK, "{settings}");
            settings["llm_status"].clone()
        }
    }

    fn credential(id: &str) -> UserId {
        contracts::ids::UserId(id.to_owned())
    }

    #[tokio::test]
    async fn settings_match_the_seed() {
        let h = harness().await;
        let credentials = h.state.db.llm_credentials();
        credentials
            .register(DEFAULT_TENANT, &credential("u1"), true, Timestamp::now())
            .await
            .expect("credential");
        credentials
            .record_call(
                "u1",
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
    async fn llm_status_is_private_to_the_signed_in_payer() {
        let h = harness().await;
        let credentials = h.state.db.llm_credentials();
        crate::test_support::link_payer(&h.state.db, DEFAULT_TENANT, "codex-1").await;
        credentials
            .record_call(
                "codex-1",
                &LlmCallOutcome {
                    at: Timestamp::now(),
                    status: LlmCallStatus::Working,
                },
            )
            .await
            .unwrap();
        assert_eq!(h.llm_status().await, Value::Null);
        credentials
            .mark_unusable("codex-1", Timestamp::now())
            .await
            .unwrap();
        assert_eq!(h.llm_status().await, Value::Null);
    }

    #[tokio::test]
    async fn llm_status_reflects_last_call() {
        let h = harness().await;
        assert_eq!(h.llm_status().await, Value::Null, "nothing checked yet");
        let credentials = h.state.db.llm_credentials();
        credentials
            .register(DEFAULT_TENANT, &credential("u1"), true, Timestamp::now())
            .await
            .expect("credential");
        assert_eq!(
            h.llm_status().await,
            Value::Null,
            "a registered credential is not a call"
        );
        credentials
            .record_call(
                "u1",
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
        let raw = json!({
            "at": "2026-11-17T08:05:00Z",
            "status": { "state": "not_working", "message": "codex: stream error 0x1f (internal detail)" },
        });
        credentials
            .record_call("u1", &serde_json::from_value(raw).expect("outcome"))
            .await
            .expect("call");
        // Without a typed cause the status says so plainly; the raw text never reaches the user.
        let failed = json!({
            "at": "2026-11-17T08:05:00Z",
            "status": { "state": "not_working", "message": OTHER_FAILURE },
        });
        assert_eq!(h.llm_status().await, failed);

        // The runner reports the credential unusable (refresh failed): the status says so from then on.
        credentials
            .register(
                DEFAULT_TENANT,
                &credential("u1"),
                false,
                at("2026-11-17T09:00:00Z"),
            )
            .await
            .expect("unusable");
        let status = h.llm_status().await;
        assert_eq!(status["at"], json!("2026-11-17T09:00:00Z"));
        assert_eq!(status["status"]["state"], json!("not_working"));
        let message = status["status"]["message"].as_str().expect("message");
        assert!(
            message.contains("Link your Codex account in Settings"),
            "{message}"
        );
        credentials
            .register(DEFAULT_TENANT, &credential("u1"), true, Timestamp::now())
            .await
            .expect("usable again");
        assert_eq!(h.llm_status().await, failed);
        credentials
            .record_call(
                "u1",
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

    /// The text a runner reports for the real Codex schema rejection (`fixtures/pr-482/codex/schema-error.jsonl`,
    /// `turn.failed`).
    fn fixture_schema_error() -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/pr-482/codex/schema-error.jsonl");
        std::fs::read_to_string(path)
            .expect("fixture")
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("JSONL"))
            .find(|event| event["type"] == json!("turn.failed"))
            .and_then(|event| event["error"]["message"].as_str().map(str::to_owned))
            .expect("turn.failed message")
    }

    /// Runs one job on credential `id` through the runner protocol (claim, result) ending with `outcome` and an LLM
    /// call that failed with `raw`, as the runner reports it.
    pub(super) async fn fail_one_job(h: &Harness, id: &str, outcome: JobOutcome, raw: &str) {
        db::repo::analyses::testing::seed_payer(&h.state.db, id)
            .await
            .unwrap();
        let analysis = seed_analysis(&h.state.db).await.expect("analysis");
        crate::test_support::link_payer(&h.state.db, DEFAULT_TENANT, id).await;
        let mut spec = job_spec(analysis, JobKind::Plan);
        spec.payer.0 = id.to_owned();
        h.state
            .db
            .jobs()
            .enqueue(
                NewJob {
                    spec,
                    priority: JobPriority::Batch,
                    stage: JobStage::Plan { batch: 1 },
                    generation: 0,
                },
                Timestamp::now(),
            )
            .await
            .expect("enqueue");
        let runner = |path: &str, bearer: &str, body: Value| {
            let request = Request::post(path)
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {bearer}"))
                .body(Body::from(body.to_string()))
                .expect("request");
            api::runner_router(h.state.clone()).oneshot(request)
        };
        let claim = ClaimRequest {
            accept_job: true,
            runner_id: "runner-1".to_owned(),
            retired: Vec::new(),
        };
        let response = runner(
            RUNNER_CLAIM.path,
            h.state.config.runner_secret.expose_secret(),
            serde_json::to_value(claim).expect("claim"),
        )
        .await
        .expect("infallible");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        let claimed: ClaimResponse = serde_json::from_slice(&bytes).expect("claim response");
        let claimed = claimed.claim.expect("a claimed job");
        let result = JobResult {
            assessment: None,
            report: None,
            job_id: claimed.spec.job_id,
            attempt: claimed.spec.attempt,
            outcome,
            session: None,
            usage: None,
            skills_commit: None,
            llm_call: Some(LlmCallOutcome {
                at: Timestamp::now(),
                status: LlmCallStatus::NotWorking {
                    message: raw.to_owned(),
                },
            }),
        };
        let response = runner(
            &RUNNER_RESULT
                .path
                .replace("{id}", &claimed.spec.job_id.to_string()),
            claimed.job_token.expose(),
            serde_json::to_value(result).expect("result"),
        )
        .await
        .expect("infallible");
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn llm_status_shows_plain_messages_for_real_provider_failures() {
        let logs = LogCapture::start();
        let raw = fixture_schema_error();
        assert!(raw.contains("codex_output_schema"), "{raw}");
        let failed = |kind| JobOutcome::Failed {
            kind,
            message: raw.clone(),
        };
        for (outcome, plain) in [
            (failed(JobFailureKind::ProviderFailed), PROVIDER_REJECTED),
            (JobOutcome::UsageLimited { resets_at: None }, USAGE_LIMIT),
            (failed(JobFailureKind::OutputNotJson), ANSWER_FORMAT),
            (failed(JobFailureKind::Timeout), TIMED_OUT),
            (failed(JobFailureKind::Internal), OTHER_FAILURE),
            (failed(JobFailureKind::CredentialUnusable), SIGN_IN_NEEDED),
        ] {
            let h = harness().await;
            fail_one_job(&h, "u1", outcome.clone(), &raw).await;
            let status = h.llm_status().await;
            assert_eq!(
                status["status"]["state"],
                json!("not_working"),
                "{outcome:?}"
            );
            assert_eq!(status["status"]["message"], json!(plain), "{outcome:?}");
            assert!(!status.to_string().contains("schema"), "{status}");
        }
        let text = logs.text();
        assert!(
            text.contains("codex_output_schema"),
            "the raw text is logged: {text}"
        );
    }

    #[tokio::test]
    async fn memcan_status_is_separate_from_the_project_profile() {
        let h = harness().await;
        let (_, settings) = h.get(GET_SETTINGS.path).await;
        assert_eq!(settings["memcan_status"]["state"], "not_checked");
        assert_eq!(settings["memcan_status"]["last_success_at"], Value::Null);
    }

    #[tokio::test]
    async fn controlled_owners_are_validated_saved_and_listed() {
        let h = harness().await;
        let put = |body: Value| {
            let h = &h;
            async move {
                h.send(
                    "PUT",
                    SET_CONTROLLED_OWNERS.path,
                    Some(&h.cookie),
                    true,
                    Some(body),
                )
                .await
            }
        };
        let (_, settings) = h.get(GET_SETTINGS.path).await;
        assert_eq!(
            settings["controlled_owners"],
            json!({"owners": [], "changed_by": null, "changed_at": null})
        );
        let too_many: Vec<String> = (0..51).map(|n| format!("o{n}")).collect();
        for body in [
            json!({"owners": ["-bad"]}),
            json!({"owners": ["a/b"]}),
            json!({"owners": [""]}),
            json!({"owners": ["x".repeat(40)]}),
            json!({"owners": too_many}),
            json!({}),
        ] {
            let (status, error) = put(body.clone()).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(error["code"], json!("invalid_request"), "{body}");
        }
        let (status, saved) = put(json!({"owners": ["dashpay", " Acme ", "DASHPAY"]})).await;
        assert_eq!(status, StatusCode::OK, "{saved}");
        assert_eq!(saved["owners"], json!(["Acme", "dashpay"]));
        assert_eq!(saved["changed_by"], json!("lklimek"));
        let (_, settings) = h.get(GET_SETTINGS.path).await;
        assert_eq!(settings["controlled_owners"], saved);
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
    async fn repo_settings_come_from_the_stored_prepass() {
        let h = harness().await;
        let id = seed_analysis(&h.state.db).await.expect("analysis");
        let repo = RepoRef {
            owner: "acme".to_owned(),
            name: "node-client".to_owned(),
        };
        let profile = analysis::tech::PROFILE_PATH;
        let prepass = analysis::prepass::prepass(&repo, &[], &[profile.to_owned()], &|_, path| {
            (path == profile).then_some("Payments service; risk lives in src/ledger.")
        })
        .expect("prepass");
        h.state
            .db
            .documents()
            .put(id, DocumentKind::Prepass, "", &prepass, Timestamp::now())
            .await
            .expect("stored");
        let (status, settings) = h.get("/api/v1/repos/Acme/Node-Client/settings").await;
        assert_eq!(status, StatusCode::OK, "{settings}");
        let settings: RepoSettings = serde_json::from_value(settings).expect("settings");
        assert_eq!(settings.repo, repo, "GitHub's spelling");
        assert_eq!(settings.rules, analysis::tech::rules());
        assert_eq!(settings.related_repos, Vec::new());
        assert_eq!(settings.project_profile, prepass.profile_status(None));
        assert_eq!(settings.project_profile.memcan, SourceCheck::NotChecked);
    }

    #[tokio::test]
    async fn repo_settings_report_the_stored_memcan_profile_lookup() {
        let h = harness().await;
        let id = seed_analysis(&h.state.db).await.expect("analysis");
        let repo = RepoRef {
            owner: "acme".to_owned(),
            name: "node-client".to_owned(),
        };
        let prepass = analysis::prepass::prepass(&repo, &[], &[], &|_, _| None).expect("prepass");
        let documents = h.state.db.documents();
        documents
            .put(id, DocumentKind::Prepass, "", &prepass, Timestamp::now())
            .await
            .expect("stored");
        for (lookup, found) in [
            (
                json!({"memcan": "checked", "text": "Payments service."}),
                Some(true),
            ),
            (json!({"memcan": "checked", "text": null}), Some(false)),
            (json!({"memcan": "not_checked"}), None),
        ] {
            documents
                .put(
                    id,
                    DocumentKind::MemcanProfile,
                    "",
                    &lookup,
                    Timestamp::now(),
                )
                .await
                .expect("lookup stored");
            let (status, settings) = h.get("/api/v1/repos/acme/node-client/settings").await;
            assert_eq!(status, StatusCode::OK, "{settings}");
            let settings: RepoSettings = serde_json::from_value(settings).expect("settings");
            assert_eq!(
                settings.project_profile,
                prepass.profile_status(found),
                "{lookup}"
            );
        }
    }

    #[tokio::test]
    async fn settings_show_only_the_tenants_data() {
        let h = harness().await;
        let credentials = h.state.db.llm_credentials();
        credentials
            .register(DEFAULT_TENANT, &credential("u1"), true, Timestamp::now())
            .await
            .expect("credential");
        credentials
            .record_call(
                "u1",
                &LlmCallOutcome {
                    at: Timestamp::now(),
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
    async fn installations_not_checked_on_github_failure_and_token_never_logged() {
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
        // GitHub failing or rejecting the token: installations are "not checked" (null), never "none" ([]).
        let (status, settings) = h.get(GET_SETTINGS.path).await;
        assert_eq!(status, StatusCode::OK, "{settings}");
        assert_eq!(settings["app_installations"], Value::Null);
        assert_eq!(settings["language"], json!("en"), "the rest is still shown");
        assert!(!settings.to_string().contains(ACCESS));
        h.github.reset().await;
        Mock::given(method("GET"))
            .and(path("/api/user/installations"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&h.github)
            .await;
        let (status, settings) = h.get(GET_SETTINGS.path).await;
        assert_eq!(status, StatusCode::OK, "{settings}");
        assert_eq!(settings["app_installations"], Value::Null);
        // GitHub's 401 does not end the session: the same cookie still reads Settings.
        let (status, settings) = h.get(GET_SETTINGS.path).await;
        assert_eq!(status, StatusCode::OK, "{settings}");
        // A user without a usable GitHub sign-in: not checked either.
        h.state
            .db
            .user_tokens()
            .end_sign_in("u1", db::repo::tokens::PairCondition::Any)
            .await
            .expect("end sign-in");
        let other = session_cookie(&h.state, DEFAULT_TENANT).await;
        let (status, settings) = h
            .send("GET", GET_SETTINGS.path, Some(&other), false, None)
            .await;
        assert_eq!(status, StatusCode::OK, "{settings}");
        assert_eq!(settings["app_installations"], Value::Null);
        let text = logs.text();
        assert!(text.contains("not checked"), "failures are logged: {text}");
        assert!(!text.contains(ACCESS), "{text}");
        assert!(!text.contains("ghr_settings_refresh_canary"), "{text}");
    }

    /// Every route of this work package with its auth (http-api.md): no session → 401; a mutation without the
    /// CSRF header → 403.
    #[tokio::test]
    async fn route_auth_table() {
        let h = harness().await;
        let analysis = "00000000-0000-4000-8000-000000000000";
        const READ: RouteAuth =
            RouteAuth::SessionOrToken(contracts::api::access_tokens::TokenScope::Read);
        let table: [(Route, RouteAuth, Option<Value>); 6] = [
            (GET_SETTINGS, READ, None),
            (
                SET_LANGUAGE,
                RouteAuth::SessionCsrf,
                Some(json!({ "language": "pl" })),
            ),
            (GET_REPO_SETTINGS, READ, None),
            (GET_REVIEW_STATE, READ, None),
            (
                PUT_REVIEW_STATE,
                RouteAuth::SessionCsrf,
                Some(json!({ "ticked_steps": [], "last_viewed": null })),
            ),
            (GET_DIFF, READ, None),
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
    #[tokio::test]
    async fn installation_model_choice_validates_csrf_and_records_the_changer() {
        let h = harness().await;
        let (_, initial) = h.get(GET_SETTINGS.path).await;
        assert_eq!(initial["llm_selection"]["current"]["model"], "gpt-6.1-sol");
        let route = "/api/v1/settings/llm";
        let choice = json!({"model":"gpt-6-astra","reasoning_effort":"medium"});
        let (status, _) = h
            .send("PUT", route, Some(&h.cookie), false, Some(choice.clone()))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        for invalid in [
            json!({"model":"unlisted","reasoning_effort":"high"}),
            json!({"model":"gpt-6.1-sol","reasoning_effort":"ultra"}),
        ] {
            let (status, _) = h
                .send("PUT", route, Some(&h.cookie), true, Some(invalid))
                .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }
        let (status, _) = h
            .send("PUT", route, None, false, Some(choice.clone()))
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let logs = LogCapture::start();
        let (status, saved) = h
            .send("PUT", route, Some(&h.cookie), true, Some(choice.clone()))
            .await;
        assert_eq!(status, StatusCode::OK, "{saved}");
        assert_eq!(saved["current"], choice);
        assert_eq!(saved["changed_by"], "lklimek");
        assert!(saved["changed_at"].is_string());
        let (_, read) = h.get(GET_SETTINGS.path).await;
        assert_eq!(read["llm_selection"], saved);
        assert!(logs.text().contains("installation LLM selection changed"));
    }

    #[tokio::test]
    async fn ask_effort_validates_csrf_keeps_the_model_and_records_the_changer() {
        let h = harness().await;
        let (_, initial) = h.get(GET_SETTINGS.path).await;
        assert_eq!(initial["llm_selection"]["ask_effort"], "medium");
        let route = "/api/v1/settings/llm/ask-effort";
        let body = json!({"reasoning_effort":"low"});
        let (status, _) = h
            .send("PUT", route, Some(&h.cookie), false, Some(body.clone()))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = h.send("PUT", route, None, false, Some(body.clone())).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = h
            .send(
                "PUT",
                route,
                Some(&h.cookie),
                true,
                Some(json!({"reasoning_effort":"ultra"})),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let logs = LogCapture::start();
        let (status, saved) = h
            .send("PUT", route, Some(&h.cookie), true, Some(body))
            .await;
        assert_eq!(status, StatusCode::OK, "{saved}");
        assert_eq!(saved["ask_effort"], "low");
        assert_eq!(
            saved["current"],
            json!({"model":"gpt-6.1-sol","reasoning_effort":"high"}),
            "the analysis jobs keep their effort"
        );
        assert_eq!(saved["changed_by"], "lklimek");
        let (_, read) = h.get(GET_SETTINGS.path).await;
        assert_eq!(read["llm_selection"], saved);
        assert!(logs.text().contains("installation Ask effort changed"));
    }

    #[tokio::test]
    async fn judge_effort_validates_csrf_keeps_the_other_efforts_and_records_the_changer() {
        let h = harness().await;
        let (_, initial) = h.get(GET_SETTINGS.path).await;
        assert_eq!(initial["llm_selection"]["judge_effort"], "low");
        let route = "/api/v1/settings/llm/judge-effort";
        let body = json!({"reasoning_effort":"medium"});
        let (status, _) = h
            .send("PUT", route, Some(&h.cookie), false, Some(body.clone()))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = h.send("PUT", route, None, false, Some(body.clone())).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = h
            .send(
                "PUT",
                route,
                Some(&h.cookie),
                true,
                Some(json!({"reasoning_effort":"ultra"})),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let logs = LogCapture::start();
        let (status, saved) = h
            .send("PUT", route, Some(&h.cookie), true, Some(body))
            .await;
        assert_eq!(status, StatusCode::OK, "{saved}");
        assert_eq!(saved["judge_effort"], "medium");
        assert_eq!(saved["ask_effort"], "medium", "the Ask effort is kept");
        assert_eq!(
            saved["current"],
            json!({"model":"gpt-6.1-sol","reasoning_effort":"high"}),
            "the analysis jobs keep their effort"
        );
        assert_eq!(saved["changed_by"], "lklimek");
        let (_, read) = h.get(GET_SETTINGS.path).await;
        assert_eq!(read["llm_selection"], saved);
        assert!(
            logs.text()
                .contains("installation dashboard ranking effort changed")
        );
    }
}

#[cfg(test)]
mod codex_route_tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    #[tokio::test]
    async fn codex_routes_require_a_signed_in_user() {
        let state = crate::state::tests::test_state().await;
        for (method, path) in [
            ("GET", "/api/v1/settings/codex"),
            ("DELETE", "/api/v1/settings/codex"),
            ("POST", "/api/v1/settings/codex/link"),
            ("GET", "/api/v1/settings/codex/link"),
        ] {
            let response = crate::api::router(state.clone())
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {path}"
            );
        }
    }
}

#[cfg(test)]
#[path = "codex_tests.rs"]
mod codex_tests;
