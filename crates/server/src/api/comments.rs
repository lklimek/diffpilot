//! Comments in the user's pending review: request parsing and the tenant check; the work lives in
//! `review::comments`.

use std::borrow::Cow;

use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use contracts::api::analyses::AnalysisSummary;
use contracts::api::comments::{
    CreateCommentRequest, CreateCommentResponse, PendingComment, PendingReview,
    UpdateCommentRequest,
};
use contracts::api::error::ApiErrorCode;
use contracts::ids::AnalysisId;
use contracts::routes::{CREATE_COMMENT, DELETE_COMMENT, GET_PENDING_REVIEW, UPDATE_COMMENT};

use crate::api::analyses::{AnalysisApiError, visible_analysis};
use crate::api::error::{ApiErrorSource, ApiFailure};
use crate::auth::access_tokens::github_read;
use crate::auth::{Session, SessionCsrf};
use crate::github::graphql::GraphqlError;
use crate::github::review::PendingReviewError;
use crate::pipeline::PrDiffError;
use crate::review::comments::{self, CommentError};
use crate::state::AppState;

/// A pending-review request failed.
#[derive(Debug, thiserror::Error)]
pub enum CommentsApiError {
    /// Malformed JSON body.
    #[error("the request body is not valid")]
    Json(#[from] JsonRejection),
    /// Malformed path.
    #[error("the path is not valid")]
    Path(#[from] PathRejection),
    /// The analysis is not visible.
    #[error("the analysis is not available")]
    Analysis(#[source] AnalysisApiError),
    /// The pending-review operation failed.
    #[error("the pending-review operation failed")]
    Comment(#[from] CommentError),
}

type CommentsResult<T> = Result<T, ApiFailure<CommentsApiError>>;

fn failure(error: impl Into<CommentsApiError>) -> ApiFailure<CommentsApiError> {
    ApiFailure(error.into())
}

const GITHUB_ERROR: &str = "GitHub did not answer as expected. Try again later.";

impl ApiErrorSource for CommentError {
    fn code(&self) -> ApiErrorCode {
        match self {
            Self::Invalid(_) => ApiErrorCode::InvalidRequest,
            Self::NotFound => ApiErrorCode::NotFound,
            Self::AppNotInstalled => ApiErrorCode::AppNotInstalled,
            Self::PrHeadChanged | Self::MisplacedCommentLeft(_) => ApiErrorCode::PrHeadChanged,
            Self::PublishedMeanwhile { .. } | Self::MisplacedPublishedRemoved => {
                ApiErrorCode::CommentPublished
            }
            Self::PositionRejected => ApiErrorCode::PositionRejected,
            Self::KeyConflict => ApiErrorCode::Conflict,
            Self::Github(PendingReviewError::Graphql(GraphqlError::Unauthorized)) => {
                ApiErrorCode::Unauthenticated
            }
            Self::OutcomeUnknown(_) | Self::Github(_) | Self::App(_) => {
                ApiErrorCode::GithubUnavailable
            }
            Self::Diff(PrDiffError::NotPrepared) => ApiErrorCode::AnalysisNotFinished,
            Self::Token(error) => error.code(),
            Self::Db(error) => error.code(),
            Self::Diff(_) | Self::Mirror(_) => ApiErrorCode::Internal,
        }
    }

    fn public_message(&self) -> Cow<'static, str> {
        Cow::Borrowed(match self {
            Self::Token(error) => return error.public_message(),
            Self::Db(error) => return error.public_message(),
            Self::Invalid(_) => {
                "The comment is not valid: write some text (at most 65536 characters) on existing lines."
            }
            Self::NotFound => {
                "The comment is not in your pending review. It may have been deleted on GitHub."
            }
            Self::AppNotInstalled => {
                "Commenting needs the smart-review GitHub App installed on this repository."
            }
            Self::PrHeadChanged => {
                "The pull request changed since this analysis. Analyse it again to comment."
            }
            Self::MisplacedCommentLeft(_) => {
                "The pull request changed while the comment was being saved, and the comment could not be removed again. It may remain in your pending review on GitHub; check it there."
            }
            Self::MisplacedPublishedRemoved => {
                "The pull request changed while the comment was being saved, and your review was published on GitHub in the meantime. The comment had been published on other code and was removed again. Analyse the pull request again to comment."
            }
            Self::PublishedMeanwhile { deleted: false } => {
                "This comment was published on GitHub in the meantime; the change was applied to the published comment."
            }
            Self::PublishedMeanwhile { deleted: true } => {
                "This comment was published on GitHub in the meantime; the published comment was deleted."
            }
            Self::PositionRejected => {
                "GitHub does not accept a comment on these lines. Copy the link to the lines instead."
            }
            Self::KeyConflict => {
                "This comment was already sent and GitHub has not confirmed it. Check your pending review before sending it again."
            }
            Self::OutcomeUnknown(_) => {
                "GitHub did not confirm the comment. Check your pending review before sending it again."
            }
            Self::Github(PendingReviewError::Graphql(GraphqlError::Unauthorized)) => {
                "Your GitHub sign-in has ended. Sign in again."
            }
            Self::Github(_) | Self::App(_) => GITHUB_ERROR,
            Self::Diff(PrDiffError::NotPrepared) => "The analysis has not finished yet.",
            Self::Diff(_) | Self::Mirror(_) => {
                "Something went wrong on the server. Try again later."
            }
        })
    }
}

impl ApiErrorSource for CommentsApiError {
    fn code(&self) -> ApiErrorCode {
        match self {
            Self::Json(rejection) => rejection.code(),
            Self::Path(rejection) => rejection.code(),
            Self::Analysis(error) => error.code(),
            Self::Comment(error) => error.code(),
        }
    }

    fn public_message(&self) -> Cow<'static, str> {
        match self {
            Self::Json(rejection) => rejection.public_message(),
            Self::Path(rejection) => rejection.public_message(),
            Self::Analysis(error) => error.public_message(),
            Self::Comment(error) => error.public_message(),
        }
    }
}

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(GET_PENDING_REVIEW.path, get(list))
        .route(CREATE_COMMENT.path, post(create))
        .route(UPDATE_COMMENT.path, patch(update))
        .route(DELETE_COMMENT.path, delete(remove))
}

async fn visible(
    state: &AppState,
    session: &Session,
    id: AnalysisId,
) -> CommentsResult<AnalysisSummary> {
    visible_analysis(state, session, id)
        .await
        .map_err(|ApiFailure(error)| failure(CommentsApiError::Analysis(error)))
}

async fn list(
    State(state): State<AppState>,
    session: Session,
    id: Result<Path<AnalysisId>, PathRejection>,
) -> CommentsResult<Json<PendingReview>> {
    let Path(id) = id.map_err(failure)?;
    let summary = visible(&state, &session, id).await?;
    let review = github_read(
        &session,
        format!("pending-review:{id}"),
        comments::list(&state, &session.user_id, &summary),
    )
    .await
    .map_err(failure)?;
    Ok(Json(review))
}

async fn create(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    id: Result<Path<AnalysisId>, PathRejection>,
    body: Result<Json<CreateCommentRequest>, JsonRejection>,
) -> CommentsResult<(StatusCode, Json<CreateCommentResponse>)> {
    let Path(id) = id.map_err(failure)?;
    let Json(request) = body.map_err(failure)?;
    let summary = visible(&state, &session, id).await?;
    let response = comments::create(&state, &session.user_id, &summary, &request)
        .await
        .map_err(failure)?;
    Ok((StatusCode::CREATED, Json(response)))
}

async fn update(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    ids: Result<Path<(AnalysisId, String)>, PathRejection>,
    body: Result<Json<UpdateCommentRequest>, JsonRejection>,
) -> CommentsResult<Json<PendingComment>> {
    let Path((id, comment_id)) = ids.map_err(failure)?;
    let Json(request) = body.map_err(failure)?;
    let summary = visible(&state, &session, id).await?;
    let comment = comments::update(
        &state,
        &session.user_id,
        &summary,
        &comment_id,
        &request.body,
    )
    .await
    .map_err(failure)?;
    Ok(Json(comment))
}

async fn remove(
    State(state): State<AppState>,
    SessionCsrf(session): SessionCsrf,
    ids: Result<Path<(AnalysisId, String)>, PathRejection>,
) -> CommentsResult<StatusCode> {
    let Path((id, comment_id)) = ids.map_err(failure)?;
    let summary = visible(&state, &session, id).await?;
    comments::delete(&state, &session.user_id, &summary, &comment_id)
        .await
        .map_err(failure)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use db::repo::DEFAULT_TENANT;
    use db::repo::analyses::testing::seed_tenant;
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tower::ServiceExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::api::router;
    use crate::review::tests::{USER_TOKEN, github_state};
    use crate::test_support::logs::LogCapture;
    use crate::test_support::session_cookie;

    const COMMENT: &str = "PRRC_fixture482_c1";

    /// Every route of this module: (method, path, body).
    fn routes(analysis: AnalysisId) -> [(&'static str, String, Option<Value>); 4] {
        let base = format!("/api/v1/analyses/{analysis}/pending-review");
        let create = json!({"idempotency_key": "4f6c5a5e-1f43-4d6b-9e4f-6f0d1c2b3a49",
            "target": {"path": "src/rpc/client.rs", "side": "head", "start_line": null, "line": 96},
            "body": "b"});
        [
            ("GET", base.clone(), None),
            ("POST", format!("{base}/comments"), Some(create)),
            (
                "PATCH",
                format!("{base}/comments/{COMMENT}"),
                Some(json!({"body": "b"})),
            ),
            ("DELETE", format!("{base}/comments/{COMMENT}"), None),
        ]
    }

    async fn send(
        state: &AppState,
        (method, uri, body): &(&'static str, String, Option<Value>),
        cookie: Option<&str>,
        csrf: bool,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder().method(*method).uri(uri);
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
        let response = router(state.clone())
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
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, body)
    }

    /// Route auth table: GET `session`; POST, PATCH, DELETE `session_csrf`; another tenant's analysis is
    /// `not_found`. GitHub is never called.
    #[tokio::test]
    async fn comment_routes_check_session_csrf_and_tenant() {
        let server = MockServer::start().await;
        Mock::given(wiremock::matchers::any())
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;
        let (state, analysis) = github_state(&server, true).await;
        seed_tenant(&state.db, "other").await.expect("tenant");
        let other = session_cookie(&state, "other").await;
        let own = session_cookie(&state, DEFAULT_TENANT).await;
        for route in routes(analysis) {
            let (status, body) = send(&state, &route, None, true).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{}: {body}", route.1);
            if route.0 != "GET" {
                let (status, body) = send(&state, &route, Some(&own), false).await;
                assert_eq!(
                    status,
                    StatusCode::FORBIDDEN,
                    "{} {}: {body}",
                    route.0,
                    route.1
                );
                assert_eq!(body["code"], json!("csrf_rejected"));
            }
            let (status, body) = send(&state, &route, Some(&other), true).await;
            assert_eq!(
                status,
                StatusCode::NOT_FOUND,
                "{} {}: {body}",
                route.0,
                route.1
            );
            assert_eq!(body["code"], json!("not_found"));
        }
    }

    /// GitHub failures of every route: plain message, no token in the answer or the logs.
    #[tokio::test]
    async fn token_never_in_logs_or_errors() {
        for (status, graphql) in [
            (401, json!({})),
            (502, json!({})),
            (
                200,
                json!({"data": null, "errors": [{"type": "FORBIDDEN", "message": USER_TOKEN}]}),
            ),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/api/graphql"))
                .respond_with(ResponseTemplate::new(status).set_body_json(graphql))
                .mount(&server)
                .await;
            let (state, analysis) = github_state(&server, true).await;
            let cookie = session_cookie(&state, DEFAULT_TENANT).await;
            let capture = LogCapture::start();
            for route in routes(analysis) {
                if route.0 == "POST" {
                    continue; // needs the mirror; its GitHub errors are covered by tests/review.rs
                }
                let (code, body) = send(&state, &route, Some(&cookie), true).await;
                assert!(code.is_client_error() || code.is_server_error(), "{code}");
                assert!(!body.to_string().contains(USER_TOKEN), "{body}");
                assert!(!body.to_string().contains("ghu_"), "{body}");
            }
            let logs = capture.text();
            assert!(logs.contains("API request"), "{logs}");
            assert!(!logs.contains(USER_TOKEN), "{status}: {logs}");
        }
    }

    #[tokio::test]
    async fn listing_reads_the_pending_review_from_github() {
        let server = MockServer::start().await;
        let none = json!({"hasNextPage": false, "endCursor": null});
        let response = json!({"data": {"repository": {"pullRequest": {
            "id": "PR_1", "headRefOid": "h",
            "reviews": {"pageInfo": none, "nodes": [{"id": "R1", "viewerDidAuthor": true, "comments": {"pageInfo": none,
                "nodes": [{"id": COMMENT, "path": "src/rpc/client.rs", "line": 96, "startLine": null,
                "subjectType": "LINE", "body": "b", "replyTo": null}]}}]},
            "reviewThreads": {"pageInfo": {"hasNextPage": false, "endCursor": null}, "nodes": [
                {"id": "PRRT_1", "diffSide": "RIGHT", "comments": {"nodes": [{"id": COMMENT}]}}]}
        }}}});
        Mock::given(method("POST"))
            .and(path("/api/graphql"))
            .and(wiremock::matchers::header(
                "authorization",
                format!("Bearer {USER_TOKEN}").as_str(),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .expect(1)
            .mount(&server)
            .await;
        let (state, analysis) = github_state(&server, true).await;
        let cookie = session_cookie(&state, DEFAULT_TENANT).await;
        let (status, body) = send(&state, &routes(analysis)[0], Some(&cookie), false).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body,
            json!({"commenting_available": true, "comments": [{"id": COMMENT, "thread_id": "PRRT_1",
                "path": "src/rpc/client.rs", "kind": "line", "side": "head", "start_line": null,
                "line": 96, "body": "b"}], "incomplete": false})
        );
    }

    #[tokio::test]
    async fn private_pending_reviews_and_foreign_comment_ids_are_scoped_to_each_user() {
        use crate::github::review::PENDING_REVIEW;
        use wiremock::matchers::{body_partial_json, header};
        let server = MockServer::start().await;
        let (state, analysis) = github_state(&server, true).await;
        let head = state
            .db
            .analyses()
            .summary(analysis)
            .await
            .expect("summary")
            .pr
            .head_sha
            .0;
        let cookie_a = session_cookie(&state, DEFAULT_TENANT).await;
        let cookie_b = crate::test_support::other_user_cookie(&state).await;
        let lock = crate::auth::lock_user("u2").await;
        crate::auth::store_sign_in_tokens(
            &state,
            &lock,
            DEFAULT_TENANT,
            &crate::github::oauth::TokenGrant {
                access_token: secrecy::SecretString::from("ghu_private_test_b"),
                expires_in: 3600,
                refresh_token: secrecy::SecretString::from("ghr_private_test_b"),
                refresh_token_expires_in: 7200,
            },
            chrono::Utc::now(),
        )
        .await
        .expect("tokens");
        drop(lock);
        let none = json!({"hasNextPage": false, "endCursor": null});
        for (index, token, cookie) in [
            (0, USER_TOKEN, cookie_a),
            (1, "ghu_private_test_b", cookie_b),
        ] {
            let reviews: Vec<_> = (0..2).map(|owner| json!({"id": format!("R{owner}"), "viewerDidAuthor": owner == index,
                "comments": {"pageInfo": none, "nodes": [{"id": format!("C{owner}"), "path":"src/rpc/client.rs", "line":96,
                "startLine":null, "subjectType":"LINE", "body":format!("PRIVATE_{owner}"), "replyTo":null}]}})).collect();
            Mock::given(method("POST"))
                .and(path("/api/graphql"))
                .and(header("authorization", format!("Bearer {token}").as_str()))
                .and(body_partial_json(json!({"query":PENDING_REVIEW})))
                .respond_with(ResponseTemplate::new(200).set_body_json(
                    json!({"data":{"repository":{"pullRequest":{
                        "id":"PR_1", "headRefOid":head, "reviews":{"pageInfo":none,"nodes":reviews},
                        "reviewThreads":{"pageInfo":none,"nodes":[]}
                    }}}}),
                ))
                .expect(3)
                .mount(&server)
                .await;
            let (status, listed) = send(&state, &routes(analysis)[0], Some(&cookie), false).await;
            assert_eq!(status, StatusCode::OK, "{listed}");
            assert_eq!(listed["comments"].as_array().expect("comments").len(), 1);
            assert_eq!(listed["comments"][0]["body"], format!("PRIVATE_{index}"));
            assert!(
                !listed
                    .to_string()
                    .contains(&format!("PRIVATE_{}", 1 - index))
            );
            // A retry of POST uses only this user's cached pending-comment response, even with
            // the same idempotency key on both browsers. No foreign local response may be returned.
            let post = routes(analysis)[1].clone();
            let request: CreateCommentRequest =
                serde_json::from_value(post.2.clone().expect("body")).expect("request");
            let user = if index == 0 { "u1" } else { "u2" };
            let key_text = request.idempotency_key.to_string();
            let key = db::repo::comments::CommentRequestKey {
                user_id: user,
                idempotency_key: &key_text,
            };
            let hash = crate::secrets::sha256(
                &serde_json::to_vec(&(&request.target, &request.body)).expect("hash"),
            );
            let started = state
                .db
                .comment_requests()
                .begin::<Value>(key, analysis, &hash, chrono::Utc::now())
                .await
                .expect("start");
            assert!(matches!(
                started,
                db::repo::comments::CommentRequestStart::Started
            ));
            let response = json!({"comment": {"id":format!("C{index}"), "thread_id":format!("T{index}"),
                "path":"src/rpc/client.rs", "kind":"line", "side":"head", "start_line":null, "line":96,
                "body":format!("PRIVATE_RESPONSE_{index}")}, "missed_by_llm_recorded":false});
            state
                .db
                .comment_requests()
                .complete(key, &response)
                .await
                .expect("complete");
            let (status, created) = send(&state, &post, Some(&cookie), true).await;
            assert_eq!(status, StatusCode::CREATED, "{created}");
            assert_eq!(created, response);
            for route in &routes(analysis)[2..] {
                let (status, body) = send(&state, route, Some(&cookie), true).await;
                assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
            }
        }
    }

    #[test]
    fn races_with_github_have_their_own_plain_outcomes() {
        let published = CommentError::PublishedMeanwhile { deleted: false };
        assert_eq!(published.code(), ApiErrorCode::CommentPublished);
        assert!(
            published
                .public_message()
                .contains("published on GitHub in the meantime")
        );
        let deleted = CommentError::PublishedMeanwhile { deleted: true };
        assert_eq!(deleted.code(), ApiErrorCode::CommentPublished);
        let removed = CommentError::MisplacedPublishedRemoved;
        assert_eq!(removed.code(), ApiErrorCode::CommentPublished);
        assert!(removed.public_message().contains("was removed"));
        let left = CommentError::MisplacedCommentLeft(PendingReviewError::MissingPullRequest);
        assert_eq!(left.code(), ApiErrorCode::PrHeadChanged);
        assert!(
            left.public_message()
                .contains("may remain in your pending review")
        );
    }
}
