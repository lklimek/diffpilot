//! Repository for `llm_credentials`: credentials runners hold, their pause and usability, and the last LLM
//! call outcome shown in Settings. Frozen signatures (cross-WP).

use chrono::{DateTime, Utc};
use contracts::job::result::{LlmCallOutcome, LlmCallStatus};
use contracts::job::spec::LlmCredentialRef;
use sqlx::sqlite::{SqliteQueryResult, SqliteRow};
use sqlx::{Row, SqlitePool};

use crate::error::DbError;
use crate::repo::jobs::{parse_ts, parse_wire, ts, wire_name};

/// Access to LLM credentials.
#[derive(Debug)]
pub struct LlmCredentials<'a> {
    pub(crate) pool: &'a SqlitePool,
}

/// A stored credential reference with its status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialRecord {
    /// The credential.
    pub credential: LlmCredentialRef,
    /// Paused by a usage limit until then.
    pub paused_until: Option<DateTime<Utc>>,
    /// Unusable since then (refresh failed; the owner must sign in again).
    pub unusable_since: Option<DateTime<Utc>>,
    /// Outcome of the last call made with it.
    pub last_call: Option<LlmCallOutcome>,
}

fn optional_ts(row: &SqliteRow, column: &'static str) -> Result<Option<DateTime<Utc>>, DbError> {
    row.try_get::<Option<String>, _>(column)?
        .map(|at| parse_ts(&at, column))
        .transpose()
}

fn last_call_from_row(row: &SqliteRow) -> Result<Option<LlmCallOutcome>, DbError> {
    let Some(at) = optional_ts(row, "last_call_at")? else {
        return Ok(None);
    };
    let ok: bool = row.try_get("last_call_ok")?;
    let status = if ok {
        LlmCallStatus::Working
    } else {
        LlmCallStatus::NotWorking {
            message: row
                .try_get::<Option<String>, _>("last_call_message")?
                .unwrap_or_default(),
        }
    };
    Ok(Some(LlmCallOutcome { at, status }))
}

fn updated(result: SqliteQueryResult) -> Result<(), DbError> {
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

impl LlmCredentials<'_> {
    /// Registers a credential reported by a runner claim, or updates its usability.
    pub async fn register(
        &self,
        tenant_id: &str,
        credential: &LlmCredentialRef,
        usable: bool,
        now: DateTime<Utc>,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO llm_credentials (id, tenant_id, kind, unusable_since) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (id) DO UPDATE SET kind = excluded.kind,
                 unusable_since = CASE WHEN ?4 IS NULL THEN NULL ELSE COALESCE(unusable_since, ?4) END",
        )
        .bind(&credential.id)
        .bind(tenant_id)
        .bind(wire_name(credential.kind)?)
        .bind((!usable).then(|| ts(now)))
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// A credential.
    pub async fn get(&self, id: &str) -> Result<CredentialRecord, DbError> {
        let row = sqlx::query(
            "SELECT id, kind, paused_until, unusable_since, last_call_at, last_call_ok, last_call_message
             FROM llm_credentials WHERE id = ?",
        )
        .bind(id)
        .fetch_one(self.pool)
        .await?;
        let kind: String = row.try_get("kind")?;
        Ok(CredentialRecord {
            credential: LlmCredentialRef {
                id: row.try_get("id")?,
                kind: parse_wire(&kind, "llm_credentials.kind")?,
            },
            paused_until: optional_ts(&row, "paused_until")?,
            unusable_since: optional_ts(&row, "unusable_since")?,
            last_call: last_call_from_row(&row)?,
        })
    }

    /// Records the outcome of a call made with the credential.
    pub async fn record_call(&self, id: &str, outcome: &LlmCallOutcome) -> Result<(), DbError> {
        let (ok, message) = match &outcome.status {
            LlmCallStatus::Working => (true, None),
            LlmCallStatus::NotWorking { message } => (false, Some(message.as_str())),
        };
        updated(
            sqlx::query(
                "UPDATE llm_credentials SET last_call_at = ?, last_call_ok = ?, last_call_message = ? WHERE id = ?",
            )
            .bind(ts(outcome.at))
            .bind(ok)
            .bind(message)
            .bind(id)
            .execute(self.pool)
            .await?,
        )
    }

    /// The most recent call outcome of the tenant over all credentials.
    pub async fn last_call(&self, tenant_id: &str) -> Result<Option<LlmCallOutcome>, DbError> {
        let row = sqlx::query(
            "SELECT last_call_at, last_call_ok, last_call_message FROM llm_credentials
             WHERE tenant_id = ? AND last_call_at IS NOT NULL ORDER BY last_call_at DESC LIMIT 1",
        )
        .bind(tenant_id)
        .fetch_optional(self.pool)
        .await?;
        match row {
            Some(row) => last_call_from_row(&row),
            None => Ok(None),
        }
    }

    /// Pauses the credential until the usage limit resets.
    pub async fn pause(&self, id: &str, until: DateTime<Utc>) -> Result<(), DbError> {
        updated(
            sqlx::query("UPDATE llm_credentials SET paused_until = ? WHERE id = ?")
                .bind(ts(until))
                .bind(id)
                .execute(self.pool)
                .await?,
        )
    }

    /// Marks the credential unusable.
    pub async fn mark_unusable(&self, id: &str, now: DateTime<Utc>) -> Result<(), DbError> {
        updated(
            sqlx::query(
                "UPDATE llm_credentials SET unusable_since = COALESCE(unusable_since, ?) WHERE id = ?",
            )
            .bind(ts(now))
            .bind(id)
            .execute(self.pool)
            .await?,
        )
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use contracts::job::spec::CredentialKind;

    use super::*;
    use crate::repo::Db;

    fn at(minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 16, 12, minute, 0)
            .single()
            .unwrap_or_default()
    }

    fn credential(id: &str) -> LlmCredentialRef {
        LlmCredentialRef {
            id: id.to_owned(),
            kind: CredentialKind::CodexChatgpt,
        }
    }

    #[tokio::test]
    async fn register_tracks_usability_and_keeps_the_first_unusable_time() {
        let db = Db::in_memory().await.expect("db");
        let credentials = db.llm_credentials();
        credentials
            .register("default", &credential("c1"), true, at(1))
            .await
            .expect("register");
        let record = credentials.get("c1").await.expect("get");
        assert_eq!(record.credential, credential("c1"));
        assert_eq!(record.unusable_since, None);
        assert_eq!(record.last_call, None);
        credentials
            .register("default", &credential("c1"), false, at(2))
            .await
            .expect("unusable");
        credentials
            .register("default", &credential("c1"), false, at(3))
            .await
            .expect("still unusable");
        assert_eq!(
            credentials.get("c1").await.expect("get").unusable_since,
            Some(at(2))
        );
        credentials
            .register("default", &credential("c1"), true, at(4))
            .await
            .expect("usable again");
        assert_eq!(
            credentials.get("c1").await.expect("get").unusable_since,
            None
        );
        credentials.mark_unusable("c1", at(5)).await.expect("mark");
        credentials.pause("c1", at(30)).await.expect("pause");
        let record = credentials.get("c1").await.expect("get");
        assert_eq!(record.unusable_since, Some(at(5)));
        assert_eq!(record.paused_until, Some(at(30)));
        assert!(matches!(
            credentials.pause("missing", at(30)).await,
            Err(DbError::NotFound)
        ));
    }

    #[tokio::test]
    async fn last_call_is_the_newest_outcome_over_all_credentials() {
        let db = Db::in_memory().await.expect("db");
        let credentials = db.llm_credentials();
        assert_eq!(credentials.last_call("default").await.expect("none"), None);
        for id in ["c1", "c2"] {
            credentials
                .register("default", &credential(id), true, at(0))
                .await
                .expect("register");
        }
        let failed = LlmCallOutcome {
            at: at(9),
            status: LlmCallStatus::NotWorking {
                message: "The sign-in expired.".to_owned(),
            },
        };
        credentials
            .record_call(
                "c1",
                &LlmCallOutcome {
                    at: at(5),
                    status: LlmCallStatus::Working,
                },
            )
            .await
            .expect("record");
        credentials
            .record_call("c2", &failed)
            .await
            .expect("record");
        assert_eq!(
            credentials.last_call("default").await.expect("last"),
            Some(failed.clone())
        );
        assert_eq!(
            credentials.get("c2").await.expect("get").last_call,
            Some(failed)
        );
        assert!(matches!(
            credentials
                .record_call(
                    "missing",
                    &LlmCallOutcome {
                        at: at(1),
                        status: LlmCallStatus::Working
                    }
                )
                .await,
            Err(DbError::NotFound)
        ));
    }
}
