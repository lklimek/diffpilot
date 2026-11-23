//! Fixed CLI-compatible device linking, bounded refresh and explicit unlink.

use super::{Error, Gateway, now};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use contracts::{
    gateway::{CredentialMode, CredentialState, CredentialStatus, DeviceLink, RelinkReason},
    ids::UserId,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub(super) const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const VERIFY_URL: &str = "https://auth.openai.com/codex/device";
const CALLBACK: &str = "https://auth.openai.com/deviceauth/callback";

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Bundle {
    pub access: String,
    pub refresh: String,
    pub id_token: String,
    pub account: String,
    pub label: String,
    pub plan: String,
    pub fedramp: bool,
    pub expires_at: i64,
    pub earliest_refresh_at: Option<i64>,
    pub scope: String,
}

pub(super) struct Access {
    pub bundle: Bundle,
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
        let mut tx = self.store.lock(user).await?;
        if tx.record.reason.is_some() {
            return Err(Error::Unlinked);
        }
        let b = tx.record.bundle.as_ref().ok_or(Error::Unlinked)?;
        let near = now()
            >= b.expires_at
                .saturating_sub(300 + i64::from(crate::secrets::sha256(user.0.as_bytes())[0] % 31))
            && b.earliest_refresh_at.is_none_or(|at| now() >= at);
        let newer = rejected.is_some_and(|generation| tx.record.generation > generation);
        if (newer || rejected.is_none()) && !near && b.expires_at > now() {
            return Ok(Access {
                bundle: b.clone(),
                generation: tx.record.generation,
            });
        }
        if !near {
            tx.record.reason = Some(RelinkReason::Terminal);
            tx.save()?;
            return Err(Error::Unlinked);
        }
        tx.begin_refresh()?;
        tx.record.inflight = true;
        tx.save()?;
        let old = tx.record.bundle.as_ref().ok_or(Error::Unlinked)?.clone();
        let exchange = self.upstream.client.post(format!("{}/oauth/token", self.upstream.auth))
            .json(&json!({"grant_type":"refresh_token","client_id":CLIENT_ID,
                "refresh_token":old.refresh,"refresh_reason":if rejected.is_some() { "unauthorized_recovery" } else { "proactive_expiry" }})).send().await;
        let replacement = match exchange {
            Ok(response) => {
                let status = response.status();
                if status == reqwest::StatusCode::UNAUTHORIZED {
                    tx.record.reason = Some(RelinkReason::Terminal);
                    tx.record.inflight = false;
                    tx.save().map_err(|_| Error::Unlinked)?;
                    tx.finish_refresh().map_err(|_| Error::Unlinked)?;
                    return Err(Error::Unlinked);
                }
                match response.json::<Value>().await {
                    Ok(value) if status.is_success() => parse_bundle(value)
                        .filter(|b| b.account == old.account && b.refresh != old.refresh),
                    Ok(value) if terminal(status, &value) => {
                        tx.record.reason = Some(RelinkReason::Terminal);
                        tx.record.inflight = false;
                        tx.save().map_err(|_| Error::Unlinked)?;
                        tx.finish_refresh().map_err(|_| Error::Unlinked)?;
                        return Err(Error::Unlinked);
                    }
                    _ => None,
                }
            }
            Err(_) => None,
        };
        let Some(bundle) = replacement else {
            tx.record.reason = Some(RelinkReason::Quarantined);
            tx.save().map_err(|_| Error::Unlinked)?;
            return Err(Error::Unlinked);
        };
        tx.record.bundle = Some(bundle.clone());
        tx.record.generation = tx.record.generation.checked_add(1).ok_or(Error::Usage)?;
        // A failed commit leaves the durable in-flight marker, never a usable replacement.
        #[cfg(test)]
        if self
            .store
            .fail_after_marker
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(Error::Io(std::io::Error::other("injected commit failure")));
        }
        tx.record.inflight = false;
        tx.save().map_err(|_| Error::Unlinked)?;
        tx.finish_refresh().map_err(|_| Error::Unlinked)?;
        Ok(Access {
            bundle,
            generation: tx.record.generation,
        })
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
            let result = async {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
                    if now() >= deadline { return Err(Error::Link); }
                    if store.lock(&user).await?.record.link_attempt != Some((attempt, deadline)) {
                        return Err(Error::Link);
                    }
                    let response = upstream.client.post(format!("{}/api/accounts/deviceauth/token", upstream.auth))
                        .json(&json!({"device_auth_id":device.device_auth_id,"user_code":device.user_code})).send().await?;
                    if matches!(response.status().as_u16(), 403 | 404) { continue; }
                    if !response.status().is_success() { return Err(Error::Link); }
                    let grant: Value = response.json().await.map_err(|_| Error::Link)?;
                    let code = grant.get("authorization_code").and_then(Value::as_str).ok_or(Error::Link)?;
                    let verifier = grant.get("code_verifier").and_then(Value::as_str).ok_or(Error::Link)?;
                    let response = upstream.client.post(format!("{}/oauth/token", upstream.auth)).form(&[
                        ("grant_type", "authorization_code"), ("client_id", CLIENT_ID), ("code", code),
                        ("redirect_uri", CALLBACK), ("code_verifier", verifier)]).send().await?;
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
                    tx.save()?;
                    tx.finish_refresh()?;
                    return Ok::<_, Error>(());
                }
            }.await;
            if let Ok(mut tx) = store.lock(&user).await
                && tx.record.link_attempt == Some((attempt, deadline)) {
                tx.record.link_attempt = None;
                let _ = tx.save();
            }
            let mut active = links.lock().await;
            if active.get(&user).is_some_and(|entry| entry.id == attempt) { active.remove(&user); }
            if result.is_err() { tracing::debug!("gateway device link did not complete; existing connection retained"); }
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
        for key in tx.record.keys.values_mut() {
            key.revoked = true;
        }
        let bundle = tx.record.bundle.take();
        tx.record.reason = Some(RelinkReason::Terminal);
        tx.record.inflight = false;
        tx.save()?;
        tx.finish_refresh()?;
        if let Some(b) = bundle {
            let body = if b.refresh.is_empty() {
                json!({"token":b.access,"token_type_hint":"access_token"})
            } else {
                json!({"token":b.refresh,"token_type_hint":"refresh_token","client_id":CLIENT_ID})
            };
            let _ = self
                .upstream
                .revoke_client
                .post(format!("{}/oauth/revoke", self.upstream.auth))
                .json(&body)
                .send()
                .await;
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
    let text = |key| {
        value
            .get(key)?
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let access = text("access_token")?;
    let refresh = text("refresh_token")?;
    let id_token = text("id_token")?;
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
    let seconds = value.get("expires_in")?.as_i64()?;
    if seconds <= 0 {
        return None;
    }
    let mut expires_at = now().checked_add(seconds)?;
    if let Some(exp) = access
        .split('.')
        .nth(1)
        .and_then(|p| URL_SAFE_NO_PAD.decode(p).ok())
        .and_then(|p| serde_json::from_slice::<Value>(&p).ok())
        .and_then(|v| v.get("exp").and_then(Value::as_i64))
    {
        expires_at = expires_at.min(exp);
    }
    if expires_at <= now() {
        return None;
    }
    let earliest_refresh_at = match value.get("earliest_refresh_at") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_i64()?),
    };
    Some(Bundle {
        access,
        refresh,
        id_token: id_token.clone(),
        account,
        label: claims.get("email")?.as_str()?.to_owned(),
        plan: auth.get("chatgpt_plan_type")?.as_str()?.to_owned(),
        fedramp: auth
            .get("chatgpt_account_is_fedramp")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        expires_at,
        earliest_refresh_at,
        scope,
    })
}
