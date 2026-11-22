//! Append-only decisions, their effects, claims under a lease and agent reports (plan §2.3, §5.2).

use std::collections::{BTreeMap, BTreeSet};

use jiff::SignedDuration;
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, Sqlite, Transaction};

use contracts::api::access_tokens::AccessTokenId;
use contracts::api::review_loop::{
    ClaimInfo, Decision, DecisionKind, DecisionTarget, DecisionVia, Effect, EffectKind,
    EffectParams, EffectReport, EffectState, Executor, FeedDecision, FeedItem, ItemState, Override,
    Recommendation, ReportOutcome,
};
use contracts::common::{LineRange, PrRef, RepoRef};
use contracts::ids::{CommentItemId, CommitSha, DecisionId, EffectId};
use contracts::time::Timestamp;

use super::super::jobs::ts;
use super::super::{read_tx, write_tx};
use super::items::{item_in, set_state_in};
use super::{
    ReviewLoop, json, json_col, opt_json_col, opt_ts_col, ts_col, u32_col, wire, wire_col,
};
use crate::error::DbError;

/// One effect of a new decision, computed by the server's effect function.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewEffect {
    /// Order, 1-based.
    pub seq: u32,
    /// `seq` this effect waits for.
    pub after: Option<u32>,
    /// Kind.
    pub kind: EffectKind,
    /// Executor.
    pub executor: Executor,
    /// Parameters.
    pub params: EffectParams,
}

/// A decision to store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewDecision {
    /// The item.
    pub item: CommentItemId,
    /// The item revision the decision is based on.
    pub item_revision: u32,
    /// Deciding user (`users.id`).
    pub user_id: String,
    /// Decision.
    pub kind: DecisionKind,
    /// Reason.
    pub reason: Option<String>,
    /// Target.
    pub target: Option<DecisionTarget>,
    /// Override flag.
    pub r#override: Option<Override>,
    /// "Resolve this thread" as submitted.
    pub resolve_ticked: bool,
    /// Reply text.
    pub reply_text: Option<String>,
    /// The owner's comment.
    pub comment: Option<String>,
    /// Recommendation shown.
    pub recommendation: Option<Recommendation>,
    /// Where.
    pub via: DecisionVia,
    /// Source of a carried decision.
    pub carried_from: Option<DecisionId>,
    /// PR head now.
    pub head: CommitSha,
    /// Effects.
    pub effects: Vec<NewEffect>,
}

/// Result of [`ReviewLoop::decide`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecideOutcome {
    /// Every decision stored, in request order.
    Stored(Vec<Decision>),
    /// Nothing stored: these items (`C<no>`) changed since the revisions named.
    ItemChanged(Vec<u32>),
}

/// A decision with the item facts the carry-over rules compare.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredDecision {
    /// The decision.
    pub decision: Decision,
    /// Its item.
    pub item: CommentItemId,
    /// The item's normalised text hash at decision time.
    pub body_hash: String,
    /// The item's lines at decision time.
    pub lines: Option<LineRange>,
}

/// Result of [`ReviewLoop::claim`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClaimOutcome {
    /// Claimed (or renewed by the same token) until the lease end.
    Claimed(Timestamp),
    /// Another token holds a live claim.
    Taken,
    /// An open effect is outside the capabilities.
    CapabilityMissing,
    /// Nothing left for an agent (superseded, done, or diffpilot[bot] effects only).
    NothingOpen,
}

/// An agent report to record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReportRequest {
    /// Outcome.
    pub outcome: ReportOutcome,
    /// Reference.
    pub reference: Option<String>,
    /// Message.
    pub message: Option<String>,
    /// Client key; a repeat by the same token is a no-op.
    pub idempotency_key: String,
}

/// Result of [`ReviewLoop::report`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportOutcomeRecord {
    /// Recorded and applied.
    Recorded,
    /// The same token sent this key before; nothing changed.
    Duplicate,
    /// The effect is not claimed by this token.
    NotClaimer,
    /// The token's lease has ended.
    LeaseExpired,
}

const DECISION_COLUMNS: &str = "d.id, d.item_id, i.item_no, d.kind, d.reason, d.target, d.override, d.resolve_ticked,
    d.reply_text, d.comment, d.recommendation, d.via, d.carried_from, d.supersedes, d.head, d.body_hash,
    d.start_line, d.end_line, u.login, d.decided_at";

fn decision_from_row(row: &SqliteRow) -> Result<StoredDecision, DbError> {
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
    let carried: Option<String> = row.try_get("carried_from")?;
    let supersedes: Option<String> = row.try_get("supersedes")?;
    let resolve: i64 = row.try_get("resolve_ticked")?;
    Ok(StoredDecision {
        decision: Decision {
            id: wire_col(row, "id")?,
            item_no: u32_col(row, "item_no")?,
            kind: wire_col(row, "kind")?,
            reason: row.try_get("reason")?,
            target: opt_json_col(row, "target")?,
            r#override: opt_json_col(row, "override")?,
            resolve_ticked: resolve == 1,
            reply_text: row.try_get("reply_text")?,
            comment: row.try_get("comment")?,
            recommendation: opt_json_col(row, "recommendation")?,
            via: wire_col(row, "via")?,
            carried_from: carried
                .map(|id| super::super::jobs::parse_wire(&id, "carried_from"))
                .transpose()?,
            supersedes: supersedes
                .map(|id| super::super::jobs::parse_wire(&id, "supersedes"))
                .transpose()?,
            head: CommitSha(row.try_get("head")?),
            decided_by: row.try_get("login")?,
            decided_at: ts_col(row, "decided_at")?,
            effects: Vec::new(),
        },
        item: wire_col(row, "item_id")?,
        body_hash: row.try_get("body_hash")?,
        lines,
    })
}

fn effect_from_row(row: &SqliteRow) -> Result<Effect, DbError> {
    let after: Option<i64> = row.try_get("after_seq")?;
    let token_name: Option<String> = row.try_get("claim_token_name")?;
    let lease = opt_ts_col(row, "lease_until")?;
    Ok(Effect {
        id: wire_col(row, "id")?,
        seq: u32_col(row, "seq")?,
        after: after
            .map(u32::try_from)
            .transpose()
            .map_err(|_| DbError::Corrupt {
                column: "after_seq",
            })?,
        kind: wire_col(row, "kind")?,
        executor: wire_col(row, "executor")?,
        params: json_col(row, "params")?,
        state: wire_col(row, "state")?,
        claim: token_name
            .zip(lease)
            .map(|(token_name, lease_until)| ClaimInfo {
                token_name,
                lease_until,
            }),
        reports: Vec::new(),
        seen_on_github_at: opt_ts_col(row, "seen_on_github_at")?,
        updated_at: ts_col(row, "updated_at")?,
    })
}

/// Fills the effects (with reports) of `decisions` inside `tx`.
async fn with_effects(
    tx: &mut Transaction<'static, Sqlite>,
    decisions: &mut [StoredDecision],
) -> Result<(), DbError> {
    for stored in decisions.iter_mut() {
        let rows = sqlx::query(
            "SELECT e.id, e.seq, e.after_seq, e.kind, e.executor, e.params, e.state, t.name AS claim_token_name,
                 e.lease_until, e.seen_on_github_at, e.updated_at
             FROM comment_effects e LEFT JOIN access_tokens t ON t.id = e.claim_token_id
             WHERE e.decision_id = ? ORDER BY e.seq",
        )
        .bind(stored.decision.id.to_string())
        .fetch_all(&mut **tx)
        .await?;
        let mut effects = rows
            .iter()
            .map(effect_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        for effect in &mut effects {
            effect.reports = sqlx::query(
                "SELECT r.outcome, r.reference, r.message, t.name, r.at
                 FROM comment_effect_reports r JOIN access_tokens t ON t.id = r.token_id
                 WHERE r.effect_id = ? ORDER BY r.id",
            )
            .bind(effect.id.to_string())
            .fetch_all(&mut **tx)
            .await?
            .iter()
            .map(|row| {
                Ok(EffectReport {
                    outcome: wire_col(row, "outcome")?,
                    reference: row.try_get("reference")?,
                    message: row.try_get("message")?,
                    token_name: row.try_get("name")?,
                    at: ts_col(row, "at")?,
                })
            })
            .collect::<Result<_, DbError>>()?;
        }
        stored.decision.effects = effects;
    }
    Ok(())
}

/// Decisions matching `filter` (a SQL condition on `d`/`i` with one `?`), oldest first, with effects.
async fn decisions_where(
    tx: &mut Transaction<'static, Sqlite>,
    filter: &str,
    value: String,
) -> Result<Vec<StoredDecision>, DbError> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {DECISION_COLUMNS} FROM comment_decisions d
         JOIN comment_items i ON i.id = d.item_id JOIN users u ON u.id = d.user_id
         WHERE {filter} ORDER BY d.decided_at, d.rowid"
    )))
    .bind(value)
    .fetch_all(&mut **tx)
    .await?;
    let mut decisions = rows
        .iter()
        .map(decision_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    with_effects(tx, &mut decisions).await?;
    Ok(decisions)
}

/// The current (not superseded) decision of an item inside `tx`.
async fn current_id(
    tx: &mut Transaction<'static, Sqlite>,
    item: CommentItemId,
) -> Result<Option<String>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT d.id FROM comment_decisions d WHERE d.item_id = ?
           AND NOT EXISTS (SELECT 1 FROM comment_decisions n WHERE n.supersedes = d.id)
         ORDER BY d.decided_at DESC, d.rowid DESC LIMIT 1",
    )
    .bind(item.to_string())
    .fetch_optional(&mut **tx)
    .await?)
}

/// The state an unclaimed effect returns to: `waiting` while the effect it waits for is not done.
const RETURN_STATE: &str = "CASE WHEN e.after_seq IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM comment_effects p WHERE p.decision_id = e.decision_id AND p.seq = e.after_seq
          AND p.state = 'done') THEN 'waiting' ELSE 'open' END";

/// Opens waiting effects of `decision` whose predecessor is done, and sets the item's state from its current
/// decision's effects (`executing` while any is open or claimed, else `done` when all done, else `decided`).
async fn settle(
    tx: &mut Transaction<'static, Sqlite>,
    decision: &str,
    now: Timestamp,
) -> Result<(), DbError> {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE comment_effects AS e SET state = {RETURN_STATE}, updated_at = ?
         WHERE e.decision_id = ? AND e.state = 'waiting'"
    )))
    .bind(ts(now))
    .bind(decision)
    .execute(&mut **tx)
    .await?;
    let row = sqlx::query(
        "SELECT d.item_id,
             SUM(e.state IN ('waiting', 'open', 'claimed', 'failed')) AS open,
             COUNT(e.id) AS total, SUM(e.state = 'done') AS done
         FROM comment_decisions d LEFT JOIN comment_effects e ON e.decision_id = d.id
         WHERE d.id = ? GROUP BY d.id",
    )
    .bind(decision)
    .fetch_one(&mut **tx)
    .await?;
    let item: CommentItemId = wire_col(&row, "item_id")?;
    if current_id(tx, item).await?.as_deref() != Some(decision) {
        return Ok(());
    }
    let open: Option<i64> = row.try_get("open")?;
    let total: i64 = row.try_get("total")?;
    let done: Option<i64> = row.try_get("done")?;
    let state = if open.unwrap_or(0) > 0 {
        ItemState::Executing
    } else if total > 0 && done == Some(total) {
        ItemState::Done
    } else {
        ItemState::Decided
    };
    let current = item_in(tx, item).await?;
    if current.state != state && current.state != ItemState::Outside {
        set_state_in(tx, item, state, now).await?;
    }
    Ok(())
}

impl ReviewLoop<'_> {
    /// Stores decisions all or nothing, each superseding its item's current decision (whose effects not yet
    /// claimed are cancelled). Every item must still be at the revision named.
    pub async fn decide(
        &self,
        tenant_id: &str,
        decisions: &[NewDecision],
        now: Timestamp,
    ) -> Result<DecideOutcome, DbError> {
        let mut tx = write_tx(self.pool).await?;
        let mut changed = Vec::new();
        let mut items = Vec::new();
        for new in decisions {
            let item = item_in(&mut tx, new.item).await?;
            if item.revision != new.item_revision {
                changed.push(item.no);
            }
            items.push(item);
        }
        if !changed.is_empty() {
            return Ok(DecideOutcome::ItemChanged(changed));
        }
        let mut ids = Vec::new();
        for (new, item) in decisions.iter().zip(items) {
            let supersedes = current_id(&mut tx, new.item).await?;
            if let Some(previous) = &supersedes {
                sqlx::query(
                    "UPDATE comment_effects SET state = 'cancelled', updated_at = ?
                     WHERE decision_id = ? AND state IN ('waiting', 'open')",
                )
                .bind(ts(now))
                .bind(previous)
                .execute(&mut *tx)
                .await?;
            }
            let id = DecisionId::new_random();
            let lines = item.location.as_ref().and_then(|location| location.lines);
            sqlx::query(
                "INSERT INTO comment_decisions (id, tenant_id, item_id, user_id, kind, reason, target, override,
                     resolve_ticked, reply_text, comment, recommendation, via, carried_from, supersedes, head,
                     body_hash, start_line, end_line, decided_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(id.to_string())
            .bind(tenant_id)
            .bind(new.item.to_string())
            .bind(&new.user_id)
            .bind(wire(new.kind)?)
            .bind(&new.reason)
            .bind(new.target.as_ref().map(json).transpose()?)
            .bind(new.r#override.as_ref().map(json).transpose()?)
            .bind(i64::from(new.resolve_ticked))
            .bind(&new.reply_text)
            .bind(&new.comment)
            .bind(new.recommendation.as_ref().map(json).transpose()?)
            .bind(wire(new.via)?)
            .bind(new.carried_from.map(|id| id.to_string()))
            .bind(supersedes)
            .bind(&new.head.0)
            .bind(&item.body_hash)
            .bind(lines.map(|lines| lines.start_line))
            .bind(lines.map(|lines| lines.end_line))
            .bind(ts(now))
            .execute(&mut *tx)
            .await?;
            for effect in &new.effects {
                sqlx::query(
                    "INSERT INTO comment_effects (id, tenant_id, decision_id, seq, after_seq, kind, executor, params,
                         state, updated_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(EffectId::new_random().to_string())
                .bind(tenant_id)
                .bind(id.to_string())
                .bind(effect.seq)
                .bind(effect.after)
                .bind(wire(effect.kind)?)
                .bind(wire(effect.executor)?)
                .bind(json(&effect.params)?)
                .bind(if effect.after.is_some() {
                    "waiting"
                } else {
                    "open"
                })
                .bind(ts(now))
                .execute(&mut *tx)
                .await?;
            }
            // The decision itself always changes the item: one revision step even when the state stays.
            set_state_in(&mut tx, new.item, item.state, now).await?;
            settle(&mut tx, &id.to_string(), now).await?;
            ids.push(id);
        }
        let mut stored = Vec::new();
        for id in ids {
            let mut found = decisions_where(&mut tx, "d.id = ?", id.to_string()).await?;
            stored.push(found.pop().ok_or(DbError::NotFound)?.decision);
        }
        tx.commit().await?;
        Ok(DecideOutcome::Stored(stored))
    }

    /// An item's decisions, oldest first.
    pub async fn item_decisions(
        &self,
        item: CommentItemId,
    ) -> Result<Vec<StoredDecision>, DbError> {
        let mut tx = read_tx(self.pool).await?;
        let decisions = decisions_where(&mut tx, "d.item_id = ?", item.to_string()).await?;
        tx.commit().await?;
        Ok(decisions)
    }

    /// A PR's decisions, oldest first.
    pub async fn pr_decisions(&self, pull_request_id: i64) -> Result<Vec<StoredDecision>, DbError> {
        let mut tx = read_tx(self.pool).await?;
        let decisions = decisions_where(
            &mut tx,
            "i.pull_request_id = ?",
            pull_request_id.to_string(),
        )
        .await?;
        tx.commit().await?;
        Ok(decisions)
    }

    /// One decision as an agent reads it (RF1b), any state.
    pub async fn feed_decision(&self, id: DecisionId) -> Result<Option<FeedDecision>, DbError> {
        let mut tx = read_tx(self.pool).await?;
        let found = feed_decisions_where(&mut tx, "d.id = ?", id.to_string()).await?;
        tx.commit().await?;
        Ok(found.into_iter().next())
    }

    /// Current decisions of the tenant with agent effects open (or under an expired lease) whose kinds all fall
    /// within `capabilities`, oldest first; optionally one repository (`owner/name`, case-insensitive).
    pub async fn claimable(
        &self,
        tenant_id: &str,
        capabilities: &[EffectKind],
        repo: Option<&RepoRef>,
        now: Timestamp,
    ) -> Result<Vec<FeedDecision>, DbError> {
        let allowed = capabilities
            .iter()
            .map(|kind| wire(*kind))
            .collect::<Result<BTreeSet<_>, _>>()?;
        let mut tx = read_tx(self.pool).await?;
        let candidates: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT d.id FROM comment_decisions d
             JOIN comment_effects e ON e.decision_id = d.id
             JOIN comment_items i ON i.id = d.item_id
             JOIN pull_requests p ON p.id = i.pull_request_id JOIN repos r ON r.id = p.repo_id
             WHERE d.tenant_id = ?1 AND e.executor = 'agent'
               AND (e.state IN ('waiting', 'open') OR (e.state = 'claimed' AND e.lease_until < ?2))
               AND NOT EXISTS (SELECT 1 FROM comment_decisions n WHERE n.supersedes = d.id)
               AND NOT EXISTS (SELECT 1 FROM comment_effects c WHERE c.decision_id = d.id
                               AND c.state = 'claimed' AND c.lease_until >= ?2)
               AND (?3 IS NULL OR (r.owner = ?3 AND r.name = ?4))
             ORDER BY d.decided_at, d.rowid",
        )
        .bind(tenant_id)
        .bind(ts(now))
        .bind(repo.map(|repo| repo.owner.clone()))
        .bind(repo.map(|repo| repo.name.clone()))
        .fetch_all(&mut *tx)
        .await?;
        let mut feed = Vec::new();
        for id in candidates {
            let Some(entry) = feed_decisions_where(&mut tx, "d.id = ?", id)
                .await?
                .into_iter()
                .next()
            else {
                continue;
            };
            let within = entry
                .decision
                .effects
                .iter()
                .filter(|effect| effect.executor == Executor::Agent && !terminal(effect.state))
                .all(|effect| wire(effect.kind).is_ok_and(|kind| allowed.contains(&kind)));
            if within {
                feed.push(entry);
            }
        }
        tx.commit().await?;
        Ok(feed)
    }

    /// Claims a current decision's open agent effects for `token` until `now + lease`. The same token renews its
    /// claim; a live claim of another token is `Taken`.
    pub async fn claim(
        &self,
        decision: DecisionId,
        token: &AccessTokenId,
        capabilities: &[EffectKind],
        lease: SignedDuration,
        now: Timestamp,
    ) -> Result<ClaimOutcome, DbError> {
        let mut tx = write_tx(self.pool).await?;
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM comment_decisions WHERE id = ?)")
                .bind(decision.to_string())
                .fetch_one(&mut *tx)
                .await?;
        if !exists {
            return Err(DbError::NotFound);
        }
        let superseded: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM comment_decisions WHERE supersedes = ?)",
        )
        .bind(decision.to_string())
        .fetch_one(&mut *tx)
        .await?;
        let rows = sqlx::query(
            "SELECT kind, state, claim_token_id, lease_until FROM comment_effects
             WHERE decision_id = ? AND executor = 'agent' AND state IN ('waiting', 'open', 'claimed')",
        )
        .bind(decision.to_string())
        .fetch_all(&mut *tx)
        .await?;
        if superseded || rows.is_empty() {
            return Ok(ClaimOutcome::NothingOpen);
        }
        let allowed = capabilities
            .iter()
            .map(|kind| wire(*kind))
            .collect::<Result<BTreeSet<_>, _>>()?;
        for row in &rows {
            let holder: Option<String> = row.try_get("claim_token_id")?;
            let lease_until = opt_ts_col(row, "lease_until")?;
            if holder.is_some_and(|holder| holder != token.0)
                && lease_until.is_some_and(|until| until >= now)
            {
                return Ok(ClaimOutcome::Taken);
            }
            let kind: String = row.try_get("kind")?;
            if !allowed.contains(&kind) {
                return Ok(ClaimOutcome::CapabilityMissing);
            }
        }
        let until = now + lease;
        sqlx::query(
            "UPDATE comment_effects SET state = 'claimed', claim_token_id = ?, lease_until = ?, updated_at = ?
             WHERE decision_id = ? AND executor = 'agent' AND state IN ('waiting', 'open', 'claimed')",
        )
        .bind(&token.0)
        .bind(ts(until))
        .bind(ts(now))
        .bind(decision.to_string())
        .execute(&mut *tx)
        .await?;
        settle(&mut tx, &decision.to_string(), now).await?;
        tx.commit().await?;
        Ok(ClaimOutcome::Claimed(until))
    }

    /// Records a report of the effect's claimer: `done` and `failed` end the effect, `released` returns every
    /// effect of the claim to the feed; each report renews the lease of the claim's other effects.
    pub async fn report(
        &self,
        tenant_id: &str,
        effect: EffectId,
        token: &AccessTokenId,
        report: &ReportRequest,
        lease: SignedDuration,
        now: Timestamp,
    ) -> Result<ReportOutcomeRecord, DbError> {
        let mut tx = write_tx(self.pool).await?;
        let duplicate: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM comment_effect_reports WHERE token_id = ? AND idempotency_key = ?)",
        )
        .bind(&token.0)
        .bind(&report.idempotency_key)
        .fetch_one(&mut *tx)
        .await?;
        if duplicate {
            return Ok(ReportOutcomeRecord::Duplicate);
        }
        let row = sqlx::query(
            "SELECT decision_id, state, claim_token_id, lease_until FROM comment_effects WHERE id = ?",
        )
        .bind(effect.to_string())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(DbError::NotFound)?;
        let decision: String = row.try_get("decision_id")?;
        let holder: Option<String> = row.try_get("claim_token_id")?;
        let state: EffectState = wire_col(&row, "state")?;
        if state != EffectState::Claimed || holder.as_deref() != Some(token.0.as_str()) {
            return Ok(ReportOutcomeRecord::NotClaimer);
        }
        if opt_ts_col(&row, "lease_until")?.is_some_and(|until| until < now) {
            return Ok(ReportOutcomeRecord::LeaseExpired);
        }
        sqlx::query(
            "INSERT INTO comment_effect_reports (tenant_id, effect_id, token_id, outcome, reference, message,
                 idempotency_key, at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(tenant_id)
        .bind(effect.to_string())
        .bind(&token.0)
        .bind(wire(report.outcome)?)
        .bind(&report.reference)
        .bind(&report.message)
        .bind(&report.idempotency_key)
        .bind(ts(now))
        .execute(&mut *tx)
        .await?;
        match report.outcome {
            ReportOutcome::Done | ReportOutcome::Failed => {
                sqlx::query(
                    "UPDATE comment_effects SET state = ?, reference = ?, message = ?, claim_token_id = NULL,
                         lease_until = NULL, updated_at = ?
                     WHERE id = ?",
                )
                .bind(if report.outcome == ReportOutcome::Done {
                    "done"
                } else {
                    "failed"
                })
                .bind(&report.reference)
                .bind(&report.message)
                .bind(ts(now))
                .bind(effect.to_string())
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    "UPDATE comment_effects SET lease_until = ?, updated_at = ?
                     WHERE decision_id = ? AND state = 'claimed' AND claim_token_id = ?",
                )
                .bind(ts(now + lease))
                .bind(ts(now))
                .bind(&decision)
                .bind(&token.0)
                .execute(&mut *tx)
                .await?;
            }
            ReportOutcome::Released => {
                release(&mut tx, &decision, Some(&token.0), now).await?;
            }
        }
        settle(&mut tx, &decision, now).await?;
        tx.commit().await?;
        Ok(ReportOutcomeRecord::Recorded)
    }

    /// Returns effects whose lease ended to the feed; the decisions affected.
    pub async fn expire_leases(&self, now: Timestamp) -> Result<Vec<DecisionId>, DbError> {
        let mut tx = write_tx(self.pool).await?;
        let decisions: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT decision_id FROM comment_effects WHERE state = 'claimed' AND lease_until < ?",
        )
        .bind(ts(now))
        .fetch_all(&mut *tx)
        .await?;
        let mut ids = Vec::new();
        for decision in decisions {
            release(&mut tx, &decision, None, now).await?;
            settle(&mut tx, &decision, now).await?;
            ids.push(super::super::jobs::parse_wire(&decision, "decision_id")?);
        }
        tx.commit().await?;
        Ok(ids)
    }

    /// Records that a server read found an effect on GitHub (fact badge).
    pub async fn effect_seen_on_github(
        &self,
        effect: EffectId,
        now: Timestamp,
    ) -> Result<(), DbError> {
        let updated = sqlx::query(
            "UPDATE comment_effects SET seen_on_github_at = COALESCE(seen_on_github_at, ?) WHERE id = ?",
        )
        .bind(ts(now))
        .bind(effect.to_string())
        .execute(self.pool)
        .await?;
        if updated.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }
}

fn terminal(state: EffectState) -> bool {
    matches!(
        state,
        EffectState::Done | EffectState::Failed | EffectState::Cancelled
    )
}

/// Returns a decision's claimed effects (of `token`, or all) to `open`/`waiting`.
async fn release(
    tx: &mut Transaction<'static, Sqlite>,
    decision: &str,
    token: Option<&str>,
    now: Timestamp,
) -> Result<(), DbError> {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE comment_effects AS e SET state = {RETURN_STATE}, claim_token_id = NULL, lease_until = NULL,
             updated_at = ?
         WHERE e.decision_id = ? AND e.state = 'claimed' AND (? IS NULL OR e.claim_token_id = ?)"
    )))
    .bind(ts(now))
    .bind(decision)
    .bind(token)
    .bind(token)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Feed entries (decision, item and PR) matching `filter`.
async fn feed_decisions_where(
    tx: &mut Transaction<'static, Sqlite>,
    filter: &str,
    value: String,
) -> Result<Vec<FeedDecision>, DbError> {
    let decisions = decisions_where(tx, filter, value).await?;
    let mut items: BTreeMap<String, (PrRef, FeedItem)> = BTreeMap::new();
    let mut feed = Vec::new();
    for stored in decisions {
        let key = stored.item.to_string();
        if !items.contains_key(&key) {
            let row = sqlx::query(
                "SELECT r.owner, r.name, p.number FROM comment_items i
                 JOIN pull_requests p ON p.id = i.pull_request_id JOIN repos r ON r.id = p.repo_id
                 WHERE i.id = ?",
            )
            .bind(&key)
            .fetch_one(&mut **tx)
            .await?;
            let item = item_in(tx, stored.item).await?;
            items.insert(
                key.clone(),
                (
                    PrRef {
                        repo: RepoRef {
                            owner: row.try_get("owner")?,
                            name: row.try_get("name")?,
                        },
                        number: u32_col(&row, "number")?,
                    },
                    FeedItem {
                        no: item.no,
                        kind: item.kind,
                        url: item.url,
                        location: item.location,
                        title: item.title,
                        body: item.body,
                    },
                ),
            );
        }
        let (pr, item) = items.get(&key).cloned().ok_or(DbError::NotFound)?;
        feed.push(FeedDecision {
            pr,
            item,
            decision: stored.decision,
        });
    }
    Ok(feed)
}
