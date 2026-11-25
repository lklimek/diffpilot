//! Provenance of imported decisions and decisions following a primary.

use contracts::api::review_loop::{
    DecisionKind, DecisionTarget, EffectKind, ImportedDecision, ItemState,
};
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
            if new.effects.iter().any(|e| {
                !matches!(
                    e.kind,
                    EffectKind::Reply | EffectKind::PrComment | EffectKind::Resolve
                )
            }) {
                return Err(DbError::Conflict);
            }
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
            if import.evidence.relay.is_some()
                || !import.binding
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

pub(super) async fn follower_artefact(
    tx: &mut Transaction<'static, Sqlite>,
    follower: DecisionId,
    item_no: u32,
) -> Result<Option<String>, DbError> {
    let artefact: Option<String> = sqlx::query_scalar(
        "SELECT e.id FROM comment_decision_origins o JOIN comment_effects e
            ON e.decision_id = o.primary_decision_id
         WHERE o.decision_id = ? AND e.kind IN ('fix', 'follow_up_pr', 'upstream_issue', 'todo')
         ORDER BY e.seq LIMIT 1",
    )
    .bind(follower.to_string())
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(id) = &artefact {
        sqlx::query(
            "UPDATE comment_effects
             SET params = json_set(params, '$.item_nos', json_insert(json_extract(params, '$.item_nos'), '$[#]', ?1),
                     '$.text', 'Review points: ' || (
                         SELECT group_concat('C' || value, ', ') FROM
                         json_each(json_insert(json_extract(params, '$.item_nos'), '$[#]', ?1))))
             WHERE id = ?2 AND state IN ('waiting', 'open')
                 AND NOT EXISTS (SELECT 1 FROM json_each(params, '$.item_nos') WHERE value = ?1)",
        ).bind(item_no).bind(id).execute(&mut **tx).await?;
    }
    Ok(artefact)
}

pub(super) async fn claimed_follower(
    tx: &mut Transaction<'static, Sqlite>,
    primary: &str,
    now: Timestamp,
) -> Result<Option<u32>, DbError> {
    for (decision, item) in followers(tx, primary).await? {
        let claimed: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM comment_effects
                WHERE decision_id = ? AND state = 'claimed' AND lease_until >= ?)",
        )
        .bind(decision)
        .bind(ts(now))
        .fetch_one(&mut **tx)
        .await?;
        if claimed {
            return Ok(Some(item_in(tx, item).await?.no));
        }
    }
    Ok(None)
}

pub(crate) async fn reopen_with_followers(
    tx: &mut Transaction<'static, Sqlite>,
    decision: &str,
    item: CommentItemId,
    now: Timestamp,
) -> Result<(), DbError> {
    let mut affected = followers(tx, decision).await?;
    affected.push((decision.to_owned(), item));
    for (decision, item) in affected {
        super::actions::cancel_unclaimed(tx, &decision, now).await?;
        if item_in(tx, item).await?.state != ItemState::Outside {
            set_state_in(tx, item, ItemState::Reopened, now).await?;
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
    /// Records one Comments entry and keeps the previous entry as the visit's notice baseline.
    pub async fn visit_comments(&self, tenant: &str, user: &str, pr: i64) -> Result<(), DbError> {
        let mut tx = write_tx(self.pool).await?;
        sqlx::query(
            "INSERT INTO comment_visits (tenant_id, user_id, pull_request_id, visited_at) VALUES (?, ?, ?, ?)
             ON CONFLICT (tenant_id, user_id, pull_request_id) DO UPDATE SET
                 previous_visited_at = comment_visits.visited_at, visited_at = excluded.visited_at",
        )
        .bind(tenant).bind(user).bind(pr).bind(ts(Timestamp::now()))
        .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Item ids of imports since the entry preceding this visit, including their decision history.
    pub async fn unseen_comment_imports(
        &self,
        tenant: &str,
        user: &str,
        pr: i64,
    ) -> Result<Vec<CommentItemId>, DbError> {
        sqlx::query(
            "SELECT d.item_id FROM comment_decision_origins o
             JOIN comment_decisions d ON d.id = o.decision_id
             JOIN comment_items i ON i.id = d.item_id
             LEFT JOIN comment_visits v ON v.tenant_id = i.tenant_id
                AND v.pull_request_id = i.pull_request_id AND v.user_id = ?
             WHERE i.tenant_id = ? AND i.pull_request_id = ? AND o.origin = 'imported_github'
                AND (v.previous_visited_at IS NULL
                     OR rtrim(d.decided_at, 'Z') > rtrim(v.previous_visited_at, 'Z'))",
        )
        .bind(user)
        .bind(tenant)
        .bind(pr)
        .fetch_all(self.pool)
        .await?
        .iter()
        .map(|row| wire_col(row, "item_id"))
        .collect()
    }

    /// Accepts a linked account's first observed id, with a settings check in the pin transaction.
    pub async fn pin_linked_account(
        &self,
        tenant: &str,
        user: &str,
        login: &str,
        github_id: u64,
    ) -> Result<bool, DbError> {
        let Ok(github_id) = i64::try_from(github_id) else {
            return Ok(false);
        };
        if github_id <= 0 {
            return Ok(false);
        }
        let mut tx = write_tx(self.pool).await?;
        sqlx::query(
            "INSERT INTO comment_linked_account_pins (tenant_id, user_id, login, github_id)
             SELECT ?1, ?2, lower(?3), ?4 WHERE EXISTS (
                 SELECT 1 FROM dashboard_settings s JOIN memberships m
                     ON m.tenant_id = s.tenant_id AND m.user_id = s.user_id
                 JOIN json_each(s.linked_logins) l
                 WHERE s.tenant_id = ?1 AND s.user_id = ?2 AND lower(l.value) = lower(?3))
             ON CONFLICT DO NOTHING",
        )
        .bind(tenant)
        .bind(user)
        .bind(login)
        .bind(github_id)
        .execute(&mut *tx)
        .await?;
        let pinned: Option<i64> = sqlx::query_scalar(
            "SELECT github_id FROM comment_linked_account_pins WHERE tenant_id = ? AND user_id = ? AND login = ?",
        )
        .bind(tenant).bind(user).bind(login).fetch_optional(&mut *tx).await?;
        tx.commit().await?;
        Ok(pinned == Some(github_id))
    }
}
