//! Dashboard settings (R6, R6b, plan §F.6): every request reads or writes the signed-in user's own row only.
//! Reading takes the session (`read`); writing takes the session and its CSRF token, never a personal access
//! token. An absent row answers the defaults. R6b only fills a missing zone; Settings (R6) changes a stored one.

use std::borrow::Cow;

use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use contracts::api::dashboard::{DashboardSettings, DashboardSettingsView, SetTimeZoneRequest};
use contracts::api::error::ApiErrorCode;
use contracts::api::settings::MemcanState;
use contracts::routes::{GET_DASHBOARD_SETTINGS, SET_DASHBOARD_SETTINGS, SET_DASHBOARD_TIME_ZONE};
use contracts::time::Timestamp;
use db::error::DbError;

use crate::api::error::{ApiErrorSource, ApiFailure};
use crate::auth::{Session, SessionCsrf};
use crate::dashboard::settings::{self, InvalidSettings};
use crate::dashboard::snapshot;
use crate::state::AppState;

/// A dashboard settings request failed.
#[derive(Debug, thiserror::Error)]
pub enum DashboardSettingsError {
    /// Malformed JSON body.
    #[error("the request body is not valid")]
    Json(#[from] JsonRejection),
    /// A rule of plan §F.6 is broken.
    #[error("the dashboard settings break a rule")]
    Invalid(#[from] InvalidSettings),
    /// The database failed.
    #[error("the database failed")]
    Db(#[from] DbError),
}

impl ApiErrorSource for DashboardSettingsError {
    fn code(&self) -> ApiErrorCode {
        match self {
            Self::Json(rejection) => rejection.code(),
            Self::Invalid(_) => ApiErrorCode::InvalidDashboardSettings,
            Self::Db(error) => error.code(),
        }
    }

    fn public_message(&self) -> Cow<'static, str> {
        match self {
            Self::Json(rejection) => rejection.public_message(),
            Self::Invalid(rule) => Cow::Owned(rule.to_string()),
            Self::Db(error) => error.public_message(),
        }
    }
}

type SettingsResult<T> = Result<T, ApiFailure<DashboardSettingsError>>;

fn failure(error: impl Into<DashboardSettingsError>) -> ApiFailure<DashboardSettingsError> {
    ApiFailure(error.into())
}

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(GET_DASHBOARD_SETTINGS.path, get(read))
        .route(SET_DASHBOARD_SETTINGS.path, put(save))
        .route(SET_DASHBOARD_TIME_ZONE.path, put(set_time_zone))
}

async fn read(
    State(state): State<AppState>,
    session: Session,
) -> SettingsResult<Json<DashboardSettingsView>> {
    let settings = settings::effective(&state.db, &session.tenant_id, &session.user_id)
        .await
        .map_err(failure)?;
    let own_login = own_login(&state, &session).await?;
    Ok(Json(view(&state, &session, settings, own_login).await?))
}

async fn save(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    body: Result<Json<DashboardSettings>, JsonRejection>,
) -> SettingsResult<Json<DashboardSettingsView>> {
    let Json(settings) = body.map_err(failure)?;
    let own_login = own_login(&state, &session).await?;
    let settings = settings::validated(settings, &own_login).map_err(failure)?;
    state
        .db
        .dashboard()
        .save_settings(
            &session.tenant_id,
            &session.user_id,
            &settings,
            Timestamp::now(),
        )
        .await
        .map_err(failure)?;
    tracing::info!(user_id = %session.user_id, "dashboard settings saved");
    Ok(Json(view(&state, &session, settings, own_login).await?))
}

async fn set_time_zone(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    body: Result<Json<SetTimeZoneRequest>, JsonRejection>,
) -> SettingsResult<StatusCode> {
    let Json(request) = body.map_err(failure)?;
    settings::check_time_zone(&request.time_zone).map_err(failure)?;
    let stored = state
        .db
        .dashboard()
        .set_time_zone_if_unset(
            &session.tenant_id,
            &session.user_id,
            &request.time_zone,
            Timestamp::now(),
        )
        .await
        .map_err(failure)?;
    if stored {
        tracing::info!(user_id = %session.user_id, time_zone = %request.time_zone, "dashboard time zone stored");
    } else {
        tracing::debug!(user_id = %session.user_id, "browser time zone ignored: the user's settings hold one");
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn own_login(state: &AppState, session: &Session) -> SettingsResult<String> {
    let user = state
        .db
        .users()
        .get(&session.user_id)
        .await
        .map_err(failure)?;
    Ok(user.profile.login)
}

async fn view(
    state: &AppState,
    session: &Session,
    settings: DashboardSettings,
    own_login: String,
) -> SettingsResult<DashboardSettingsView> {
    let memcan_configured = state.config.memcan.status().state != MemcanState::NotConfigured;
    let reported = snapshot::unreachable(state, &session.tenant_id, &session.user_id)
        .await
        .map_err(failure)?;
    Ok(settings::view(
        settings,
        own_login,
        memcan_configured,
        &reported,
    ))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use contracts::api::dashboard::RepositoryRule;
    use contracts::common::RepoRef;
    use db::repo::analyses::testing::seed_analysis;
    use db::repo::{DEFAULT_TENANT, Db};
    use http_body_util::BodyExt;
    use secrecy::SecretString;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;
    use crate::api;
    use crate::config::tests::test_config;
    use crate::test_support::{other_user_cookie, session_cookie};

    struct Harness {
        state: AppState,
        cookie: String,
    }

    async fn harness_with(configure: impl FnOnce(&mut crate::config::Config)) -> Harness {
        let mut config = test_config();
        configure(&mut config);
        let state = AppState::with_queue(config, Db::in_memory().await.expect("db"));
        seed_analysis(&state.db).await.expect("user u1");
        let cookie = session_cookie(&state, DEFAULT_TENANT).await;
        Harness { state, cookie }
    }

    async fn harness() -> Harness {
        harness_with(|_| {}).await
    }

    impl Harness {
        async fn send(
            &self,
            method: &str,
            uri: &str,
            headers: &[(&str, &str)],
            body: Option<Value>,
        ) -> (StatusCode, Value) {
            let mut request = Request::builder().method(method).uri(uri);
            for (name, value) in headers {
                request = request.header(*name, *value);
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

        async fn read_as(&self, cookie: &str) -> Value {
            let (status, body) = self
                .send(
                    "GET",
                    GET_DASHBOARD_SETTINGS.path,
                    &[("cookie", cookie)],
                    None,
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            body
        }

        async fn read(&self) -> Value {
            self.read_as(&self.cookie).await
        }

        async fn save_as(&self, cookie: &str, settings: Value) -> (StatusCode, Value) {
            self.send(
                "PUT",
                SET_DASHBOARD_SETTINGS.path,
                &[("cookie", cookie), ("x-csrf-token", "csrf")],
                Some(settings),
            )
            .await
        }

        async fn save(&self, settings: Value) -> (StatusCode, Value) {
            self.save_as(&self.cookie, settings).await
        }

        async fn set_zone(&self, zone: &str) -> (StatusCode, Value) {
            self.send(
                "PUT",
                SET_DASHBOARD_TIME_ZONE.path,
                &[("cookie", &self.cookie), ("x-csrf-token", "csrf")],
                Some(json!({ "time_zone": zone })),
            )
            .await
        }
    }

    fn defaults() -> Value {
        serde_json::to_value(DashboardSettings::default()).expect("JSON")
    }

    fn filled() -> Value {
        let mut settings = DashboardSettings {
            linked_logins: vec!["alice-ci".to_owned()],
            memcan_extra_projects: vec!["release-notes".to_owned()],
            ready_label: "  ready ".to_owned(),
            time_zone: Some("Europe/Warsaw".to_owned()),
            ..DashboardSettings::default()
        };
        for (name, alias) in [("platform", Some("PLAT")), ("legacy-tools", None)] {
            settings.repositories.push(RepositoryRule {
                repo: RepoRef {
                    owner: "acme".to_owned(),
                    name: name.to_owned(),
                },
                alias: alias.map(str::to_owned),
                watch_issues: true,
                ownership_policy: None,
                branch_patterns: Vec::new(),
                ignored_checks: Vec::new(),
            });
        }
        serde_json::to_value(settings).expect("JSON")
    }

    #[tokio::test]
    async fn absent_settings_answer_the_defaults_with_the_own_login() {
        let h = harness().await;
        let view = h.read().await;
        assert_eq!(view["settings"], defaults());
        assert_eq!(view["defaults"], defaults());
        assert_eq!(view["own_login"], "lklimek");
        assert_eq!(view["memcan_configured"], true);
        assert_eq!(view["memcan_projects"], json!([]));
        assert_eq!(view["unreachable"], json!([]));
        assert_eq!(view["settings"]["include_todos"], true, "TODOs default on");
    }

    #[tokio::test]
    async fn memcan_without_a_token_is_not_configured() {
        let h = harness_with(|config| config.memcan.token = SecretString::from("")).await;
        assert_eq!(h.read().await["memcan_configured"], false);
    }

    #[tokio::test]
    async fn saved_settings_are_normalised_stored_and_read_back() {
        let h = harness().await;
        let (status, saved) = h.save(filled()).await;
        assert_eq!(status, StatusCode::OK, "{saved}");
        assert_eq!(saved["settings"]["ready_label"], "ready", "trimmed");
        assert_eq!(
            saved["memcan_projects"],
            json!(["platform", "legacy-tools", "release-notes"])
        );
        assert_eq!(saved["defaults"], defaults());
        assert_eq!(h.read().await, saved);
        let mut changed = filled();
        changed["repositories"] = json!([]);
        changed["include_todos"] = json!(false);
        let (status, saved) = h.save(changed).await;
        assert_eq!(status, StatusCode::OK, "{saved}");
        assert_eq!(saved["memcan_projects"], json!(["release-notes"]));
        assert_eq!(h.read().await["settings"]["include_todos"], false);
    }

    #[tokio::test]
    async fn a_broken_rule_is_422_naming_the_field_and_nothing_is_stored() {
        let h = harness().await;
        let mut clash = filled();
        clash["repositories"][1]["repo"] = json!({"owner": "fork", "name": "Platform"});
        clash["repositories"][0]["alias"] = Value::Null;
        let mut own = filled();
        own["linked_logins"] = json!(["LKlimek"]);
        let mut zone = filled();
        zone["time_zone"] = json!("not a zone");
        for (body, message) in [
            (
                clash,
                "Repository 2: its name Platform is already another row's short name; set a different short name.",
            ),
            (
                own,
                "Linked logins: \"LKlimek\" is your own login, which is always scanned.",
            ),
            (zone, "Time zone: \"not a zone\" is not a known time zone."),
        ] {
            let (status, error) = h.save(body).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{error}");
            assert_eq!(
                error,
                json!({"code": "invalid_dashboard_settings", "message": message})
            );
        }
        assert_eq!(h.read().await["settings"], defaults());
    }

    #[tokio::test]
    async fn a_malformed_body_is_400() {
        let h = harness().await;
        let mut body = filled();
        body["lookback_days"] = Value::Null;
        let (status, error) = h.save(body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
        assert_eq!(error["code"], "invalid_request");
    }

    #[tokio::test]
    async fn writes_need_the_session_and_its_csrf_token() {
        let h = harness().await;
        let cookie = h.cookie.clone();
        for (method, path, body) in [
            ("PUT", SET_DASHBOARD_SETTINGS.path, filled()),
            (
                "PUT",
                SET_DASHBOARD_TIME_ZONE.path,
                json!({"time_zone": "Europe/Warsaw"}),
            ),
        ] {
            let (status, error) = h
                .send(method, path, &[("cookie", &cookie)], Some(body.clone()))
                .await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{path}: {error}");
            assert_eq!(error["code"], "csrf_rejected");
            let (status, _) = h
                .send(
                    method,
                    path,
                    &[
                        ("authorization", "Bearer srp_AbCdEfGh_c2VjcmV0"),
                        ("x-csrf-token", "csrf"),
                    ],
                    Some(body),
                )
                .await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{path}: a bearer token never writes"
            );
        }
        let (status, _) = h.send("GET", GET_DASHBOARD_SETTINGS.path, &[], None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(h.read().await["settings"], defaults());
    }

    #[tokio::test]
    async fn dashboard_settings_of_another_user_are_never_served() {
        let h = harness().await;
        let other = other_user_cookie(&h.state).await;
        let (status, _) = h.save(filled()).await;
        assert_eq!(status, StatusCode::OK);
        let theirs = h.read_as(&other).await;
        assert_eq!(theirs["settings"], defaults());
        assert_eq!(theirs["own_login"], "colleague");
        let mut their_settings = defaults();
        their_settings["lookback_days"] = json!(14);
        let (status, _) = h.save_as(&other, their_settings).await;
        assert_eq!(status, StatusCode::OK);
        let mine = h.read().await;
        assert_eq!(mine["settings"]["lookback_days"], 7);
        assert_eq!(mine["settings"]["linked_logins"], json!(["alice-ci"]));
        assert_eq!(h.read_as(&other).await["settings"]["lookback_days"], 14);
    }

    #[tokio::test]
    async fn time_zone_is_stored_once_alone_and_unknown_zones_are_422() {
        let h = harness().await;
        let (status, body) = h.set_zone("America/Argentina/Buenos_Aires").await;
        assert_eq!((status, body), (StatusCode::NO_CONTENT, Value::Null));
        let mut expected = defaults();
        expected["time_zone"] = json!("America/Argentina/Buenos_Aires");
        assert_eq!(
            h.read().await["settings"],
            expected,
            "a defaults row with the zone"
        );
        let (status, _) = h.save(filled()).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = h.set_zone("UTC").await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let read = h.read().await;
        assert_eq!(
            read["settings"]["time_zone"], "Europe/Warsaw",
            "the first-visit save never replaces a stored zone"
        );
        let mut no_zone = filled();
        no_zone["time_zone"] = Value::Null;
        let (status, _) = h.save(no_zone).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = h.set_zone("UTC").await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let read = h.read().await;
        assert_eq!(read["settings"]["time_zone"], "UTC");
        assert_eq!(
            read["settings"]["linked_logins"],
            json!(["alice-ci"]),
            "the rest kept"
        );
        let (status, error) = h.set_zone("../etc/passwd").await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            error,
            json!({
                "code": "invalid_dashboard_settings",
                "message": "Time zone: \"../etc/passwd\" is not a known time zone."
            })
        );
        assert_eq!(h.read().await["settings"]["time_zone"], "UTC");
    }
}
