//! Notification channels, agent subscriptions with push targets, and deliveries (plan §5.5). Secrets are stored and
//! returned as the server's ciphertext only; `Debug` never shows them.

use std::fmt;

use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use contracts::api::access_tokens::AccessTokenId;
use contracts::api::review_loop::{
    ChannelKind, DeliveryOutcome, DeliveryStatus, EffectKind, LastDelivery, UserEvent,
    UserEventKind,
};
use contracts::common::{PrRef, RepoRef};
use contracts::ids::{ChannelId, DecisionId, UserEventId};
use contracts::time::Timestamp;

use super::super::jobs::ts;
use super::super::write_tx;
use super::{
    ReviewLoop, json, json_col, opt_ts_col, opt_wire_col, ts_col, u32_col, wire, wire_col,
};
use crate::error::DbError;

/// What to do with a stored secret.
#[derive(Clone, PartialEq, Eq)]
pub enum SecretUpdate {
    /// Keep it (the request had no secret).
    Keep,
    /// Replace it with this ciphertext.
    Set(Vec<u8>),
    /// Remove it.
    Clear,
}

impl fmt::Debug for SecretUpdate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Keep => "Keep",
            Self::Set(_) => "Set([redacted])",
            Self::Clear => "Clear",
        })
    }
}

/// A channel to save.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelInput {
    /// URL.
    pub url: String,
    /// ntfy topic.
    pub topic: Option<String>,
    /// Secret change.
    pub secret: SecretUpdate,
    /// Events.
    pub events: Vec<UserEventKind>,
    /// On.
    pub enabled: bool,
}

/// A stored channel.
#[derive(Clone, PartialEq, Eq)]
pub struct ChannelRecord {
    /// Id.
    pub id: ChannelId,
    /// Owner.
    pub user_id: String,
    /// Kind.
    pub kind: ChannelKind,
    /// URL.
    pub url: String,
    /// ntfy topic.
    pub topic: Option<String>,
    /// Secret ciphertext.
    pub secret: Option<Vec<u8>>,
    /// Events.
    pub events: Vec<UserEventKind>,
    /// On.
    pub enabled: bool,
    /// Delivery state.
    pub delivery: DeliveryStatus,
}

impl fmt::Debug for ChannelRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChannelRecord")
            .field("id", &self.id)
            .field("user_id", &self.user_id)
            .field("kind", &self.kind)
            .field("url", &self.url)
            .field("topic", &self.topic)
            .field("secret", &self.secret.as_ref().map(|_| "[redacted]"))
            .field("events", &self.events)
            .field("enabled", &self.enabled)
            .field("delivery", &self.delivery)
            .finish()
    }
}

/// A push target to save for a token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PushInput {
    /// Kind.
    pub kind: ChannelKind,
    /// URL.
    pub url: String,
    /// ntfy topic.
    pub topic: Option<String>,
    /// Secret change.
    pub secret: SecretUpdate,
}

/// A token's feed subscription.
#[derive(Clone, PartialEq, Eq)]
pub struct SubscriptionRecord {
    /// Token.
    pub token_id: AccessTokenId,
    /// Token name.
    pub token_name: String,
    /// Capabilities.
    pub capabilities: Vec<EffectKind>,
    /// Last activity.
    pub last_seen_at: Timestamp,
    /// Push target kind, URL and topic.
    pub push: Option<(ChannelKind, String, Option<String>)>,
    /// Push secret ciphertext.
    pub push_secret: Option<Vec<u8>>,
    /// Push delivery state.
    pub delivery: DeliveryStatus,
}

impl fmt::Debug for SubscriptionRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SubscriptionRecord")
            .field("token_id", &self.token_id)
            .field("token_name", &self.token_name)
            .field("capabilities", &self.capabilities)
            .field("last_seen_at", &self.last_seen_at)
            .field("push", &self.push)
            .field(
                "push_secret",
                &self.push_secret.as_ref().map(|_| "[redacted]"),
            )
            .field("delivery", &self.delivery)
            .finish()
    }
}

/// How a delivery outcome changed a channel's health (log once per change).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureTransition {
    /// No change.
    Unchanged,
    /// It started failing now.
    TurnedFailing,
    /// It recovered now.
    Recovered,
}

/// Where a delivery goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeliveryTarget {
    /// A user event to a channel.
    Channel {
        /// Event.
        event: UserEventId,
        /// Channel.
        channel: ChannelId,
    },
    /// A claimable decision to an agent's push target.
    Push {
        /// Decision.
        decision: DecisionId,
        /// Token.
        token: AccessTokenId,
    },
}

/// A pending delivery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeliveryRecord {
    /// Id.
    pub id: i64,
    /// Target.
    pub target: DeliveryTarget,
    /// Attempts made.
    pub attempts: u32,
    /// Next attempt.
    pub next_attempt_at: Timestamp,
}

fn delivery_status(
    row: &SqliteRow,
    event_column: Option<&'static str>,
) -> Result<DeliveryStatus, DbError> {
    let at = opt_ts_col(row, "last_delivery_at")?;
    let outcome: Option<DeliveryOutcome> = opt_wire_col(row, "last_delivery_outcome")?;
    let event: Option<UserEventKind> = match event_column {
        Some(column) => opt_wire_col(row, column)?,
        None => None,
    };
    Ok(DeliveryStatus {
        last: at
            .zip(outcome)
            .map(|(at, outcome)| LastDelivery { event, outcome, at }),
        last_error: row.try_get("last_error")?,
        failing_since: opt_ts_col(row, "failing_since")?,
    })
}

fn channel_from_row(row: &SqliteRow) -> Result<ChannelRecord, DbError> {
    let enabled: i64 = row.try_get("enabled")?;
    Ok(ChannelRecord {
        id: wire_col(row, "id")?,
        user_id: row.try_get("user_id")?,
        kind: wire_col(row, "kind")?,
        url: row.try_get("url")?,
        topic: row.try_get("topic")?,
        secret: row.try_get("secret")?,
        events: json_col(row, "events")?,
        enabled: enabled == 1,
        delivery: delivery_status(row, Some("last_delivery_event"))?,
    })
}

fn subscription_from_row(row: &SqliteRow) -> Result<SubscriptionRecord, DbError> {
    let kind: Option<ChannelKind> = opt_wire_col(row, "push_kind")?;
    let url: Option<String> = row.try_get("push_url")?;
    Ok(SubscriptionRecord {
        token_id: AccessTokenId(row.try_get("token_id")?),
        token_name: row.try_get("name")?,
        capabilities: json_col(row, "capabilities")?,
        last_seen_at: ts_col(row, "last_seen_at")?,
        push: kind
            .zip(url)
            .map(|(kind, url)| -> Result<_, DbError> {
                Ok((kind, url, row.try_get("push_topic")?))
            })
            .transpose()?,
        push_secret: row.try_get("push_secret")?,
        delivery: delivery_status(row, None)?,
    })
}

const CHANNEL_COLUMNS: &str =
    "id, user_id, kind, url, topic, secret, events, enabled, last_delivery_at,
    last_delivery_event, last_delivery_outcome, last_error, failing_since";
const SUBSCRIPTION_SELECT: &str = "SELECT s.token_id, t.name, s.capabilities, s.last_seen_at, s.push_kind,
    s.push_url, s.push_topic, s.push_secret, s.last_delivery_at, s.last_delivery_outcome, s.last_error,
    s.failing_since FROM agent_subscriptions s JOIN access_tokens t ON t.id = s.token_id";

/// SQL setting a delivery outcome on a health row; binds `?1` at, `?2` outcome, `?3` error.
const HEALTH_SET: &str = "last_delivery_at = ?1, last_delivery_outcome = ?2,
    last_error = CASE WHEN ?2 = 'failed' THEN ?3 ELSE last_error END,
    failing_since = CASE WHEN ?2 = 'failed' THEN COALESCE(failing_since, ?1) ELSE NULL END";

fn transition(was_failing: bool, outcome: DeliveryOutcome) -> FailureTransition {
    match (was_failing, outcome) {
        (false, DeliveryOutcome::Failed) => FailureTransition::TurnedFailing,
        (true, DeliveryOutcome::Delivered) => FailureTransition::Recovered,
        _ => FailureTransition::Unchanged,
    }
}

impl ReviewLoop<'_> {
    /// Creates or replaces the user's channel of `kind`; the delivery state is kept.
    pub async fn put_channel(
        &self,
        tenant_id: &str,
        user_id: &str,
        kind: ChannelKind,
        input: &ChannelInput,
    ) -> Result<ChannelRecord, DbError> {
        let mut tx = write_tx(self.pool).await?;
        let (keep, secret) = match &input.secret {
            SecretUpdate::Keep => (true, None),
            SecretUpdate::Set(ciphertext) => (false, Some(ciphertext.clone())),
            SecretUpdate::Clear => (false, None),
        };
        sqlx::query(
            "INSERT INTO notification_channels (id, tenant_id, user_id, kind, url, topic, secret, events, enabled)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT (user_id, kind) DO UPDATE SET url = excluded.url, topic = excluded.topic,
                 secret = CASE WHEN ?10 THEN notification_channels.secret ELSE excluded.secret END,
                 events = excluded.events, enabled = excluded.enabled",
        )
        .bind(ChannelId::new_random().to_string())
        .bind(tenant_id)
        .bind(user_id)
        .bind(wire(kind)?)
        .bind(&input.url)
        .bind(&input.topic)
        .bind(secret)
        .bind(json(&input.events)?)
        .bind(i64::from(input.enabled))
        .bind(keep)
        .execute(&mut *tx)
        .await?;
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {CHANNEL_COLUMNS} FROM notification_channels WHERE user_id = ? AND kind = ?"
        )))
        .bind(user_id)
        .bind(wire(kind)?)
        .fetch_one(&mut *tx)
        .await?;
        let channel = channel_from_row(&row)?;
        tx.commit().await?;
        Ok(channel)
    }

    /// The user's channels.
    pub async fn channels(&self, user_id: &str) -> Result<Vec<ChannelRecord>, DbError> {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {CHANNEL_COLUMNS} FROM notification_channels WHERE user_id = ? ORDER BY kind"
        )))
        .bind(user_id)
        .fetch_all(self.pool)
        .await?
        .iter()
        .map(channel_from_row)
        .collect()
    }

    /// Removes the user's channel of `kind`; false when there was none.
    pub async fn delete_channel(&self, user_id: &str, kind: ChannelKind) -> Result<bool, DbError> {
        let deleted =
            sqlx::query("DELETE FROM notification_channels WHERE user_id = ? AND kind = ?")
                .bind(user_id)
                .bind(wire(kind)?)
                .execute(self.pool)
                .await?;
        Ok(deleted.rows_affected() > 0)
    }

    /// Records a channel's delivery outcome (also of a test); reports whether its health changed.
    pub async fn record_channel_delivery(
        &self,
        channel: ChannelId,
        event: UserEventKind,
        outcome: DeliveryOutcome,
        error: Option<&str>,
        now: Timestamp,
    ) -> Result<FailureTransition, DbError> {
        let mut tx = write_tx(self.pool).await?;
        let failing: Option<Option<String>> =
            sqlx::query_scalar("SELECT failing_since FROM notification_channels WHERE id = ?")
                .bind(channel.to_string())
                .fetch_optional(&mut *tx)
                .await?;
        let was_failing = failing.ok_or(DbError::NotFound)?.is_some();
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE notification_channels SET {HEALTH_SET}, last_delivery_event = ?4 WHERE id = ?5"
        )))
        .bind(ts(now))
        .bind(wire(outcome)?)
        .bind(error)
        .bind(wire(event)?)
        .bind(channel.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(transition(was_failing, outcome))
    }

    /// Records a token's feed activity and capabilities (RF1, RF2).
    pub async fn subscribe(
        &self,
        tenant_id: &str,
        token: &AccessTokenId,
        capabilities: &[EffectKind],
        now: Timestamp,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO agent_subscriptions (token_id, tenant_id, capabilities, last_seen_at) VALUES (?, ?, ?, ?)
             ON CONFLICT (token_id) DO UPDATE SET capabilities = excluded.capabilities,
                 last_seen_at = excluded.last_seen_at",
        )
        .bind(&token.0)
        .bind(tenant_id)
        .bind(json(&capabilities)?)
        .bind(ts(now))
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Sets the token's push target (RF5), creating its subscription when missing.
    pub async fn put_push_target(
        &self,
        tenant_id: &str,
        token: &AccessTokenId,
        input: &PushInput,
        now: Timestamp,
    ) -> Result<SubscriptionRecord, DbError> {
        let mut tx = write_tx(self.pool).await?;
        let (keep, secret) = match &input.secret {
            SecretUpdate::Keep => (true, None),
            SecretUpdate::Set(ciphertext) => (false, Some(ciphertext.clone())),
            SecretUpdate::Clear => (false, None),
        };
        sqlx::query(
            "INSERT INTO agent_subscriptions (token_id, tenant_id, capabilities, last_seen_at, push_kind, push_url,
                 push_topic, push_secret)
             VALUES (?1, ?2, '[]', ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (token_id) DO UPDATE SET push_kind = excluded.push_kind, push_url = excluded.push_url,
                 push_topic = excluded.push_topic,
                 push_secret = CASE WHEN ?8 THEN agent_subscriptions.push_secret ELSE excluded.push_secret END",
        )
        .bind(&token.0)
        .bind(tenant_id)
        .bind(ts(now))
        .bind(wire(input.kind)?)
        .bind(&input.url)
        .bind(&input.topic)
        .bind(secret)
        .bind(keep)
        .execute(&mut *tx)
        .await?;
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "{SUBSCRIPTION_SELECT} WHERE s.token_id = ?"
        )))
        .bind(&token.0)
        .fetch_one(&mut *tx)
        .await?;
        let record = subscription_from_row(&row)?;
        tx.commit().await?;
        Ok(record)
    }

    /// Removes the token's push target; false when it had none.
    pub async fn delete_push_target(&self, token: &AccessTokenId) -> Result<bool, DbError> {
        let updated = sqlx::query(
            "UPDATE agent_subscriptions SET push_kind = NULL, push_url = NULL, push_topic = NULL,
                 push_secret = NULL WHERE token_id = ? AND push_kind IS NOT NULL",
        )
        .bind(&token.0)
        .execute(self.pool)
        .await?;
        Ok(updated.rows_affected() > 0)
    }

    /// The tenant's subscriptions of tokens that are neither revoked nor expired, most recently seen first.
    pub async fn subscriptions(
        &self,
        tenant_id: &str,
        now: Timestamp,
    ) -> Result<Vec<SubscriptionRecord>, DbError> {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "{SUBSCRIPTION_SELECT} WHERE s.tenant_id = ? AND t.revoked_at IS NULL AND t.expires_at > ?
             ORDER BY s.last_seen_at DESC"
        )))
        .bind(tenant_id)
        .bind(ts(now))
        .fetch_all(self.pool)
        .await?
        .iter()
        .map(subscription_from_row)
        .collect()
    }

    /// Records a push delivery outcome; reports whether its health changed.
    pub async fn record_push_delivery(
        &self,
        token: &AccessTokenId,
        outcome: DeliveryOutcome,
        error: Option<&str>,
        now: Timestamp,
    ) -> Result<FailureTransition, DbError> {
        let mut tx = write_tx(self.pool).await?;
        let failing: Option<Option<String>> =
            sqlx::query_scalar("SELECT failing_since FROM agent_subscriptions WHERE token_id = ?")
                .bind(&token.0)
                .fetch_optional(&mut *tx)
                .await?;
        let was_failing = failing.ok_or(DbError::NotFound)?.is_some();
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE agent_subscriptions SET {HEALTH_SET} WHERE token_id = ?4"
        )))
        .bind(ts(now))
        .bind(wire(outcome)?)
        .bind(error)
        .bind(&token.0)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(transition(was_failing, outcome))
    }

    /// Queues a delivery due now; an existing one for the same target is kept (`false`).
    pub async fn enqueue_delivery(
        &self,
        tenant_id: &str,
        target: &DeliveryTarget,
        now: Timestamp,
    ) -> Result<bool, DbError> {
        let (event, channel, decision, token) = match target {
            DeliveryTarget::Channel { event, channel } => (
                Some(event.to_string()),
                Some(channel.to_string()),
                None,
                None,
            ),
            DeliveryTarget::Push { decision, token } => (
                None,
                None,
                Some(decision.to_string()),
                Some(token.0.clone()),
            ),
        };
        let inserted = sqlx::query(
            "INSERT INTO notification_deliveries (tenant_id, user_event_id, channel_id, decision_id, token_id,
                 state, next_attempt_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?6, ?6)
             ON CONFLICT DO NOTHING",
        )
        .bind(tenant_id)
        .bind(event)
        .bind(channel)
        .bind(decision)
        .bind(token)
        .bind(ts(now))
        .execute(self.pool)
        .await?;
        Ok(inserted.rows_affected() > 0)
    }

    /// Pending deliveries due at `now`, oldest first; at most `limit`.
    pub async fn due_deliveries(
        &self,
        now: Timestamp,
        limit: u32,
    ) -> Result<Vec<DeliveryRecord>, DbError> {
        sqlx::query(
            "SELECT id, user_event_id, channel_id, decision_id, token_id, attempts, next_attempt_at
             FROM notification_deliveries WHERE state = 'pending' AND next_attempt_at <= ?
             ORDER BY next_attempt_at, id LIMIT ?",
        )
        .bind(ts(now))
        .bind(limit)
        .fetch_all(self.pool)
        .await?
        .iter()
        .map(|row| {
            let event: Option<String> = row.try_get("user_event_id")?;
            let channel: Option<String> = row.try_get("channel_id")?;
            let decision: Option<String> = row.try_get("decision_id")?;
            let token: Option<String> = row.try_get("token_id")?;
            let target = match (event, channel, decision, token) {
                (Some(event), Some(channel), None, None) => DeliveryTarget::Channel {
                    event: super::super::jobs::parse_wire(&event, "user_event_id")?,
                    channel: super::super::jobs::parse_wire(&channel, "channel_id")?,
                },
                (None, None, Some(decision), Some(token)) => DeliveryTarget::Push {
                    decision: super::super::jobs::parse_wire(&decision, "decision_id")?,
                    token: AccessTokenId(token),
                },
                _ => {
                    return Err(DbError::Corrupt {
                        column: "notification_deliveries",
                    });
                }
            };
            Ok(DeliveryRecord {
                id: row.try_get("id")?,
                target,
                attempts: u32_col(row, "attempts")?,
                next_attempt_at: ts_col(row, "next_attempt_at")?,
            })
        })
        .collect()
    }

    /// One channel by id.
    pub async fn channel(&self, id: ChannelId) -> Result<Option<ChannelRecord>, DbError> {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {CHANNEL_COLUMNS} FROM notification_channels WHERE id = ?"
        )))
        .bind(id.to_string())
        .fetch_optional(self.pool)
        .await?
        .as_ref()
        .map(channel_from_row)
        .transpose()
    }

    /// The token's subscription while the token is neither revoked nor expired.
    pub async fn subscription(
        &self,
        token: &AccessTokenId,
        now: Timestamp,
    ) -> Result<Option<SubscriptionRecord>, DbError> {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "{SUBSCRIPTION_SELECT} WHERE s.token_id = ? AND t.revoked_at IS NULL AND t.expires_at > ?"
        )))
        .bind(&token.0)
        .bind(ts(now))
        .fetch_optional(self.pool)
        .await?
        .as_ref()
        .map(subscription_from_row)
        .transpose()
    }

    /// One per-user event by id, for its delivery.
    pub async fn user_event(&self, id: UserEventId) -> Result<Option<UserEvent>, DbError> {
        let Some(row) = sqlx::query(
            "SELECT e.id, e.kind, r.owner, r.name, p.number, e.count, e.text, e.url, e.at
             FROM user_events e LEFT JOIN pull_requests p ON p.id = e.pull_request_id
             LEFT JOIN repos r ON r.id = p.repo_id WHERE e.id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(self.pool)
        .await?
        else {
            return Ok(None);
        };
        let number: Option<i64> = row.try_get("number")?;
        let pr = match (row.try_get("owner")?, row.try_get("name")?, number) {
            (Some(owner), Some(name), Some(number)) => Some(PrRef {
                repo: RepoRef { owner, name },
                number: u32::try_from(number).map_err(|_| DbError::Corrupt { column: "number" })?,
            }),
            _ => None,
        };
        Ok(Some(UserEvent {
            id: wire_col(&row, "id")?,
            kind: wire_col(&row, "kind")?,
            pr,
            count: u32_col(&row, "count")?,
            text: row.try_get("text")?,
            url: row.try_get("url")?,
            at: ts_col(&row, "at")?,
        }))
    }

    /// Queues one delivery per enabled channel subscribed to the event's kind, for every user event after `since`
    /// (tests are sent by RN3 itself); a pair queued before is kept. Returns how many were queued.
    pub async fn enqueue_event_deliveries(
        &self,
        since: Timestamp,
        now: Timestamp,
    ) -> Result<u64, DbError> {
        let inserted = sqlx::query(
            "INSERT INTO notification_deliveries (tenant_id, user_event_id, channel_id, state, next_attempt_at,
                 created_at, updated_at)
             SELECT e.tenant_id, e.id, c.id, 'pending', ?2, ?2, ?2
             FROM user_events e JOIN notification_channels c ON c.user_id = e.user_id
             WHERE e.at > ?1 AND e.kind <> 'test' AND c.enabled = 1
               AND EXISTS (SELECT 1 FROM json_each(c.events) j WHERE j.value = e.kind)
             ON CONFLICT DO NOTHING",
        )
        .bind(ts(since))
        .bind(ts(now))
        .execute(self.pool)
        .await?;
        Ok(inserted.rows_affected())
    }

    /// Moves a pending delivery to `at` without counting an attempt (the channel's hourly limit is used up).
    pub async fn postpone_delivery(&self, delivery: i64, at: Timestamp) -> Result<(), DbError> {
        let updated = sqlx::query(
            "UPDATE notification_deliveries SET next_attempt_at = ? WHERE id = ? AND state = 'pending'",
        )
        .bind(ts(at))
        .bind(delivery)
        .execute(self.pool)
        .await?;
        if updated.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    /// Time of the newest event of `kind` about a pull request, any user.
    pub async fn last_user_event_at(
        &self,
        pull_request_id: i64,
        kind: UserEventKind,
    ) -> Result<Option<Timestamp>, DbError> {
        let at: Option<String> = sqlx::query_scalar(
            "SELECT MAX(at) FROM user_events WHERE pull_request_id = ? AND kind = ?",
        )
        .bind(pull_request_id)
        .bind(wire(kind)?)
        .fetch_one(self.pool)
        .await?;
        at.map(|at| super::super::jobs::parse_ts(&at, "at"))
            .transpose()
    }

    /// Records one attempt: delivered, failed for good (`retry_at` none) or failed with a retry at `retry_at`.
    pub async fn record_attempt(
        &self,
        delivery: i64,
        outcome: DeliveryOutcome,
        retry_at: Option<Timestamp>,
        http_status: Option<u16>,
        error: Option<&str>,
        now: Timestamp,
    ) -> Result<(), DbError> {
        let state = match (outcome, retry_at) {
            (DeliveryOutcome::Delivered, _) => "delivered",
            (DeliveryOutcome::Failed, Some(_)) => "pending",
            (DeliveryOutcome::Failed, None) => "failed",
        };
        let next = match outcome {
            DeliveryOutcome::Delivered => None,
            DeliveryOutcome::Failed => retry_at,
        };
        let updated = sqlx::query(
            "UPDATE notification_deliveries SET attempts = attempts + 1, state = ?, next_attempt_at = ?,
                 http_status = ?, error = ?, updated_at = ?
             WHERE id = ? AND state = 'pending'",
        )
        .bind(state)
        .bind(next.map(ts))
        .bind(http_status)
        .bind(error)
        .bind(ts(now))
        .bind(delivery)
        .execute(self.pool)
        .await?;
        if updated.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }
}
