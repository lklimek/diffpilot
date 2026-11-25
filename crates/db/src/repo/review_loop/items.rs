//! PR read state, items with revisions, LLM assessments and assessment passes.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, Sqlite, Transaction};

use contracts::api::review_loop::{
    AuthorKind, CommentAssessment, CommentItemKind, GateMark, ItemGithubFacts, ItemLocation,
    ItemState, OutsideResolution, ReadFailure,
};
use contracts::common::{LineRange, RepoRef};
use contracts::ids::{CommentItemId, CommitSha, JobId, PassId, QuestionId};
use contracts::time::Timestamp;

use super::super::jobs::ts;
use super::super::{read_tx, write_tx};
use super::marks::{ACTIVE_MARK_COLUMN, active_in_pass, end_marks, log_changes};
use super::{
    ReviewLoop, json, json_col, opt_json_col, opt_ts_col, opt_wire_col, ts_col, u32_col, wire,
    wire_col,
};
use crate::error::DbError;

/// The review-loop state of one pull request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommentPrRecord {
    /// `pull_requests.id`.
    pub pull_request_id: i64,
    /// Last successful read.
    pub read_at: Option<Timestamp>,
    /// Failure of the latest read and when.
    pub read_failure: Option<(ReadFailure, Timestamp)>,
    /// The server's snapshot of the last successful read.
    pub github_snapshot: Option<serde_json::Value>,
    /// Bot rounds read.
    pub round_count: u32,
    /// The PR purpose of the latest pass (LLM).
    pub purpose: Option<String>,
}

/// Outcome of a GitHub read.
#[derive(Clone, Debug, PartialEq)]
pub enum ReadResult {
    /// Read: the server's snapshot and the rounds counted.
    Read {
        /// Snapshot document.
        snapshot: serde_json::Value,
        /// Rounds.
        round_count: u32,
    },
    /// Failed; the last good read stays.
    Failed(ReadFailure),
}

/// An item as the server observed it (GitHub read, or a PR-level question it built).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservedItem {
    /// Source.
    pub kind: CommentItemKind,
    /// Stable key (plan §2.1).
    pub stable_key: String,
    /// Author login.
    pub author: String,
    /// Author kind.
    pub author_kind: AuthorKind,
    /// A bot PR status or summary comment.
    pub status_comment: bool,
    /// GitHub link.
    pub url: Option<String>,
    /// Discussed lines.
    pub location: Option<ItemLocation>,
    /// Title.
    pub title: String,
    /// Comment text.
    pub body: String,
    /// SHA-256 hex of the normalised finding text.
    pub body_hash: String,
    /// PR head at this read.
    pub head: CommitSha,
    /// Bot round at this read.
    pub round: Option<u32>,
    /// The bot's severity words.
    pub bot_severity: Option<String>,
    /// GitHub facts.
    pub github: ItemGithubFacts,
    /// Resolved outside diffpilot.
    pub outside: Option<OutsideResolution>,
}

/// A stored item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemRecord {
    /// Candidate grounded and authorised by the host for this point.
    #[serde(skip)]
    pub import_candidate: Option<super::ImportCandidate>,
    /// Active internal disposition.
    #[serde(skip)]
    pub active_mark: Option<super::ActiveMark>,
    /// Versioned source for a grounded body/comment point.
    pub source_id: Option<i64>,
    /// Verbatim source span.
    pub quote: Option<String>,
    /// Linked duplicate item, without merging independent thread identities.
    pub duplicate_of: Option<CommentItemId>,
    /// Id.
    pub id: CommentItemId,
    /// `pull_requests.id`.
    pub pull_request_id: i64,
    /// `C<no>`.
    pub no: u32,
    /// Source.
    pub kind: CommentItemKind,
    /// Stable key.
    pub stable_key: String,
    /// Author login.
    pub author: String,
    /// Author kind.
    pub author_kind: AuthorKind,
    /// A bot PR status or summary comment.
    #[serde(default)]
    pub status_comment: bool,
    /// GitHub link.
    pub url: Option<String>,
    /// Discussed lines.
    pub location: Option<ItemLocation>,
    /// Title.
    pub title: String,
    /// Comment text.
    pub body: String,
    /// Normalised text hash.
    pub body_hash: String,
    /// Head at first sight.
    pub first_seen_head: CommitSha,
    /// Round at first sight.
    pub first_round: Option<u32>,
    /// The bot's severity words.
    pub bot_severity: Option<String>,
    /// Gates.
    pub gates: Vec<GateMark>,
    /// GitHub facts.
    pub github: ItemGithubFacts,
    /// State.
    pub state: ItemState,
    /// Revision.
    pub revision: u32,
    /// Resolved outside diffpilot.
    pub outside: Option<OutsideResolution>,
    /// The item whose decision this repeat carries or re-asks.
    pub repeat_of: Option<CommentItemId>,
    /// When a complete read first no longer produced it; `None` while reads produce it.
    pub gone_at: Option<Timestamp>,
    /// Last change.
    pub updated_at: Timestamp,
}

/// Result of [`ReviewLoop::upsert_item`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpsertOutcome {
    /// First sight: numbered and stored.
    New(ItemRecord),
    /// Observed values changed: revision raised.
    Changed(ItemRecord),
    /// Nothing changed.
    Unchanged(ItemRecord),
}

/// A stored assessment with its fingerprint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredAssessment {
    /// The assessment.
    pub assessment: CommentAssessment,
    /// Fingerprint of comment text, code at its lines and PR purpose.
    pub fingerprint: String,
    /// Description from the primary method's report, when provided.
    pub description: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct AssessmentBody<T> {
    #[serde(flatten)]
    assessment: T,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

fn stored_assessment(
    body: AssessmentBody<CommentAssessment>,
    fingerprint: String,
) -> StoredAssessment {
    StoredAssessment {
        assessment: body.assessment,
        fingerprint,
        description: body.description,
    }
}

/// What started an assessment pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PassCause {
    /// A session requested a pass for the PR's comments.
    Opened,
    /// A read found new comments or a moved head.
    Read,
    /// "Assess again" on an item.
    AssessAgain,
    /// An owner question without an analysis.
    Question,
    /// A watched PR's detection.
    Watch,
}

/// An item sent with a pass and the fingerprint it was sent at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SentItem {
    /// The item.
    pub item: CommentItemId,
    /// Its `C<no>`.
    pub no: u32,
    /// Fingerprint of comment text and code at its lines when sent.
    pub fingerprint: String,
}

/// A pass to start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewPass {
    /// `pull_requests.id`.
    pub pull_request_id: i64,
    /// User charged (daily cap); none when nobody is.
    pub user_id: Option<String>,
    /// Cause.
    pub cause: PassCause,
    /// Head assessed.
    pub head: CommitSha,
    /// Items sent.
    pub sent: Vec<SentItem>,
    /// The question a question pass answers.
    pub question: Option<QuestionId>,
}

/// State of a pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PassState {
    /// Waiting for its job.
    Queued,
    /// Its job is submitted.
    Running,
    /// Ingested.
    Finished,
    /// Ended without an ingested result.
    Failed,
}

/// A stored pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PassRecord {
    /// Id.
    pub id: PassId,
    /// `pull_requests.id`.
    pub pull_request_id: i64,
    /// User charged.
    pub user_id: Option<String>,
    /// Cause.
    pub cause: PassCause,
    /// Head assessed.
    pub head: CommitSha,
    /// State.
    pub state: PassState,
    /// Its `verify_comments` job, once submitted.
    pub job_id: Option<JobId>,
    /// Items sent.
    pub sent: Vec<SentItem>,
    /// The question a question pass answers.
    pub question: Option<QuestionId>,
    /// The batch whose continuation this question interrupted.
    pub resume_after_question: Option<PassId>,
    /// Whether the primary method can still produce progress, or already did.
    pub may_continue: bool,
    /// When the live pass was admitted.
    pub created_at: Timestamp,
    /// When it ended (finished or failed).
    pub finished_at: Option<Timestamp>,
}

// The primary job is linked on admission. A terminal primary without a card promises no follow-up,
// even while the secondary method still holds the pass open.
const PASS_MAY_CONTINUE: &str = "NOT EXISTS (
    SELECT 1 FROM jobs j LEFT JOIN assessment_method_runs r ON r.job_id=j.id
    WHERE j.id=p.job_id AND (
        j.state IN ('failed','cancelled') OR j.cancel_requested OR
        r.status IN ('failed','cancelled') OR (r.status='succeeded' AND NOT EXISTS (
            SELECT 1 FROM assessment_comparison_items c
            JOIN comment_assessments a ON a.item_id=c.primary_item_id AND a.pass_id=c.pass_id
            WHERE c.pass_id=r.pass_id AND c.method=r.method
        ))
    )) AS may_continue";

const PASS_COLUMNS: &str = "p.id, p.pull_request_id, p.user_id, p.cause, p.head, p.state, p.job_id, p.sent, p.question_id, p.created_at, p.finished_at";

// Keep the sent-item array readable by older binaries when adding a question handoff.
#[derive(Deserialize, Serialize)]
struct PassInputItem<T> {
    #[serde(flatten)]
    item: T,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resume_after_question: Option<PassId>,
}

fn pass_from_row(row: &SqliteRow) -> Result<PassRecord, DbError> {
    let input: Vec<PassInputItem<SentItem>> = json_col(row, "sent")?;
    let resume_after_question = input.first().and_then(|entry| entry.resume_after_question);
    Ok(PassRecord {
        id: wire_col(row, "id")?,
        pull_request_id: row.try_get("pull_request_id")?,
        user_id: row.try_get("user_id")?,
        cause: wire_col(row, "cause")?,
        head: CommitSha(row.try_get("head")?),
        state: wire_col(row, "state")?,
        job_id: opt_wire_col(row, "job_id")?,
        sent: input.into_iter().map(|entry| entry.item).collect(),
        question: opt_wire_col(row, "question_id")?,
        resume_after_question,
        may_continue: row.try_get("may_continue")?,
        created_at: ts_col(row, "created_at")?,
        finished_at: opt_ts_col(row, "finished_at")?,
    })
}

/// Result of [`ReviewLoop::start_pass`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassStart {
    /// Queued.
    Started(PassId),
    /// Another pass of the PR is queued or running (one per PR).
    Busy(PassId),
    /// A sent item was marked while preparing the batch.
    MarkedItem,
}

const ITEM_COLUMNS: &str = "id, pull_request_id, item_no, kind, stable_key, author, author_kind, url, path,
    start_line, end_line, title, body, body_hash, first_seen_head, first_round, bot_severity, gate_marks, github,
    state, revision, outside, repeat_of, gone_at, updated_at, source_id, quote, duplicate_of, status_comment,
    CASE WHEN EXISTS (SELECT 1 FROM memberships m WHERE m.tenant_id = comment_items.tenant_id
        AND m.user_id = json_extract(comment_items.import_candidate, '$.user_id'))
        THEN import_candidate END AS import_candidate";

fn item_from_row(row: &SqliteRow) -> Result<ItemRecord, DbError> {
    let path: Option<String> = row.try_get("path")?;
    let start: Option<i64> = row.try_get("start_line")?;
    let end: Option<i64> = row.try_get("end_line")?;
    let lines = match (start, end) {
        (Some(start), Some(end)) => Some(LineRange {
            start_line: u32::try_from(start).map_err(|_| DbError::Corrupt {
                column: "start_line",
            })?,
            end_line: u32::try_from(end).map_err(|_| DbError::Corrupt { column: "end_line" })?,
        }),
        _ => None,
    };
    let first_round: Option<i64> = row.try_get("first_round")?;
    Ok(ItemRecord {
        import_candidate: opt_json_col(row, "import_candidate")?,
        active_mark: opt_json_col(row, "active_mark")?,
        source_id: row.try_get("source_id")?,
        quote: row.try_get("quote")?,
        duplicate_of: opt_wire_col(row, "duplicate_of")?,
        id: wire_col(row, "id")?,
        pull_request_id: row.try_get("pull_request_id")?,
        no: u32_col(row, "item_no")?,
        kind: wire_col(row, "kind")?,
        stable_key: row.try_get("stable_key")?,
        author: row.try_get("author")?,
        author_kind: wire_col(row, "author_kind")?,
        status_comment: row.try_get("status_comment")?,
        url: row.try_get("url")?,
        location: path.map(|path| ItemLocation { path, lines }),
        title: row.try_get("title")?,
        body: row.try_get("body")?,
        body_hash: row.try_get("body_hash")?,
        first_seen_head: CommitSha(row.try_get("first_seen_head")?),
        first_round: first_round
            .map(u32::try_from)
            .transpose()
            .map_err(|_| DbError::Corrupt {
                column: "first_round",
            })?,
        bot_severity: row.try_get("bot_severity")?,
        gates: json_col(row, "gate_marks")?,
        github: json_col(row, "github")?,
        state: wire_col(row, "state")?,
        revision: u32_col(row, "revision")?,
        outside: opt_json_col(row, "outside")?,
        repeat_of: opt_wire_col(row, "repeat_of")?,
        gone_at: opt_ts_col(row, "gone_at")?,
        updated_at: ts_col(row, "updated_at")?,
    })
}

/// The item `id` inside `tx`.
pub(super) async fn item_in(
    tx: &mut Transaction<'static, Sqlite>,
    id: CommentItemId,
) -> Result<ItemRecord, DbError> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {ITEM_COLUMNS}, {ACTIVE_MARK_COLUMN} FROM comment_items WHERE id = ?"
    )))
    .bind(id.to_string())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(DbError::NotFound)?;
    item_from_row(&row)
}

/// Sets an item's state and raises its revision inside `tx`.
pub(super) async fn set_state_in(
    tx: &mut Transaction<'static, Sqlite>,
    id: CommentItemId,
    state: ItemState,
    now: Timestamp,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE comment_items SET state = ?, revision = revision + 1, updated_at = ? WHERE id = ?",
    )
    .bind(wire(state)?)
    .bind(ts(now))
    .bind(id.to_string())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn lines_of(location: Option<&ItemLocation>) -> (Option<&str>, Option<u32>, Option<u32>) {
    match location {
        Some(location) => (
            Some(location.path.as_str()),
            location.lines.map(|lines| lines.start_line),
            location.lines.map(|lines| lines.end_line),
        ),
        None => (None, None, None),
    }
}

impl ReviewLoop<'_> {
    /// Whether a primary duplicate link already covers this exact point/code version.
    pub async fn duplicate_is_current(
        &self,
        item: CommentItemId,
        fingerprint: &str,
    ) -> Result<bool, DbError> {
        let mut tx = read_tx(self.pool).await?;
        let current = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM comment_items WHERE id=? AND duplicate_of IS NOT NULL AND duplicate_fingerprint=?)")
            .bind(item.to_string()).bind(fingerprint).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(current)
    }

    /// Binds a grounded point to its source version; SQL enforces PR/tenant and duplicate ownership.
    pub async fn bind_source(
        &self,
        item: CommentItemId,
        source: i64,
        quote: &str,
        duplicate: Option<CommentItemId>,
    ) -> Result<(), DbError> {
        let mut tx = super::super::write_tx(self.pool).await?;
        let changed =
            sqlx::query("UPDATE comment_items SET source_id=?,quote=?,duplicate_of=? WHERE id=?")
                .bind(source)
                .bind(quote)
                .bind(duplicate.map(|id| id.to_string()))
                .bind(item.to_string())
                .execute(&mut *tx)
                .await?
                .rows_affected();
        if changed != 1 {
            return Err(DbError::NotFound);
        }
        tx.commit().await?;
        Ok(())
    }

    /// Creates the PR's review-loop row if missing (idempotent).
    pub async fn ensure_pr(
        &self,
        tenant_id: &str,
        pull_request_id: i64,
        now: Timestamp,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO comment_prs (pull_request_id, tenant_id, created_at) VALUES (?, ?, ?)
             ON CONFLICT (pull_request_id) DO NOTHING",
        )
        .bind(pull_request_id)
        .bind(tenant_id)
        .bind(ts(now))
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Records a GitHub read: a success replaces the snapshot and clears the failure; a failure keeps the last
    /// good read.
    pub async fn record_read(
        &self,
        pull_request_id: i64,
        result: &ReadResult,
        now: Timestamp,
    ) -> Result<(), DbError> {
        let updated = match result {
            ReadResult::Read {
                snapshot,
                round_count,
            } => sqlx::query(
                "UPDATE comment_prs SET read_at = ?, read_failure = NULL, read_failed_at = NULL,
                     github_snapshot = ?, round_count = ? WHERE pull_request_id = ?",
            )
            .bind(ts(now))
            .bind(json(snapshot)?)
            .bind(round_count)
            .bind(pull_request_id),
            ReadResult::Failed(failure) => sqlx::query(
                "UPDATE comment_prs SET read_failure = ?, read_failed_at = ? WHERE pull_request_id = ?",
            )
            .bind(wire(failure)?)
            .bind(ts(now))
            .bind(pull_request_id),
        }
        .execute(self.pool)
        .await?;
        if updated.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    /// Stores the PR purpose of the latest pass (LLM text).
    pub async fn set_purpose(&self, pull_request_id: i64, purpose: &str) -> Result<(), DbError> {
        sqlx::query("UPDATE comment_prs SET purpose = ? WHERE pull_request_id = ?")
            .bind(purpose)
            .bind(pull_request_id)
            .execute(self.pool)
            .await?;
        Ok(())
    }

    /// The PR's review-loop row.
    pub async fn pr_state(&self, pull_request_id: i64) -> Result<Option<CommentPrRecord>, DbError> {
        pr_state_with(self.pool, pull_request_id).await
    }

    /// Stores what a read observed. A new stable key gets the next `C<no>` and `state_if_new`; a known one is
    /// updated and its revision raised only when an observed value changed or it was gone. `outside` set moves the
    /// item to `outside`; cleared again (reopened on GitHub) it returns to `state_if_new`. Needs
    /// [`Self::ensure_pr`].
    pub async fn upsert_item(
        &self,
        tenant_id: &str,
        pull_request_id: i64,
        observed: &ObservedItem,
        state_if_new: ItemState,
        now: Timestamp,
    ) -> Result<UpsertOutcome, DbError> {
        let mut tx = write_tx(self.pool).await?;
        let outcome = upsert_item_in(
            &mut tx,
            tenant_id,
            pull_request_id,
            observed,
            state_if_new,
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(outcome)
    }

    /// After a complete read that produced the items keyed `read_keys`: marks the PR's other GitHub items (not its
    /// PR-level questions) gone, raising their revision; rows and decisions stay. Returns the newly gone `C<no>`s.
    pub async fn mark_gone(
        &self,
        pull_request_id: i64,
        read_keys: &[&str],
        now: Timestamp,
    ) -> Result<Vec<u32>, DbError> {
        let rows = sqlx::query(
            "UPDATE comment_items SET gone_at = ?1, revision = revision + 1, updated_at = ?1
             WHERE pull_request_id = ?2 AND kind <> 'pr_question' AND source_id IS NULL AND gone_at IS NULL
               AND stable_key NOT IN (SELECT value FROM json_each(?3))
             RETURNING item_no",
        )
        .bind(ts(now))
        .bind(pull_request_id)
        .bind(json(&read_keys)?)
        .fetch_all(self.pool)
        .await?;
        let mut gone = rows
            .iter()
            .map(|row| u32_col(row, "item_no"))
            .collect::<Result<Vec<u32>, DbError>>()?;
        gone.sort_unstable();
        Ok(gone)
    }

    /// The PR's items by `C<no>`.
    pub async fn items(&self, pull_request_id: i64) -> Result<Vec<ItemRecord>, DbError> {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {ITEM_COLUMNS}, {ACTIVE_MARK_COLUMN} FROM comment_items WHERE pull_request_id = ? ORDER BY item_no"
        )))
        .bind(pull_request_id)
        .fetch_all(self.pool)
        .await?
        .iter()
        .map(item_from_row)
        .collect()
    }

    /// The item `C<no>` of a PR.
    pub async fn item_by_no(
        &self,
        pull_request_id: i64,
        no: u32,
    ) -> Result<Option<ItemRecord>, DbError> {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {ITEM_COLUMNS}, {ACTIVE_MARK_COLUMN} FROM comment_items WHERE pull_request_id = ? AND item_no = ?"
        )))
        .bind(pull_request_id)
        .bind(no)
        .fetch_optional(self.pool)
        .await?
        .map(|row| item_from_row(&row))
        .transpose()
    }

    /// Sets an item's state when it is still at `revision` (`Conflict` otherwise); returns the new record.
    /// `outside` is set only by [`Self::upsert_item`].
    pub async fn set_item_state(
        &self,
        id: CommentItemId,
        revision: u32,
        state: ItemState,
        now: Timestamp,
    ) -> Result<ItemRecord, DbError> {
        let mut tx = write_tx(self.pool).await?;
        let item = item_in(&mut tx, id).await?;
        if item.revision != revision || state == ItemState::Outside {
            return Err(DbError::Conflict);
        }
        set_state_in(&mut tx, id, state, now).await?;
        let item = item_in(&mut tx, id).await?;
        tx.commit().await?;
        Ok(item)
    }

    /// Replaces an item's gates when it is still at `revision` (`Conflict` otherwise).
    pub async fn set_gates(
        &self,
        (tenant, user): (&str, &str),
        id: CommentItemId,
        revision: u32,
        gates: &[GateMark],
        now: Timestamp,
    ) -> Result<ItemRecord, DbError> {
        let mut tx = write_tx(self.pool).await?;
        let updated = sqlx::query(
            "UPDATE comment_items SET gate_marks = ?, revision = revision + 1, updated_at = ?
             WHERE id = ? AND revision = ?",
        )
        .bind(json(&gates)?)
        .bind(ts(now))
        .bind(id.to_string())
        .bind(revision)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() == 0 {
            item_in(&mut tx, id).await?;
            return Err(DbError::Conflict);
        }
        if !gates.is_empty() {
            super::origins::reopen_follower(&mut tx, id, now).await?;
        }
        let item = item_in(&mut tx, id).await?;
        if !gates.is_empty()
            && item
                .active_mark
                .as_ref()
                .is_some_and(|m| m.mark.follows.is_some())
        {
            let marks = end_marks(&mut tx, tenant, id, user, now).await?;
            let item = item_in(&mut tx, id).await?;
            tx.commit().await?;
            log_changes(marks);
            return Ok(item);
        }
        tx.commit().await?;
        Ok(item)
    }

    /// Stores an assessment of an item. A recommendation or gates other than the latest assessment's raise the item's
    /// revision, so a decision prepared on what was shown before is refused as changed.
    /// Returns false for status comments and actively marked items.
    pub async fn add_assessment(
        &self,
        tenant_id: &str,
        item: CommentItemId,
        assessment: &CommentAssessment,
        fingerprint: &str,
    ) -> Result<bool, DbError> {
        self.add_assessment_with_description(tenant_id, item, assessment, fingerprint, None)
            .await
    }

    /// Stores a validated assessment with its report description, including when it is reused at a new head.
    pub async fn add_assessment_with_description(
        &self,
        tenant_id: &str,
        item: CommentItemId,
        assessment: &CommentAssessment,
        fingerprint: &str,
        description: Option<&str>,
    ) -> Result<bool, DbError> {
        let mut tx = write_tx(self.pool).await?;
        let stored = add_assessment_in(
            &mut tx,
            tenant_id,
            item,
            assessment,
            fingerprint,
            description,
        )
        .await?;
        tx.commit().await?;
        Ok(stored)
    }

    /// The newest assessment of an item.
    pub async fn latest_assessment(
        &self,
        item: CommentItemId,
    ) -> Result<Option<StoredAssessment>, DbError> {
        sqlx::query(
            "SELECT body, fingerprint FROM comment_assessments WHERE item_id = ? ORDER BY id DESC LIMIT 1",
        )
        .bind(item.to_string())
        .fetch_optional(self.pool)
        .await?
        .map(|row| {
            Ok(stored_assessment(json_col(&row, "body")?, row.try_get("fingerprint")?))
        })
        .transpose()
    }
    /// Queues a pass unless one of the PR is queued or running.
    pub async fn start_pass(
        &self,
        tenant_id: &str,
        pass: &NewPass,
        now: Timestamp,
    ) -> Result<PassStart, DbError> {
        self.start_pass_with_continuation(tenant_id, pass, now, None)
            .await
    }

    /// Queues a pass and stores the interrupted batch with a question, if there was one.
    pub async fn start_pass_with_continuation(
        &self,
        tenant_id: &str,
        pass: &NewPass,
        now: Timestamp,
        resume_after_question: Option<PassId>,
    ) -> Result<PassStart, DbError> {
        let mut tx = write_tx(self.pool).await?;
        let live: Option<String> = sqlx::query_scalar(
            "SELECT id FROM comment_passes WHERE pull_request_id = ? AND state IN ('queued', 'running')",
        )
        .bind(pass.pull_request_id)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(live) = live {
            return Ok(PassStart::Busy(super::super::jobs::parse_wire(
                &live, "id",
            )?));
        }
        if active_in_pass(&mut tx, tenant_id, &pass.sent).await? {
            return Ok(PassStart::MarkedItem);
        }
        let id = PassId::new_random();
        let items =
            u32::try_from(pass.sent.len()).map_err(|_| DbError::Corrupt { column: "items" })?;
        sqlx::query(
            "INSERT INTO comment_passes (id, tenant_id, pull_request_id, user_id, cause, head, items, state,
                 created_at, sent, question_id)
             VALUES (?, ?, ?, ?, ?, ?, ?, 'queued', ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(tenant_id)
        .bind(pass.pull_request_id)
        .bind(&pass.user_id)
        .bind(wire(pass.cause)?)
        .bind(&pass.head.0)
        .bind(items)
        .bind(ts(now))
        .bind(match (resume_after_question, pass.cause, pass.sent.as_slice()) {
            (Some(id), PassCause::Question, [item]) => json(&[PassInputItem { item, resume_after_question: Some(id) }])?,
            (Some(_), _, _) => return Err(DbError::Conflict),
            (None, _, items) => json(&items)?,
        })
        .bind(pass.question.map(|question| question.to_string()))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(PassStart::Started(id))
    }

    /// Marks a queued pass running with its job.
    pub async fn pass_running(&self, pass: PassId, job: JobId) -> Result<(), DbError> {
        let updated = sqlx::query(
            "UPDATE comment_passes SET state = 'running', job_id = ? WHERE id = ? AND state = 'queued'",
        )
        .bind(job.to_string())
        .bind(pass.to_string())
        .execute(self.pool)
        .await?;
        if updated.rows_affected() == 0 {
            return Err(DbError::Conflict);
        }
        Ok(())
    }

    /// Ends a live pass (`succeeded` false = failed).
    pub async fn finish_pass(
        &self,
        pass: PassId,
        succeeded: bool,
        now: Timestamp,
    ) -> Result<(), DbError> {
        let updated = sqlx::query(
            "UPDATE comment_passes SET state = ?, finished_at = ? WHERE id = ? AND state IN ('queued', 'running')",
        )
        .bind(if succeeded { "finished" } else { "failed" })
        .bind(ts(now))
        .bind(pass.to_string())
        .execute(self.pool)
        .await?;
        if updated.rows_affected() == 0 {
            return Err(DbError::Conflict);
        }
        Ok(())
    }

    /// A pass.
    pub async fn pass(&self, id: PassId) -> Result<PassRecord, DbError> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {PASS_COLUMNS}, {PASS_MAY_CONTINUE} FROM comment_passes p WHERE p.id = ?"
        )))
        .bind(id.to_string())
        .fetch_optional(self.pool)
        .await?
        .ok_or(DbError::NotFound)?;
        pass_from_row(&row)
    }

    /// The PR's latest assessment pass.
    pub async fn latest_pass(&self, pull_request_id: i64) -> Result<Option<PassRecord>, DbError> {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {PASS_COLUMNS}, {PASS_MAY_CONTINUE} FROM comment_passes p WHERE p.pull_request_id = ?
             ORDER BY p.created_at DESC, p.rowid DESC LIMIT 1"
        )))
        .bind(pull_request_id)
        .fetch_optional(self.pool)
        .await?
        .as_ref()
        .map(pass_from_row)
        .transpose()
    }

    /// Queued and running passes of every pull request, oldest first (server start: follow them again).
    pub async fn live_passes(&self) -> Result<Vec<PassRecord>, DbError> {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {PASS_COLUMNS}, {PASS_MAY_CONTINUE} FROM comment_passes p WHERE p.state IN ('queued', 'running')
             ORDER BY p.created_at, p.rowid"
        )))
        .fetch_all(self.pool)
        .await?
        .iter()
        .map(pass_from_row)
        .collect()
    }

    /// The item `id`.
    pub async fn item(&self, id: CommentItemId) -> Result<ItemRecord, DbError> {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {ITEM_COLUMNS}, {ACTIVE_MARK_COLUMN} FROM comment_items WHERE id = ?"
        )))
        .bind(id.to_string())
        .fetch_optional(self.pool)
        .await?
        .map(|row| item_from_row(&row))
        .transpose()?
        .ok_or(DbError::NotFound)
    }

    /// For each of `prs` (repository and number) that the tenant read successfully or `user_id` watches: its comment
    /// count of current comment items after a successful read, and whether the user watches it, in list order.
    /// One read-only statement for the whole list.
    pub async fn comment_tallies(
        &self,
        tenant_id: &str,
        user_id: &str,
        prs: &[(RepoRef, u32)],
    ) -> Result<Vec<PrCommentTally>, DbError> {
        let wanted = serde_json::Value::Array(
            prs.iter()
                .map(|(repo, number)| serde_json::json!([repo.owner, repo.name, number]))
                .collect(),
        );
        let rows: Vec<(i64, i64, bool, bool, i64, i64)> = sqlx::query_as(
            "WITH wanted AS (
                 SELECT CAST(j.key AS INTEGER) AS idx, json_extract(j.value, '$[0]') AS owner,
                     json_extract(j.value, '$[1]') AS name, json_extract(j.value, '$[2]') AS number
                 FROM json_each(?1) j),
             known AS (
                 SELECT w.idx, p.id AS pr,
                     EXISTS (SELECT 1 FROM comment_prs c WHERE c.pull_request_id = p.id AND c.tenant_id = ?2
                         AND c.read_at IS NOT NULL) AS was_read,
                     EXISTS (SELECT 1 FROM pr_watches pw WHERE pw.pull_request_id = p.id AND pw.tenant_id = ?2
                         AND pw.user_id = ?3 AND pw.state <> 'ended') AS watched
                 FROM wanted w
                 JOIN repos r ON r.tenant_id = ?2 AND r.owner = w.owner AND r.name = w.name
                 JOIN pull_requests p ON p.tenant_id = ?2 AND p.repo_id = r.id AND p.number = w.number)
             SELECT k.idx, k.pr, k.was_read, k.watched, COUNT(i.id),
                 (SELECT COUNT(*) FROM comment_decision_origins o
                    JOIN comment_decisions d ON d.id = o.decision_id
                    JOIN comment_items ci ON ci.id = d.item_id
                    LEFT JOIN comment_visits v ON v.tenant_id = ci.tenant_id
                        AND v.pull_request_id = ci.pull_request_id AND v.user_id = ?3
                    WHERE ci.tenant_id = ?2 AND ci.pull_request_id = k.pr
                        AND o.origin = 'imported_github'
                        AND NOT EXISTS (SELECT 1 FROM comment_decision_undos u WHERE u.decision_id = d.id)
                        AND NOT EXISTS (SELECT 1 FROM comment_decisions n WHERE n.supersedes = d.id)
                        AND (v.visited_at IS NULL OR rtrim(d.decided_at, 'Z') > rtrim(v.visited_at, 'Z')))
             FROM known k
             LEFT JOIN comment_items i ON k.was_read AND i.pull_request_id = k.pr AND i.kind <> 'pr_question'
                 AND i.gone_at IS NULL
             WHERE k.was_read OR k.watched
             GROUP BY k.idx
             ORDER BY k.idx",
        )
        .bind(wanted.to_string())
        .bind(tenant_id)
        .bind(user_id)
        .fetch_all(self.pool)
        .await?;
        rows.into_iter()
            .map(
                |(index, pull_request_id, was_read, watched, total, unseen)| {
                    Ok(PrCommentTally {
                        index: usize::try_from(index).unwrap_or(usize::MAX),
                        pull_request_id,
                        total: was_read
                            .then(|| {
                                u32::try_from(total).map_err(|_| DbError::Corrupt {
                                    column: "comment_items",
                                })
                            })
                            .transpose()?,
                        imported_unseen: u32::try_from(unseen).unwrap_or(u32::MAX),
                        watched,
                    })
                },
            )
            .collect()
    }

    /// Reads the queue inputs for all requested dashboard PRs in one transaction.
    /// A malformed PR is returned as a row-local error.
    pub async fn comment_tally_views(
        &self,
        tenant_id: &str,
        prs: &[i64],
    ) -> Result<HashMap<i64, Result<PrTallyView, DbError>>, DbError> {
        if prs.is_empty() {
            return Ok(HashMap::new());
        }
        let wanted = json(&prs)?;
        let mut tx = read_tx(self.pool).await?;
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT c.pull_request_id, c.read_at, c.read_failure, c.read_failed_at, NULL AS github_snapshot,
                    c.round_count, c.purpose, p.id, p.user_id, p.cause, p.head, p.state, p.job_id,
                    p.sent, p.question_id, p.created_at, p.finished_at, {PASS_MAY_CONTINUE}
             FROM comment_prs c LEFT JOIN comment_passes p ON p.id = (
                 SELECT id FROM comment_passes WHERE pull_request_id = c.pull_request_id
                 ORDER BY created_at DESC, rowid DESC LIMIT 1)
             WHERE c.tenant_id = ? AND c.read_at IS NOT NULL
               AND c.pull_request_id IN (SELECT value FROM json_each(?))",
        )))
        .bind(tenant_id)
        .bind(wanted)
        .fetch_all(&mut *tx)
        .await?;
        let mut views = HashMap::with_capacity(rows.len());
        for row in &rows {
            let id: i64 = row.try_get("pull_request_id")?;
            views.insert(
                id,
                (|| {
                    Ok(PrTallyView {
                        pr: PrView {
                            row: Some(pr_state_from_row(row)?),
                            items: vec![],
                            assessments: vec![],
                            decisions: vec![],
                        },
                        latest_pass: row
                            .try_get::<Option<String>, _>("id")?
                            .map(|_| pass_from_row(row))
                            .transpose()?,
                    })
                })(),
            );
        }
        let ids: Vec<i64> = views
            .iter()
            .filter_map(|(&id, view)| view.is_ok().then_some(id))
            .collect();
        if ids.is_empty() {
            tx.commit().await?;
            return Ok(views);
        }
        let ids = json(&ids)?;
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT i.*, i.pull_request_id AS tally_pr, a.body AS assessment_body, a.fingerprint AS assessment_fingerprint
             FROM (SELECT {ITEM_COLUMNS}, {ACTIVE_MARK_COLUMN} FROM comment_items WHERE pull_request_id IN (SELECT value FROM json_each(?))) i
             LEFT JOIN comment_assessments a ON a.id = (SELECT MAX(l.id) FROM comment_assessments l WHERE l.item_id = i.id)
             ORDER BY i.pull_request_id, i.item_no",
        )))
        .bind(&ids)
        .fetch_all(&mut *tx)
        .await?;
        apply_tally_rows(&mut views, &rows, |view, row| {
            let item = item_from_row(row)?;
            if let Some(assessment) = opt_json_col(row, "assessment_body")? {
                view.pr.assessments.push((
                    item.id,
                    stored_assessment(assessment, row.try_get("assessment_fingerprint")?),
                ));
            }
            view.pr.items.push(item);
            Ok(())
        })?;
        use super::decisions::{DECISION_COLUMNS, ORIGIN_JOIN, decision_from_row, effect_from_row};
        let decisions = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {DECISION_COLUMNS}, i.pull_request_id AS tally_pr
             FROM comment_decisions d JOIN comment_items i ON i.id = d.item_id
             JOIN users u ON u.id = d.user_id {ORIGIN_JOIN}
             WHERE i.pull_request_id IN (SELECT value FROM json_each(?))
             ORDER BY d.decided_at, d.rowid",
        )))
        .bind(&ids)
        .fetch_all(&mut *tx)
        .await?;
        let mut by_decision = HashMap::new();
        apply_tally_rows(&mut views, &decisions, |view, row| {
            let decision = decision_from_row(row)?;
            by_decision.insert(decision.decision.id.to_string(), view.pr.decisions.len());
            view.pr.decisions.push(decision);
            Ok(())
        })?;
        // Only follow-up effects affect queue membership.
        let effects = sqlx::query(
            "SELECT i.pull_request_id AS tally_pr, e.decision_id,
                 e.id, e.seq, e.after_seq, e.kind, e.executor, e.params, e.state,
                 NULL AS claim_token_name, e.lease_until, e.seen_on_github_at, e.updated_at
             FROM comment_effects e JOIN comment_decisions d ON d.id = e.decision_id
             JOIN comment_items i ON i.id = d.item_id
             WHERE i.pull_request_id IN (SELECT value FROM json_each(?))
               AND d.kind = 'merge_with_follow_ups' AND e.kind = 'follow_up_pr'
             ORDER BY e.seq",
        )
        .bind(ids)
        .fetch_all(&mut *tx)
        .await?;
        apply_tally_rows(&mut views, &effects, |view, row| {
            let id: &str = row.try_get("decision_id")?;
            if let Some(&index) = by_decision.get(id) {
                view.pr.decisions[index]
                    .decision
                    .effects
                    .push(effect_from_row(row)?);
            }
            Ok(())
        })?;
        tx.commit().await?;
        Ok(views)
    }

    /// Passes charged to a user since `since` (the daily cap) and the end of the PR's latest pass (spacing).
    pub async fn pass_usage(
        &self,
        user_id: &str,
        since: Timestamp,
        pull_request_id: i64,
    ) -> Result<(u32, Option<Timestamp>), DbError> {
        let mut tx = read_tx(self.pool).await?;
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM comment_passes WHERE user_id = ? AND created_at >= ?",
        )
        .bind(user_id)
        .bind(ts(since))
        .fetch_one(&mut *tx)
        .await?;
        let last: Option<String> = sqlx::query_scalar(
            "SELECT MAX(finished_at) FROM comment_passes WHERE pull_request_id = ?",
        )
        .bind(pull_request_id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok((
            u32::try_from(count).map_err(|_| DbError::Corrupt { column: "count" })?,
            last.map(|text| super::super::jobs::parse_ts(&text, "finished_at"))
                .transpose()?,
        ))
    }
}

/// [`ReviewLoop::upsert_item`] inside `tx`.
pub(crate) async fn upsert_item_in(
    tx: &mut Transaction<'static, Sqlite>,
    tenant_id: &str,
    pull_request_id: i64,
    observed: &ObservedItem,
    state_if_new: ItemState,
    now: Timestamp,
) -> Result<UpsertOutcome, DbError> {
    let existing = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {ITEM_COLUMNS}, {ACTIVE_MARK_COLUMN} FROM comment_items WHERE pull_request_id = ? AND stable_key = ?"
    )))
    .bind(pull_request_id)
    .bind(&observed.stable_key)
    .fetch_optional(&mut **tx)
    .await?
    .map(|row| item_from_row(&row))
    .transpose()?;
    let (path, start, end) = lines_of(observed.location.as_ref());
    let outcome = match existing {
        None => {
            let no: i64 = sqlx::query_scalar(
                "UPDATE comment_prs SET next_item_no = next_item_no + 1 WHERE pull_request_id = ?
                 RETURNING next_item_no - 1",
            )
            .bind(pull_request_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(DbError::NotFound)?;
            let id = CommentItemId::new_random();
            let state = if observed.outside.is_some() {
                ItemState::Outside
            } else {
                state_if_new
            };
            sqlx::query(
                "INSERT INTO comment_items (id, tenant_id, pull_request_id, item_no, kind, stable_key, author,
                     author_kind, url, path, start_line, end_line, title, body, body_hash, first_seen_head,
                     first_round, bot_severity, github, state, outside, created_at, updated_at, status_comment)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(id.to_string())
            .bind(tenant_id)
            .bind(pull_request_id)
            .bind(no)
            .bind(wire(observed.kind)?)
            .bind(&observed.stable_key)
            .bind(&observed.author)
            .bind(wire(observed.author_kind)?)
            .bind(&observed.url)
            .bind(path)
            .bind(start)
            .bind(end)
            .bind(&observed.title)
            .bind(&observed.body)
            .bind(&observed.body_hash)
            .bind(&observed.head.0)
            .bind(observed.round)
            .bind(&observed.bot_severity)
            .bind(json(&observed.github)?)
            .bind(wire(state)?)
            .bind(observed.outside.as_ref().map(json).transpose()?)
            .bind(ts(now))
            .bind(ts(now))
            .bind(observed.status_comment)
            .execute(&mut **tx)
            .await?;
            UpsertOutcome::New(item_in(tx, id).await?)
        }
        Some(item) => {
            let state = match (&observed.outside, item.state) {
                (Some(_), _) => ItemState::Outside,
                (None, ItemState::Outside) => state_if_new,
                (None, state) => state,
            };
            let changed = item.kind != observed.kind
                || item.author != observed.author
                || item.author_kind != observed.author_kind
                || item.status_comment != observed.status_comment
                || item.url != observed.url
                || item.location != observed.location
                || item.title != observed.title
                || item.body != observed.body
                || item.body_hash != observed.body_hash
                || item.bot_severity != observed.bot_severity
                || item.github != observed.github
                || item.outside != observed.outside
                || item.state != state
                || item.gone_at.is_some();
            if changed {
                sqlx::query(
                    "UPDATE comment_items SET kind = ?, author = ?, author_kind = ?, url = ?, path = ?,
                         start_line = ?, end_line = ?, title = ?, body = ?, body_hash = ?, bot_severity = ?,
                         github = ?, state = ?, outside = ?, gone_at = NULL, revision = revision + 1,
                         updated_at = ?, status_comment = ?
                     WHERE id = ?",
                )
                .bind(wire(observed.kind)?)
                .bind(&observed.author)
                .bind(wire(observed.author_kind)?)
                .bind(&observed.url)
                .bind(path)
                .bind(start)
                .bind(end)
                .bind(&observed.title)
                .bind(&observed.body)
                .bind(&observed.body_hash)
                .bind(&observed.bot_severity)
                .bind(json(&observed.github)?)
                .bind(wire(state)?)
                .bind(observed.outside.as_ref().map(json).transpose()?)
                .bind(ts(now))
                .bind(observed.status_comment)
                .bind(item.id.to_string())
                .execute(&mut **tx)
                .await?;
                UpsertOutcome::Changed(item_in(tx, item.id).await?)
            } else {
                UpsertOutcome::Unchanged(item)
            }
        }
    };
    Ok(outcome)
}

/// The comment counts and watch of one pull request of a [`ReviewLoop::comment_tallies`] list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrCommentTally {
    /// Position in the requested list.
    pub index: usize,
    /// The stored pull request.
    pub pull_request_id: i64,
    /// Comment items; `None` before the first successful read.
    pub total: Option<u32>,
    /// GitHub imports made since the reader last entered Comments.
    pub imported_unseen: u32,
    /// The user watches the pull request.
    pub watched: bool,
}

/// The review-loop state of one pull request as one transaction reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct PrView {
    /// Its review-loop row; `None` before its first read.
    pub row: Option<CommentPrRecord>,
    /// Its items, by number.
    pub items: Vec<ItemRecord>,
    /// The latest assessment of each assessed item.
    pub assessments: Vec<(CommentItemId, StoredAssessment)>,
    /// Its decisions, oldest first.
    pub decisions: Vec<super::StoredDecision>,
}

/// A dashboard PR's queue inputs, without its GitHub snapshot or effect reports.
pub struct PrTallyView {
    /// Items, their latest assessments and the decisions needed for queue membership.
    pub pr: PrView,
    /// The latest assessment pass.
    pub latest_pass: Option<PassRecord>,
}

fn apply_tally_rows(
    views: &mut HashMap<i64, Result<PrTallyView, DbError>>,
    rows: &[SqliteRow],
    mut read: impl FnMut(&mut PrTallyView, &SqliteRow) -> Result<(), DbError>,
) -> Result<(), DbError> {
    for row in rows {
        let id: i64 = row.try_get("tally_pr")?;
        let Some(entry) = views.get_mut(&id) else {
            continue;
        };
        if let Ok(view) = entry
            && let Err(error) = read(view, row)
        {
            *entry = Err(error);
        }
    }
    Ok(())
}

/// The [`PrView`] of a pull request inside `tx`.
pub(super) async fn pr_view_in(
    tx: &mut Transaction<'static, Sqlite>,
    pull_request_id: i64,
) -> Result<PrView, DbError> {
    let row = pr_state_with(&mut **tx, pull_request_id).await?;
    let items = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {ITEM_COLUMNS}, {ACTIVE_MARK_COLUMN} FROM comment_items WHERE pull_request_id = ? ORDER BY item_no"
    )))
    .bind(pull_request_id)
    .fetch_all(&mut **tx)
    .await?
    .iter()
    .map(item_from_row)
    .collect::<Result<Vec<_>, _>>()?;
    let assessments = sqlx::query(
        "SELECT a.item_id, a.body, a.fingerprint FROM comment_assessments a
         WHERE a.id IN (SELECT MAX(l.id) FROM comment_assessments l JOIN comment_items i ON i.id = l.item_id
                        WHERE i.pull_request_id = ? GROUP BY l.item_id)",
    )
    .bind(pull_request_id)
    .fetch_all(&mut **tx)
    .await?
    .iter()
    .map(|row| {
        Ok((
            wire_col(row, "item_id")?,
            stored_assessment(json_col(row, "body")?, row.try_get("fingerprint")?),
        ))
    })
    .collect::<Result<Vec<_>, DbError>>()?;
    let decisions =
        super::decisions::decisions_where(tx, "i.pull_request_id = ?", pull_request_id.to_string())
            .await?;
    Ok(PrView {
        row,
        items,
        assessments,
        decisions,
    })
}

/// The review-loop row of a pull request.
async fn pr_state_with<'e>(
    executor: impl sqlx::Executor<'e, Database = Sqlite>,
    pull_request_id: i64,
) -> Result<Option<CommentPrRecord>, DbError> {
    let Some(row) = sqlx::query(
        "SELECT pull_request_id, read_at, read_failure, read_failed_at, github_snapshot, round_count, purpose
         FROM comment_prs WHERE pull_request_id = ?",
    )
    .bind(pull_request_id)
    .fetch_optional(executor)
    .await?
    else {
        return Ok(None);
    };
    Ok(Some(pr_state_from_row(&row)?))
}

fn pr_state_from_row(row: &SqliteRow) -> Result<CommentPrRecord, DbError> {
    let failure: Option<ReadFailure> = opt_wire_col(row, "read_failure")?;
    let failed_at = opt_ts_col(row, "read_failed_at")?;
    Ok(CommentPrRecord {
        pull_request_id: row.try_get("pull_request_id")?,
        read_at: opt_ts_col(row, "read_at")?,
        read_failure: failure.zip(failed_at),
        github_snapshot: opt_json_col(row, "github_snapshot")?,
        round_count: u32_col(row, "round_count")?,
        purpose: row.try_get("purpose")?,
    })
}

/// Store a validated primary assessment under the caller's acceptance transaction.
pub(crate) async fn add_assessment_in(
    tx: &mut Transaction<'static, Sqlite>,
    tenant_id: &str,
    item: CommentItemId,
    assessment: &CommentAssessment,
    fingerprint: &str,
    description: Option<&str>,
) -> Result<bool, DbError> {
    let excluded: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM comment_items i WHERE i.id = ? AND (
            i.status_comment OR EXISTS (
                SELECT 1 FROM comment_item_marks m WHERE m.item_id = i.id AND m.undone_at IS NULL
            )
        ))",
    )
    .bind(item.to_string())
    .fetch_one(&mut **tx)
    .await?;
    if excluded {
        return Ok(false);
    }
    let latest = sqlx::query(
        "SELECT body, fingerprint FROM comment_assessments WHERE item_id = ? ORDER BY id DESC LIMIT 1",
    )
    .bind(item.to_string())
    .fetch_optional(&mut **tx)
    .await?
    .map(|row| -> Result<StoredAssessment, DbError> {
        Ok(stored_assessment(json_col(&row, "body")?, row.try_get("fingerprint")?))
    })
    .transpose()?;
    let description = description.or_else(|| {
        latest
            .as_ref()
            .filter(|prior| {
                prior.assessment.head == assessment.head && prior.fingerprint == fingerprint
            })
            .and_then(|prior| prior.description.as_deref())
    });
    sqlx::query(
            "INSERT INTO comment_assessments (tenant_id, item_id, job_id, head, fingerprint, body, prompt_version,
                 assessed_at,pass_id,method,payer_id)
             SELECT ?, ?, ?, ?, ?, ?, ?, ?,j.assessment_pass_id,j.assessment_method,
                 CASE WHEN j.assessment_pass_id IS NOT NULL THEN j.payer_id END
             FROM (SELECT 1) LEFT JOIN jobs j ON j.id=? AND ?=0",
        )
        .bind(tenant_id)
        .bind(item.to_string())
        .bind(assessment.job.to_string())
        .bind(&assessment.head.0)
        .bind(fingerprint)
        .bind(json(&AssessmentBody { assessment, description: description.map(str::to_owned) })?)
        .bind(assessment.prompt_version)
        .bind(ts(assessment.assessed_at))
        .bind(assessment.job.to_string())
        .bind(assessment.reused_since.is_some())
        .execute(&mut **tx)
        .await?;
    let shown_before = latest.is_some_and(|latest| {
        latest.assessment.recommendation == assessment.recommendation
            && latest.assessment.gates == assessment.gates
    });
    if !shown_before {
        sqlx::query(
            "UPDATE comment_items SET revision = revision + 1, updated_at = ? WHERE id = ?",
        )
        .bind(ts(assessment.assessed_at))
        .bind(item.to_string())
        .execute(&mut **tx)
        .await?;
    }
    if !assessment.gates.is_empty() {
        super::origins::reopen_follower(tx, item, assessment.assessed_at).await?;
    }
    Ok(true)
}
