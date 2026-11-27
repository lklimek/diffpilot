//! Authenticates and durably acknowledges GitHub App deliveries.

use std::net::IpAddr;

use aws_lc_rs::hmac::{HMAC_SHA256, Key, verify};
use axum::body::Bytes;
use axum::extract::{FromRequest, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use contracts::time::Timestamp;
use db::repo::github::{DeliveryError, Effect, NewDelivery, Outcome};
use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;
use uuid::Uuid;

use super::changes::{self, Changes};
use super::{Warning, Webhooks};

pub(super) async fn receive(State(hooks): State<Webhooks>, request: Request) -> Response {
    let Some(secret) = hooks.config.github_webhook_secret.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({
                "code": "webhook_not_configured",
                "message": "GitHub webhook secret is not configured."
            })),
        )
            .into_response();
    };
    let headers = request.headers();
    if !header(headers, header::CONTENT_TYPE.as_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("application/json"))
    {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
    }
    let Some(event) = header(headers, "x-github-event").filter(|event| !event.is_empty()) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Some(id) =
        header(headers, "x-github-delivery").and_then(|value| Uuid::parse_str(value).ok())
    else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Some(signature) = signature(headers) else {
        return reject_signature(&hooks, cf_ip(headers));
    };
    let cf_ip = cf_ip(headers);
    let target_type = header(headers, "x-github-hook-installation-target-type")
        .filter(|value| changes::code(value))
        .map(str::to_owned);
    let target_id = header(headers, "x-github-hook-installation-target-id")
        .and_then(|value| value.parse::<u64>().ok());
    let event = if changes::code(event) {
        event
    } else {
        "unknown"
    }
    .to_owned();
    let body = match Bytes::from_request(request, &()).await {
        Ok(body) => body,
        Err(error) => return error.status().into_response(),
    };
    let which = if verifies(secret, &body, &signature) {
        "current"
    } else if hooks
        .config
        .github_webhook_secret_previous
        .as_ref()
        .is_some_and(|secret| verifies(secret, &body, &signature))
    {
        "previous"
    } else {
        return reject_signature(&hooks, cf_ip);
    };
    if target_type.as_deref() != Some("integration")
        || target_id != Some(hooks.config.github.app_id)
    {
        if let Some(count) = hooks.warning(Warning::Target) {
            tracing::warn!(delivery_id = %id, ?target_type, ?target_id, count, code = "hook_target",
                "Verified GitHub webhook targets a different hook");
        }
        return StatusCode::UNAUTHORIZED.into_response();
    }
    tracing::debug!(secret = %which, "GitHub webhook signature verified");

    let json = serde_json::from_slice::<Value>(&body);
    let payload = json.as_ref().ok().filter(|value| value.is_object());
    let action = payload
        .and_then(|value| value["action"].as_str())
        .map(|value| {
            if changes::code(value) {
                value
            } else {
                "unknown"
            }
            .to_owned()
        });
    let action = action.as_deref();
    let installation_id =
        payload.and_then(|value| changes::positive_id(&value["installation"]["id"]));
    let changes = match payload {
        Some(value) => changes::extract(&event, action, value, installation_id)
            .map_err(|_| DeliveryError::InvalidPayload),
        None if json.is_err() => Err(DeliveryError::InvalidJson),
        None => Err(DeliveryError::InvalidPayload),
    };
    let audit_repo = payload.and_then(changes::event_repository);
    drop(json);
    let hash = hex::encode(crate::secrets::sha256(&body));
    drop(body);
    let now = Timestamp::now();
    let effect = match &changes {
        Ok(Changes::Ignore) => audit_repo
            .as_ref()
            .map_or(Effect::Ignore, Effect::IgnoreRepository),
        Ok(changes) => changes.effect(),
        Err(code) => Effect::Reject(audit_repo.as_ref(), *code),
    };
    let delivery = NewDelivery {
        id,
        hash: &hash,
        event: &event,
        action,
        installation_id,
        effect,
    };
    let outcome = match hooks.db.github().record(&delivery, now).await {
        Ok(outcome) => outcome,
        Err(_) => {
            tracing::warn!(%id, code = "record_delivery", "Could not record GitHub webhook delivery");
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
    };
    if outcome == Outcome::NotAllowed {
        hooks.metrics.drop_delivery(now);
    }
    let changes = match changes {
        Ok(changes) => changes,
        Err(code) => {
            let warning = if matches!(code, DeliveryError::InvalidJson) {
                Warning::InvalidJson
            } else {
                Warning::InvalidPayload
            };
            if let Some(count) = hooks.warning(warning) {
                tracing::warn!(delivery_id = %id, %event, ?action, ?installation_id, %code, count,
                    "Verified GitHub webhook payload rejected");
            }
            return StatusCode::BAD_REQUEST.into_response();
        }
    };
    let (repo, number) = match &changes {
        Changes::Signal(repo, hints, _) => {
            (Some(&repo.name), hints.first().and_then(|hint| hint.number))
        }
        _ => (audit_repo.as_ref().map(|repo| &repo.name), None),
    };
    let outcome_name = match outcome {
        Outcome::Queued => "queued",
        Outcome::Duplicate => "duplicate",
        Outcome::Ignored => "ignored",
        Outcome::NotAllowed => "not_allowed",
        Outcome::InstallationCapped => "installation_capped",
    };
    match (&changes, outcome) {
        (Changes::Ping { hook_id, events }, Outcome::Ignored) => {
            tracing::info!(delivery_id = %id, %event, hook_id = *hook_id, ?events, outcome = %outcome_name, "GitHub webhook ping received");
        }
        (_, Outcome::Duplicate) => {
            tracing::info!(delivery_id = %id, %event, ?action, ?installation_id, ?repo, ?number, outcome = %outcome_name, "GitHub webhook delivery deduplicated");
        }
        (_, Outcome::Ignored) if event != "github_app_authorization" => {
            tracing::debug!(delivery_id = %id, %event, ?action, ?installation_id, outcome = %outcome_name, "GitHub webhook delivery ignored");
        }
        (_, Outcome::NotAllowed) if !hooks.log_drop(repo, installation_id, now) => {}
        (_, Outcome::InstallationCapped) if hooks.first_installation_cap() => {
            tracing::warn!(delivery_id = %id, ?installation_id, code = %DeliveryError::InstallationCap,
                "GitHub webhook installation update reached its repository limit");
        }
        _ => {
            tracing::info!(delivery_id = %id, %event, ?action, ?installation_id, ?repo, ?number, outcome = %outcome_name, "GitHub webhook delivery recorded");
        }
    }
    match outcome {
        Outcome::Queued => {
            hooks.wake.notify_one();
            StatusCode::ACCEPTED.into_response()
        }
        Outcome::Duplicate => StatusCode::OK.into_response(),
        Outcome::Ignored if matches!(changes, Changes::Ping { .. }) => {
            StatusCode::OK.into_response()
        }
        Outcome::Ignored | Outcome::NotAllowed | Outcome::InstallationCapped => {
            StatusCode::ACCEPTED.into_response()
        }
    }
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).into_iter();
    let first = values.next()?;
    values
        .next()
        .is_none()
        .then(|| first.to_str().ok())
        .flatten()
}

pub(super) fn signature(headers: &HeaderMap) -> Option<[u8; 32]> {
    let encoded = header(headers, "x-hub-signature-256")?.strip_prefix("sha256=")?;
    if encoded.len() != 64
        || !encoded
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return None;
    }
    let mut decoded = [0; 32];
    hex::decode_to_slice(encoded, &mut decoded).ok()?;
    Some(decoded)
}

fn verifies(secret: &SecretString, body: &[u8], signature: &[u8]) -> bool {
    verify(
        &Key::new(HMAC_SHA256, secret.expose_secret().as_bytes()),
        body,
        signature,
    )
    .is_ok()
}

fn cf_ip(headers: &HeaderMap) -> Option<IpAddr> {
    header(headers, "cf-connecting-ip")?.parse().ok()
}

fn reject_signature(hooks: &Webhooks, cf_ip: Option<IpAddr>) -> Response {
    if let Some(count) = hooks.warning(Warning::Signature) {
        tracing::warn!(
            count,
            cf_ip = cf_ip.map(tracing::field::display),
            code = "signature",
            "GitHub webhook signature rejected"
        );
    }
    StatusCode::UNAUTHORIZED.into_response()
}
