//! Mapping of handler errors to `ApiError` responses (`docs/contracts/http-api.md`). Each module implements
//! [`ApiErrorSource`] for its own error enum and returns `ApiFailure<E>`; this file is not edited per module.

use std::borrow::Cow;

use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use contracts::api::error::{ApiError, ApiErrorCode};
use db::error::DbError;

use crate::jobs::executor::ExecutorError;

const INTERNAL_MESSAGE: &str = "Something went wrong on the server. Try again later.";
const INVALID_REQUEST_MESSAGE: &str = "The request is not valid.";

/// The HTTP status of an API error code (single source: `ApiErrorCode::http_status`).
pub fn status(code: ApiErrorCode) -> StatusCode {
    StatusCode::from_u16(code.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
}

/// An error a handler can return: a closed code and a plain message that never contains internal error text.
pub trait ApiErrorSource: std::error::Error {
    /// The API error code.
    fn code(&self) -> ApiErrorCode;
    /// Plain, user-facing message.
    fn public_message(&self) -> Cow<'static, str>;
}

/// Handler error wrapper that renders an [`ApiError`] response and logs the full error chain.
#[derive(Debug)]
pub struct ApiFailure<E>(pub E);

impl<E: ApiErrorSource> From<E> for ApiFailure<E> {
    fn from(error: E) -> Self {
        Self(error)
    }
}

impl<E: ApiErrorSource> IntoResponse for ApiFailure<E> {
    fn into_response(self) -> Response {
        let code = self.0.code();
        let status = status(code);
        if status.is_server_error() {
            tracing::error!(error = ?self.0, ?code, "API request failed with a server error");
        } else {
            tracing::debug!(error = ?self.0, ?code, "API request rejected");
        }
        let body = ApiError {
            code,
            message: self.0.public_message().into_owned(),
        };
        (status, Json(body)).into_response()
    }
}

impl ApiErrorSource for DbError {
    fn code(&self) -> ApiErrorCode {
        match self {
            Self::NotFound => ApiErrorCode::NotFound,
            Self::Conflict => ApiErrorCode::Conflict,
            Self::NotImplemented { .. }
            | Self::Corrupt { .. }
            | Self::Json(_)
            | Self::Storage(_) => ApiErrorCode::Internal,
        }
    }

    fn public_message(&self) -> Cow<'static, str> {
        Cow::Borrowed(match self {
            Self::NotFound => "The requested item was not found.",
            Self::Conflict => "The request conflicts with the current state. Reload and try again.",
            Self::NotImplemented { .. }
            | Self::Corrupt { .. }
            | Self::Json(_)
            | Self::Storage(_) => INTERNAL_MESSAGE,
        })
    }
}

impl ApiErrorSource for ExecutorError {
    fn code(&self) -> ApiErrorCode {
        match self {
            Self::Db(error) => error.code(),
            Self::UnknownJob(_) => ApiErrorCode::NotFound,
            Self::Closed | Self::NotImplemented { .. } => ApiErrorCode::Internal,
        }
    }

    fn public_message(&self) -> Cow<'static, str> {
        match self {
            Self::Db(error) => error.public_message(),
            Self::UnknownJob(_) => Cow::Borrowed("The requested job was not found."),
            Self::Closed | Self::NotImplemented { .. } => Cow::Borrowed(INTERNAL_MESSAGE),
        }
    }
}

macro_rules! invalid_request {
    ($($rejection:ty),+) => {$(
        impl ApiErrorSource for $rejection {
            fn code(&self) -> ApiErrorCode {
                ApiErrorCode::InvalidRequest
            }

            fn public_message(&self) -> Cow<'static, str> {
                Cow::Borrowed(INVALID_REQUEST_MESSAGE)
            }
        }
    )+};
}

invalid_request!(JsonRejection, QueryRejection, PathRejection);

#[cfg(test)]
mod tests {
    use http_body_util::BodyExt;
    use serde_json::json;

    use super::*;

    async fn render<E: ApiErrorSource>(error: E) -> (StatusCode, serde_json::Value) {
        let response = ApiFailure(error).into_response();
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        (status, serde_json::from_slice(&bytes).expect("JSON body"))
    }

    #[test]
    fn status_mapping_covers_every_code() {
        for code in ApiErrorCode::ALL {
            assert_eq!(status(code).as_u16(), code.http_status(), "{code:?}");
        }
    }

    #[tokio::test]
    async fn responses_carry_code_and_plain_message() {
        let (status, body) = render(DbError::NotFound).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(
            body,
            json!({"code": "not_found", "message": "The requested item was not found."})
        );
        let (status, body) = render(DbError::Conflict).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["code"], json!("conflict"));
    }

    #[tokio::test]
    async fn internal_errors_hide_their_details() {
        let json_error =
            serde_json::from_str::<u32>("secret-looking internal text").expect_err("not JSON");
        let detail = json_error.to_string();
        let (status, body) = render(ExecutorError::Db(DbError::Json(json_error))).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            body,
            json!({"code": "internal", "message": INTERNAL_MESSAGE})
        );
        assert!(!body.to_string().contains(&detail));
        let (_, body) = render(DbError::NotImplemented {
            operation: "jobs::claim",
        })
        .await;
        assert!(!body.to_string().contains("jobs::claim"));
    }
}
