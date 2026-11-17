//! Repository for `review_state`: each user's ticks and last viewed block per analysis.

use chrono::{DateTime, Utc};
use contracts::api::review::ReviewState;
use contracts::ids::AnalysisId;
use sqlx::SqlitePool;

use super::Db;
use super::jobs::ts;
use crate::error::DbError;

/// Access to review states.
#[derive(Debug)]
pub struct ReviewStates<'a> {
    pool: &'a SqlitePool,
}

impl Db {
    /// Review states (ticks, last viewed block).
    pub fn review_states(&self) -> ReviewStates<'_> {
        ReviewStates { pool: &self.pool }
    }
}

impl ReviewStates<'_> {
    /// The user's review state of the analysis; empty before the first write.
    pub async fn get(&self, user_id: &str, analysis: AnalysisId) -> Result<ReviewState, DbError> {
        let row: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT ticked_steps, last_viewed FROM review_states WHERE user_id = ? AND analysis_id = ?",
        )
        .bind(user_id)
        .bind(analysis.to_string())
        .fetch_optional(self.pool)
        .await?;
        let Some((ticked_steps, last_viewed)) = row else {
            return Ok(ReviewState::default());
        };
        Ok(ReviewState {
            ticked_steps: serde_json::from_str(&ticked_steps)?,
            last_viewed: last_viewed
                .map(|block| serde_json::from_str(&block))
                .transpose()?,
        })
    }

    /// Replaces the user's review state of the analysis in one statement (writing it again changes nothing);
    /// `NotFound` for an unknown analysis.
    pub async fn put(
        &self,
        user_id: &str,
        analysis: AnalysisId,
        state: &ReviewState,
        now: DateTime<Utc>,
    ) -> Result<(), DbError> {
        let last_viewed = state
            .last_viewed
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let stored = sqlx::query(
            "INSERT INTO review_states (user_id, analysis_id, tenant_id, ticked_steps, last_viewed, updated_at)
             SELECT ?, id, tenant_id, ?, ?, ? FROM analyses WHERE id = ?
             ON CONFLICT (user_id, analysis_id) DO UPDATE SET ticked_steps = excluded.ticked_steps,
                 last_viewed = excluded.last_viewed, updated_at = excluded.updated_at",
        )
        .bind(user_id)
        .bind(serde_json::to_string(&state.ticked_steps)?)
        .bind(last_viewed)
        .bind(ts(now))
        .bind(analysis.to_string())
        .execute(self.pool)
        .await?;
        if stored.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use contracts::api::review::{BlockRef, QuietRowRef};
    use contracts::common::NotAnalysedCategory;
    use contracts::ids::StepId;

    use super::*;
    use crate::repo::analyses::testing::seed_analysis;

    fn state(ticks: &[&str], last_viewed: Option<BlockRef>) -> ReviewState {
        ReviewState {
            ticked_steps: ticks.iter().map(|id| StepId((*id).to_owned())).collect(),
            last_viewed,
        }
    }

    async fn rows(db: &Db) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM review_states")
            .fetch_one(&db.pool)
            .await
            .expect("count")
    }

    #[tokio::test]
    async fn review_state_is_empty_before_the_first_write() {
        let db = Db::in_memory().await.expect("db");
        let analysis = seed_analysis(&db).await.expect("analysis");
        let read = db.review_states().get("u1", analysis).await;
        assert_eq!(read.expect("state"), ReviewState::default());
    }

    #[tokio::test]
    async fn review_state_put_replaces_and_is_idempotent() {
        let db = Db::in_memory().await.expect("db");
        let analysis = seed_analysis(&db).await.expect("analysis");
        let first = state(
            &["f1.1", "f2.1"],
            Some(BlockRef::Quiet {
                row: QuietRowRef::NotAnalysed {
                    category: NotAnalysedCategory::Tests,
                },
            }),
        );
        let states = db.review_states();
        for _ in 0..2 {
            states
                .put("u1", analysis, &first, Utc::now())
                .await
                .expect("put");
            assert_eq!(states.get("u1", analysis).await.expect("get"), first);
        }
        assert_eq!(rows(&db).await, 1);
        let second = state(&[], None);
        states
            .put("u1", analysis, &second, Utc::now())
            .await
            .expect("put");
        assert_eq!(states.get("u1", analysis).await.expect("get"), second);
        assert_eq!(rows(&db).await, 1);
    }

    #[tokio::test]
    async fn review_state_is_per_user_and_takes_the_analysis_tenant() {
        let db = Db::in_memory().await.expect("db");
        let analysis = seed_analysis(&db).await.expect("analysis");
        sqlx::query(
            "INSERT INTO users (id, github_id, login, created_at, updated_at)
             VALUES ('u2', 2, 'bob', '2026-11-16T00:00:00Z', '2026-11-16T00:00:00Z')",
        )
        .execute(&db.pool)
        .await
        .expect("user");
        let ticked = state(&["f1.1"], Some(BlockRef::PrContext));
        db.review_states()
            .put("u1", analysis, &ticked, Utc::now())
            .await
            .expect("put");
        assert_eq!(
            db.review_states().get("u2", analysis).await.expect("get"),
            ReviewState::default()
        );
        let tenant: String = sqlx::query_scalar("SELECT tenant_id FROM review_states")
            .fetch_one(&db.pool)
            .await
            .expect("tenant");
        assert_eq!(tenant, crate::repo::DEFAULT_TENANT);
    }

    #[tokio::test]
    async fn review_state_put_for_an_unknown_analysis_is_not_found() {
        let db = Db::in_memory().await.expect("db");
        seed_analysis(&db).await.expect("analysis");
        let result = db
            .review_states()
            .put(
                "u1",
                AnalysisId::new_random(),
                &state(&[], None),
                Utc::now(),
            )
            .await;
        assert!(matches!(result, Err(DbError::NotFound)), "{result:?}");
    }
}
