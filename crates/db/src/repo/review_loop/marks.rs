//! Internal comment dispositions and their audit history.

use contracts::api::review_loop::{
    Decision, DecisionKind, ItemMark, MarkLogEntry, MarkedItem, MarkedItems,
};
use contracts::common::{PrRef, RepoRef};
use contracts::ids::{CommentItemId, DecisionId};
use contracts::time::Timestamp;
use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, Sqlite, Transaction};

use super::super::jobs::{parse_ts, ts};
use super::items::{ItemRecord, SentItem, set_state_in};
use super::{NewDecision, NewOrigin, ReviewLoop, opt_ts_col, ts_col, u32_col, wire_col};
use crate::error::DbError;

/// Active mark and the user ID needed to attribute marks on later duplicates.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveMark {
    /// Server user who marked the primary.
    pub user_id: String,
    /// Facts exposed to the reader.
    pub mark: ItemMark,
}

/// Indexed projection included in every item read.
pub(super) const ACTIVE_MARK_COLUMN: &str = "(SELECT json_object(
    'user_id', m.user_id, 'mark', json_object('by', u.login, 'at', m.marked_at, 'follows', pi.item_no))
    FROM comment_item_marks m
    JOIN users u ON u.id = m.user_id
    LEFT JOIN comment_item_marks p ON p.id = m.follows_mark_id
    LEFT JOIN comment_items pi ON pi.id = p.item_id
    WHERE m.item_id = comment_items.id AND m.undone_at IS NULL) AS active_mark";

/// Checks a frozen pass's item IDs under its admission write lock.
pub(in crate::repo) async fn active_in_pass(
    tx: &mut Transaction<'static, Sqlite>,
    tenant: &str,
    sent: &[SentItem],
) -> Result<bool, DbError> {
    let ids: Vec<_> = sent.iter().map(|sent| sent.item).collect();
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM json_each(?) sent
            JOIN comment_item_marks m ON m.item_id = sent.value
            WHERE m.tenant_id = ? AND m.undone_at IS NULL
        )",
    )
    .bind(super::json(&ids)?)
    .bind(tenant)
    .fetch_one(&mut **tx)
    .await?)
}

pub(super) struct MarkChange {
    pub(super) pr: i64,
    pub(super) no: u32,
    pub(super) user: String,
    pub(super) undone: bool,
}

pub(super) fn log_changes(changes: Vec<MarkChange>) {
    for MarkChange {
        pr,
        no,
        user,
        undone,
    } in changes
    {
        if undone {
            tracing::info!(pr, item = no, %user, "not-a-review-point mark undone");
        } else {
            tracing::info!(pr, item = no, %user, "item marked as not a review point");
        }
    }
}

pub(super) async fn store_mark(
    tx: &mut Transaction<'static, Sqlite>,
    tenant: &str,
    item: &ItemRecord,
    new: &NewDecision,
    now: Timestamp,
    changes: &mut Vec<MarkChange>,
) -> Result<Option<Decision>, DbError> {
    let (primary, follows) = match &new.origin {
        Some(NewOrigin::Follows(primary)) => {
            let row = sqlx::query(
                "SELECT m.id, i.item_no FROM comment_item_marks m JOIN comment_items i ON i.id = m.item_id
                 WHERE m.item_id = ? AND m.tenant_id = ? AND i.pull_request_id = ?
                   AND m.undone_at IS NULL AND m.follows_mark_id IS NULL",
            )
            .bind(primary.to_string())
            .bind(tenant)
            .bind(item.pull_request_id)
            .fetch_one(&mut **tx)
            .await?;
            (
                Some(row.try_get::<String, _>("id")?),
                Some(u32_col(&row, "item_no")?),
            )
        }
        None => (None, None),
        Some(NewOrigin::Imported(_)) => return Err(DbError::Conflict),
    };
    let id = DecisionId::new_random();
    let saved = sqlx::query(
        "INSERT INTO comment_item_marks (id, tenant_id, item_id, user_id, mark, reason, follows_mark_id, marked_at)
         SELECT ?, i.tenant_id, i.id, ?, 'not_a_review_point', ?, ?, ?
         FROM comment_items i JOIN memberships member ON member.tenant_id = i.tenant_id AND member.user_id = ?
         WHERE i.id = ? AND i.tenant_id = ?
         AND NOT EXISTS (SELECT 1 FROM comment_item_marks old WHERE old.item_id = i.id AND old.follows_mark_id = ?)",
    )
    .bind(id.to_string())
    .bind(&new.user_id)
    .bind(&new.reason)
    .bind(&primary)
    .bind(ts(now))
    .bind(&new.user_id)
    .bind(item.id.to_string())
    .bind(tenant)
    .bind(&primary)
    .execute(&mut **tx)
    .await?;
    if saved.rows_affected() == 0 {
        // A manually undone follower stays undone for this primary mark.
        return if primary.is_some() {
            Ok(None)
        } else {
            Err(DbError::Conflict)
        };
    }
    set_state_in(tx, item.id, item.state, now).await?;
    let login = sqlx::query_scalar("SELECT login FROM users WHERE id = ?")
        .bind(&new.user_id)
        .fetch_one(&mut **tx)
        .await?;
    changes.push(MarkChange {
        pr: item.pull_request_id,
        no: item.no,
        user: new.user_id.clone(),
        undone: false,
    });
    Ok(Some(Decision {
        id,
        item_no: item.no,
        kind: DecisionKind::NotAReviewPoint,
        reason: new.reason.clone(),
        target: None,
        r#override: None,
        resolve_ticked: false,
        reply_text: None,
        comment: None,
        recommendation: None,
        via: new.via,
        carried_from: None,
        supersedes: None,
        head: new.head.clone(),
        decided_by: login,
        decided_at: parse_ts(&ts(now), "marked_at")?,
        undone_at: None,
        effects: Vec::new(),
        imported: None,
        follows,
    }))
}

/// Ends a standing mark and any followers of that exact mark in the caller's transaction.
pub(super) async fn end_marks(
    tx: &mut Transaction<'static, Sqlite>,
    tenant: &str,
    item: CommentItemId,
    user: &str,
    now: Timestamp,
) -> Result<Vec<MarkChange>, DbError> {
    let Some(id) = sqlx::query_scalar::<_, String>(
        "SELECT id FROM comment_item_marks WHERE tenant_id = ? AND item_id = ? AND undone_at IS NULL",
    )
    .bind(tenant)
    .bind(item.to_string())
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Ok(Vec::new());
    };
    let ended: Vec<String> = sqlx::query_scalar(
        "UPDATE comment_item_marks SET undone_at = ?, undone_by = ?
         WHERE tenant_id = ? AND undone_at IS NULL AND (id = ? OR follows_mark_id = ?)
         RETURNING item_id",
    )
    .bind(ts(now))
    .bind(user)
    .bind(tenant)
    .bind(&id)
    .bind(&id)
    .fetch_all(&mut **tx)
    .await?;
    let rows = sqlx::query(
        "UPDATE comment_items SET revision = revision + 1, updated_at = ?
         WHERE id IN (SELECT value FROM json_each(?)) RETURNING pull_request_id, item_no",
    )
    .bind(ts(now))
    .bind(super::json(&ended)?)
    .fetch_all(&mut **tx)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(MarkChange {
                pr: row.try_get("pull_request_id")?,
                no: u32_col(row, "item_no")?,
                user: user.to_owned(),
                undone: true,
            })
        })
        .collect()
}

fn mark_from_row(row: &SqliteRow) -> Result<ItemMark, DbError> {
    Ok(ItemMark {
        by: row.try_get("login")?,
        at: ts_col(row, "marked_at")?,
        follows: row.try_get("follows_no")?,
    })
}

impl ReviewLoop<'_> {
    /// A pull request's marks and undo history, oldest first.
    pub async fn mark_log(&self, tenant: &str, pr: i64) -> Result<Vec<MarkLogEntry>, DbError> {
        sqlx::query(
            "SELECT m.id, i.item_no, u.login, m.marked_at, pi.item_no AS follows_no,
                    m.reason, m.undone_at, undo.login AS undone_by
             FROM comment_item_marks m
             JOIN comment_items i ON i.id = m.item_id
             JOIN users u ON u.id = m.user_id
             LEFT JOIN users undo ON undo.id = m.undone_by
             LEFT JOIN comment_item_marks p ON p.id = m.follows_mark_id
             LEFT JOIN comment_items pi ON pi.id = p.item_id
             WHERE m.tenant_id = ? AND i.pull_request_id = ? ORDER BY m.marked_at, m.id",
        )
        .bind(tenant)
        .bind(pr)
        .fetch_all(self.pool)
        .await?
        .iter()
        .map(|row| {
            Ok(MarkLogEntry {
                id: wire_col(row, "id")?,
                item_no: u32_col(row, "item_no")?,
                mark: mark_from_row(row)?,
                reason: row.try_get("reason")?,
                undone_at: opt_ts_col(row, "undone_at")?,
                undone_by: row.try_get("undone_by")?,
            })
        })
        .collect()
    }

    /// One page of active marks in a tenant, newest first.
    pub async fn marked_items(
        &self,
        tenant: &str,
        before: Option<(Timestamp, DecisionId)>,
    ) -> Result<MarkedItems, DbError> {
        const PAGE: usize = 100;
        let rows = sqlx::query(
            "SELECT m.id, m.marked_at, u.login, i.item_no, i.kind, i.author, i.author_kind,
                    i.title, i.url, i.status_comment, r.owner, r.name, p.number
             FROM comment_item_marks m
             JOIN comment_items i ON i.id = m.item_id AND i.tenant_id = m.tenant_id
             JOIN users u ON u.id = m.user_id
             JOIN pull_requests p ON p.id = i.pull_request_id AND p.tenant_id = m.tenant_id
             JOIN repos r ON r.id = p.repo_id AND r.tenant_id = m.tenant_id
             WHERE m.tenant_id = ? AND m.undone_at IS NULL
               AND (? IS NULL OR (m.marked_at, m.id) < (?, ?))
             ORDER BY m.marked_at DESC, m.id DESC LIMIT ?",
        )
        .bind(tenant)
        .bind(before.as_ref().map(|(at, _)| ts(*at)))
        .bind(before.as_ref().map(|(at, _)| ts(*at)))
        .bind(before.map(|(_, id)| id.to_string()))
        .bind((PAGE + 1) as i64)
        .fetch_all(self.pool)
        .await?;
        let items = rows
            .iter()
            .take(PAGE)
            .map(|row| {
                Ok(MarkedItem {
                    pr: PrRef {
                        repo: RepoRef {
                            owner: row.try_get("owner")?,
                            name: row.try_get("name")?,
                        },
                        number: u32_col(row, "number")?,
                    },
                    no: u32_col(row, "item_no")?,
                    kind: wire_col(row, "kind")?,
                    author: row.try_get("author")?,
                    author_kind: wire_col(row, "author_kind")?,
                    title: row.try_get("title")?,
                    url: row.try_get("url")?,
                    by: row.try_get("login")?,
                    at: ts_col(row, "marked_at")?,
                    status_comment: row.try_get("status_comment")?,
                })
            })
            .collect::<Result<Vec<_>, DbError>>()?;
        let next_cursor = if rows.len() > PAGE {
            let last = &rows[PAGE - 1];
            let id: DecisionId = wire_col(last, "id")?;
            Some(format!("{}/{}", ts_col(last, "marked_at")?, id))
        } else {
            None
        };
        Ok(MarkedItems { items, next_cursor })
    }
}
