//! Output of the `verify_comments` job (ADR-023, plan §2.2, §4.2): per item at the PR head the LLM assessment, the
//! recommendation and the code context, and once per pass the PR's purpose. Read-only; re-validated by the server.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::api::review_loop::{
    AssessmentEvidence, CodeSummary, DecisionKind, Elsewhere, FixSize, Gate, NecessityVerdict,
    Validity,
};

/// One pass over the items of a pull request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct VerifyCommentsOutput {
    /// The pull request's purpose in one sentence.
    pub purpose: String,
    /// One assessment per item given in the prompt.
    pub items: Vec<ItemAssessment>,
    /// The answer to the owner's question in a question pass; null otherwise.
    pub answer: Option<String>,
}

/// The assessment of one item at the head.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ItemAssessment {
    /// The item's id from the prompt, e.g. `C14`.
    pub item: String,
    /// Whether the comment is right about the code at the head.
    pub validity: Validity,
    /// Lines read at the head that support the validity.
    pub evidence: Vec<AssessmentEvidence>,
    /// The problem exists on the base branch already.
    pub pre_existing: bool,
    /// Category slug from the catalog in the prompt.
    pub category: String,
    /// Whether the fix belongs in this pull request.
    pub necessity: NecessityVerdict,
    /// Evidence for `in`, or the reason for `out`.
    pub necessity_reason: String,
    /// A better-fitting pull request for `out` (`owner/repo#n`); null otherwise.
    pub better_pr: Option<String>,
    /// Blocker gates the comment's problem trips; empty when none.
    pub gates: Vec<Gate>,
    /// Open pull requests, issues or upstream work that already handle it.
    pub elsewhere: Vec<Elsewhere>,
    /// How a fix would look; null when no fix is recommended.
    pub fix_sketch: Option<String>,
    /// Size of that fix; null when no fix is recommended.
    pub fix_size: Option<FixSize>,
    /// Reply drafts in English that say "per maintainer decision".
    pub reply_drafts: Vec<String>,
    /// The recommended decision.
    pub recommendation: RecommendationOutput,
    /// The enclosing function or block of the discussed lines and what it does; null without lines.
    pub code: Option<CodeSummary>,
}

/// A recommended thread decision (D1–D7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum RecommendedDecision {
    /// Already fixed: resolve.
    Resolve,
    /// Fix in this pull request.
    FixHere,
    /// Reject, with a reason.
    Reject,
    /// Handled in an existing pull request (`target_pr`).
    HandledInPr,
    /// Create a follow-up pull request (optional `title`).
    FollowUpPr,
    /// Upstream issue (`target_url`, or null to create one).
    UpstreamIssue,
    /// Out of scope: TODO (`title`).
    Todo,
}

impl From<RecommendedDecision> for DecisionKind {
    fn from(value: RecommendedDecision) -> Self {
        match value {
            RecommendedDecision::Resolve => Self::Resolve,
            RecommendedDecision::FixHere => Self::FixHere,
            RecommendedDecision::Reject => Self::Reject,
            RecommendedDecision::HandledInPr => Self::HandledInPr,
            RecommendedDecision::FollowUpPr => Self::FollowUpPr,
            RecommendedDecision::UpstreamIssue => Self::UpstreamIssue,
            RecommendedDecision::Todo => Self::Todo,
        }
    }
}

/// The recommendation with the target its decision needs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct RecommendationOutput {
    /// Decision.
    pub kind: RecommendedDecision,
    /// Why, in one or two sentences.
    pub reason: String,
    /// A short class of the reason (a few words) shared by items with the same reason.
    pub reason_class: String,
    /// Existing pull request number for `handled_in_pr`; null otherwise.
    pub target_pr: Option<u32>,
    /// Issue URL for `upstream_issue`; null otherwise or to create one.
    pub target_url: Option<String>,
    /// Title for `follow_up_pr` or `todo`; null otherwise.
    pub title: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::strict;

    #[test]
    fn output_schema_is_in_the_strict_subset() {
        let schema = strict::output_schema_for::<VerifyCommentsOutput>().expect("strict subset");
        let required = schema["required"].as_array().expect("required");
        assert_eq!(required.len(), 3);
    }

    /// A recommendation is stored as the decision of the same wire name.
    #[test]
    fn recommended_decisions_keep_their_wire_name() {
        for recommended in [
            RecommendedDecision::Resolve,
            RecommendedDecision::FixHere,
            RecommendedDecision::Reject,
            RecommendedDecision::HandledInPr,
            RecommendedDecision::FollowUpPr,
            RecommendedDecision::UpstreamIssue,
            RecommendedDecision::Todo,
        ] {
            assert_eq!(
                serde_json::to_value(recommended).expect("serialises"),
                serde_json::to_value(DecisionKind::from(recommended)).expect("serialises")
            );
        }
    }
}
