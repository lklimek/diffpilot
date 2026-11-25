//! Append-only decisions, their effects, claims under a lease and agent reports (plan §2.3, §5.2).

use std::collections::{BTreeMap, BTreeSet};

use jiff::SignedDuration;
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, Sqlite, Transaction};

use contracts::api::access_tokens::AccessTokenId;
use contracts::api::review_loop::{
    ClaimInfo, Decision, DecisionKind, DecisionTarget, DecisionVia, Effect, EffectKind,
    EffectParams, EffectReport, EffectState, Executor, FeedDecision, FeedItem, ImportedDecision,
    ItemState, Override, Recommendation, ReportOutcome,
};
use contracts::common::{LineRange, PrRef, RepoRef};
use contracts::ids::{CommentItemId, CommitSha, DecisionId, EffectId};
use contracts::time::Timestamp;

use super::super::jobs::ts;
use super::super::{read_tx, write_tx};
use super::items::{PrView, item_in, pr_view_in, set_state_in};
use super::{
    ReviewLoop, json, json_col, opt_json_col, opt_ts_col, opt_wire_col, ts_col, u32_col, wire,
    wire_col,
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
    /// Automatic origin, kept beside the decision row.
    pub origin: Option<super::NewOrigin>,
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
    /// Server user who owns this decision.
    pub user_id: String,
    /// Exact decision this follower depends on.
    pub primary_decision: Option<DecisionId>,
    /// A withdrawal has authority over its own point only.
    pub author_withdrawal: bool,
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
    /// The token's lease has ended (the effect may be back in the feed).
    LeaseExpired,
    /// The effect already ended (done, failed or cancelled).
    Ended,
    /// `done` on an effect whose `after` effect is not done.
    Waiting,
}

/// A claimable decision as the feed tracks it: no live claim, with these kinds of agent effects not yet ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenFeedEntry {
    /// Tenant.
    pub tenant_id: String,
    /// Decision.
    pub decision: DecisionId,
    /// Pull request.
    pub pr: PrRef,
    /// Kinds of its open agent effects, each once, in `seq` order.
    pub kinds: Vec<EffectKind>,
}

/// Where a decision (or one of its effects) belongs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecisionOwner {
    /// Tenant.
    pub tenant_id: String,
    /// `pull_requests.id`.
    pub pull_request_id: i64,
    /// The decision.
    pub decision: DecisionId,
}

pub(super) const DECISION_COLUMNS: &str ="d.id, d.item_id, i.item_no, d.kind, d.reason, d.target, d.override, d.resolve_ticked,
    d.reply_text, d.comment, d.recommendation, d.via, d.carried_from, d.supersedes, d.head, d.body_hash,
    d.start_line, d.end_line, d.user_id, u.login, d.decided_at, o.origin, o.primary_decision_id,
    o.by_login, o.by_github_id, o.relay_login, o.recorded_at, o.evidence_url, o.quote, o.author_withdrawal,
    pi.item_no AS follows_no,
    (SELECT un.at FROM comment_decision_undos un WHERE un.decision_id = d.id) AS undone_at";

pub(super) const ORIGIN_JOIN: &str = "LEFT JOIN comment_decision_origins o ON o.decision_id = d.id
     LEFT JOIN comment_decisions pd ON pd.id = o.primary_decision_id
     LEFT JOIN comment_items pi ON pi.id = pd.item_id";

pub(super) fn decision_from_row(row: &SqliteRow) -> Result<StoredDecision, DbError> {
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
    let (via, imported, follows) = match row.try_get::<Option<&str>, _>("origin")? {
        Some("imported_github") => (
            DecisionVia::ImportedGithub,
            Some(ImportedDecision {
                by: row.try_get("by_login")?,
                by_id: row
                    .try_get::<Option<i64>, _>("by_github_id")?
                    .and_then(|id| u64::try_from(id).ok()),
                at: ts_col(row, "recorded_at")?,
                evidence_url: row.try_get("evidence_url")?,
                quote: row.try_get("quote")?,
                relay: row.try_get("relay_login")?,
            }),
            None,
        ),
        Some("follows_primary") => (
            DecisionVia::FollowsPrimary,
            None,
            Some(u32_col(row, "follows_no")?),
        ),
        _ => (wire_col(row, "via")?, None, None),
    };
    Ok(StoredDecision {
        user_id: row.try_get("user_id")?,
        primary_decision: opt_wire_col(row, "primary_decision_id")?,
        author_withdrawal: row
            .try_get::<Option<bool>, _>("author_withdrawal")?
            .unwrap_or(false),
        decision: Decision {
            imported,
            follows,
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
            via,
            carried_from: carried
                .map(|id| super::super::jobs::parse_wire(&id, "carried_from"))
                .transpose()?,
            supersedes: supersedes
                .map(|id| super::super::jobs::parse_wire(&id, "supersedes"))
                .transpose()?,
            head: CommitSha(row.try_get("head")?),
            decided_by: row.try_get("login")?,
            decided_at: ts_col(row, "decided_at")?,
            undone_at: opt_ts_col(row, "undone_at")?,
            effects: Vec::new(),
        },
        item: wire_col(row, "item_id")?,
        body_hash: row.try_get("body_hash")?,
        lines,
    })
}

pub(super) fn effect_from_row(row: &SqliteRow) -> Result<Effect, DbError> {
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
pub(super) async fn decisions_where(
    tx: &mut Transaction<'static, Sqlite>,
    filter: &str,
    value: String,
) -> Result<Vec<StoredDecision>, DbError> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {DECISION_COLUMNS} FROM comment_decisions d
         JOIN comment_items i ON i.id = d.item_id JOIN users u ON u.id = d.user_id {ORIGIN_JOIN}
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

/// Cancels the unclaimed effects of `decision` that wait on a failed or cancelled effect, along their chain: what
/// comes after a failed effect never runs (plan §2.3 D7). A claimed one is cancelled once released or its lease
/// ends; a `done` report on it is refused meanwhile.
async fn cancel_dependents(
    tx: &mut Transaction<'static, Sqlite>,
    decision: &str,
    now: Timestamp,
) -> Result<(), DbError> {
    loop {
        let cancelled = sqlx::query(
            "UPDATE comment_effects AS e SET state = 'cancelled', updated_at = ?
             WHERE e.decision_id = ? AND e.state IN ('waiting', 'open') AND EXISTS (
                 SELECT 1 FROM comment_effects p WHERE p.decision_id = e.decision_id AND p.seq = e.after_seq
                   AND p.state IN ('failed', 'cancelled'))",
        )
        .bind(ts(now))
        .bind(decision)
        .execute(&mut **tx)
        .await?;
        if cancelled.rows_affected() == 0 {
            return Ok(());
        }
    }
}

/// States the owner holds: a claim, report or lease end on the earlier decision leaves them as they are.
fn asked(state: ItemState) -> bool {
    matches!(
        state,
        ItemState::Undecided | ItemState::Assessing | ItemState::AskedBack | ItemState::Reopened
    )
}

/// Opens waiting effects of `decision` whose predecessor is done (cancelling those after a failed one), and sets the
/// item's state from its current decision's effects (`executing` while any is open or claimed, else `done` when all
/// done, else `decided`). After a claim, report or lease end (`by_decision` false) an item the owner is asked about
/// keeps its state, and the decision's unclaimed effects of an item asked again are cancelled.
async fn settle(
    tx: &mut Transaction<'static, Sqlite>,
    decision: &str,
    by_decision: bool,
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
    cancel_dependents(tx, decision, now).await?;
    let undone: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM comment_decision_undos WHERE decision_id = ?)",
    )
    .bind(decision)
    .fetch_one(&mut **tx)
    .await?;
    if undone {
        return super::actions::cancel_unclaimed(tx, decision, now).await;
    }
    let item: CommentItemId = wire_col(
        &sqlx::query("SELECT item_id FROM comment_decisions WHERE id = ?")
            .bind(decision)
            .fetch_one(&mut **tx)
            .await?,
        "item_id",
    )?;
    if current_id(tx, item).await?.as_deref() != Some(decision) {
        return Ok(());
    }
    let current = item_in(tx, item).await?;
    if !by_decision && asked(current.state) {
        if current.state == ItemState::Reopened {
            super::actions::cancel_unclaimed(tx, decision, now).await?;
        }
        return Ok(());
    }
    let row = sqlx::query(
        "SELECT SUM(e.state IN ('waiting', 'open', 'claimed', 'failed')) AS open,
             COUNT(e.id) AS total, SUM(e.state = 'done') AS done
         FROM comment_decisions d LEFT JOIN comment_effects e ON e.decision_id = d.id
         WHERE d.id = ? GROUP BY d.id",
    )
    .bind(decision)
    .fetch_one(&mut **tx)
    .await?;
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
        let outcome = store(&mut tx, tenant_id, decisions, now).await?;
        tx.commit().await?;
        Ok(outcome)
    }

    /// [`Self::decide`] on the decisions `plan` makes of the pull request's state as this write transaction reads
    /// it, so what is stored was planned on the state it is stored against; an error of `plan` stores nothing.
    pub async fn decide_planned<E: From<DbError>>(
        &self,
        tenant_id: &str,
        pull_request_id: i64,
        now: Timestamp,
        plan: impl FnOnce(PrView) -> Result<Vec<NewDecision>, E>,
    ) -> Result<DecideOutcome, E> {
        let mut tx = write_tx(self.pool).await?;
        let decisions = plan(pr_view_in(&mut tx, pull_request_id).await?)?;
        let outcome = store(&mut tx, tenant_id, &decisions, now).await?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(outcome)
    }

    /// The review-loop state of a pull request, read in one transaction.
    pub async fn pr_view(&self, pull_request_id: i64) -> Result<PrView, DbError> {
        let mut tx = read_tx(self.pool).await?;
        let view = pr_view_in(&mut tx, pull_request_id).await?;
        tx.commit().await?;
        Ok(view)
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
               AND NOT EXISTS (SELECT 1 FROM comment_decision_undos u WHERE u.decision_id = d.id)
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
        // A claim whose lease ended ends here as the expiry sweep would end it, and the decision is settled (a
        // failed effect's dependents and a reopened item's work cancelled) before anything is handed out again.
        let lapsed: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM comment_effects WHERE decision_id = ? AND state = 'claimed'
                            AND lease_until < ?)",
        )
        .bind(decision.to_string())
        .bind(ts(now))
        .fetch_one(&mut *tx)
        .await?;
        if lapsed {
            release(&mut tx, &decision.to_string(), None, now).await?;
            settle(&mut tx, &decision.to_string(), false, now).await?;
        }
        let refused = claim_refusal(&mut tx, decision, token, capabilities, now).await?;
        if let Some(refused) = refused {
            tx.commit().await?;
            return Ok(refused);
        }
        let until = now + lease;
        sqlx::query(
            "UPDATE comment_effects SET state = 'claimed', claim_token_id = ?, lease_until = ?, updated_at = ?,
                 lapsed_token_id = NULL
             WHERE decision_id = ? AND executor = 'agent' AND state IN ('waiting', 'open', 'claimed')",
        )
        .bind(&token.0)
        .bind(ts(until))
        .bind(ts(now))
        .bind(decision.to_string())
        .execute(&mut *tx)
        .await?;
        settle(&mut tx, &decision.to_string(), false, now).await?;
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
            "SELECT EXISTS (SELECT 1 FROM comment_effect_reports
                            WHERE token_id = ? AND effect_id = ? AND idempotency_key = ?)",
        )
        .bind(&token.0)
        .bind(effect.to_string())
        .bind(&report.idempotency_key)
        .fetch_one(&mut *tx)
        .await?;
        if duplicate {
            return Ok(ReportOutcomeRecord::Duplicate);
        }
        let row = sqlx::query(
            "SELECT e.decision_id, e.state, e.claim_token_id, e.lease_until, e.lapsed_token_id,
                 (SELECT p.state FROM comment_effects p WHERE p.decision_id = e.decision_id
                    AND p.seq = e.after_seq) AS after_state
             FROM comment_effects e WHERE e.id = ?",
        )
        .bind(effect.to_string())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(DbError::NotFound)?;
        let decision: String = row.try_get("decision_id")?;
        let holder: Option<String> = row.try_get("claim_token_id")?;
        let lapsed: Option<String> = row.try_get("lapsed_token_id")?;
        let state: EffectState = wire_col(&row, "state")?;
        let token_id = Some(token.0.as_str());
        if terminal(state) {
            return Ok(ReportOutcomeRecord::Ended);
        }
        if state != EffectState::Claimed || holder.as_deref() != token_id {
            return Ok(if lapsed.as_deref() == token_id {
                ReportOutcomeRecord::LeaseExpired
            } else {
                ReportOutcomeRecord::NotClaimer
            });
        }
        if opt_ts_col(&row, "lease_until")?.is_some_and(|until| until < now) {
            return Ok(ReportOutcomeRecord::LeaseExpired);
        }
        let after: Option<EffectState> = opt_wire_col(&row, "after_state")?;
        if report.outcome == ReportOutcome::Done
            && after.is_some_and(|after| after != EffectState::Done)
        {
            return Ok(ReportOutcomeRecord::Waiting);
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
        settle(&mut tx, &decision, false, now).await?;
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
            settle(&mut tx, &decision, false, now).await?;
            ids.push(super::super::jobs::parse_wire(&decision, "decision_id")?);
        }
        tx.commit().await?;
        Ok(ids)
    }

    /// Every claimable decision of every tenant (the set [`Self::claimable`] filters), oldest first.
    pub async fn open_feed(&self, now: Timestamp) -> Result<Vec<OpenFeedEntry>, DbError> {
        let rows = sqlx::query(
            "SELECT d.id, d.tenant_id, r.owner, r.name, p.number, e.kind FROM comment_decisions d
             JOIN comment_effects e ON e.decision_id = d.id
             JOIN comment_items i ON i.id = d.item_id
             JOIN pull_requests p ON p.id = i.pull_request_id JOIN repos r ON r.id = p.repo_id
             WHERE e.executor = 'agent' AND e.state IN ('waiting', 'open', 'claimed')
               AND NOT EXISTS (SELECT 1 FROM comment_decisions n WHERE n.supersedes = d.id)
               AND NOT EXISTS (SELECT 1 FROM comment_decision_undos u WHERE u.decision_id = d.id)
               AND NOT EXISTS (SELECT 1 FROM comment_effects c WHERE c.decision_id = d.id
                               AND c.state = 'claimed' AND c.lease_until >= ?)
             ORDER BY d.decided_at, d.rowid, e.seq",
        )
        .bind(ts(now))
        .fetch_all(self.pool)
        .await?;
        let mut entries: Vec<OpenFeedEntry> = Vec::new();
        for row in &rows {
            let decision: DecisionId = wire_col(row, "id")?;
            let kind: EffectKind = wire_col(row, "kind")?;
            match entries.last_mut() {
                Some(last) if last.decision == decision => {
                    if !last.kinds.contains(&kind) {
                        last.kinds.push(kind);
                    }
                }
                _ => entries.push(OpenFeedEntry {
                    tenant_id: row.try_get("tenant_id")?,
                    decision,
                    pr: PrRef {
                        repo: RepoRef {
                            owner: row.try_get("owner")?,
                            name: row.try_get("name")?,
                        },
                        number: u32_col(row, "number")?,
                    },
                    kinds: vec![kind],
                }),
            }
        }
        Ok(entries)
    }

    /// The tenant and pull request of a decision.
    pub async fn decision_owner(
        &self,
        decision: DecisionId,
    ) -> Result<Option<DecisionOwner>, DbError> {
        sqlx::query(
            "SELECT d.id, d.tenant_id, i.pull_request_id FROM comment_decisions d
             JOIN comment_items i ON i.id = d.item_id WHERE d.id = ?",
        )
        .bind(decision.to_string())
        .fetch_optional(self.pool)
        .await?
        .as_ref()
        .map(owner_from_row)
        .transpose()
    }

    /// The tenant, pull request and decision of an effect.
    pub async fn effect_owner(&self, effect: EffectId) -> Result<Option<DecisionOwner>, DbError> {
        sqlx::query(
            "SELECT d.id, d.tenant_id, i.pull_request_id FROM comment_effects e
             JOIN comment_decisions d ON d.id = e.decision_id
             JOIN comment_items i ON i.id = d.item_id WHERE e.id = ?",
        )
        .bind(effect.to_string())
        .fetch_optional(self.pool)
        .await?
        .as_ref()
        .map(owner_from_row)
        .transpose()
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

fn owner_from_row(row: &SqliteRow) -> Result<DecisionOwner, DbError> {
    Ok(DecisionOwner {
        tenant_id: row.try_get("tenant_id")?,
        pull_request_id: row.try_get("pull_request_id")?,
        decision: wire_col(row, "id")?,
    })
}

fn terminal(state: EffectState) -> bool {
    matches!(
        state,
        EffectState::Done | EffectState::Failed | EffectState::Cancelled
    )
}

/// Returns a decision's claimed effects (of `token`, or all) to `open`/`waiting`. Without `token` (lapsed leases) the
/// former holder is kept in `lapsed_token_id`, so its late report is told the lease ended.
async fn release(
    tx: &mut Transaction<'static, Sqlite>,
    decision: &str,
    token: Option<&str>,
    now: Timestamp,
) -> Result<(), DbError> {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE comment_effects AS e SET state = {RETURN_STATE},
             lapsed_token_id = CASE WHEN ?2 IS NULL THEN e.claim_token_id END,
             claim_token_id = NULL, lease_until = NULL, updated_at = ?1
         WHERE e.decision_id = ?3 AND e.state = 'claimed' AND (?2 IS NULL OR e.claim_token_id = ?2)"
    )))
    .bind(ts(now))
    .bind(token)
    .bind(decision)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Why `token` cannot claim the decision now, if it cannot.
async fn claim_refusal(
    tx: &mut Transaction<'static, Sqlite>,
    decision: DecisionId,
    token: &AccessTokenId,
    capabilities: &[EffectKind],
    now: Timestamp,
) -> Result<Option<ClaimOutcome>, DbError> {
    let ended: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM comment_decisions WHERE supersedes = ?1
                        UNION ALL SELECT 1 FROM comment_decision_undos WHERE decision_id = ?1)",
    )
    .bind(decision.to_string())
    .fetch_one(&mut **tx)
    .await?;
    let rows = sqlx::query(
        "SELECT kind, state, claim_token_id, lease_until FROM comment_effects
         WHERE decision_id = ? AND executor = 'agent' AND state IN ('waiting', 'open', 'claimed')",
    )
    .bind(decision.to_string())
    .fetch_all(&mut **tx)
    .await?;
    if ended || rows.is_empty() {
        return Ok(Some(ClaimOutcome::NothingOpen));
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
            return Ok(Some(ClaimOutcome::Taken));
        }
        let kind: String = row.try_get("kind")?;
        if !allowed.contains(&kind) {
            return Ok(Some(ClaimOutcome::CapabilityMissing));
        }
    }
    Ok(None)
}

/// Stores `decisions` inside `tx`; nothing when an item is no longer at the revision named.
async fn store(
    tx: &mut Transaction<'static, Sqlite>,
    tenant_id: &str,
    decisions: &[NewDecision],
    now: Timestamp,
) -> Result<DecideOutcome, DbError> {
    // Import visibility and Comments visits share the writer lock.
    let now = if decisions
        .iter()
        .any(|new| matches!(new.origin, Some(super::NewOrigin::Imported(_))))
    {
        Timestamp::now()
    } else {
        now
    };
    let mut changed = Vec::new();
    let mut items = Vec::new();
    for new in decisions {
        let item = item_in(tx, new.item).await?;
        if item.revision != new.item_revision {
            changed.push(item.no);
        }
        items.push(item);
    }
    if !changed.is_empty() {
        return Ok(DecideOutcome::ItemChanged(changed));
    }
    let mut ids = Vec::new();
    let mut superseded = Vec::new();
    for (new, item) in decisions.iter().zip(items) {
        let supersedes = current_id(tx, new.item).await?;
        if let Some(previous) = &supersedes {
            super::actions::cancel_unclaimed(tx, previous, now).await?;
            superseded.push((previous.clone(), &new.user_id));
        }
        let id = DecisionId::new_random();
        let lines = item.location.as_ref().and_then(|location| location.lines);
        sqlx::query(
            "INSERT INTO comment_decisions (id, tenant_id, item_id, user_id, kind, reason, target, override,
                 resolve_ticked, reply_text, comment, recommendation, via, carried_from, supersedes, head,
                 body_hash, start_line, end_line, decided_at, source_version)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                (SELECT s.body_hash FROM comment_items i JOIN comment_sources s ON s.id=i.source_id WHERE i.id=?))",
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
        .bind(wire(if new.origin.is_some() { DecisionVia::Question } else { new.via })?)
        .bind(new.carried_from.map(|id| id.to_string()))
        .bind(supersedes)
        .bind(&new.head.0)
        .bind(&item.body_hash)
        .bind(lines.map(|lines| lines.start_line))
        .bind(lines.map(|lines| lines.end_line))
        .bind(ts(now))
        .bind(new.item.to_string())
        .execute(&mut **tx)
        .await?;
        if let Some(origin) = &new.origin {
            super::origins::store(tx, id, origin, new).await?;
        }
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
            .execute(&mut **tx)
            .await?;
        }
        // The decision itself always changes the item: one revision step even when the state stays.
        let state = if matches!(new.origin, Some(super::NewOrigin::Imported(_))) {
            ItemState::Decided
        } else {
            item.state
        };
        set_state_in(tx, new.item, state, now).await?;
        settle(tx, &id.to_string(), true, now).await?;
        ids.push(id);
    }
    for (primary, user) in superseded {
        for (decision, item) in super::origins::followers(tx, &primary).await? {
            super::origins::undo(tx, tenant_id, user, &decision, item, now).await?;
        }
    }
    let mut stored = Vec::new();
    for id in ids {
        let mut found = decisions_where(tx, "d.id = ?", id.to_string()).await?;
        stored.push(found.pop().ok_or(DbError::NotFound)?.decision);
    }
    for decision in &stored {
        super::super::assessment_primary::capture_owner(tx, tenant_id, decision.id).await?;
    }
    Ok(DecideOutcome::Stored(stored))
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
