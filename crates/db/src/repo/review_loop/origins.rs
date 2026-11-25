//! Provenance of imported decisions and decisions following a primary.

use contracts::api::review_loop::{DecisionKind, DecisionTarget, ImportedDecision, ItemState};
use contracts::ids::{CommentItemId, DecisionId};
use contracts::time::Timestamp;
use serde::{Deserialize, Serialize};
use sqlx::{Row, Sqlite, Transaction};

use super::super::jobs::ts;
use super::super::write_tx;
use super::items::{item_in, set_state_in};
use super::{ReviewLoop, wire_col};
use crate::error::DbError;

/// An assessment claim authenticated by the host against the frozen GitHub read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportCandidate {
    /// Grounded GitHub evidence.
    pub evidence: ImportedDecision,
    /// Member owning the statement, or the pass payer for an external author's withdrawal.
    pub user_id: String,
    /// Recorded disposition.
    pub kind: DecisionKind,
    /// Target, when present.
    pub target: Option<DecisionTarget>,
    /// Whether a deterministic cue elevated the statement above a proposal.
    pub binding: bool,
    /// Only the author's point is affected.
    pub author_withdrawal: bool,
    /// Fingerprint of the GitHub source and evidence at assessment time.
    pub version: String,
}

/// An automatic decision's provenance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NewOrigin {
    /// A primary item whose current decision is stored before the follower.
    Follows(CommentItemId),
    /// A binding statement from GitHub.
    Imported(Box<ImportCandidate>),
}

pub(super) async fn store(
    tx: &mut Transaction<'static, Sqlite>,
    decision: DecisionId,
    origin: &NewOrigin,
    new: &super::NewDecision,
) -> Result<(), DbError> {
    match origin {
        NewOrigin::Follows(primary) => {
            let primary = super::actions::standing(tx, *primary)
                .await?
                .ok_or(DbError::Conflict)?;
            if !new.effects.is_empty() || new.reply_text.is_some() {
                let imported: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM comment_decision_origins
                                   WHERE decision_id = ? AND origin = 'imported_github')",
                )
                .bind(&primary)
                .fetch_one(&mut **tx)
                .await?;
                if imported {
                    return Err(DbError::Conflict);
                }
            }
            sqlx::query(
                "INSERT INTO comment_decision_origins (decision_id, origin, primary_decision_id)
                         VALUES (?, 'follows_primary', ?)",
            )
            .bind(decision.to_string())
            .bind(primary)
            .execute(&mut **tx)
            .await?;
        }
        NewOrigin::Imported(import) => {
            let e = &import.evidence;
            let github_id = e
                .by_id
                .and_then(|id| i64::try_from(id).ok())
                .filter(|id| *id > 0)
                .ok_or(DbError::Conflict)?;
            if !import.binding
                || new.kind != import.kind
                || new.user_id != import.user_id
                || !new.effects.is_empty()
                || new.reply_text.is_some()
            {
                return Err(DbError::Conflict);
            }
            sqlx::query(
                "INSERT INTO comment_decision_origins (decision_id, origin, by_login, by_github_id,
                             relay_login, recorded_at, evidence_url, quote, author_withdrawal)
                         VALUES (?, 'imported_github', ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(decision.to_string())
            .bind(&e.by)
            .bind(github_id)
            .bind(&e.relay)
            .bind(ts(e.at))
            .bind(&e.evidence_url)
            .bind(&e.quote)
            .bind(import.author_withdrawal)
            .execute(&mut **tx)
            .await?;
        }
    }
    Ok(())
}

pub(super) async fn followers(
    tx: &mut Transaction<'static, Sqlite>,
    primary: &str,
) -> Result<Vec<(String, CommentItemId)>, DbError> {
    sqlx::query(
        "SELECT d.id, d.item_id FROM comment_decisions d
         JOIN comment_decision_origins o ON o.decision_id = d.id
         WHERE o.primary_decision_id = ?
           AND NOT EXISTS (SELECT 1 FROM comment_decisions n WHERE n.supersedes = d.id)
           AND NOT EXISTS (SELECT 1 FROM comment_decision_undos u WHERE u.decision_id = d.id)",
    )
    .bind(primary)
    .fetch_all(&mut **tx)
    .await?
    .iter()
    .map(|row| Ok((row.try_get("id")?, wire_col(row, "item_id")?)))
    .collect()
}

pub(super) async fn reopen_follower(
    tx: &mut Transaction<'static, Sqlite>,
    item: CommentItemId,
    now: Timestamp,
) -> Result<(), DbError> {
    let current = item_in(tx, item).await?;
    if !matches!(
        current.state,
        ItemState::Decided | ItemState::Executing | ItemState::Done | ItemState::Repeat
    ) {
        return Ok(());
    }
    if let Some(decision) = super::actions::standing(tx, item).await? {
        let follows: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM comment_decision_origins
                           WHERE decision_id = ? AND origin = 'follows_primary')",
        )
        .bind(&decision)
        .fetch_one(&mut **tx)
        .await?;
        if follows {
            super::actions::cancel_unclaimed(tx, &decision, now).await?;
            set_state_in(tx, item, ItemState::Reopened, now).await?;
        }
    }
    Ok(())
}

pub(super) async fn undo(
    tx: &mut Transaction<'static, Sqlite>,
    tenant: &str,
    user: &str,
    decision: &str,
    item: CommentItemId,
    now: Timestamp,
) -> Result<(), DbError> {
    super::actions::cancel_unclaimed(tx, decision, now).await?;
    sqlx::query("INSERT INTO comment_decision_undos (decision_id, tenant_id, user_id, at) VALUES (?, ?, ?, ?)")
        .bind(decision)
        .bind(tenant)
        .bind(user)
        .bind(ts(now))
        .execute(&mut **tx)
        .await?;
    let current = item_in(tx, item).await?;
    if !matches!(
        current.state,
        ItemState::Reopened | ItemState::AskedBack | ItemState::Outside
    ) {
        set_state_in(tx, item, ItemState::Undecided, now).await?;
    }
    Ok(())
}

impl ReviewLoop<'_> {
    /// Counts current imports committed since this reader's last Comments overview and advances its cursor.
    pub async fn visit_comments(&self, tenant: &str, user: &str, pr: i64) -> Result<u32, DbError> {
        let mut tx = write_tx(self.pool).await?;
        let now = Timestamp::now();
        let previous = sqlx::query(
            "SELECT visited_at FROM comment_visits WHERE tenant_id = ? AND user_id = ? AND pull_request_id = ?",
        )
        .bind(tenant)
        .bind(user)
        .bind(pr)
        .fetch_optional(&mut *tx)
        .await?
        .map(|row| super::ts_col(&row, "visited_at"))
        .transpose()?;
        let imports = sqlx::query(
            "SELECT d.decided_at FROM comment_decisions d
             JOIN comment_items i ON i.id = d.item_id
             JOIN comment_decision_origins o ON o.decision_id = d.id
             WHERE i.tenant_id = ? AND i.pull_request_id = ? AND o.origin = 'imported_github'
               AND NOT EXISTS (SELECT 1 FROM comment_decisions n WHERE n.supersedes = d.id)
               AND NOT EXISTS (SELECT 1 FROM comment_decision_undos u WHERE u.decision_id = d.id)",
        )
        .bind(tenant)
        .bind(pr)
        .fetch_all(&mut *tx)
        .await?;
        let mut unseen = 0_u32;
        for row in &imports {
            let at = super::ts_col(row, "decided_at")?;
            if previous.is_none_or(|previous| at > previous) {
                unseen = unseen.saturating_add(1);
            }
        }
        sqlx::query(
            "INSERT INTO comment_visits (tenant_id, user_id, pull_request_id, visited_at) VALUES (?, ?, ?, ?)
             ON CONFLICT (tenant_id, user_id, pull_request_id) DO UPDATE SET visited_at = excluded.visited_at",
        )
        .bind(tenant)
        .bind(user)
        .bind(pr)
        .bind(ts(now))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(unseen)
    }
}
