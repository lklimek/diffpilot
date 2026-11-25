//! Owner actions on items besides deciding (plan §3.2, migration 0019): Undo, Reopen and "Skip for now"; and the
//! lookups the server's queue and views need (the reader's skips, a decision's user).

use contracts::api::review_loop::ItemState;
use contracts::ids::{CommentItemId, DecisionId};
use contracts::time::Timestamp;

use super::super::jobs::ts;
use super::super::write_tx;
use super::items::{item_in, set_state_in, upsert_item_in};
use super::{ItemRecord, ObservedItem, ReviewLoop, UpsertOutcome, u32_col, wire_col};
use crate::error::DbError;

/// Result of [`ReviewLoop::undo`] and [`ReviewLoop::reopen`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionOutcome {
    /// Done; the item as it is now.
    Done(Box<ItemRecord>),
    /// The item changed since the revision named.
    ItemChanged,
    /// The item has no standing decision to act on.
    NoDecision,
    /// An effect of the decision is claimed or reported already (Undo only).
    EffectClaimed,
}

/// The standing (not superseded, not undone) decision of an item.
async fn standing(
    tx: &mut sqlx::Transaction<'static, sqlx::Sqlite>,
    item: CommentItemId,
) -> Result<Option<String>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT d.id FROM comment_decisions d WHERE d.item_id = ?
           AND NOT EXISTS (SELECT 1 FROM comment_decisions n WHERE n.supersedes = d.id)
           AND NOT EXISTS (SELECT 1 FROM comment_decision_undos u WHERE u.decision_id = d.id)
         ORDER BY d.decided_at DESC, d.rowid DESC LIMIT 1",
    )
    .bind(item.to_string())
    .fetch_optional(&mut **tx)
    .await?)
}

/// Cancels the effects of `decision` that no agent holds.
pub(crate) async fn cancel_unclaimed(
    tx: &mut sqlx::Transaction<'static, sqlx::Sqlite>,
    decision: &str,
    now: Timestamp,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE comment_effects SET state = 'cancelled', updated_at = ?
         WHERE decision_id = ? AND state IN ('waiting', 'open')",
    )
    .bind(ts(now))
    .bind(decision)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

impl ReviewLoop<'_> {
    /// Undoes the item's standing decision while none of its effects is claimed or reported: its effects are
    /// cancelled, the undo is recorded beside the append-only decision and the item is undecided again.
    pub async fn undo(
        &self,
        tenant_id: &str,
        item: CommentItemId,
        revision: u32,
        user_id: &str,
        now: Timestamp,
    ) -> Result<ActionOutcome, DbError> {
        let mut tx = write_tx(self.pool).await?;
        if item_in(&mut tx, item).await?.revision != revision {
            return Ok(ActionOutcome::ItemChanged);
        }
        let Some(decision) = standing(&mut tx, item).await? else {
            return Ok(ActionOutcome::NoDecision);
        };
        let touched: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM comment_effects WHERE decision_id = ?
                 AND state IN ('claimed', 'done', 'failed'))",
        )
        .bind(&decision)
        .fetch_one(&mut *tx)
        .await?;
        if touched {
            return Ok(ActionOutcome::EffectClaimed);
        }
        cancel_unclaimed(&mut tx, &decision, now).await?;
        sqlx::query(
            "INSERT INTO comment_decision_undos (decision_id, tenant_id, user_id, at) VALUES (?, ?, ?, ?)",
        )
        .bind(&decision)
        .bind(tenant_id)
        .bind(user_id)
        .bind(ts(now))
        .execute(&mut *tx)
        .await?;
        set_state_in(&mut tx, item, ItemState::Undecided, now).await?;
        let record = item_in(&mut tx, item).await?;
        tx.commit().await?;
        Ok(ActionOutcome::Done(Box::new(record)))
    }

    /// Asks a decided (or carried) item again: effects of its standing decision not yet claimed are cancelled
    /// and the item becomes `reopened`; the next decision supersedes the standing one.
    pub async fn reopen(
        &self,
        item: CommentItemId,
        revision: u32,
        now: Timestamp,
    ) -> Result<ActionOutcome, DbError> {
        let mut tx = write_tx(self.pool).await?;
        let record = item_in(&mut tx, item).await?;
        if record.revision != revision {
            return Ok(ActionOutcome::ItemChanged);
        }
        let Some(decision) = standing(&mut tx, item).await? else {
            return Ok(ActionOutcome::NoDecision);
        };
        if record.state == ItemState::Outside {
            return Ok(ActionOutcome::NoDecision);
        }
        cancel_unclaimed(&mut tx, &decision, now).await?;
        set_state_in(&mut tx, item, ItemState::Reopened, now).await?;
        let record = item_in(&mut tx, item).await?;
        tx.commit().await?;
        Ok(ActionOutcome::Done(Box::new(record)))
    }

    /// [`Self::upsert_item`] of a decided item raised again outside the carry-over rules, asked again in the same
    /// transaction as [`Self::reopen`] asks it: the read's observation, the question to the owner and the
    /// cancellation of the effects no agent holds commit together, so a failure leaves the earlier observation for
    /// the next read to find raised again.
    pub async fn upsert_asking_again(
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
        let (UpsertOutcome::New(record)
        | UpsertOutcome::Changed(record)
        | UpsertOutcome::Unchanged(record)) = &outcome;
        let (id, outside) = (record.id, record.state == ItemState::Outside);
        let decision = if outside {
            None
        } else {
            standing(&mut tx, id).await?
        };
        let Some(decision) = decision else {
            tx.commit().await?;
            return Ok(outcome);
        };
        cancel_unclaimed(&mut tx, &decision, now).await?;
        set_state_in(&mut tx, id, ItemState::Reopened, now).await?;
        let record = item_in(&mut tx, id).await?;
        tx.commit().await?;
        Ok(UpsertOutcome::Changed(record))
    }

    /// An answer to the owner's question about an undecided item landed: it becomes `asked_back` (queue rank 3)
    /// until decided. Other states stay; `true` when it changed.
    pub async fn asked_back(&self, item: CommentItemId, now: Timestamp) -> Result<bool, DbError> {
        let updated = sqlx::query(
            "UPDATE comment_items SET state = 'asked_back', revision = revision + 1, updated_at = ?
             WHERE id = ? AND state = 'undecided' AND NOT status_comment",
        )
        .bind(ts(now))
        .bind(item.to_string())
        .execute(self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    /// Records the reader's "Skip for now" of an item at `revision` (replacing an earlier skip).
    pub async fn skip(
        &self,
        tenant_id: &str,
        user_id: &str,
        item: CommentItemId,
        revision: u32,
        now: Timestamp,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO comment_item_skips (tenant_id, user_id, item_id, revision, at) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (user_id, item_id) DO UPDATE SET revision = excluded.revision, at = excluded.at",
        )
        .bind(tenant_id)
        .bind(user_id)
        .bind(item.to_string())
        .bind(revision)
        .bind(ts(now))
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// The reader's skips of the PR's items: item and the revision it was skipped at.
    pub async fn skips(
        &self,
        user_id: &str,
        pull_request_id: i64,
    ) -> Result<Vec<(CommentItemId, u32)>, DbError> {
        sqlx::query(
            "SELECT s.item_id, s.revision FROM comment_item_skips s JOIN comment_items i ON i.id = s.item_id
             WHERE s.user_id = ? AND i.pull_request_id = ?",
        )
        .bind(user_id)
        .bind(pull_request_id)
        .fetch_all(self.pool)
        .await?
        .iter()
        .map(|row| Ok((wire_col(row, "item_id")?, u32_col(row, "revision")?)))
        .collect()
    }

    /// The user (`users.id`) who made a decision.
    pub async fn decision_user(&self, decision: DecisionId) -> Result<String, DbError> {
        sqlx::query_scalar("SELECT user_id FROM comment_decisions WHERE id = ?")
            .bind(decision.to_string())
            .fetch_optional(self.pool)
            .await?
            .ok_or(DbError::NotFound)
    }
}
