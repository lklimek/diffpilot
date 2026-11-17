//! Repository for `feedback`. Frozen signatures (cross-WP: WP3-REVIEW records `missed_by_llm` through
//! [`FeedbackRepo::upsert`], WP3-MEMCAN the rest). One entry per (user, analysis, target, value) (CCR-F1).

use chrono::{DateTime, Utc};
use contracts::api::feedback::{Feedback, FeedbackMemory, FeedbackTarget, FeedbackValue};
use contracts::common::LineSide;
use contracts::ids::{AnalysisId, AnnotationId, FeedbackId, StepId};
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqlitePool};

use super::jobs::{parse_ts, parse_wire, ts, wire_name};
use crate::error::DbError;

/// Access to feedback.
#[derive(Debug)]
pub struct FeedbackRepo<'a> {
    pub(crate) pool: &'a SqlitePool,
}

/// Feedback to record; replaces the note of the user's entry with the same target and value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewFeedback {
    /// Feedback id (kept when replacing).
    pub id: FeedbackId,
    /// Analysis.
    pub analysis_id: AnalysisId,
    /// User.
    pub user_id: String,
    /// Target.
    pub target: FeedbackTarget,
    /// Value.
    pub value: FeedbackValue,
    /// Optional note.
    pub note: Option<String>,
}

/// A feedback revision still to be written to MemCan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingFeedback {
    /// Analysis.
    pub analysis_id: AnalysisId,
    /// The entry as of `revision`.
    pub feedback: Feedback,
    /// Revision of the entry; every replacement or removal increments it.
    pub revision: u32,
    /// The user removed the entry: its memory is to be deleted from MemCan.
    pub removed: bool,
    /// Failed attempts for the current revision.
    pub attempts: u32,
}

/// Whether a MemCan outcome was recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemcanAck {
    /// The acknowledged revision is current; the state was stored.
    Recorded,
    /// The entry was replaced meanwhile; it stays pending for its newer revision.
    Superseded,
}

/// Outcome of one MemCan operation (write or delete) for a revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemcanOutcome {
    /// MemCan confirmed it (stored, or deleted / absent for a removed entry).
    Confirmed,
    /// MemCan answered with a refusal; nothing was done. Retried at `retry_at`.
    Failed {
        /// Next attempt.
        retry_at: DateTime<Utc>,
    },
    /// No usable answer: the operation may still take effect until `settles_at`. Retried at `retry_at`.
    Unknown {
        /// Next attempt.
        retry_at: DateTime<Utc>,
        /// Time after which the operation can no longer land.
        settles_at: DateTime<Utc>,
    },
}

macro_rules! columns {
    () => {
        "id, analysis_id, target_kind, target_key, value, note, created_at, revision, removed_at, memcan_state,
         attempts"
    };
}

/// `(target_kind, target_key)`; a line is keyed `<side>:<line>:<path>`.
fn target_key(target: &FeedbackTarget) -> Result<(&'static str, String), DbError> {
    Ok(match target {
        FeedbackTarget::Step { step_id } => ("step", step_id.0.clone()),
        FeedbackTarget::Annotation { annotation_id } => ("annotation", annotation_id.0.clone()),
        FeedbackTarget::Line { path, side, line } => {
            ("line", format!("{}:{line}:{path}", wire_name(*side)?))
        }
    })
}

fn parse_target(kind: &str, key: String) -> Result<FeedbackTarget, DbError> {
    const COLUMN: &str = "feedback.target_key";
    match kind {
        "step" => Ok(FeedbackTarget::Step {
            step_id: StepId(key),
        }),
        "annotation" => Ok(FeedbackTarget::Annotation {
            annotation_id: AnnotationId(key),
        }),
        "line" => {
            let mut parts = key.splitn(3, ':');
            let (Some(side), Some(line), Some(path)) = (parts.next(), parts.next(), parts.next())
            else {
                return Err(DbError::Corrupt { column: COLUMN });
            };
            Ok(FeedbackTarget::Line {
                path: path.to_owned(),
                side: parse_wire::<LineSide>(side, COLUMN)?,
                line: line
                    .parse()
                    .map_err(|_| DbError::Corrupt { column: COLUMN })?,
            })
        }
        _ => Err(DbError::Corrupt {
            column: "feedback.target_kind",
        }),
    }
}

/// A row selected with `columns!()`.
fn from_row(row: &SqliteRow) -> Result<PendingFeedback, DbError> {
    let id: String = row.try_get("id")?;
    let analysis: String = row.try_get("analysis_id")?;
    let kind: String = row.try_get("target_kind")?;
    let value: String = row.try_get("value")?;
    let created_at: String = row.try_get("created_at")?;
    let revision: i64 = row.try_get("revision")?;
    let removed_at: Option<String> = row.try_get("removed_at")?;
    let memory: String = row.try_get("memcan_state")?;
    let attempts: i64 = row.try_get("attempts")?;
    Ok(PendingFeedback {
        analysis_id: parse_wire(&analysis, "feedback.analysis_id")?,
        feedback: Feedback {
            id: parse_wire(&id, "feedback.id")?,
            target: parse_target(&kind, row.try_get("target_key")?)?,
            value: parse_wire(&value, "feedback.value")?,
            note: row.try_get("note")?,
            created_at: parse_ts(&created_at, "feedback.created_at")?,
            memory: parse_wire::<FeedbackMemory>(&memory, "feedback.memcan_state")?,
        },
        revision: u32::try_from(revision).map_err(|_| DbError::Corrupt {
            column: "feedback.revision",
        })?,
        removed: removed_at.is_some(),
        attempts: u32::try_from(attempts).map_err(|_| DbError::Corrupt {
            column: "feedback.attempts",
        })?,
    })
}

/// The value a calibration value excludes on the same target.
fn excluded_by(value: FeedbackValue) -> Option<FeedbackValue> {
    match value {
        FeedbackValue::Overrated => Some(FeedbackValue::Underrated),
        FeedbackValue::Underrated => Some(FeedbackValue::Overrated),
        _ => None,
    }
}

impl FeedbackRepo<'_> {
    /// Inserts the user's entry for (target, value) or replaces its note; a replacement keeps the id, increments
    /// the revision, revives a removed entry and resets the MemCan state to pending. `overrated` and
    /// `underrated` exclude each other: recording one removes the other in the same transaction.
    /// `NotFound` if the analysis does not exist.
    pub async fn upsert(
        &self,
        feedback: NewFeedback,
        now: DateTime<Utc>,
    ) -> Result<Feedback, DbError> {
        let (kind, key) = target_key(&feedback.target)?;
        let now = ts(now);
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(concat!(
            "INSERT INTO feedback (id, tenant_id, analysis_id, user_id, target_kind, target_key, value, note,
                 created_at, updated_at)
             SELECT ?, tenant_id, id, ?, ?, ?, ?, ?, ?, ? FROM analyses WHERE id = ?
             ON CONFLICT (user_id, analysis_id, target_kind, target_key, value) DO UPDATE SET
                 note = excluded.note, revision = revision + 1, memcan_state = 'pending', attempts = 0,
                 next_attempt_at = NULL,
                 created_at = CASE WHEN removed_at IS NULL THEN created_at ELSE excluded.created_at END,
                 removed_at = NULL, updated_at = excluded.updated_at
             RETURNING ",
            columns!()
        ))
        .bind(feedback.id.to_string())
        .bind(&feedback.user_id)
        .bind(kind)
        .bind(&key)
        .bind(wire_name(feedback.value)?)
        .bind(&feedback.note)
        .bind(&now)
        .bind(&now)
        .bind(feedback.analysis_id.to_string())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(DbError::NotFound)?;
        let stored = from_row(&row)?.feedback;
        if let Some(excluded) = excluded_by(feedback.value) {
            sqlx::query(
                "UPDATE feedback SET removed_at = ?, revision = revision + 1, memcan_state = 'pending',
                     attempts = 0, next_attempt_at = NULL, updated_at = ?
                 WHERE user_id = ? AND analysis_id = ? AND target_kind = ? AND target_key = ? AND value = ?
                     AND removed_at IS NULL",
            )
            .bind(&now)
            .bind(&now)
            .bind(&feedback.user_id)
            .bind(feedback.analysis_id.to_string())
            .bind(kind)
            .bind(&key)
            .bind(wire_name(excluded)?)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(stored)
    }

    /// The user's feedback on an analysis, oldest first (removed entries excluded).
    pub async fn list(
        &self,
        analysis: AnalysisId,
        user_id: &str,
    ) -> Result<Vec<Feedback>, DbError> {
        let rows = sqlx::query(concat!(
            "SELECT ",
            columns!(),
            " FROM feedback WHERE analysis_id = ? AND user_id = ? AND removed_at IS NULL
             ORDER BY created_at, id"
        ))
        .bind(analysis.to_string())
        .bind(user_id)
        .fetch_all(self.pool)
        .await?;
        rows.iter()
            .map(|row| from_row(row).map(|entry| entry.feedback))
            .collect()
    }

    /// Removes the user's entry `id` of the analysis: it leaves [`Self::list`] at once and stays as a pending
    /// tombstone until its MemCan memory is deleted. `NotFound` if the user has no such live entry.
    pub async fn remove(
        &self,
        analysis: AnalysisId,
        user_id: &str,
        id: FeedbackId,
        now: DateTime<Utc>,
    ) -> Result<(), DbError> {
        let now = ts(now);
        let removed = sqlx::query(
            "UPDATE feedback SET removed_at = ?, revision = revision + 1, memcan_state = 'pending', attempts = 0,
                 next_attempt_at = NULL, updated_at = ?
             WHERE id = ? AND analysis_id = ? AND user_id = ? AND removed_at IS NULL",
        )
        .bind(&now)
        .bind(&now)
        .bind(id.to_string())
        .bind(analysis.to_string())
        .bind(user_id)
        .execute(self.pool)
        .await?;
        if removed.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    /// Entries whose current revision is not yet confirmed in MemCan and whose next attempt is due at `now`:
    /// fewest failed attempts first, then oldest.
    pub async fn pending_memcan(
        &self,
        now: DateTime<Utc>,
        limit: u32,
    ) -> Result<Vec<PendingFeedback>, DbError> {
        let rows = sqlx::query(concat!(
            "SELECT ",
            columns!(),
            " FROM feedback WHERE memcan_state != 'written' AND (next_attempt_at IS NULL OR next_attempt_at <= ?)
             ORDER BY attempts, updated_at, id LIMIT ?"
        ))
        .bind(ts(now))
        .bind(i64::from(limit))
        .fetch_all(self.pool)
        .await?;
        rows.iter().map(from_row).collect()
    }

    /// Records the outcome of a MemCan operation for `revision`, sent at `now`.
    ///
    /// An `Unknown` outcome extends the entry's unsettled period even when `revision` is no longer current (the
    /// operation may still land over a newer one). Everything else applies only to the current revision
    /// (`Superseded` otherwise). `Confirmed` while an earlier unknown operation may still land keeps the entry
    /// pending until the period ends (re-assert); otherwise it marks the entry written, or deletes a removed
    /// entry's tombstone. `Failed` / `Unknown` count an attempt and schedule the retry. `NotFound` if the entry
    /// does not exist.
    pub async fn record_memcan_outcome(
        &self,
        id: FeedbackId,
        revision: u32,
        outcome: MemcanOutcome,
        now: DateTime<Utc>,
    ) -> Result<MemcanAck, DbError> {
        let id = id.to_string();
        let now = ts(now);
        let mut tx = self.pool.begin().await?;
        if let MemcanOutcome::Unknown { settles_at, .. } = outcome {
            sqlx::query(
                "UPDATE feedback SET unsettled_until = MAX(COALESCE(unsettled_until, ''), ?) WHERE id = ?",
            )
            .bind(ts(settles_at))
            .bind(&id)
            .execute(&mut *tx)
            .await?;
        }
        let (current, removed_at, unsettled_until): (i64, Option<String>, Option<String>) =
            sqlx::query_as(
                "SELECT revision, removed_at, unsettled_until FROM feedback WHERE id = ?",
            )
            .bind(&id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(DbError::NotFound)?;
        if current != i64::from(revision) {
            tx.commit().await?;
            return Ok(MemcanAck::Superseded);
        }
        let query = match outcome {
            MemcanOutcome::Confirmed => match unsettled_until.filter(|until| *until > now) {
                Some(until) => sqlx::query(
                    "UPDATE feedback SET memcan_state = 'pending', attempts = 0, next_attempt_at = ? WHERE id = ?",
                )
                .bind(until)
                .bind(&id),
                None if removed_at.is_some() => sqlx::query("DELETE FROM feedback WHERE id = ?").bind(&id),
                None => sqlx::query(
                    "UPDATE feedback SET memcan_state = 'written', attempts = 0, next_attempt_at = NULL,
                         unsettled_until = NULL
                     WHERE id = ?",
                )
                .bind(&id),
            },
            MemcanOutcome::Failed { retry_at } | MemcanOutcome::Unknown { retry_at, .. } => sqlx::query(
                "UPDATE feedback SET memcan_state = 'failed', attempts = attempts + 1, next_attempt_at = ?
                 WHERE id = ?",
            )
            .bind(ts(retry_at))
            .bind(&id),
        };
        query.execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(MemcanAck::Recorded)
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::repo::Db;
    use crate::repo::analyses::testing::seed_analysis;

    fn at(minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 17, 12, minute, 0)
            .single()
            .unwrap_or_default()
    }

    fn failed(retry_at: DateTime<Utc>) -> MemcanOutcome {
        MemcanOutcome::Failed { retry_at }
    }

    fn step(id: &str) -> FeedbackTarget {
        FeedbackTarget::Step {
            step_id: StepId(id.to_owned()),
        }
    }

    fn new(analysis: AnalysisId, user: &str, value: FeedbackValue, note: &str) -> NewFeedback {
        NewFeedback {
            id: FeedbackId::new_random(),
            analysis_id: analysis,
            user_id: user.to_owned(),
            target: step("f3.1"),
            value,
            note: Some(note.to_owned()),
        }
    }

    async fn setup() -> (Db, AnalysisId) {
        let db = Db::in_memory().await.expect("db");
        let analysis = seed_analysis(&db).await.expect("seed");
        sqlx::raw_sql(
            "INSERT INTO users (id, github_id, login, created_at, updated_at)
             VALUES ('u2', 2, 'other', '2026-11-16T00:00:00.000000Z', '2026-11-16T00:00:00.000000Z')",
        )
        .execute(&db.pool)
        .await
        .expect("second user");
        (db, analysis)
    }

    /// `(memcan_state, revision, removed)` of a stored row.
    async fn state_of(db: &Db, id: FeedbackId) -> Option<(String, i64, bool)> {
        sqlx::query_as::<_, (String, i64, Option<String>)>(
            "SELECT memcan_state, revision, removed_at FROM feedback WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&db.pool)
        .await
        .expect("state")
        .map(|(state, revision, removed)| (state, revision, removed.is_some()))
    }

    #[tokio::test]
    async fn posting_a_value_again_replaces_its_note_and_keeps_the_id() {
        let (db, analysis) = setup().await;
        let repo = db.feedback();
        let first = repo
            .upsert(new(analysis, "u1", FeedbackValue::Wrong, "one"), at(1))
            .await
            .expect("first");
        repo.record_memcan_outcome(first.id, 1, MemcanOutcome::Confirmed, at(30))
            .await
            .expect("ack");
        let second = repo
            .upsert(new(analysis, "u1", FeedbackValue::Wrong, "two"), at(2))
            .await
            .expect("second");
        assert_eq!(second.id, first.id, "same entry");
        assert_eq!(second.note.as_deref(), Some("two"));
        assert_eq!(second.created_at, at(1), "recorded at the first post");
        assert_eq!(
            state_of(&db, first.id).await,
            Some(("pending".to_owned(), 2, false)),
            "the new revision is pending"
        );
        assert_eq!(repo.list(analysis, "u1").await.expect("list"), [second]);
    }

    #[tokio::test]
    async fn several_values_coexist_on_one_target() {
        let (db, analysis) = setup().await;
        let repo = db.feedback();
        let wrong = repo
            .upsert(new(analysis, "u1", FeedbackValue::Wrong, "w"), at(1))
            .await
            .expect("wrong");
        let overrated = repo
            .upsert(new(analysis, "u1", FeedbackValue::Overrated, "o"), at(2))
            .await
            .expect("overrated");
        assert_eq!(
            repo.list(analysis, "u1").await.expect("list"),
            [wrong, overrated]
        );
    }

    #[tokio::test]
    async fn overrated_and_underrated_exclude_each_other() {
        let (db, analysis) = setup().await;
        let repo = db.feedback();
        let wrong = repo
            .upsert(new(analysis, "u1", FeedbackValue::Wrong, "w"), at(1))
            .await
            .expect("wrong");
        let overrated = repo
            .upsert(new(analysis, "u1", FeedbackValue::Overrated, "o"), at(2))
            .await
            .expect("overrated");
        let underrated = repo
            .upsert(new(analysis, "u1", FeedbackValue::Underrated, "u"), at(3))
            .await
            .expect("underrated");
        assert_eq!(
            repo.list(analysis, "u1").await.expect("list"),
            [wrong.clone(), underrated.clone()]
        );
        assert_eq!(
            state_of(&db, overrated.id).await,
            Some(("pending".to_owned(), 2, true)),
            "overrated removed, its memory deletion pending"
        );
        let back = repo
            .upsert(new(analysis, "u1", FeedbackValue::Overrated, "o2"), at(4))
            .await
            .expect("overrated again");
        assert_eq!(back.id, overrated.id, "revived");
        assert_eq!(back.created_at, at(4), "a revived entry is recorded anew");
        assert_eq!(
            repo.list(analysis, "u1").await.expect("list"),
            [wrong, back]
        );
        assert_eq!(state_of(&db, underrated.id).await.map(|s| s.2), Some(true));
    }

    #[tokio::test]
    async fn exclusivity_is_per_user_and_target() {
        let (db, analysis) = setup().await;
        let repo = db.feedback();
        let mine = repo
            .upsert(new(analysis, "u1", FeedbackValue::Overrated, "o"), at(1))
            .await
            .expect("mine");
        let mut other_step = new(analysis, "u1", FeedbackValue::Underrated, "u");
        other_step.target = step("f3.2");
        repo.upsert(other_step, at(2)).await.expect("other step");
        repo.upsert(new(analysis, "u2", FeedbackValue::Underrated, "u"), at(3))
            .await
            .expect("other user");
        assert_eq!(state_of(&db, mine.id).await.map(|s| s.2), Some(false));
    }

    #[tokio::test]
    async fn list_shows_only_the_users_live_entries_of_the_analysis() {
        let (db, analysis) = setup().await;
        let repo = db.feedback();
        repo.upsert(new(analysis, "u2", FeedbackValue::Wrong, "theirs"), at(1))
            .await
            .expect("theirs");
        let other = seed_analysis(&db).await.expect("other analysis");
        repo.upsert(new(other, "u1", FeedbackValue::Wrong, "elsewhere"), at(1))
            .await
            .expect("elsewhere");
        assert!(repo.list(analysis, "u1").await.expect("list").is_empty());
    }

    #[tokio::test]
    async fn removal_hides_the_entry_until_memcan_deleted_it() {
        let (db, analysis) = setup().await;
        let repo = db.feedback();
        let entry = repo
            .upsert(new(analysis, "u1", FeedbackValue::Correct, "c"), at(1))
            .await
            .expect("entry");
        repo.remove(analysis, "u1", entry.id, at(2))
            .await
            .expect("remove");
        assert!(repo.list(analysis, "u1").await.expect("list").is_empty());
        let pending = repo.pending_memcan(at(59), 10).await.expect("pending");
        assert_eq!(pending.len(), 1);
        assert!(pending[0].removed);
        assert_eq!(pending[0].revision, 2);
        assert!(matches!(
            repo.remove(analysis, "u1", entry.id, at(3)).await,
            Err(DbError::NotFound)
        ));
        assert_eq!(
            repo.record_memcan_outcome(entry.id, 2, MemcanOutcome::Confirmed, at(30))
                .await
                .expect("ack"),
            MemcanAck::Recorded
        );
        assert_eq!(state_of(&db, entry.id).await, None, "tombstone deleted");
    }

    #[tokio::test]
    async fn removal_of_another_users_or_analysis_entry_is_not_found() {
        let (db, analysis) = setup().await;
        let repo = db.feedback();
        let theirs = repo
            .upsert(new(analysis, "u2", FeedbackValue::Wrong, "t"), at(1))
            .await
            .expect("theirs");
        let other = seed_analysis(&db).await.expect("other analysis");
        for (analysis, user) in [(analysis, "u1"), (other, "u2")] {
            assert!(matches!(
                repo.remove(analysis, user, theirs.id, at(2)).await,
                Err(DbError::NotFound)
            ));
        }
        assert_eq!(state_of(&db, theirs.id).await.map(|s| s.2), Some(false));
    }

    #[tokio::test]
    async fn stale_acks_leave_the_newer_revision_pending() {
        let (db, analysis) = setup().await;
        let repo = db.feedback();
        let entry = repo
            .upsert(new(analysis, "u1", FeedbackValue::Wrong, "a"), at(1))
            .await
            .expect("entry");
        repo.upsert(new(analysis, "u1", FeedbackValue::Wrong, "b"), at(2))
            .await
            .expect("replace");
        assert_eq!(
            repo.record_memcan_outcome(entry.id, 1, MemcanOutcome::Confirmed, at(30))
                .await
                .expect("ack"),
            MemcanAck::Superseded
        );
        assert_eq!(
            state_of(&db, entry.id).await,
            Some(("pending".to_owned(), 2, false))
        );
        repo.remove(analysis, "u1", entry.id, at(3))
            .await
            .expect("remove");
        assert_eq!(
            repo.record_memcan_outcome(entry.id, 2, MemcanOutcome::Confirmed, at(30))
                .await
                .expect("ack"),
            MemcanAck::Superseded,
            "a write of the old text never deletes the tombstone"
        );
        assert!(state_of(&db, entry.id).await.is_some());
        assert!(matches!(
            repo.record_memcan_outcome(
                FeedbackId::new_random(),
                1,
                MemcanOutcome::Confirmed,
                at(30)
            )
            .await,
            Err(DbError::NotFound)
        ));
    }

    #[tokio::test]
    async fn pending_lists_fresh_entries_before_retries_oldest_first() {
        let (db, analysis) = setup().await;
        let repo = db.feedback();
        let mut ids = Vec::new();
        for (minute, value) in [
            (1, FeedbackValue::Wrong),
            (2, FeedbackValue::Correct),
            (3, FeedbackValue::Irrelevant),
        ] {
            let entry = repo
                .upsert(new(analysis, "u1", value, "n"), at(minute))
                .await
                .expect("entry");
            ids.push(entry.id);
        }
        repo.record_memcan_outcome(ids[0], 1, failed(at(40)), at(30))
            .await
            .expect("failed");
        repo.record_memcan_outcome(ids[1], 1, MemcanOutcome::Confirmed, at(30))
            .await
            .expect("written");
        let pending = repo.pending_memcan(at(59), 10).await.expect("pending");
        let order: Vec<FeedbackId> = pending.iter().map(|p| p.feedback.id).collect();
        assert_eq!(order, [ids[2], ids[0]]);
        assert_eq!(pending[0].analysis_id, analysis);
        assert_eq!(
            repo.pending_memcan(at(59), 1).await.expect("limit").len(),
            1
        );
    }

    #[tokio::test]
    async fn line_targets_round_trip_with_colons_in_the_path() {
        let (db, analysis) = setup().await;
        let mut missed = new(analysis, "u1", FeedbackValue::MissedByLlm, "");
        missed.note = None;
        missed.target = FeedbackTarget::Line {
            path: "src/a:b.rs".to_owned(),
            side: LineSide::Base,
            line: 96,
        };
        let stored = db
            .feedback()
            .upsert(missed.clone(), at(1))
            .await
            .expect("missed");
        assert_eq!(stored.target, missed.target);
        assert_eq!(stored.value, FeedbackValue::MissedByLlm);
        assert_eq!(stored.note, None);
    }

    #[tokio::test]
    async fn feedback_on_an_unknown_analysis_is_not_found() {
        let (db, _) = setup().await;
        let result = db
            .feedback()
            .upsert(
                new(AnalysisId::new_random(), "u1", FeedbackValue::Wrong, "x"),
                at(1),
            )
            .await;
        assert!(matches!(result, Err(DbError::NotFound)));
    }

    #[tokio::test]
    async fn entries_carry_the_state_of_their_memcan_copy() {
        let (db, analysis) = setup().await;
        let repo = db.feedback();
        let entry = repo
            .upsert(new(analysis, "u1", FeedbackValue::Overrated, "o"), at(1))
            .await
            .expect("entry");
        assert_eq!(entry.memory, FeedbackMemory::Pending);
        repo.record_memcan_outcome(entry.id, 1, failed(at(5)), at(2))
            .await
            .expect("failed");
        let listed = repo.list(analysis, "u1").await.expect("list");
        assert_eq!(listed[0].memory, FeedbackMemory::Failed);
        repo.record_memcan_outcome(entry.id, 1, MemcanOutcome::Confirmed, at(5))
            .await
            .expect("written");
        let listed = repo.list(analysis, "u1").await.expect("list");
        assert_eq!(listed[0].memory, FeedbackMemory::Written);
    }

    #[tokio::test]
    async fn retries_wait_for_their_own_schedule_without_blocking_others() {
        let (db, analysis) = setup().await;
        let repo = db.feedback();
        let broken = repo
            .upsert(new(analysis, "u1", FeedbackValue::Wrong, "b"), at(1))
            .await
            .expect("broken");
        repo.record_memcan_outcome(broken.id, 1, failed(at(10)), at(2))
            .await
            .expect("failed");
        let fresh = repo
            .upsert(new(analysis, "u1", FeedbackValue::Correct, "c"), at(3))
            .await
            .expect("fresh");
        let db = &db;
        let due = |now| async move {
            db.feedback()
                .pending_memcan(now, 10)
                .await
                .expect("pending")
                .iter()
                .map(|entry| (entry.feedback.id, entry.attempts))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            due(at(4)).await,
            [(fresh.id, 0)],
            "not due before its retry"
        );
        assert_eq!(
            due(at(10)).await,
            [(fresh.id, 0), (broken.id, 1)],
            "fresh entries first"
        );
        repo.upsert(new(analysis, "u1", FeedbackValue::Wrong, "b2"), at(11))
            .await
            .expect("new revision");
        assert_eq!(
            due(at(11)).await,
            [(fresh.id, 0), (broken.id, 0)],
            "a new revision is due at once"
        );
    }

    #[tokio::test]
    async fn a_confirmation_before_an_unknown_write_settles_is_re_asserted() {
        let (db, analysis) = setup().await;
        let repo = db.feedback();
        let entry = repo
            .upsert(new(analysis, "u1", FeedbackValue::Overrated, "a"), at(1))
            .await
            .expect("entry");
        let unknown = MemcanOutcome::Unknown {
            retry_at: at(3),
            settles_at: at(20),
        };
        repo.record_memcan_outcome(entry.id, 1, unknown, at(2))
            .await
            .expect("unknown");
        repo.remove(analysis, "u1", entry.id, at(4))
            .await
            .expect("remove");
        assert_eq!(
            repo.record_memcan_outcome(entry.id, 2, MemcanOutcome::Confirmed, at(5))
                .await
                .expect("deleted"),
            MemcanAck::Recorded
        );
        assert_eq!(
            state_of(&db, entry.id).await,
            Some(("pending".to_owned(), 2, true)),
            "tombstone kept: the unknown write may still land"
        );
        assert!(
            repo.pending_memcan(at(19), 10)
                .await
                .expect("due")
                .is_empty()
        );
        assert_eq!(repo.pending_memcan(at(20), 10).await.expect("due").len(), 1);
        repo.record_memcan_outcome(entry.id, 2, MemcanOutcome::Confirmed, at(21))
            .await
            .expect("deleted again");
        assert_eq!(state_of(&db, entry.id).await, None, "settled: dropped");
    }

    #[tokio::test]
    async fn an_unknown_write_of_an_old_revision_still_unsettles_the_entry() {
        let (db, analysis) = setup().await;
        let repo = db.feedback();
        let entry = repo
            .upsert(new(analysis, "u1", FeedbackValue::Wrong, "a"), at(1))
            .await
            .expect("entry");
        repo.upsert(new(analysis, "u1", FeedbackValue::Wrong, "b"), at(2))
            .await
            .expect("revision 2");
        let unknown = MemcanOutcome::Unknown {
            retry_at: at(3),
            settles_at: at(20),
        };
        assert_eq!(
            repo.record_memcan_outcome(entry.id, 1, unknown, at(2))
                .await
                .expect("unknown"),
            MemcanAck::Superseded
        );
        repo.record_memcan_outcome(entry.id, 2, MemcanOutcome::Confirmed, at(4))
            .await
            .expect("written");
        assert_eq!(
            repo.list(analysis, "u1").await.expect("list")[0].memory,
            FeedbackMemory::Pending,
            "re-asserted after the old write settles"
        );
        assert!(
            repo.pending_memcan(at(19), 10)
                .await
                .expect("due")
                .is_empty()
        );
        repo.record_memcan_outcome(entry.id, 2, MemcanOutcome::Confirmed, at(20))
            .await
            .expect("written");
        assert_eq!(
            repo.list(analysis, "u1").await.expect("list")[0].memory,
            FeedbackMemory::Written
        );
    }
}
