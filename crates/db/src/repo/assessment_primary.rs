//! Primary-only projection under the method result's attempt fence.

use super::review_loop;
use crate::error::DbError;
use contracts::api::review_loop::{CommentAssessment, ItemState};
use contracts::assessment::{AssessmentComparisonItem, AssessmentPass};
use contracts::ids::CommentItemId;
use sqlx::{Sqlite, Transaction};

/// A grounded primary row; secondary adapters never submit these writes.
#[derive(Clone, Debug)]
pub struct PrimaryPoint {
    /// A snapshot-bound standing decision and whether its point must be asked again.
    pub carry: Option<PrimaryCarry>,
    /// Common comparison key.
    pub key: String,
    /// Existing item and its frozen text hash; absent for a new body point.
    pub existing: Option<(CommentItemId, String)>,
    /// New body point, never an invented inline thread.
    pub observed: Option<review_loop::ObservedItem>,
    /// Requested CMT number, used only when it is still free.
    pub preferred_no: u32,
    /// Frozen source URL, SHA-256 and verbatim quote.
    pub source: Option<(String, String, String)>,
    /// Direct canonical comparison key.
    pub duplicate: Option<String>,
    /// Validated card; absent for duplicates or merged-only thread members.
    pub assessment: Option<CommentAssessment>,
    /// Frozen normalized point/code fingerprint.
    pub fingerprint: String,
}

/// Restatement provenance; no new decision or effect is created by carrying.
#[derive(Clone, Debug)]
pub struct PrimaryCarry {
    /// Actual frozen diffpilot decision ID.
    pub decision: contracts::ids::DecisionId,
    /// Reopen only for changed code or an authoritative contradictory decision.
    pub reason: Option<contracts::api::review_loop::ReaskReason>,
    /// Grounded link and quote of the restatement.
    pub evidence: contracts::api::review_loop::CarryEvidence,
}

pub(super) async fn apply(
    tx: &mut Transaction<'static, Sqlite>,
    tenant: &str,
    pass: &AssessmentPass,
    points: &[PrimaryPoint],
    rows: &mut [AssessmentComparisonItem],
) -> Result<(), DbError> {
    let mut applied = std::collections::HashMap::new();
    for point in points
        .iter()
        .filter(|p| p.duplicate.is_none())
        .chain(points.iter().filter(|p| p.duplicate.is_some()))
    {
        let id = if let Some((id, hash)) = &point.existing {
            let current: Option<String> = sqlx::query_scalar("SELECT body_hash FROM comment_items WHERE id=? AND tenant_id=? AND pull_request_id=?")
                .bind(id.to_string()).bind(tenant).bind(pass.input.pull_request_id).fetch_optional(&mut **tx).await?;
            if current.as_ref() != Some(hash) {
                continue;
            }
            *id
        } else if let Some(observed) = &point.observed {
            if observed.head != pass.input.head
                || observed.kind == contracts::api::review_loop::CommentItemKind::Thread
            {
                return Err(DbError::Conflict);
            }
            sqlx::query("UPDATE comment_prs SET next_item_no=max(next_item_no,?) WHERE pull_request_id=? AND NOT EXISTS(SELECT 1 FROM comment_items WHERE pull_request_id=? AND item_no=?)")
                .bind(point.preferred_no).bind(pass.input.pull_request_id).bind(pass.input.pull_request_id).bind(point.preferred_no).execute(&mut **tx).await?;
            match review_loop::upsert_item_in(
                tx,
                tenant,
                pass.input.pull_request_id,
                observed,
                ItemState::Undecided,
                pass.admitted_at,
            )
            .await?
            {
                review_loop::UpsertOutcome::New(i)
                | review_loop::UpsertOutcome::Changed(i)
                | review_loop::UpsertOutcome::Unchanged(i) => i.id,
            }
        } else {
            continue;
        };
        if point.existing.is_none() {
            sqlx::query("UPDATE comment_items SET item_no=? WHERE id=? AND NOT EXISTS(SELECT 1 FROM comment_items WHERE pull_request_id=? AND item_no=?)")
                .bind(point.preferred_no).bind(id.to_string()).bind(pass.input.pull_request_id).bind(point.preferred_no).execute(&mut **tx).await?;
        }
        let duplicate = point
            .duplicate
            .as_ref()
            .and_then(|key| applied.get(key))
            .copied();
        if point.duplicate.is_some() && duplicate.is_none() {
            return Err(DbError::Conflict);
        }
        if let Some((url, version, quote)) = &point.source {
            let source: i64 = sqlx::query_scalar("SELECT id FROM comment_sources WHERE tenant_id=? AND pull_request_id=? AND url=? AND body_hash=?")
                .bind(tenant).bind(pass.input.pull_request_id).bind(url).bind(version).fetch_one(&mut **tx).await?;
            sqlx::query("UPDATE comment_items SET source_id=?,quote=?,duplicate_of=? WHERE id=?")
                .bind(source)
                .bind(quote)
                .bind(duplicate.map(|id: CommentItemId| id.to_string()))
                .bind(id.to_string())
                .execute(&mut **tx)
                .await?;
        } else if duplicate.is_some() {
            sqlx::query("UPDATE comment_items SET duplicate_of=? WHERE id=?")
                .bind(duplicate.map(|id| id.to_string()))
                .bind(id.to_string())
                .execute(&mut **tx)
                .await?;
        }
        if let Some(assessment) = &point.assessment {
            if assessment.head != pass.input.head {
                return Err(DbError::Conflict);
            }
            review_loop::add_assessment_in(tx, tenant, id, assessment, &point.fingerprint).await?;
        }
        let hash: String = sqlx::query_scalar("SELECT body_hash FROM comment_items WHERE id=?")
            .bind(id.to_string())
            .fetch_one(&mut **tx)
            .await?;
        if let Some(row) = rows.iter_mut().find(|row| row.comparison_key == point.key) {
            row.primary_item_id = Some(id);
            row.primary_item_hash = Some(hash);
        }
        applied.insert(point.key.clone(), id);
        if let Some(carry) = &point.carry {
            let unchanged:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM comment_decisions d WHERE d.id=? AND d.item_id=? AND NOT EXISTS(SELECT 1 FROM comment_decision_undos u WHERE u.decision_id=d.id) AND NOT EXISTS(SELECT 1 FROM comment_decisions n WHERE n.supersedes=d.id))")
                .bind(carry.decision.to_string()).bind(id.to_string()).fetch_one(&mut **tx).await?;
            if unchanged {
                sqlx::query("INSERT INTO comment_carry_events(id,tenant_id,pull_request_id,item_id,source_decision_id,kind,reason,evidence,at) VALUES(?,?,?,?,?,?,?,?,?)")
                    .bind(contracts::ids::CarryEventId::new_random().to_string()).bind(tenant).bind(pass.input.pull_request_id).bind(id.to_string()).bind(carry.decision.to_string())
                    .bind(if carry.reason.is_some(){"asked_again"}else{"carried"}).bind(carry.reason.map(super::jobs::wire_name).transpose()?).bind(serde_json::to_string(&carry.evidence)?).bind(super::jobs::ts(pass.admitted_at)).execute(&mut **tx).await?;
                if carry.reason.is_some() {
                    review_loop::cancel_unclaimed_in(
                        tx,
                        &carry.decision.to_string(),
                        pass.admitted_at,
                    )
                    .await?;
                    sqlx::query(
                        "UPDATE comment_items SET state='reopened',revision=revision+1 WHERE id=?",
                    )
                    .bind(id.to_string())
                    .execute(&mut **tx)
                    .await?;
                }
            }
        }
    }
    Ok(())
}

/// Capture every actual revision without coupling model evidence to owner truth.
pub(super) async fn capture_owner(
    tx: &mut Transaction<'static, Sqlite>,
    tenant: &str,
    id: contracts::ids::DecisionId,
) -> Result<(), DbError> {
    sqlx::query("WITH RECURSIVE history(id,supersedes) AS (SELECT id,supersedes FROM comment_decisions WHERE id=? UNION SELECT d.id,d.supersedes FROM comment_decisions d JOIN history h ON d.id=h.supersedes)
        INSERT INTO assessment_owner_decisions(pass_id,method,comparison_key,tenant_id,payer_id,primary_item_id,decision_id,revision,owner_id,decided_at,kind,target)
        SELECT p.id,p.primary_method,c.comparison_key,p.tenant_id,p.payer_id,c.primary_item_id,d.id,(SELECT count(*) FROM history),d.user_id,d.decided_at,d.kind,d.target
        FROM comment_decisions d JOIN assessment_comparison_items c ON c.primary_item_id=d.item_id AND c.tenant_id=d.tenant_id
        JOIN assessment_passes p ON p.id=c.pass_id AND p.primary_method=c.method JOIN memberships m ON m.tenant_id=d.tenant_id AND m.user_id=d.user_id
        WHERE d.id=? AND d.tenant_id=? AND d.via<>'carried' AND c.status='matched' AND d.head=p.head AND d.body_hash=c.primary_item_hash
        AND (json_extract(c.source,'$.kind')='inline' OR d.source_version=json_extract(c.source,'$.version'))
        ON CONFLICT(pass_id,comparison_key,decision_id) DO NOTHING")
        .bind(id.to_string()).bind(id.to_string()).bind(tenant).execute(&mut **tx).await?;
    Ok(())
}
