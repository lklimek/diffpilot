//! Notifications (plan §5.5, RN1–RN3, RF5): a user's ntfy and webhook channels, an agent token's push target, and
//! the delivery task: one attempt now, retries after about 1 and 5 minutes, then the target is failing; health
//! (last delivery, last error, failing since) is logged once per change. At most [`MESSAGES_PER_HOUR`] messages go
//! to one channel or push target an hour, counted from the stored sends. Secrets are stored AES-GCM under
//! `SR_SECRET_KEY` (`secrets`), shown as their last 4 characters, sent only to their own target (a changed kind or
//! origin needs the secret again), never logged.

pub mod outbound;

use std::sync::Arc;

use contracts::api::review_loop::{
    AgentPushEvent, AgentPushPayload, ChannelKind, ChannelTestResult, DeliveryOutcome,
    NotificationChannel, NotificationSettings, PushTarget, PutChannelRequest, PutPushTargetRequest,
    SecretChange, UserEvent, UserEventKind, WebhookPayload,
};
use contracts::common::PrRef;
use contracts::ids::{DecisionId, UserEventId};
use contracts::time::Timestamp;
use db::error::DbError;
use db::repo::review_loop::{
    ChannelInput, ChannelRecord, DeliveryRecord, DeliveryTarget, FailureTransition, PushInput,
    SecretUpdate, SubscriptionRecord,
};
use futures::StreamExt;
use jiff::SignedDuration;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use secrecy::{ExposeSecret, SecretString};
use tokio::sync::Notify;
use url::Url;

use self::outbound::{Outbound, Reach, SendError, UrlError, check_url};
use crate::auth::{Session, TokenPrincipal};
use crate::secrets::{Aad, SecretError, TokenCipher};
use crate::state::AppState;

/// Attempts per delivery.
pub const ATTEMPTS: u32 = 3;
/// Waits before the 2nd and 3rd attempt: attempts at about now, +1 min and +5 min.
pub const RETRY_AFTER: [SignedDuration; 2] =
    [SignedDuration::from_mins(1), SignedDuration::from_mins(4)];
/// Messages per hour to one channel or push target.
pub const MESSAGES_PER_HOUR: u32 = 30;
const HOUR: SignedDuration = SignedDuration::from_hours(1);
/// The delivery task looks for due deliveries at least this often.
const DELIVERY_TICK: std::time::Duration = std::time::Duration::from_secs(5);
/// Deliveries taken per round and sent at once.
const BATCH: u32 = 50;
const PARALLEL: usize = 8;
/// Secret length bounds (characters).
const SECRET_CHARS: std::ops::RangeInclusive<usize> = 8..=4096;
/// Longest ntfy topic (ntfy's own limit).
const MAX_TOPIC_CHARS: usize = 64;
/// Title of every ntfy message.
const NTFY_TITLE: &str = "diffpilot";

/// A notification request failed.
#[derive(Debug, thiserror::Error)]
pub enum NotifyError {
    /// The URL is not allowed.
    #[error("{0}")]
    Url(#[from] UrlError),
    /// The ntfy topic is missing or malformed, or a webhook names one.
    #[error("{0}")]
    Topic(&'static str),
    /// The secret is too short, too long or not usable as a header value.
    #[error("the secret must be 8 to 4096 visible characters")]
    Secret,
    /// No channel of this kind (RN3) or no push target.
    #[error("the channel is not set up")]
    NotFound,
    /// A kept secret would follow a changed kind or URL origin to another destination.
    #[error(
        "the kind or the URL's scheme, host or port changed, so the secret must be replaced or removed"
    )]
    SecretRequired,
    /// RN3: [`MESSAGES_PER_HOUR`] messages went to the channel within the last hour.
    #[error("{MESSAGES_PER_HOUR} messages were sent to this channel in the last hour")]
    Limited {
        /// When the next message may go.
        retry_at: Timestamp,
    },
    /// A secret could not be encrypted or decrypted.
    #[error("a stored secret could not be used")]
    Cipher(#[from] SecretError),
    /// The database failed.
    #[error("the notification settings could not be read or changed")]
    Db(#[from] DbError),
}

fn channel_aad<'a>(row_id: &'a str) -> Aad<'a> {
    Aad {
        table: "notification_channels",
        column: "secret",
        row_id,
    }
}

fn push_aad(row_id: &str) -> Aad<'_> {
    Aad {
        table: "agent_subscriptions",
        column: "push_secret",
        row_id,
    }
}

/// Row id bound into a channel secret's ciphertext: the channel's unique key (user, kind).
fn channel_row(user_id: &str, kind: ChannelKind) -> String {
    format!("{user_id}/{}", kind_name(kind))
}

fn kind_name(kind: ChannelKind) -> &'static str {
    match kind {
        ChannelKind::Ntfy => "ntfy",
        ChannelKind::Webhook => "webhook",
    }
}

fn cipher(state: &AppState) -> TokenCipher {
    TokenCipher::new(&state.config.secret_key)
}

/// Wakes the delivery task (one server process sends every notification).
#[derive(Clone, Debug, Default)]
pub struct NotifyHub(Arc<Notify>);

impl NotifyHub {
    /// Asks the delivery task to look for due deliveries now.
    pub fn wake_deliveries(&self) {
        self.0.notify_one();
    }

    async fn deliveries_wanted(&self) {
        self.0.notified().await;
    }
}

/// Starts the delivery task; it stops with the server.
pub fn spawn(state: AppState) {
    tokio::spawn(run_deliveries(state));
}

/// The hourly limit's key of a channel (its user and kind, so removing and saving it again keeps the count).
fn channel_key(user_id: &str, kind: ChannelKind) -> String {
    format!("channel:{}", channel_row(user_id, kind))
}

fn push_key(token: &contracts::api::access_tokens::AccessTokenId) -> String {
    format!("push:{}", token.0)
}

/// Takes one of the hourly messages of `target`; `Err(at)` when the next may go.
async fn take_message(
    state: &AppState,
    tenant_id: &str,
    target: &str,
    now: Timestamp,
) -> Result<Result<(), Timestamp>, DbError> {
    state
        .db
        .review_loop()
        .take_send_slot(tenant_id, target, MESSAGES_PER_HOUR, HOUR, now)
        .await
}

/// The last 4 characters of a stored secret; `None` without one or when it cannot be decrypted.
fn last4(state: &AppState, aad: Aad<'_>, stored: Option<&[u8]>) -> Option<String> {
    let plain = cipher(state).decrypt(aad, stored?).ok()?;
    let chars: Vec<char> = plain.expose_secret().chars().collect();
    Some(chars[chars.len().saturating_sub(4)..].iter().collect())
}

/// A checked URL and topic of a channel or push target.
struct Checked {
    url: Url,
    topic: Option<String>,
}

/// The stored target a save replaces, when it holds a secret: its kind and URL.
struct Stored<'a> {
    kind: ChannelKind,
    url: &'a str,
}

fn check_target(
    state: &AppState,
    kind: ChannelKind,
    url: &str,
    topic: Option<&str>,
    secret: &SecretChange,
    stored: Option<Stored<'_>>,
) -> Result<Checked, NotifyError> {
    let url = check_url(url, Reach::of(&state.config))?;
    let topic = topic.map(str::trim).filter(|topic| !topic.is_empty());
    let topic = match (kind, topic) {
        (ChannelKind::Ntfy, None) => {
            return Err(NotifyError::Topic("an ntfy channel needs a topic"));
        }
        (ChannelKind::Ntfy, Some(topic)) => {
            let valid = topic.chars().count() <= MAX_TOPIC_CHARS
                && topic
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
            if !valid {
                return Err(NotifyError::Topic(
                    "an ntfy topic is 1 to 64 letters, digits, - or _",
                ));
            }
            Some(topic.to_owned())
        }
        (ChannelKind::Webhook, None) => None,
        (ChannelKind::Webhook, Some(_)) => {
            return Err(NotifyError::Topic("a webhook has no topic"));
        }
    };
    match secret {
        SecretChange::Replace { value } => {
            let usable = SECRET_CHARS.contains(&value.chars().count())
                && HeaderValue::from_str(&authorization(kind, value)).is_ok();
            if !usable {
                return Err(NotifyError::Secret);
            }
        }
        SecretChange::Keep => {
            let moved = stored.is_some_and(|stored| {
                stored.kind != kind
                    || Url::parse(stored.url).map_or(true, |old| old.origin() != url.origin())
            });
            if moved {
                return Err(NotifyError::SecretRequired);
            }
        }
        SecretChange::Remove => {}
    }
    Ok(Checked { url, topic })
}

/// The `Authorization` value sent: ntfy access tokens as `Bearer`, a webhook's value as given.
fn authorization(kind: ChannelKind, secret: &str) -> String {
    match kind {
        ChannelKind::Ntfy => format!("Bearer {secret}"),
        ChannelKind::Webhook => secret.to_owned(),
    }
}

fn secret_update(
    cipher: &TokenCipher,
    aad: Aad<'_>,
    change: &SecretChange,
) -> Result<SecretUpdate, NotifyError> {
    Ok(match change {
        SecretChange::Keep => SecretUpdate::Keep,
        SecretChange::Replace { value } => SecretUpdate::Set(cipher.encrypt(aad, value)?),
        SecretChange::Remove => SecretUpdate::Clear,
    })
}

fn channel_view(state: &AppState, record: ChannelRecord) -> NotificationChannel {
    let row = channel_row(&record.user_id, record.kind);
    NotificationChannel {
        kind: record.kind,
        secret_last4: last4(state, channel_aad(&row), record.secret.as_deref()),
        url: record.url,
        topic: record.topic,
        events: record.events,
        enabled: record.enabled,
        delivery: record.delivery,
    }
}

/// A token's push target as shown in RF5 and the decision log (V3); `None` without one. Without `detail` (another
/// user's token in V3) only its kind and delivery state: no URL, topic, secret tail or error text.
pub fn push_view(
    state: &AppState,
    record: &SubscriptionRecord,
    detail: bool,
) -> Option<PushTarget> {
    let (kind, url, topic) = record.push.clone()?;
    if !detail {
        return Some(PushTarget {
            kind,
            url: None,
            topic: None,
            secret_last4: None,
            delivery: contracts::api::review_loop::DeliveryStatus {
                last_error: None,
                ..record.delivery.clone()
            },
        });
    }
    Some(PushTarget {
        kind,
        url: Some(url),
        topic,
        secret_last4: last4(
            state,
            push_aad(&record.token_id.0),
            record.push_secret.as_deref(),
        ),
        delivery: record.delivery.clone(),
    })
}

/// RN1: the session user's channels, secrets masked.
pub async fn settings(
    state: &AppState,
    session: &Session,
) -> Result<NotificationSettings, NotifyError> {
    let channels = state.db.review_loop().channels(&session.user_id).await?;
    Ok(NotificationSettings {
        channels: channels
            .into_iter()
            .map(|record| channel_view(state, record))
            .collect(),
    })
}

/// RN2 `PUT`: creates or replaces the user's channel of `kind`; the delivery state is kept.
pub async fn put_channel(
    state: &AppState,
    session: &Session,
    kind: ChannelKind,
    request: &PutChannelRequest,
) -> Result<NotificationChannel, NotifyError> {
    let existing = state
        .db
        .review_loop()
        .channels(&session.user_id)
        .await?
        .into_iter()
        .find(|channel| channel.kind == kind && channel.secret.is_some());
    let checked = check_target(
        state,
        kind,
        &request.url,
        request.topic.as_deref(),
        &request.secret,
        existing.as_ref().map(|channel| Stored {
            kind: channel.kind,
            url: &channel.url,
        }),
    )?;
    let row = channel_row(&session.user_id, kind);
    let secret = secret_update(&cipher(state), channel_aad(&row), &request.secret)?;
    let mut events = Vec::new();
    for event in &request.events {
        if !events.contains(event) {
            events.push(*event);
        }
    }
    let input = ChannelInput {
        url: String::from(checked.url),
        topic: checked.topic,
        secret,
        events,
        enabled: request.enabled,
    };
    let record = state
        .db
        .review_loop()
        .put_channel(&session.tenant_id, &session.user_id, kind, &input)
        .await?;
    tracing::info!(
        user = %session.user_id,
        kind = kind_name(kind),
        enabled = record.enabled,
        "notification channel saved"
    );
    Ok(channel_view(state, record))
}

/// RN2 `DELETE`: removes the user's channel of `kind` with its secret (nothing to remove is fine).
pub async fn delete_channel(
    state: &AppState,
    session: &Session,
    kind: ChannelKind,
) -> Result<(), NotifyError> {
    if state
        .db
        .review_loop()
        .delete_channel(&session.user_id, kind)
        .await?
    {
        tracing::info!(user = %session.user_id, kind = kind_name(kind), "notification channel removed");
    }
    Ok(())
}

/// RN3: one test message through the user's channel of `kind`, without retry; the outcome is the channel's
/// latest delivery. Over the hourly limit nothing is sent and the channel's health is unchanged.
pub async fn test_channel(
    state: &AppState,
    session: &Session,
    kind: ChannelKind,
) -> Result<ChannelTestResult, NotifyError> {
    let rl = state.db.review_loop();
    let channel = rl
        .channels(&session.user_id)
        .await?
        .into_iter()
        .find(|channel| channel.kind == kind)
        .ok_or(NotifyError::NotFound)?;
    let event = UserEvent {
        id: UserEventId::new_random(),
        kind: UserEventKind::Test,
        pr: None,
        count: 0,
        text: "diffpilot test message: this channel works".to_owned(),
        url: Some("/settings#notifications".to_owned()),
        at: Timestamp::now(),
    };
    let key = channel_key(&channel.user_id, channel.kind);
    if let Err(retry_at) = take_message(state, &session.tenant_id, &key, event.at).await? {
        return Err(NotifyError::Limited { retry_at });
    }
    let sent = match Outbound::new(Reach::of(&state.config)) {
        Ok(outbound) => send_event(state, &outbound, &channel, &event).await,
        Err(error) => {
            tracing::error!(%error, "the notification HTTP client for a test could not be built");
            Err(Failure::Client)
        }
    };
    let (outcome, http_status, error) = match &sent {
        Ok(status) => (DeliveryOutcome::Delivered, Some(*status), None),
        Err(failure) => (
            DeliveryOutcome::Failed,
            failure.http_status(),
            Some(failure.to_string()),
        ),
    };
    let transition = rl
        .record_channel_delivery(
            channel.id,
            UserEventKind::Test,
            outcome,
            error.as_deref(),
            Timestamp::now(),
        )
        .await?;
    log_transition(&Target::Channel(channel), transition, error.as_deref());
    Ok(ChannelTestResult {
        outcome,
        http_status,
        error,
    })
}

/// RF5 `PUT`: sets the calling token's push target.
pub async fn put_push_target(
    state: &AppState,
    principal: &TokenPrincipal,
    request: &PutPushTargetRequest,
) -> Result<PushTarget, NotifyError> {
    let existing = state
        .db
        .review_loop()
        .subscription(&principal.token_id, Timestamp::now())
        .await?
        .filter(|subscription| subscription.push_secret.is_some());
    let checked = check_target(
        state,
        request.kind,
        &request.url,
        request.topic.as_deref(),
        &request.secret,
        existing
            .as_ref()
            .and_then(|subscription| subscription.push.as_ref())
            .map(|(kind, url, _)| Stored { kind: *kind, url }),
    )?;
    let secret = secret_update(
        &cipher(state),
        push_aad(&principal.token_id.0),
        &request.secret,
    )?;
    let record = state
        .db
        .review_loop()
        .put_push_target(
            &principal.tenant_id,
            &principal.token_id,
            &PushInput {
                kind: request.kind,
                url: String::from(checked.url),
                topic: checked.topic,
                secret,
            },
            Timestamp::now(),
        )
        .await?;
    tracing::info!(
        target: "audit",
        token_id = %principal.token_id,
        kind = kind_name(request.kind),
        "an agent set its push target"
    );
    push_view(state, &record, true).ok_or(NotifyError::NotFound)
}

/// RF5 `DELETE`: removes the calling token's push target (nothing to remove is fine).
pub async fn delete_push_target(
    state: &AppState,
    principal: &TokenPrincipal,
) -> Result<(), NotifyError> {
    if state
        .db
        .review_loop()
        .delete_push_target(&principal.token_id)
        .await?
    {
        tracing::info!(target: "audit", token_id = %principal.token_id, "an agent removed its push target");
    }
    Ok(())
}

/// Why an attempt did not deliver.
#[derive(Debug, thiserror::Error)]
enum Failure {
    /// The request failed or was refused.
    #[error(transparent)]
    Send(#[from] SendError),
    /// The stored URL no longer parses (stored before a rule change).
    #[error("{0}")]
    Url(#[from] UrlError),
    /// The stored secret cannot be decrypted (`SR_SECRET_KEY` changed): set it again.
    #[error("the stored secret could not be decrypted; set it again")]
    Secret,
    /// The notification client could not be built.
    #[error("the notification client could not start")]
    Client,
}

impl Failure {
    fn http_status(&self) -> Option<u16> {
        match self {
            Self::Send(error) => error.http_status(),
            _ => None,
        }
    }

    /// Whether a later attempt may succeed.
    fn retryable(&self) -> bool {
        matches!(self, Self::Send(_) | Self::Client)
    }
}

fn header(value: &str) -> Option<HeaderValue> {
    HeaderValue::from_str(value).ok()
}

/// URL, headers and body of one message to a channel or push target.
fn request(
    kind: ChannelKind,
    url: &str,
    topic: Option<&str>,
    secret: Option<&SecretString>,
    click: Option<&str>,
    ntfy_text: String,
    json: Vec<u8>,
) -> Result<(Url, HeaderMap, Vec<u8>), Failure> {
    let mut headers = HeaderMap::new();
    if let Some(value) =
        secret.and_then(|secret| header(&authorization(kind, secret.expose_secret())))
    {
        headers.insert(AUTHORIZATION, value);
    }
    match kind {
        ChannelKind::Ntfy => {
            let mut target = Url::parse(url).map_err(|_| UrlError::Invalid)?;
            target
                .path_segments_mut()
                .map_err(|()| UrlError::Invalid)?
                .pop_if_empty()
                .push(topic.unwrap_or_default());
            headers.insert(
                HeaderName::from_static("title"),
                HeaderValue::from_static(NTFY_TITLE),
            );
            if let Some(click) = click.and_then(header) {
                headers.insert(HeaderName::from_static("click"), click);
            }
            headers.insert(
                CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            );
            Ok((target, headers, ntfy_text.into_bytes()))
        }
        ChannelKind::Webhook => {
            headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
            Ok((
                Url::parse(url).map_err(|_| UrlError::Invalid)?,
                headers,
                json,
            ))
        }
    }
}

/// `owner/name#number` of an event's pull request.
fn pr_name(pr: &PrRef) -> String {
    format!("{}/{}#{}", pr.repo.owner, pr.repo.name, pr.number)
}

/// The absolute link of a stored event link (a path on this instance, joined to `SR_PUBLIC_URL`).
fn absolute(state: &AppState, link: Option<&str>) -> Option<String> {
    state.config.public_url.join(link?).ok().map(String::from)
}

/// The path of a PR's Comments page on this instance, as user events store it.
pub fn comments_path(pr: &PrRef) -> String {
    format!(
        "/pr/{}/{}/{}/comments",
        pr.repo.owner, pr.repo.name, pr.number
    )
}

/// The push message of a claimable decision: ids and the fetch path only (T28).
pub fn push_payload(decision: DecisionId, pr: &PrRef) -> AgentPushPayload {
    AgentPushPayload {
        v: 1,
        event: AgentPushEvent::DecisionAvailable,
        decision_id: decision,
        pr: pr_name(pr),
        fetch: super::feed::fetch_path(decision),
    }
}

/// Sends a push of `decision` (of `pr`) to the subscription's target (the caller took the hourly slot).
async fn send_push(
    state: &AppState,
    outbound: &Outbound,
    subscription: &SubscriptionRecord,
    decision: DecisionId,
    pr: &PrRef,
) -> Result<u16, Failure> {
    let Some((kind, url, topic)) = &subscription.push else {
        return Err(UrlError::Invalid.into());
    };
    let secret = subscription
        .push_secret
        .as_deref()
        .map(|stored| cipher(state).decrypt(push_aad(&subscription.token_id.0), stored))
        .transpose()
        .map_err(|_| Failure::Secret)?;
    let json = serde_json::to_vec(&push_payload(decision, pr)).unwrap_or_default();
    let text = String::from_utf8_lossy(&json).into_owned();
    let (url, headers, body) = request(
        *kind,
        url,
        topic.as_deref(),
        secret.as_ref(),
        None,
        text,
        json,
    )?;
    Ok(outbound.post(&url, headers, body).await?)
}

/// The target of a delivery, kept for its health record.
enum Target {
    Channel(ChannelRecord),
    Push(SubscriptionRecord),
}

fn log_transition(health: &Target, transition: FailureTransition, error: Option<&str>) {
    let (target, user_or_token, kind) = match health {
        Target::Channel(channel) => ("channel", channel.user_id.as_str(), kind_name(channel.kind)),
        Target::Push(subscription) => (
            "push target",
            subscription.token_id.0.as_str(),
            subscription
                .push
                .as_ref()
                .map_or("none", |(kind, ..)| kind_name(*kind)),
        ),
    };
    match transition {
        FailureTransition::TurnedFailing => tracing::warn!(
            target_kind = target,
            owner = user_or_token,
            kind,
            error,
            "a notification target started failing; its owner sees the error in Settings or the decision log"
        ),
        FailureTransition::Recovered => tracing::info!(
            target_kind = target,
            owner = user_or_token,
            kind,
            "a failing notification target delivers again"
        ),
        FailureTransition::Unchanged => {}
    }
}

/// One attempt of a due delivery; the outcome and, after the last attempt or a success, the target's health
/// are stored.
async fn attempt(state: &AppState, outbound: &Outbound, delivery: DeliveryRecord, now: Timestamp) {
    if let Err(error) = try_attempt(state, outbound, &delivery, now).await {
        tracing::warn!(%error, delivery = delivery.id, "a notification delivery could not be recorded; retried");
    }
}

async fn try_attempt(
    state: &AppState,
    outbound: &Outbound,
    delivery: &DeliveryRecord,
    now: Timestamp,
) -> Result<(), DbError> {
    let rl = state.db.review_loop();
    let give_up = |reason: &'static str| {
        tracing::debug!(
            delivery = delivery.id,
            reason,
            "notification delivery dropped"
        );
        rl.record_attempt(
            delivery.id,
            DeliveryOutcome::Failed,
            None,
            None,
            Some(reason),
            now,
        )
    };
    let (sent, health, event_kind) = match &delivery.target {
        DeliveryTarget::Channel { event, channel } => {
            let Some(channel) = rl.channel(*channel).await? else {
                return give_up("channel removed").await;
            };
            if !channel.enabled {
                return give_up("channel turned off").await;
            }
            let Some(event) = rl.user_event(*event).await? else {
                return give_up("event removed").await;
            };
            if !channel.events.contains(&event.kind) {
                return give_up("channel no longer takes this event").await;
            }
            let key = channel_key(&channel.user_id, channel.kind);
            if let Err(later) = take_message(state, &delivery.tenant_id, &key, now).await? {
                tracing::debug!(
                    delivery = delivery.id,
                    "hourly message limit reached; delivery postponed"
                );
                return rl.postpone_delivery(delivery.id, later).await;
            }
            let sent = send_event(state, outbound, &channel, &event).await;
            (sent, Target::Channel(channel), Some(event.kind))
        }
        DeliveryTarget::Push { decision, token } => {
            let Some(subscription) = rl.subscription(token, now).await? else {
                return give_up("token revoked or expired, or its user is no longer allowed").await;
            };
            if subscription.push.is_none() {
                return give_up("push target removed").await;
            }
            if state.feed.is_connected(token) {
                return give_up("the agent holds a live feed stream").await;
            }
            let Some(pr) = rl.feed_decision(*decision).await?.map(|found| found.pr) else {
                return give_up("decision removed").await;
            };
            if let Err(later) =
                take_message(state, &delivery.tenant_id, &push_key(token), now).await?
            {
                tracing::debug!(
                    delivery = delivery.id,
                    "hourly message limit reached; push postponed"
                );
                return rl.postpone_delivery(delivery.id, later).await;
            }
            let sent = send_push(state, outbound, &subscription, *decision, &pr).await;
            (sent, Target::Push(subscription), None)
        }
    };
    let made = delivery.attempts + 1;
    let retry_at = match &sent {
        Err(failure) if failure.retryable() && made < ATTEMPTS => usize::try_from(made - 1)
            .ok()
            .and_then(|index| RETRY_AFTER.get(index))
            .map(|wait| now + *wait),
        _ => None,
    };
    let (outcome, status, error) = match &sent {
        Ok(status) => (DeliveryOutcome::Delivered, Some(*status), None),
        Err(failure) => (
            DeliveryOutcome::Failed,
            failure.http_status(),
            Some(failure.to_string()),
        ),
    };
    rl.record_attempt(
        delivery.id,
        outcome,
        retry_at,
        status,
        error.as_deref(),
        now,
    )
    .await?;
    if retry_at.is_some() {
        tracing::debug!(
            delivery = delivery.id,
            attempt = made,
            "notification attempt failed; retried later"
        );
        return Ok(());
    }
    let shown = error.map(|error| {
        if made > 1 {
            format!("{error} ({made} attempts)")
        } else {
            error
        }
    });
    let transition = match &health {
        Target::Channel(channel) => {
            rl.record_channel_delivery(
                channel.id,
                event_kind.unwrap_or(UserEventKind::Test),
                outcome,
                shown.as_deref(),
                now,
            )
            .await?
        }
        Target::Push(subscription) => {
            rl.record_push_delivery(&subscription.token_id, outcome, shown.as_deref(), now)
                .await?
        }
    };
    log_transition(&health, transition, shown.as_deref());
    Ok(())
}

/// Sends a user event to a channel (the caller took the hourly slot).
async fn send_event(
    state: &AppState,
    outbound: &Outbound,
    channel: &ChannelRecord,
    event: &UserEvent,
) -> Result<u16, Failure> {
    let row = channel_row(&channel.user_id, channel.kind);
    let secret = channel
        .secret
        .as_deref()
        .map(|stored| cipher(state).decrypt(channel_aad(&row), stored))
        .transpose()
        .map_err(|_| Failure::Secret)?;
    let link = absolute(state, event.url.as_deref());
    let payload = WebhookPayload {
        v: 1,
        event: event.kind,
        text: event.text.clone(),
        url: link.clone(),
        pr: event.pr.as_ref().map(pr_name),
        at: event.at,
    };
    let json = serde_json::to_vec(&payload).unwrap_or_default();
    let (url, headers, body) = request(
        channel.kind,
        &channel.url,
        channel.topic.as_deref(),
        secret.as_ref(),
        link.as_deref(),
        event.text.clone(),
        json,
    )?;
    Ok(outbound.post(&url, headers, body).await?)
}

/// Makes one attempt of every delivery due at `now` (user events queue theirs when stored).
pub async fn deliver_due(
    state: &AppState,
    outbound: &Outbound,
    now: Timestamp,
) -> Result<(), DbError> {
    let due = state.db.review_loop().due_deliveries(now, BATCH).await?;
    futures::stream::iter(due)
        .map(|delivery| attempt(state, outbound, delivery, now))
        .buffer_unordered(PARALLEL)
        .collect::<Vec<()>>()
        .await;
    Ok(())
}

/// The delivery task: every few seconds and when woken, until the server stops.
async fn run_deliveries(state: AppState) {
    let outbound = match Outbound::new(Reach::of(&state.config)) {
        Ok(outbound) => outbound,
        Err(error) => {
            tracing::error!(%error, "the notification HTTP client could not be built; no notification is sent");
            return;
        }
    };
    let mut stopping = state.stopping.subscribe();
    tracing::info!("notification delivery task started");
    loop {
        if let Err(error) = deliver_due(&state, &outbound, Timestamp::now()).await {
            tracing::warn!(%error, "due notifications could not be read; retried shortly");
        }
        tokio::select! {
            () = tokio::time::sleep(DELIVERY_TICK) => {}
            () = state.notify.deliveries_wanted() => {}
            _ = stopping.wait_for(|stop| *stop) => break,
        }
    }
    tracing::info!("notification delivery task stopped");
}
