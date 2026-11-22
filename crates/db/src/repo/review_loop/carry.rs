//! The carry-over log (plan §2.5, RQ3): decisions carried to repeats and repeats asked again, with evidence.

use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use contracts::api::review_loop::{
    CarryCounts, CarryEvent, CarryEvidence, CarryKind, ItemState, ReaskReason,
};
use contracts::common::{PrRef, RepoRef};
use contracts::ids::{CarryEventId, CommentItemId, DecisionId};
use contracts::time::Timestamp;

use super::super::jobs::ts;
use super::super::{read_tx, write_tx};
use super::items::{item_in, set_state_in};
use super::{ReviewLoop, json, json_col, opt_wire_col, ts_col, u32_col, wire, wire_col};
use crate::error::DbError;

/// A carry-over event to record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewCarryEvent {
    /// The repeat.
    pub item: CommentItemId,
    /// The decision carried or re-asked.
    pub source_decision: DecisionId,
    /// Kind.
    pub kind: CarryKind,
    /// Why asked again (`asked_again` only).
    pub reason: Option<ReaskReason>,
    /// Evidence.
    pub evidence: CarryEvidence,
}

const EVENT_SELECT: &str =
    "SELECT c.id, r.owner, r.name, p.number, i.item_no, s.item_no AS source_item_no,
        c.source_decision_id, d.kind AS source_kind, c.kind, c.reason, c.evidence, c.at,
        (SELECT COUNT(*) FROM comment_effects e WHERE e.decision_id = c.source_decision_id
           AND e.state IN ('waiting', 'open', 'claimed')) AS effects_pending
    FROM comment_carry_events c
    JOIN comment_items i ON i.id = c.item_id
    JOIN comment_decisions d ON d.id = c.source_decision_id
    JOIN comment_items s ON s.id = d.item_id
    JOIN pull_requests p ON p.id = c.pull_request_id JOIN repos r ON r.id = p.repo_id";

fn event_from_row(row: &SqliteRow) -> Result<CarryEvent, DbError> {
    Ok(CarryEvent {
        id: wire_col(row, "id")?,
        pr: PrRef {
            repo: RepoRef {
                owner: row.try_get("owner")?,
                name: row.try_get("name")?,
            },
            number: u32_col(row, "number")?,
        },
        item_no: u32_col(row, "item_no")?,
        source_item_no: u32_col(row, "source_item_no")?,
        source_decision: wire_col(row, "source_decision_id")?,
        source_kind: wire_col(row, "source_kind")?,
        kind: wire_col(row, "kind")?,
        reason: opt_wire_col(row, "reason")?,
        evidence: json_col(row, "evidence")?,
        effects_pending: u32_col(row, "effects_pending")?,
        at: ts_col(row, "at")?,
    })
}

impl ReviewLoop<'_> {
    /// Records a carry or re-ask: the repeat becomes `repeat` (carried) or `reopened` (asked again) and names the
    /// source decision's item when it is another item.
    pub async fn record_carry(
        &self,
        tenant_id: &str,
        event: &NewCarryEvent,
        now: Timestamp,
    ) -> Result<CarryEventId, DbError> {
        if (event.kind == CarryKind::AskedAgain) != event.reason.is_some() {
            return Err(DbError::Conflict);
        }
        let mut tx = write_tx(self.pool).await?;
        let item = item_in(&mut tx, event.item).await?;
        let source_item: String =
            sqlx::query_scalar("SELECT item_id FROM comment_decisions WHERE id = ?")
                .bind(event.source_decision.to_string())
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(DbError::NotFound)?;
        let id = CarryEventId::new_random();
        sqlx::query(
            "INSERT INTO comment_carry_events (id, tenant_id, pull_request_id, item_id, source_decision_id, kind,
                 reason, evidence, at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(tenant_id)
        .bind(item.pull_request_id)
        .bind(event.item.to_string())
        .bind(event.source_decision.to_string())
        .bind(wire(event.kind)?)
        .bind(event.reason.map(wire).transpose()?)
        .bind(json(&event.evidence)?)
        .bind(ts(now))
        .execute(&mut *tx)
        .await?;
        if source_item != event.item.to_string() {
            sqlx::query("UPDATE comment_items SET repeat_of = ? WHERE id = ?")
                .bind(&source_item)
                .bind(event.item.to_string())
                .execute(&mut *tx)
                .await?;
        }
        let state = match event.kind {
            CarryKind::Carried => ItemState::Repeat,
            CarryKind::AskedAgain => ItemState::Reopened,
        };
        set_state_in(&mut tx, event.item, state, now).await?;
        tx.commit().await?;
        Ok(id)
    }

    /// Carry events newest first, of one PR or the tenant's, before the `(at, id)` cursor; at most `limit`.
    pub async fn carry_events(
        &self,
        tenant_id: &str,
        pull_request_id: Option<i64>,
        before: Option<(Timestamp, CarryEventId)>,
        limit: u32,
    ) -> Result<Vec<CarryEvent>, DbError> {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "{EVENT_SELECT}
             WHERE c.tenant_id = ?1 AND (?2 IS NULL OR c.pull_request_id = ?2)
               AND (?3 IS NULL OR c.at < ?3 OR (c.at = ?3 AND c.id < ?4))
             ORDER BY c.at DESC, c.id DESC LIMIT ?5"
        )))
        .bind(tenant_id)
        .bind(pull_request_id)
        .bind(before.map(|(at, _)| ts(at)))
        .bind(before.map(|(_, id)| id.to_string()))
        .bind(limit)
        .fetch_all(self.pool)
        .await?
        .iter()
        .map(event_from_row)
        .collect()
    }

    /// Counts per PR (one PR or every PR of the tenant with events), with the PR's `pull_requests.id`.
    pub async fn carry_counts(
        &self,
        tenant_id: &str,
        pull_request_id: Option<i64>,
    ) -> Result<Vec<(i64, CarryCounts)>, DbError> {
        let mut tx = read_tx(self.pool).await?;
        let rows = sqlx::query(
            "SELECT c.pull_request_id,
                 SUM(c.kind = 'carried') AS carried,
                 COALESCE(SUM(c.reason = 'bot_edited'), 0) AS bot_edited,
                 COALESCE(SUM(c.reason = 'code_changed'), 0) AS code_changed,
                 COUNT(DISTINCT CASE WHEN c.kind = 'carried' AND EXISTS (
                     SELECT 1 FROM comment_effects e WHERE e.decision_id = c.source_decision_id
                       AND e.state IN ('waiting', 'open', 'claimed')) THEN c.id END) AS pending
             FROM comment_carry_events c
             WHERE c.tenant_id = ?1 AND (?2 IS NULL OR c.pull_request_id = ?2)
             GROUP BY c.pull_request_id ORDER BY c.pull_request_id",
        )
        .bind(tenant_id)
        .bind(pull_request_id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        rows.iter()
            .map(|row| {
                Ok((
                    row.try_get("pull_request_id")?,
                    CarryCounts {
                        carried: u32_col(row, "carried")?,
                        asked_again_bot_edited: u32_col(row, "bot_edited")?,
                        asked_again_code_changed: u32_col(row, "code_changed")?,
                        effects_pending: u32_col(row, "pending")?,
                    },
                ))
            })
            .collect()
    }
}
