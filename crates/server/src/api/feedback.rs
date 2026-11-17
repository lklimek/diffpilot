//! Feedback on steps, annotations and lines: list, record, remove (session, CSRF and tenant check; the work
//! lives in `memcan::feedback`).

use std::borrow::Cow;

use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::Utc;
use contracts::api::analyses::AnalysisSummary;
use contracts::api::error::ApiErrorCode;
use contracts::api::feedback::{Feedback, FeedbackRequest};
use contracts::ids::{AnalysisId, FeedbackId};
use contracts::routes::{DELETE_FEEDBACK, LIST_FEEDBACK, POST_FEEDBACK};
use db::error::DbError;

use crate::api::analyses::{AnalysisApiError, visible_analysis};
use crate::api::error::{ApiErrorSource, ApiFailure};
use crate::auth::{Session, SessionCsrf};
use crate::memcan::feedback::{self, FeedbackError};
use crate::state::AppState;

/// A feedback request failed.
#[derive(Debug, thiserror::Error)]
pub enum FeedbackApiError {
    /// Malformed JSON body (also a value clients may not send, such as `missed_by_llm`).
    #[error("the request body is not valid")]
    Json(#[from] JsonRejection),
    /// Malformed path.
    #[error("the path is not valid")]
    Path(#[from] PathRejection),
    /// The analysis is unknown or not visible.
    #[error("the analysis is not available")]
    Analysis(#[from] AnalysisApiError),
    /// The feedback could not be recorded.
    #[error("the feedback could not be recorded")]
    Feedback(#[source] FeedbackError),
    /// The database failed.
    #[error("the database failed")]
    Db(#[from] DbError),
}

impl From<FeedbackError> for FeedbackApiError {
    fn from(error: FeedbackError) -> Self {
        match error {
            FeedbackError::Diff(error) => Self::Analysis(error.into()),
            FeedbackError::Db(error) => Self::Db(error),
            error => Self::Feedback(error),
        }
    }
}

impl ApiErrorSource for FeedbackApiError {
    fn code(&self) -> ApiErrorCode {
        match self {
            Self::Json(rejection) => rejection.code(),
            Self::Path(rejection) => rejection.code(),
            Self::Analysis(error) => error.code(),
            Self::Feedback(FeedbackError::NoteTooLong | FeedbackError::UnknownTarget) => {
                ApiErrorCode::InvalidRequest
            }
            Self::Feedback(FeedbackError::Diff(_)) => ApiErrorCode::Internal,
            Self::Feedback(FeedbackError::Db(error)) | Self::Db(error) => error.code(),
        }
    }

    fn public_message(&self) -> Cow<'static, str> {
        match self {
            Self::Json(rejection) => rejection.public_message(),
            Self::Path(rejection) => rejection.public_message(),
            Self::Analysis(error) => error.public_message(),
            Self::Feedback(FeedbackError::NoteTooLong) => {
                Cow::Borrowed("The note is too long; write at most 2000 characters.")
            }
            Self::Feedback(FeedbackError::UnknownTarget) => {
                Cow::Borrowed("The step, annotation or changed line is not part of this analysis.")
            }
            Self::Feedback(FeedbackError::Diff(_)) => {
                Cow::Borrowed("Something went wrong on the server. Try again later.")
            }
            Self::Feedback(FeedbackError::Db(error)) | Self::Db(error) => error.public_message(),
        }
    }
}

type FeedbackResult<T> = Result<T, ApiFailure<FeedbackApiError>>;

fn failure(error: impl Into<FeedbackApiError>) -> ApiFailure<FeedbackApiError> {
    ApiFailure(error.into())
}

/// The analysis if the session's tenant may see it; `not_found` otherwise.
async fn visible(
    state: &AppState,
    session: &Session,
    id: AnalysisId,
) -> FeedbackResult<AnalysisSummary> {
    visible_analysis(state, session, id)
        .await
        .map_err(|ApiFailure(error)| failure(error))
}

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(LIST_FEEDBACK.path, get(list))
        .route(POST_FEEDBACK.path, post(create))
        .route(DELETE_FEEDBACK.path, delete(remove))
}

async fn list(
    State(state): State<AppState>,
    session: Session,
    id: Result<Path<AnalysisId>, PathRejection>,
) -> FeedbackResult<Json<Vec<Feedback>>> {
    let Path(id) = id.map_err(failure)?;
    let analysis = visible(&state, &session, id).await?;
    let entries = state
        .db
        .feedback()
        .list(analysis.id, &session.user_id)
        .await
        .map_err(failure)?;
    Ok(Json(entries))
}

async fn create(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    id: Result<Path<AnalysisId>, PathRejection>,
    body: Result<Json<FeedbackRequest>, JsonRejection>,
) -> FeedbackResult<Json<Feedback>> {
    let Path(id) = id.map_err(failure)?;
    let Json(request) = body.map_err(failure)?;
    let analysis = visible(&state, &session, id).await?;
    let stored = feedback::record(&state, &session.user_id, &analysis, request, Utc::now())
        .await
        .map_err(failure)?;
    Ok(Json(stored))
}

async fn remove(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    ids: Result<Path<(AnalysisId, FeedbackId)>, PathRejection>,
) -> FeedbackResult<StatusCode> {
    let Path((id, feedback_id)) = ids.map_err(failure)?;
    let analysis = visible(&state, &session, id).await?;
    feedback::remove(
        &state.db,
        &session.user_id,
        analysis.id,
        feedback_id,
        Utc::now(),
    )
    .await
    .map_err(failure)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use std::path::Path as FsPath;

    use axum::body::Body;
    use axum::http::{Request, Response};
    use contracts::agent::analyse_flow::FlowAnalysisOutput;
    use contracts::api::feedback::FeedbackValue;
    use db::repo::DEFAULT_TENANT;
    use db::repo::analyses::testing::{seed_analysis, seed_tenant};
    use db::repo::documents::DocumentKind;
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;
    use crate::api::router;
    use crate::state::tests::test_state;
    use crate::test_support::session_cookie;

    struct Fixture {
        state: AppState,
        analysis: AnalysisId,
        cookie: String,
    }

    async fn fixture() -> Fixture {
        let state = test_state().await;
        let analysis = seed_analysis(&state.db).await.expect("seed");
        let path =
            FsPath::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pr-482/agent/flow-3.json");
        let flow: FlowAnalysisOutput =
            serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("flow");
        state
            .db
            .documents()
            .put(analysis, DocumentKind::Flow, "f3", &flow, Utc::now())
            .await
            .expect("flow");
        let cookie = session_cookie(&state, DEFAULT_TENANT).await;
        Fixture {
            state,
            analysis,
            cookie,
        }
    }

    fn path(analysis: AnalysisId) -> String {
        LIST_FEEDBACK.path.replace("{id}", &analysis.to_string())
    }

    impl Fixture {
        async fn send(&self, request: Request<Body>) -> (StatusCode, Value) {
            let response: Response<Body> = router(self.state.clone())
                .oneshot(request)
                .await
                .expect("infallible");
            let status = response.status();
            let body = response
                .into_body()
                .collect()
                .await
                .expect("body")
                .to_bytes();
            (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
        }

        async fn post_as(
            &self,
            analysis: AnalysisId,
            cookie: &str,
            csrf: Option<&str>,
            body: Value,
        ) -> (StatusCode, Value) {
            let mut request = Request::post(path(analysis))
                .header("cookie", cookie)
                .header("content-type", "application/json");
            if let Some(csrf) = csrf {
                request = request.header("x-csrf-token", csrf);
            }
            self.send(request.body(Body::from(body.to_string())).expect("request"))
                .await
        }

        async fn post(&self, body: Value) -> (StatusCode, Value) {
            self.post_as(self.analysis, &self.cookie, Some("csrf"), body)
                .await
        }

        async fn list_as(&self, analysis: AnalysisId, cookie: &str) -> (StatusCode, Value) {
            self.send(
                Request::get(path(analysis))
                    .header("cookie", cookie)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
        }

        async fn list(&self) -> Vec<Value> {
            let (status, body) = self.list_as(self.analysis, &self.cookie).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            body.as_array().cloned().unwrap_or_default()
        }

        async fn delete_as(
            &self,
            analysis: AnalysisId,
            id: &str,
            cookie: &str,
            csrf: Option<&str>,
        ) -> StatusCode {
            let mut request = Request::delete(
                DELETE_FEEDBACK
                    .path
                    .replace("{id}", &analysis.to_string())
                    .replace("{feedback_id}", id),
            )
            .header("cookie", cookie);
            if let Some(csrf) = csrf {
                request = request.header("x-csrf-token", csrf);
            }
            self.send(request.body(Body::empty()).expect("request"))
                .await
                .0
        }
    }

    fn on_step(value: &str, note: Option<&str>) -> Value {
        json!({"target": {"target": "step", "step_id": "f3.1"}, "value": value, "note": note})
    }

    #[tokio::test]
    async fn private_feedback_notes_and_foreign_ids_are_hidden_from_the_other_reader() {
        let f = fixture().await;
        let (_, own) = f.post(on_step("wrong", Some("PRIVATE_NOTE"))).await;
        let cookie = crate::test_support::other_user_cookie(&f.state).await;
        let (status, listed) = f.list_as(f.analysis, &cookie).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(listed, json!([]));
        assert_eq!(
            f.delete_as(
                f.analysis,
                own["id"].as_str().expect("id"),
                &cookie,
                Some("csrf")
            )
            .await,
            StatusCode::NOT_FOUND
        );
        let (status, other) = f
            .post_as(
                f.analysis,
                &cookie,
                Some("csrf"),
                on_step("wrong", Some("OTHER_NOTE")),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_ne!(own["id"], other["id"]);
        assert_eq!(f.list().await, [own]);
    }

    #[tokio::test]
    async fn hard_case_1_wrong_and_overrated_are_both_stored() {
        let f = fixture().await;
        let (status, wrong) = f.post(on_step("wrong", Some("  Not a defect.  "))).await;
        assert_eq!(status, StatusCode::OK, "{wrong}");
        assert_eq!(wrong["note"], "Not a defect.");
        let (status, overrated) = f.post(on_step("overrated", None)).await;
        assert_eq!(status, StatusCode::OK, "{overrated}");
        let values: Vec<Value> = f
            .list()
            .await
            .iter()
            .map(|entry| entry["value"].clone())
            .collect();
        assert_eq!(values, [json!("wrong"), json!("overrated")]);
        let pending = f
            .state
            .db
            .feedback()
            .pending_memcan(Utc::now(), 10)
            .await
            .expect("queue");
        assert_eq!(pending.len(), 2, "queued for MemCan");
    }

    #[tokio::test]
    async fn posting_a_value_again_replaces_its_note_and_underrated_removes_overrated() {
        let f = fixture().await;
        let (_, first) = f.post(on_step("overrated", Some("a"))).await;
        let (_, again) = f.post(on_step("overrated", Some("b"))).await;
        assert_eq!(again["id"], first["id"]);
        assert_eq!(f.list().await, [again]);
        let (status, underrated) = f.post(on_step("underrated", Some(""))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(underrated["note"], Value::Null, "a blank note is no note");
        assert_eq!(f.list().await, [underrated]);
    }

    #[tokio::test]
    async fn missed_by_llm_is_rejected_from_clients() {
        let f = fixture().await;
        let body = json!({
            "target": {"target": "line", "path": "src/rpc/client.rs", "side": "head", "line": 96},
            "value": "missed_by_llm",
            "note": null
        });
        let (status, error) = f.post(body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(error["code"], "invalid_request");
        assert!(f.list().await.is_empty());
    }

    #[tokio::test]
    async fn unknown_targets_and_long_notes_are_invalid_requests() {
        let f = fixture().await;
        let long = "n".repeat(FeedbackRequest::MAX_NOTE_CHARS + 1);
        for body in [
            json!({"target": {"target": "step", "step_id": "f3.9"}, "value": "wrong", "note": null}),
            json!({"target": {"target": "annotation", "annotation_id": "f3.1.a9"}, "value": "wrong", "note": null}),
            json!({"target": {"target": "line", "path": "", "side": "head", "line": 3}, "value": "wrong", "note": null}),
            json!({"target": {"target": "line", "path": "a.rs", "side": "head", "line": 0}, "value": "wrong", "note": null}),
            on_step("wrong", Some(&long)),
        ] {
            let (status, error) = f.post(body.clone()).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(error["code"], "invalid_request", "{body}");
        }
        let exact = "n".repeat(FeedbackRequest::MAX_NOTE_CHARS);
        assert_eq!(
            f.post(on_step("correct", Some(&exact))).await.0,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn delete_removes_one_value_of_the_users_own_entries() {
        let f = fixture().await;
        let (_, wrong) = f.post(on_step("wrong", None)).await;
        let (_, overrated) = f.post(on_step("overrated", None)).await;
        let id = overrated["id"].as_str().expect("id");
        assert_eq!(
            f.delete_as(f.analysis, id, &f.cookie, Some("csrf")).await,
            StatusCode::NO_CONTENT
        );
        assert_eq!(f.list().await, [wrong]);
        assert_eq!(
            f.delete_as(f.analysis, id, &f.cookie, Some("csrf")).await,
            StatusCode::NOT_FOUND
        );
        let other = seed_analysis(&f.state.db).await.expect("other analysis");
        let wrong_id = f.list().await[0]["id"].as_str().expect("id").to_owned();
        assert_eq!(
            f.delete_as(other, &wrong_id, &f.cookie, Some("csrf")).await,
            StatusCode::NOT_FOUND,
            "id of another analysis"
        );
        assert_eq!(
            f.delete_as(f.analysis, "not-a-uuid", &f.cookie, Some("csrf"))
                .await,
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn mutations_need_session_and_csrf() {
        let f = fixture().await;
        let (status, error) = f
            .post_as(f.analysis, &f.cookie, None, on_step("wrong", None))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(error["code"], "csrf_rejected");
        let (status, _) = f
            .post_as(f.analysis, &f.cookie, Some("other"), on_step("wrong", None))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = f
            .post_as(f.analysis, "", Some("csrf"), on_step("wrong", None))
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (_, kept) = f.post(on_step("wrong", None)).await;
        let id = kept["id"].as_str().expect("id");
        assert_eq!(
            f.delete_as(f.analysis, id, &f.cookie, None).await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(f.list_as(f.analysis, "").await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(
            f.list().await,
            [kept],
            "nothing changed by rejected requests"
        );
    }

    #[tokio::test]
    async fn another_tenants_analysis_is_not_found() {
        let f = fixture().await;
        let (_, mine) = f.post(on_step("wrong", None)).await;
        let id = mine["id"].as_str().expect("id");
        seed_tenant(&f.state.db, "other").await.expect("tenant");
        let foreign = session_cookie(&f.state, "other").await;
        assert_eq!(
            f.list_as(f.analysis, &foreign).await.0,
            StatusCode::NOT_FOUND
        );
        let (status, error) = f
            .post_as(f.analysis, &foreign, Some("csrf"), on_step("correct", None))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(error["code"], "not_found");
        assert_eq!(
            f.delete_as(f.analysis, id, &foreign, Some("csrf")).await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            f.list_as(AnalysisId::new_random(), &f.cookie).await.0,
            StatusCode::NOT_FOUND
        );
        let entries = f
            .state
            .db
            .feedback()
            .list(f.analysis, "u1")
            .await
            .expect("list");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].value, FeedbackValue::Wrong);
    }

    #[tokio::test]
    async fn only_canonical_step_and_annotation_ids_are_accepted() {
        let f = fixture().await;
        assert_eq!(f.post(on_step("overrated", None)).await.0, StatusCode::OK);
        for alias in [
            json!({"target": {"target": "step", "step_id": "f3.01"}, "value": "underrated", "note": null}),
            json!({"target": {"target": "annotation", "annotation_id": "f3.1.a02"}, "value": "underrated", "note": null}),
        ] {
            let (status, error) = f.post(alias.clone()).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{alias}");
            assert_eq!(error["code"], "invalid_request");
        }
        let values: Vec<Value> = f
            .list()
            .await
            .iter()
            .map(|entry| entry["value"].clone())
            .collect();
        assert_eq!(
            values,
            [json!("overrated")],
            "the alias cannot bypass the exclusivity"
        );
    }

    #[tokio::test]
    async fn entries_report_their_memcan_copy_as_pending_until_confirmed() {
        let f = fixture().await;
        let (_, stored) = f.post(on_step("overrated", None)).await;
        assert_eq!(stored["memory"], "pending");
        assert_eq!(f.list().await[0]["memory"], "pending");
    }
}
