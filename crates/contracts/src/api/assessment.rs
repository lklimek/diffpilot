//! Settings and read-only paired comparison HTTP contracts; routes are implemented by W5/W6.

use crate::assessment::{
    AssessmentComparisonItem, AssessmentConfig, AssessmentMethodRun, AssessmentPass,
    AssessmentQuota,
};
use crate::time::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// GET /settings/comment-assessment; PUT uses AssessmentSettingsUpdate and never schedules work itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AssessmentSettings {
    /// Opaque content revision required for an atomic conditional write.
    pub revision: String,
    /// Effective per-user configuration.
    pub config: AssessmentConfig,
    /// Safe remaining/reset status.
    pub quota: AssessmentQuota,
}

/// Conditional settings write; stale revisions fail with HTTP 409 and never schedule work.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct AssessmentSettingsUpdate {
    /// Configuration the caller wants to save.
    pub config: AssessmentConfig,
    /// Revision obtained with the configuration the caller edited.
    pub revision: String,
}

/// GET …/comments/assessment-comparison, scoped to payer and PR.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AssessmentComparison {
    /// Exact comparison counts with explicit denominators.
    #[serde(default)]
    pub summary: ComparisonSummary,
    /// Frozen logical pass.
    pub pass: AssessmentPass,
    /// Owner revisions are selected as of this cutoff.
    pub cutoff: Timestamp,
    /// Independent method state and metering.
    pub runs: Vec<AssessmentMethodRun>,
    /// Includes unmatched, missing, duplicate and failed rows explicitly.
    pub items: Vec<AssessmentComparisonItem>,
}

/// Counts exclude duplicates, mismatched heads and unaligned secondary points from scoring.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ComparisonSummary {
    /// Rows whose two available normalized verdicts can be compared.
    pub verdict_compared: u32,
    /// Different normalized verdicts among verdict_compared.
    pub verdict_disagreements: u32,
    /// Primary points waiting for an actual owner choice.
    pub pending_owner: u32,
    /// Per-method coverage and exact proposal-kind/target agreement.
    pub methods: Vec<MethodComparisonScore>,
}

/// Explicit per-method denominators; native verdict and proposal remain separately nullable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct MethodComparisonScore {
    /// Assessment adapter.
    pub method: crate::assessment::AssessmentMethod,
    /// Method rows with an available normalized verdict.
    pub assessed: u32,
    /// Unaligned points retained in the report.
    pub unmatched: u32,
    /// Expected but absent rows, including failed methods.
    pub missing: u32,
    /// Available rows without a normalized verdict.
    pub missing_verdict: u32,
    /// Available rows without a machine proposal.
    pub missing_proposal: u32,
    /// Eligible actual owner choices, whether a proposal is present or not.
    pub owner_decided: u32,
    /// Exact kind and target matches among owner_decided.
    pub owner_agreed: u32,
    /// Whether this method failed or was cancelled.
    pub failed: bool,
}

impl AssessmentComparison {
    /// Recompute the summary from the read-only rows at this report's cutoff.
    pub fn summarize(&mut self) {
        use crate::assessment::{AssessmentRunStatus as Run, ComparisonItemStatus as State};
        let mut summary = ComparisonSummary::default();
        let primary = self.pass.config.primary_method;
        for row in self
            .items
            .iter()
            .filter(|r| r.run.method == primary && r.status == State::Matched)
        {
            if row.owner_decision.is_none() {
                summary.pending_owner += 1;
            }
            if let Some(other) = self.items.iter().find(|r| {
                r.run.method != primary
                    && r.comparison_key == row.comparison_key
                    && crate::assessment::sources_align(
                        &r.source,
                        &row.source,
                        r.primary_item_id.is_some()
                            && r.primary_item_id == row.primary_item_id
                            && r.primary_item_hash == row.primary_item_hash,
                    )
                    && r.status == State::Matched
            }) && let (Some(a), Some(b)) = (row.verdict, other.verdict)
            {
                summary.verdict_compared += 1;
                summary.verdict_disagreements += u32::from(a != b);
            }
        }
        for run in &self.runs {
            let mut score = MethodComparisonScore {
                method: run.reference.method,
                assessed: 0,
                unmatched: 0,
                missing: 0,
                missing_verdict: 0,
                missing_proposal: 0,
                owner_decided: 0,
                owner_agreed: 0,
                failed: matches!(run.status, Run::Failed | Run::Cancelled),
            };
            for row in self.items.iter().filter(|r| r.run == run.reference) {
                match row.status {
                    State::Duplicate | State::HeadMismatch | State::Context => continue,
                    State::MissingRow | State::MethodFailed => {
                        score.missing += 1;
                        continue;
                    }
                    State::Unmatched => {
                        score.unmatched += 1;
                        continue;
                    }
                    State::Matched => {}
                }
                score.assessed += u32::from(row.verdict.is_some());
                score.missing_verdict += u32::from(row.verdict.is_none());
                score.missing_proposal += u32::from(row.proposal.is_none());
                if let Some(owner) = &row.owner_decision {
                    score.owner_decided += 1;
                    score.owner_agreed += u32::from(
                        row.proposal
                            .as_ref()
                            .is_some_and(|p| proposal_agrees(p, owner)),
                    );
                }
            }
            summary.methods.push(score);
        }
        self.summary = summary;
    }
}

fn proposal_agrees(
    proposal: &crate::agent::comment_report::ProposedDecision,
    owner: &crate::assessment::AssessmentOwnerDecision,
) -> bool {
    use crate::api::review_loop::{DecisionTarget as Target, ReviewDecisionKind as Kind};
    let kind = serde_json::to_value(proposal.kind).ok();
    if kind != serde_json::to_value(owner.kind).ok() {
        return false;
    }
    match (&owner.target, owner.kind) {
        (Some(Target::ExistingPr { number }), Kind::HandledInPr) => {
            proposal.target_pr == Some(*number) && proposal.target_url.is_none()
        }
        (Some(Target::FollowUpPr { title }), Kind::FollowUpPr) => {
            &proposal.title == title
                && proposal.target_pr.is_none()
                && proposal.target_url.is_none()
        }
        (Some(Target::UpstreamIssue { url }), Kind::UpstreamIssue) => &proposal.target_url == url,
        (Some(Target::Todo { title }), Kind::Todo) => proposal.title.as_ref() == Some(title),
        (None, _) => {
            proposal.target_pr.is_none()
                && proposal.target_url.is_none()
                && proposal.title.is_none()
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::comment_report::{ProposedDecision, ProposedDecisionKind};
    use crate::api::review_loop::{DecisionTarget, ReviewDecisionKind};
    use crate::assessment::AssessmentOwnerDecision;
    use crate::ids::{DecisionId, UserId};

    #[test]
    fn comparison_agreement_uses_exact_kind_and_target() {
        let mut owner = AssessmentOwnerDecision {
            id: DecisionId::new_random(),
            revision: 1,
            owner: UserId("owner".into()),
            at: Timestamp::UNIX_EPOCH,
            kind: ReviewDecisionKind::FixHere,
            target: None,
        };
        let mut proposal = ProposedDecision {
            kind: ProposedDecisionKind::Resolve,
            fixed_in: Some("later".into()),
            target_pr: None,
            target_url: None,
            title: None,
        };
        assert!(!proposal_agrees(&proposal, &owner));
        proposal.kind = ProposedDecisionKind::FixHere;
        assert!(proposal_agrees(&proposal, &owner));
        owner.kind = ReviewDecisionKind::HandledInPr;
        owner.target = Some(DecisionTarget::ExistingPr { number: 12 });
        proposal.kind = ProposedDecisionKind::HandledInPr;
        proposal.target_pr = Some(13);
        assert!(!proposal_agrees(&proposal, &owner));
        proposal.target_pr = Some(12);
        assert!(proposal_agrees(&proposal, &owner));
    }
}
