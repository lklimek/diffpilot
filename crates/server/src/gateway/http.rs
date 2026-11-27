//! Separate worker and scoped administration listeners.

use super::{Error, Gateway};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, Request, State, rejection::QueryRejection},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use contracts::{
    gateway::{LinkCredential, MintKey, RevokeKeys, UnlinkCredential},
    ids::UserId,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

pub(super) fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

pub(super) fn model_router(gateway: Arc<Gateway>) -> Router {
    Router::new()
        .route("/v1/responses", post(responses))
        .route("/v1/models", get(models))
        .layer(DefaultBodyLimit::max(4 * 1024 * 1024))
        .layer(middleware::from_fn(log_rejected_model_request))
        .with_state(gateway)
}

#[derive(Clone, Copy)]
pub(super) struct ModelRejection {
    pub(super) reason: &'static str,
    pub(super) upstream_status: Option<StatusCode>,
}

pub(super) fn upstream_rejection(mut response: Response, upstream_status: StatusCode) -> Response {
    response.extensions_mut().insert(ModelRejection {
        reason: "upstream_rejected",
        upstream_status: Some(upstream_status),
    });
    response
}

async fn log_rejected_model_request(request: Request, next: Next) -> Response {
    // Only named protocol routes enter the log. Unknown paths and all query values are untrusted.
    let route = match request.uri().path() {
        "/v1/responses" => "responses",
        "/v1/responses/compact" => "responses_compact",
        "/v1/models" => "models",
        _ => "other",
    };
    let response = next.run(request).await;
    let status = response.status();
    let rejection = response.extensions().get::<ModelRejection>().copied();
    if status.is_client_error() || status.is_server_error() || rejection.is_some() {
        let ModelRejection {
            reason,
            upstream_status,
        } = rejection.unwrap_or(ModelRejection {
            reason: match status {
                StatusCode::NOT_FOUND => "unsupported_route",
                StatusCode::METHOD_NOT_ALLOWED => "unsupported_method",
                StatusCode::PAYLOAD_TOO_LARGE => "request_too_large",
                _ => "request_rejected",
            },
            upstream_status: None,
        });
        tracing::warn!(
            route,
            reason,
            status = status.as_u16(),
            upstream_status = upstream_status.map(|status| status.as_u16()),
            "gateway model request rejected"
        );
    }
    response
}

pub(super) fn admin_router(gateway: Arc<Gateway>) -> Router {
    let credentials = Router::new()
        .route("/admin/credentials/{user}", get(status).delete(unlink))
        .route(
            "/admin/credentials/{user}/link",
            post(link).get(link_status),
        )
        .route("/admin/credentials/{user}/usage", get(usage))
        .route("/admin/credentials/{user}/limits", get(limits))
        .route_layer(middleware::from_fn_with_state(
            gateway.clone(),
            credentials_scope,
        ));
    let keys = Router::new()
        .route("/admin/keys/mint", post(mint))
        .route("/admin/keys/revoke", post(revoke))
        .route("/admin/keys", get(list_keys))
        .route("/admin/keys/{id}/usage", get(key_usage))
        .route_layer(middleware::from_fn_with_state(gateway.clone(), keys_scope));
    Router::new()
        .merge(credentials)
        .merge(keys)
        .layer(DefaultBodyLimit::max(16384))
        .with_state(gateway)
}

async fn credentials_scope(
    State(g): State<Arc<Gateway>>,
    request: Request,
    next: Next,
) -> Result<Response, Error> {
    check_scope(request.headers(), &g.credentials_scope)?;
    Ok(next.run(request).await)
}
async fn keys_scope(
    State(g): State<Arc<Gateway>>,
    request: Request,
    next: Next,
) -> Result<Response, Error> {
    check_scope(request.headers(), &g.keys_scope)?;
    Ok(next.run(request).await)
}
fn check_scope(headers: &HeaderMap, scope: &contracts::job::access::Secret) -> Result<(), Error> {
    if !bearer(headers).is_some_and(|key| crate::secrets::secrets_equal(key, scope.expose())) {
        return Err(Error::Forbidden);
    }
    Ok(())
}

async fn responses(
    State(g): State<Arc<Gateway>>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Result<Response, Error> {
    let body = serde_json::from_slice(&bytes).map_err(|_| Error::ModelPolicy("invalid_json"))?;
    g.proxy(headers, None, Some(body)).await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelsQuery {
    client_version: Option<String>,
}
async fn models(
    State(g): State<Arc<Gateway>>,
    headers: HeaderMap,
    query: Result<Query<ModelsQuery>, QueryRejection>,
) -> Result<Response, Error> {
    // Keep Axum's existing query rejection response, adding a safe diagnostic for the model listener.
    let Query(query) = match query {
        Ok(query) => query,
        Err(error) => {
            let mut response = error.into_response();
            response.extensions_mut().insert(ModelRejection {
                reason: "invalid_models_query",
                upstream_status: None,
            });
            return Ok(response);
        }
    };
    if query.client_version.as_ref().is_some_and(|v| {
        v.len() > 64
            || !v
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_'))
    }) {
        return Err(Error::ModelPolicy("invalid_client_version"));
    }
    g.proxy(headers, query.client_version, None).await
}
async fn status(
    State(g): State<Arc<Gateway>>,
    Path(user): Path<String>,
) -> Result<Response, Error> {
    Ok(Json(g.status(&UserId(user)).await?).into_response())
}
async fn link(
    State(g): State<Arc<Gateway>>,
    Path(user): Path<String>,
    Json(_): Json<LinkCredential>,
) -> Result<Response, Error> {
    Ok(Json(g.begin_link(&UserId(user)).await?).into_response())
}
async fn link_status(
    State(g): State<Arc<Gateway>>,
    Path(user): Path<String>,
) -> Result<Response, Error> {
    Ok(Json(g.status_view(&UserId(user), true).await?).into_response())
}

async fn unlink(
    State(g): State<Arc<Gateway>>,
    Path(user): Path<String>,
    Json(command): Json<UnlinkCredential>,
) -> Result<Response, Error> {
    if command.user != UserId(user) {
        return Err(Error::Body);
    }
    g.unlink(&command.user).await?;
    Ok(axum::http::StatusCode::NO_CONTENT.into_response())
}
async fn usage(State(g): State<Arc<Gateway>>, Path(user): Path<String>) -> Result<Response, Error> {
    Ok(Json(g.usage(&UserId(user), None).await?).into_response())
}
async fn limits(
    State(g): State<Arc<Gateway>>,
    Path(user): Path<String>,
) -> Result<Response, Error> {
    let tx = g.store.lock(&UserId(user)).await?;
    Ok(Json(&tx.record.limits).into_response())
}
async fn mint(
    State(g): State<Arc<Gateway>>,
    Json(command): Json<MintKey>,
) -> Result<Response, Error> {
    Ok(Json(g.mint(&command.container).await?).into_response())
}
async fn revoke(
    State(g): State<Arc<Gateway>>,
    Json(command): Json<RevokeKeys>,
) -> Result<Response, Error> {
    g.revoke(&command.container).await?;
    Ok(axum::http::StatusCode::NO_CONTENT.into_response())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeysQuery {
    user: String,
}
async fn list_keys(
    State(g): State<Arc<Gateway>>,
    Query(query): Query<KeysQuery>,
) -> Result<Response, Error> {
    let tx = g.store.lock(&UserId(query.user)).await?;
    let list: Vec<_> = tx.record.keys.iter().filter(|(_,k)| !k.revoked && k.expires_at > super::now())
        .map(|(id,k)| json!({"id":id,"container":k.container,"expires_at":contracts::time::Timestamp::from_second(k.expires_at)})).collect();
    Ok(Json(list).into_response())
}
async fn key_usage(
    State(g): State<Arc<Gateway>>,
    Path(id): Path<String>,
) -> Result<Response, Error> {
    let user = g.store.key_user(&id)?;
    Ok(Json(g.usage(&user, Some(&id)).await?).into_response())
}
