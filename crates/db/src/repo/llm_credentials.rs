//! Per-user gateway-observed connection status, usage pauses and last-call outcomes; no provider secrets.

use contracts::ids::UserId;
use contracts::job::result::{LlmCallOutcome, LlmCallStatus};
use contracts::time::Timestamp;
use sqlx::sqlite::{SqliteQueryResult, SqliteRow};
use sqlx::{Row, SqlitePool};

use crate::error::DbError;
use crate::repo::jobs::{parse_ts, ts};

/// Access to LLM credentials.
#[derive(Debug)]
pub struct LlmCredentials<'a> {
    pub(crate) pool: &'a SqlitePool,
}

/// A stored credential reference with its status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialRecord {
    /// The credential.
    pub payer: UserId,
    /// Paused by a usage limit until then.
    pub paused_until: Option<Timestamp>,
    /// Unusable since then (refresh failed; the owner must sign in again).
    pub unusable_since: Option<Timestamp>,
    /// Outcome of the last call made with it.
    pub last_call: Option<LlmCallOutcome>,
}

fn optional_ts(row: &SqliteRow, column: &'static str) -> Result<Option<Timestamp>, DbError> {
    row.try_get::<Option<String>, _>(column)?
        .map(|at| parse_ts(&at, column))
        .transpose()
}

pub(crate) fn last_call_from_row(row: &SqliteRow) -> Result<Option<LlmCallOutcome>, DbError> {
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
    /// Payers linked by the gateway; runner claims cannot create or revive credentials.
    pub async fn linked_payers(&self) -> Result<Vec<String>, DbError> {
        let mut tx = super::read_tx(self.pool).await?;
        let users = sqlx::query_scalar(
            "SELECT user_id FROM llm_credentials WHERE state='linked' ORDER BY user_id",
        )
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(users)
    }

    /// Removes local status after an explicit gateway unlink; this never revokes an upstream credential.
    pub async fn remove_status(&self, user: &str) -> Result<bool, DbError> {
        let mut tx = super::write_tx(self.pool).await?;
        let removed = sqlx::query("DELETE FROM llm_credentials WHERE user_id=?")
            .bind(user)
            .execute(&mut *tx)
            .await?
            .rows_affected()
            == 1;
        tx.commit().await?;
        Ok(removed)
    }

    /// Merges gateway status, retaining longer pauses and calls without a newer gateway observation.
    pub async fn put_status(
        &self,
        tenant: &str,
        user: &str,
        status: &contracts::gateway::CredentialStatus,
    ) -> Result<(), DbError> {
        let mut tx = super::write_tx(self.pool).await?;
        sqlx::query("INSERT INTO llm_credentials(user_id,tenant_id,state,relink_reason,plan,label,paused_until,last_call_at,last_call_ok,last_call_message)
            VALUES(?,?,?,?,?,?,?,?,?,?) ON CONFLICT(user_id) DO UPDATE SET state=excluded.state,relink_reason=excluded.relink_reason,
            plan=excluded.plan,label=excluded.label,
            paused_until=CASE WHEN excluded.paused_until IS NULL THEN llm_credentials.paused_until
                ELSE MAX(COALESCE(llm_credentials.paused_until,excluded.paused_until),excluded.paused_until) END,
            last_call_at=CASE WHEN excluded.last_call_at>COALESCE(llm_credentials.last_call_at,'')
                THEN excluded.last_call_at ELSE llm_credentials.last_call_at END,
            last_call_ok=CASE WHEN excluded.last_call_at>COALESCE(llm_credentials.last_call_at,'')
                THEN excluded.last_call_ok ELSE llm_credentials.last_call_ok END,
            last_call_message=CASE WHEN excluded.last_call_at>COALESCE(llm_credentials.last_call_at,'')
                THEN excluded.last_call_message ELSE llm_credentials.last_call_message END,
            last_call_failure=CASE WHEN excluded.last_call_at>COALESCE(llm_credentials.last_call_at,'')
                THEN NULL ELSE llm_credentials.last_call_failure END")
            .bind(user).bind(tenant).bind(crate::repo::jobs::wire_name(status.state)?)
            .bind(status.relink_reason.map(crate::repo::jobs::wire_name).transpose()?).bind(&status.plan).bind(&status.label)
            .bind(status.paused_until.map(ts)).bind(status.last_call.as_ref().map(|c|ts(c.at)))
            .bind(status.last_call.as_ref().map(|c|matches!(c.status,LlmCallStatus::Working)))
            .bind(status.last_call.as_ref().and_then(|c|match &c.status {LlmCallStatus::Working=>None,LlmCallStatus::NotWorking {message}=>Some(message)}))
            .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Reads a user's common connection status; gateway rotation state is stored elsewhere.
    pub async fn status(
        &self,
        user: &str,
    ) -> Result<Option<contracts::gateway::CredentialStatus>, DbError> {
        let mut tx = super::read_tx(self.pool).await?;
        let row = sqlx::query("SELECT * FROM llm_credentials WHERE user_id=?")
            .bind(user)
            .fetch_optional(&mut *tx)
            .await?;
        tx.commit().await?;
        row.map(|r| {
            Ok(contracts::gateway::CredentialStatus {
                telemetry: Default::default(),
                challenge: None,
                mode: contracts::gateway::CredentialMode::ChatgptPlan,
                state: crate::repo::jobs::parse_wire(
                    &r.try_get::<String, _>("state")?,
                    "llm_credentials.state",
                )?,
                relink_reason: r
                    .try_get::<Option<String>, _>("relink_reason")?
                    .map(|s| crate::repo::jobs::parse_wire(&s, "llm_credentials.relink_reason"))
                    .transpose()?,
                plan: r.try_get("plan")?,
                label: r.try_get("label")?,
                paused_until: optional_ts(&r, "paused_until")?,
                last_call: last_call_from_row(&r)?,
            })
        })
        .transpose()
    }

    /// Seeds safe payer status in isolated test databases.
    #[cfg(any(test, feature = "test-util"))]
    pub async fn register(
        &self,
        tenant_id: &str,
        credential: &UserId,
        usable: bool,
        now: Timestamp,
    ) -> Result<(), DbError> {
        let mut tx = super::write_tx(self.pool).await?;
        sqlx::query(
            "INSERT INTO llm_credentials (user_id, tenant_id, state, relink_reason, relink_needed_at)
             VALUES (?1, ?2, CASE WHEN ?3 THEN 'linked' ELSE 'relink_needed' END,
                 CASE WHEN ?3 THEN NULL ELSE 'terminal' END, ?4)
             ON CONFLICT (user_id) DO UPDATE SET state = excluded.state, relink_reason = excluded.relink_reason,
                 relink_needed_at = CASE WHEN ?4 IS NULL THEN NULL ELSE COALESCE(relink_needed_at, ?4) END",
        )
        .bind(&credential.0)
        .bind(tenant_id)
        .bind(usable)
        .bind((!usable).then(|| ts(now)))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// A credential.
    pub async fn get(&self, id: &str) -> Result<CredentialRecord, DbError> {
        let mut tx = super::read_tx(self.pool).await?;
        let row = sqlx::query(
            "SELECT user_id AS id, paused_until, relink_needed_at AS unusable_since, last_call_at, last_call_ok, last_call_message
             FROM llm_credentials WHERE user_id = ?",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(CredentialRecord {
            payer: contracts::ids::UserId(row.try_get("id")?),
            paused_until: optional_ts(&row, "paused_until")?,
            unusable_since: optional_ts(&row, "unusable_since")?,
            last_call: last_call_from_row(&row)?,
        })
    }

    /// Records the outcome of a call made with the credential.
    pub async fn record_call(&self, id: &str, outcome: &LlmCallOutcome) -> Result<(), DbError> {
        let mut tx = super::write_tx(self.pool).await?;
        let (ok, message) = match &outcome.status {
            LlmCallStatus::Working => (true, None),
            LlmCallStatus::NotWorking { message } => (false, Some(message.as_str())),
        };
        updated(
            sqlx::query(
                "UPDATE llm_credentials SET last_call_at = ?, last_call_ok = ?, last_call_message = ?,
                     last_call_failure = NULL WHERE user_id = ?",
            )
            .bind(ts(outcome.at))
            .bind(ok)
            .bind(message)
            .bind(id)
            .execute(&mut *tx)
            .await?,
        )?;
        tx.commit().await?;
        Ok(())
    }

    /// Pauses the credential until the usage limit resets.
    pub async fn pause(&self, id: &str, until: Timestamp) -> Result<(), DbError> {
        let mut tx = super::write_tx(self.pool).await?;
        updated(
            sqlx::query("UPDATE llm_credentials SET paused_until = ? WHERE user_id = ?")
                .bind(ts(until))
                .bind(id)
                .execute(&mut *tx)
                .await?,
        )?;
        tx.commit().await?;
        Ok(())
    }

    /// Marks the credential unusable.
    pub async fn mark_unusable(&self, id: &str, now: Timestamp) -> Result<(), DbError> {
        let mut tx = super::write_tx(self.pool).await?;
        updated(
            sqlx::query(
                "UPDATE llm_credentials SET state = 'relink_needed', relink_reason = 'terminal', relink_needed_at = COALESCE(relink_needed_at, ?) WHERE user_id = ?",
            )
            .bind(ts(now))
            .bind(id)
            .execute(&mut *tx)
            .await?,
        )?;
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::repo::Db;

    fn at(minute: u32) -> Timestamp {
        format!("2026-11-16T12:{minute:02}:00Z")
            .parse::<Timestamp>()
            .expect("time")
    }

    async fn fixture() -> Db {
        let db = Db::in_memory().await.unwrap();
        sqlx::query("INSERT INTO users(id,github_id,login,created_at,updated_at) VALUES ('c1',1,'c1','t','t'),('c2',2,'c2','t','t')").execute(&db.pool).await.unwrap();
        db
    }

    fn credential(id: &str) -> UserId {
        contracts::ids::UserId(id.to_owned())
    }

    #[tokio::test]
    async fn register_tracks_usability_and_keeps_the_first_unusable_time() {
        let db = fixture().await;
        let credentials = db.llm_credentials();
        credentials
            .register("default", &credential("c1"), true, at(1))
            .await
            .expect("register");
        let record = credentials.get("c1").await.expect("get");
        assert_eq!(record.payer, credential("c1"));
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
    async fn last_call_is_private_to_the_payer() {
        let db = fixture().await;
        let credentials = db.llm_credentials();
        assert_eq!(credentials.status("c1").await.unwrap(), None);
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
            credentials.get("c2").await.expect("last").last_call,
            Some(failed.clone())
        );
        assert_eq!(
            credentials.get("c2").await.expect("get").last_call,
            Some(failed)
        );
        assert!(matches!(
            credentials
                .get("c1")
                .await
                .unwrap()
                .last_call
                .unwrap()
                .status,
            LlmCallStatus::Working
        ));
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
