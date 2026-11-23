//! Fixed CLI-compatible device linking, bounded refresh and explicit unlink.

use super::{Error, Gateway, now};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use contracts::{
    gateway::{CredentialMode, CredentialState, CredentialStatus, DeviceLink, RelinkReason},
    ids::UserId,
};
use secrecy::{ExposeSecret, SecretString, zeroize::Zeroize};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub(super) const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const VERIFY_URL: &str = "https://auth.openai.com/codex/device";
const CALLBACK: &str = "https://auth.openai.com/deviceauth/callback";

#[derive(Clone, Debug)]
pub(super) struct Token(SecretString);
impl Token {
    pub fn expose(&self) -> &str {
        self.0.expose_secret()
    }
}
impl From<String> for Token {
    fn from(value: String) -> Self {
        Self(value.into())
    }
}
impl From<&str> for Token {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}
impl Serialize for Token {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.expose())
    }
}
impl<'de> Deserialize<'de> for Token {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Into::into)
    }
}

struct TokenReply(Value);
impl std::ops::Deref for TokenReply {
    type Target = Value;
    fn deref(&self) -> &Value {
        &self.0
    }
}
impl Drop for TokenReply {
    fn drop(&mut self) {
        for name in ["access_token", "refresh_token", "id_token"] {
            if let Some(Value::String(token)) = self.0.get_mut(name) {
                token.zeroize();
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
pub(super) struct Bundle {
    pub access: Token,
    pub refresh: Token,
    pub id_token: Token,
    pub account: String,
    pub label: String,
    pub plan: String,
    pub fedramp: bool,
    pub expires_at: i64,
    pub earliest_refresh_at: Option<i64>,
    pub scope: String,
}

pub(super) struct Access {
    pub token: Token,
    pub account: String,
    pub fedramp: bool,
    pub generation: u64,
}

#[derive(Deserialize)]
struct Device {
    device_auth_id: String,
    user_code: String,
    interval: Value,
}

impl Gateway {
    pub(super) async fn status(&self, user: &UserId) -> Result<CredentialStatus, Error> {
        let tx = self.store.lock(user).await?;
        let r = &tx.record;
        let b = r.bundle.as_ref();
        let reason = r.reason.or(if b.is_none() {
            Some(RelinkReason::Terminal)
        } else {
            None
        });
        Ok(CredentialStatus {
            mode: CredentialMode::ChatgptPlan,
            state: if b.is_none() && r.link_attempt.is_some_and(|(_, at)| at > now()) {
                CredentialState::Linking
            } else if reason.is_some() {
                CredentialState::RelinkNeeded
            } else {
                CredentialState::Linked
            },
            relink_reason: if b.is_none() && r.link_attempt.is_some_and(|(_, at)| at > now()) {
                None
            } else {
                reason
            },
            label: b.map(|b| b.label.clone()),
            plan: b.map(|b| b.plan.clone()),
            paused_until: r
                .resets_at
                .and_then(contracts::time::Timestamp::from_second),
            last_call: r.last_call.clone(),
        })
    }

    pub(super) async fn access(
        &self,
        user: &UserId,
        rejected: Option<u64>,
    ) -> Result<Access, Error> {
        let tx = self.store.lock(user).await?;
        self.access_locked(user, tx, rejected).await
    }

    pub(super) async fn access_locked(
        &self,
        user: &UserId,
        mut tx: super::store::Locked,
        rejected: Option<u64>,
    ) -> Result<Access, Error> {
        if tx.record.reason.is_some() {
            return Err(Error::Unlinked);
        }
        let b = tx.record.bundle.as_ref().ok_or(Error::Unlinked)?;
        let near = now()
            >= b.expires_at
                .saturating_sub(300 + i64::from(crate::secrets::sha256(user.0.as_bytes())[0] % 31))
            && b.earliest_refresh_at.is_none_or(|at| now() >= at);
        if !near && b.expires_at > now() {
            return model_access(&tx.record);
        }
        if !near {
            tx.record.reason = Some(RelinkReason::Terminal);
            tx.outcome("access", "terminal", None, Some("expired"), None);
            tx.save()?;
            return Err(Error::Unlinked);
        }
        // Ownership moves to a gateway task before the exchange; caller cancellation cannot consume it.
        let mut tasks = self.refreshes.lock().await;
        while tasks.try_join_next().is_some() {}
        if self.stopping.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(Error::Busy);
        }
        let upstream = self.upstream.clone();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        tasks.spawn(async move {
            let result = refresh(&mut tx, &upstream, rejected.is_some()).await;
            let _ = sender.send(result);
        });
        drop(tasks);
        receiver.await.map_err(|_| Error::Unlinked)?
    }

    pub(super) async fn begin_link(&self, user: &UserId) -> Result<DeviceLink, Error> {
        let attempt = uuid::Uuid::new_v4();
        let deadline = now() + 900;
        {
            let mut tx = self.store.lock(user).await?;
            tx.record.link_attempt = Some((attempt, deadline));
            tx.save()?;
            let mut active = self.links.lock().await;
            if let Some(previous) = active.insert(
                user.clone(),
                super::LinkAttempt {
                    id: attempt,
                    task: None,
                },
            ) && let Some(task) = previous.task
            {
                task.abort();
            }
        }
        let mut status = None;
        let mut id = None;
        let start = async {
            let response = self
                .upstream
                .client
                .post(format!(
                    "{}/api/accounts/deviceauth/usercode",
                    self.upstream.auth
                ))
                .json(&json!({"client_id":CLIENT_ID}))
                .send()
                .await?;
            status = Some(response.status().as_u16());
            id = request_id(response.headers());
            if response.status() == reqwest::StatusCode::NOT_FOUND {
                return Err(Error::DeviceDisabled);
            }
            if !response.status().is_success() {
                return Err(Error::Link);
            }
            let device: Device = response.json().await.map_err(|_| Error::Link)?;
            if device.user_code.is_empty() || device.device_auth_id.is_empty() {
                return Err(Error::Link);
            }
            let interval = device
                .interval
                .as_u64()
                .or_else(|| device.interval.as_str().and_then(|s| s.parse().ok()))
                .filter(|v| (1..=900).contains(v))
                .ok_or(Error::Link)?;
            Ok((device, interval))
        }
        .await;
        let mut tx = self.store.lock(user).await?;
        if tx.record.link_attempt != Some((attempt, deadline)) {
            return Err(Error::Link);
        }
        let mut active = self.links.lock().await;
        if !active.get(user).is_some_and(|entry| entry.id == attempt) {
            return Err(Error::Link);
        }
        let (device, interval) = match start {
            Ok(start) => start,
            Err(error) => {
                tx.record.link_attempt = None;
                tx.outcome("link", "start_failed", status, None, id);
                tx.save()?;
                active.remove(user);
                return Err(error);
            }
        };
        drop(tx);
        let code = DeviceLink {
            user_code: device.user_code.clone(),
            verification_url: VERIFY_URL.into(),
            expires_at: contracts::time::Timestamp::from_second(deadline).ok_or(Error::Link)?,
        };
        let store = self.store.clone();
        let upstream = self.upstream.clone();
        let user_key = user.clone();
        let user = user.clone();
        let links = self.links.clone();
        let mut tasks = self.tasks.lock().await;
        while tasks.try_join_next().is_some() {}
        let handle = tasks.spawn(async move {
            let mut step = "poll_failed";
            let mut status = None;
            let mut id = None;
            let result = async {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
                    step = "poll_failed";
                    status = None;
                    id = None;
                    if now() >= deadline { return Err(Error::Link); }
                    if store.lock(&user).await?.record.link_attempt != Some((attempt, deadline)) {
                        return Err(Error::Link);
                    }
                    let response = upstream.client.post(format!("{}/api/accounts/deviceauth/token", upstream.auth))
                        .json(&json!({"device_auth_id":device.device_auth_id,"user_code":device.user_code})).send().await?;
                    status = Some(response.status().as_u16());
                    id = request_id(response.headers());
                    if matches!(response.status().as_u16(), 403 | 404) { continue; }
                    if !response.status().is_success() { return Err(Error::Link); }
                    let grant: Value = response.json().await.map_err(|_| Error::Link)?;
                    let code = grant.get("authorization_code").and_then(Value::as_str).ok_or(Error::Link)?;
                    let verifier = grant.get("code_verifier").and_then(Value::as_str).ok_or(Error::Link)?;
                    step = "exchange_failed";
                    status = None;
                    id = None;
                    let response = upstream.client.post(format!("{}/oauth/token", upstream.auth)).form(&[
                        ("grant_type", "authorization_code"), ("client_id", CLIENT_ID), ("code", code),
                        ("redirect_uri", CALLBACK), ("code_verifier", verifier)]).send().await?;
                    status = Some(response.status().as_u16());
                    id = request_id(response.headers());
                    if !response.status().is_success() { return Err(Error::Link); }
                    let bundle = parse_bundle(response.json().await.map_err(|_| Error::Link)?).ok_or(Error::Link)?;
                    let mut tx = store.lock(&user).await?;
                    let active = links.lock().await;
                    if tx.record.link_attempt != Some((attempt, deadline)) || now() >= deadline
                        || !active.get(&user).is_some_and(|entry| entry.id == attempt) { return Err(Error::Link); }
                    tx.record.link_attempt = None;
                    tx.record.bundle = Some(bundle);
                    tx.record.reason = None;
                    tx.record.inflight = false;
                    tx.record.generation = tx.record.generation.checked_add(1).ok_or(Error::Usage)?;
                    tx.outcome("link", "committed", status, None, id.take());
                    tx.save()?;
                    tx.finish_refresh()?;
                    return Ok::<_, Error>(());
                }
            }.await;
            if let Ok(mut tx) = store.lock(&user).await
                && tx.record.link_attempt == Some((attempt, deadline)) {
                tx.record.link_attempt = None;
                if result.is_err() { tx.outcome("link", step, status, None, id); }
                let _ = tx.save();
            }
            let mut active = links.lock().await;
            if active.get(&user).is_some_and(|entry| entry.id == attempt) { active.remove(&user); }
        });
        if let Some(entry) = active.get_mut(&user_key) {
            entry.task = Some(handle);
        }
        Ok(code)
    }

    pub(super) async fn unlink(&self, user: &UserId) -> Result<(), Error> {
        // Lock ordering is always credential -> link attempts. A refresh cannot overlap revoke.
        let mut tx = self.store.lock(user).await?;
        if let Some(entry) = self.links.lock().await.remove(user)
            && let Some(task) = entry.task
        {
            task.abort();
        }
        tx.record.link_attempt = None;
        for (id, key) in tx.record.keys.iter_mut().filter(|(_, key)| !key.revoked) {
            key.revoked = true;
            self.store.retire_key(id)?;
        }
        let bundle = tx.record.bundle.take();
        tx.record.reason = Some(RelinkReason::Terminal);
        tx.record.inflight = false;
        tx.outcome("unlink", "locally_unlinked", None, None, None);
        tx.save()?;
        tx.finish_refresh()?;
        if let Some(b) = bundle {
            let body = if b.refresh.expose().is_empty() {
                json!({"token":b.access.expose(),"token_type_hint":"access_token"})
            } else {
                json!({"token":b.refresh.expose(),"token_type_hint":"refresh_token","client_id":CLIENT_ID})
            };
            let result = self
                .upstream
                .revoke_client
                .post(format!("{}/oauth/revoke", self.upstream.auth))
                .json(&body)
                .send()
                .await;
            let (status, request_id, outcome) = match result {
                Ok(response) => (
                    Some(response.status().as_u16()),
                    request_id(response.headers()),
                    if response.status().is_success() {
                        "revoked"
                    } else {
                        "revoke_failed"
                    },
                ),
                Err(_) => (None, None, "revoke_transport_failed"),
            };
            tx.outcome("unlink", outcome, status, None, request_id);
            tx.save()?;
        }
        Ok(())
    }
}

fn terminal(status: reqwest::StatusCode, value: &Value) -> bool {
    let code = value
        .pointer("/error/code")
        .and_then(Value::as_str)
        .or_else(|| value.get("error").and_then(Value::as_str));
    status.as_u16() == 401
        || (status.as_u16() == 400 && code == Some("invalid_grant"))
        || matches!(
            code,
            Some("refresh_token_expired" | "refresh_token_reused" | "refresh_token_invalidated")
        )
}

pub(super) fn parse_bundle(value: Value) -> Option<Bundle> {
    let value = TokenReply(value);
    let text = |key| {
        value
            .get(key)?
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let access: Token = text("access_token")?.into();
    let refresh: Token = text("refresh_token")?.into();
    let id_token: Token = text("id_token")?.into();
    let scope = text("scope")?;
    if !["openid", "offline_access"]
        .iter()
        .all(|required| scope.split_whitespace().any(|v| v == *required))
        || !value
            .get("token_type")?
            .as_str()?
            .eq_ignore_ascii_case("bearer")
    {
        return None;
    }
    let (account, label, plan, fedramp) = identity(id_token.expose(), None)?;
    let (expires_at, earliest_refresh_at) = expiry(&value, access.expose())?;
    Some(Bundle {
        access,
        refresh,
        id_token,
        account,
        label,
        plan,
        fedramp,
        scope,
        expires_at,
        earliest_refresh_at,
    })
}

fn model_access(record: &super::store::Record) -> Result<Access, Error> {
    let bundle = record.bundle.as_ref().ok_or(Error::Unlinked)?;
    Ok(Access {
        token: bundle.access.clone(),
        account: bundle.account.clone(),
        fedramp: bundle.fedramp,
        generation: record.generation,
    })
}

pub(super) fn request_id(headers: &axum::http::HeaderMap) -> Option<String> {
    // Preserve canonical support IDs; arbitrary header content may contain secrets or log controls.
    headers
        .get("x-request-id")
        .or_else(|| headers.get("x-oai-request-id"))
        .map(|id| {
            let value = id.to_str().unwrap_or_default();
            let canonical = value.strip_prefix("req_").is_some_and(|suffix| {
                suffix.len() == 32 && suffix.bytes().all(|c| c.is_ascii_hexdigit())
            }) || uuid::Uuid::parse_str(value)
                .is_ok_and(|id| id.hyphenated().to_string() == value);
            if canonical {
                value.to_owned()
            } else {
                format!("sha256:{}", super::store::digest(id.as_bytes()))
            }
        })
}

async fn refresh(
    tx: &mut super::store::Locked,
    upstream: &super::Upstream,
    rejected: bool,
) -> Result<Access, Error> {
    tx.record.inflight = true;
    if let Err(error) = tx.save().and_then(|()| tx.begin_refresh()) {
        tx.record.inflight = false;
        tx.outcome("refresh", "pre_send_failed", None, None, None);
        tx.save()?;
        tx.finish_refresh()?;
        return Err(error);
    }
    let old = tx.record.bundle.as_ref().ok_or(Error::Unlinked)?;
    let exchange = upstream.client.post(format!("{}/oauth/token", upstream.auth))
        .json(&json!({"grant_type":"refresh_token","client_id":CLIENT_ID,
            "refresh_token":old.refresh.expose(),"refresh_reason":if rejected { "unauthorized_recovery" } else { "proactive_expiry" }}))
        .send().await;
    let mut status = None;
    let mut id = None;
    let mut code = None;
    let mut reason = RelinkReason::Quarantined;
    let replacement = match exchange {
        Ok(response) => {
            let http = response.status();
            status = Some(http.as_u16());
            id = request_id(response.headers());
            let value = response.json::<Value>().await.ok();
            code = value.as_ref().and_then(error_code);
            if http == reqwest::StatusCode::UNAUTHORIZED
                || value.as_ref().is_some_and(|v| terminal(http, v))
            {
                reason = RelinkReason::Terminal;
                None
            } else if http.is_success() {
                value.and_then(|v| parse_refresh(v, old))
            } else {
                None
            }
        }
        Err(error) if error.is_connect() => {
            tx.record.inflight = false;
            tx.outcome("refresh", "pre_send_failed", None, None, None);
            tx.save()?;
            tx.finish_refresh()?;
            return Err(Error::Http(error));
        }
        Err(_) => None,
    };
    if let Some(bundle) = replacement {
        tx.record.bundle = Some(bundle);
        tx.record.generation = tx.record.generation.checked_add(1).ok_or(Error::Usage)?;
        #[cfg(test)]
        if tx.fail_after_marker() {
            return Err(Error::Io(std::io::Error::other("injected commit failure")));
        }
        tx.record.inflight = false;
        tx.outcome("refresh", "committed", status, code, id);
        tx.save().map_err(|_| Error::Unlinked)?;
        tx.finish_refresh().map_err(|_| Error::Unlinked)?;
        model_access(&tx.record)
    } else {
        tx.record.reason = Some(reason);
        tx.record.inflight = false;
        tx.outcome(
            "refresh",
            if reason == RelinkReason::Terminal {
                "terminal"
            } else {
                "quarantined"
            },
            status,
            code,
            id,
        );
        tx.save().map_err(|_| Error::Unlinked)?;
        tx.finish_refresh().map_err(|_| Error::Unlinked)?;
        Err(Error::Unlinked)
    }
}

fn error_code(value: &Value) -> Option<&'static str> {
    let code = value
        .pointer("/error/code")
        .and_then(Value::as_str)
        .or_else(|| value.get("error").and_then(Value::as_str));
    match code {
        Some("invalid_grant") => Some("invalid_grant"),
        Some("refresh_token_expired") => Some("refresh_token_expired"),
        Some("refresh_token_reused") => Some("refresh_token_reused"),
        Some("refresh_token_invalidated") => Some("refresh_token_invalidated"),
        Some(_) => Some("unknown"),
        None => None,
    }
}

fn parse_refresh(value: Value, old: &Bundle) -> Option<Bundle> {
    let value = TokenReply(value);
    // Refresh has its own wire contract; omitted optional tokens preserve the committed values.
    let object = value.as_object()?;
    let text = |key| {
        object
            .get(key)?
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let access: Token = text("access_token")?.into();
    let refresh = match object.get("refresh_token") {
        None | Some(Value::Null) => old.refresh.clone(),
        _ => text("refresh_token")?.into(),
    };
    let (id_token, account, label, plan, fedramp) = match object.get("id_token") {
        None | Some(Value::Null) => (
            old.id_token.clone(),
            old.account.clone(),
            old.label.clone(),
            old.plan.clone(),
            old.fedramp,
        ),
        _ => {
            let token: Token = text("id_token")?.into();
            let claims = identity(token.expose(), Some(old))?;
            (token, claims.0, claims.1, claims.2, claims.3)
        }
    };
    if account != old.account {
        return None;
    }
    let scope = match object.get("scope") {
        None => old.scope.clone(),
        _ => text("scope")?,
    };
    if !valid_scope(&scope)
        || object
            .get("token_type")
            .is_some_and(|v| !v.as_str().is_some_and(|s| s.eq_ignore_ascii_case("bearer")))
    {
        return None;
    }
    let (expires_at, earliest_refresh_at) = expiry(&value, access.expose())?;
    Some(Bundle {
        access,
        refresh,
        id_token,
        account,
        label,
        plan,
        fedramp,
        scope,
        expires_at,
        earliest_refresh_at,
    })
}

fn valid_scope(scope: &str) -> bool {
    ["openid", "offline_access"]
        .iter()
        .all(|required| scope.split_whitespace().any(|s| s == *required))
}

fn identity(id_token: &str, old: Option<&Bundle>) -> Option<(String, String, String, bool)> {
    // Claims come only from the fixed TLS token endpoint, never from a caller's JWT.
    let claims: Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(id_token.split('.').nth(1)?).ok()?).ok()?;
    if claims.get("iss")?.as_str()? != "https://auth.openai.com"
        || claims.get("exp")?.as_i64()? <= now()
    {
        return None;
    }
    let aud = claims.get("aud")?;
    if aud.as_str() != Some(CLIENT_ID)
        && !aud
            .as_array()
            .is_some_and(|a| a.iter().any(|a| a.as_str() == Some(CLIENT_ID)))
    {
        return None;
    }
    let auth = claims.get("https://api.openai.com/auth")?;
    let account = auth.get("chatgpt_account_id")?.as_str()?.to_owned();
    if account.is_empty() {
        return None;
    }
    Some((
        account,
        match claims.get("email") {
            None | Some(Value::Null) => old?.label.clone(),
            Some(value) => value.as_str()?.to_owned(),
        },
        match auth.get("chatgpt_plan_type") {
            None | Some(Value::Null) => old?.plan.clone(),
            Some(value) => value.as_str()?.to_owned(),
        },
        auth.get("chatgpt_account_is_fedramp")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    ))
}

fn expiry(value: &Value, access: &str) -> Option<(i64, Option<i64>)> {
    let jwt_exp = access
        .split('.')
        .nth(1)
        .and_then(|p| URL_SAFE_NO_PAD.decode(p).ok())
        .and_then(|p| serde_json::from_slice::<Value>(&p).ok())
        .and_then(|v| v.get("exp").and_then(Value::as_i64));
    let expires_at = match value.get("expires_in") {
        None => jwt_exp?,
        Some(value) => {
            let seconds = value.as_i64()?;
            if seconds <= 0 {
                return None;
            }
            let expires_at = now().checked_add(seconds)?;
            jwt_exp.map_or(expires_at, |exp| expires_at.min(exp))
        }
    };
    if expires_at <= now() {
        return None;
    }
    let earliest_refresh_at = match value.get("earliest_refresh_at") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_i64()?),
    };
    Some((expires_at, earliest_refresh_at))
}
