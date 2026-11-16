//! Output of the `pr_context` job: the problem, related work and fit questions.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::common::{Evidence, FitTopic, RelatedWorkKind};

/// The pull request at project scale.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PrContextOutput {
    /// The problem the pull request solves.
    pub problem: String,
    /// Why the change is needed.
    pub why_needed: String,
    /// What the change gives users or developers.
    pub what_it_gives: String,
    /// Related pull requests, issues or mechanisms; may be empty.
    pub related_work: Vec<RelatedWork>,
    /// Questions about the fit of the change, never verdicts.
    pub fit_questions: Vec<FitQuestion>,
}

/// One piece of related work.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct RelatedWork {
    /// URL or `owner/repo#number` of the related item.
    pub reference: String,
    /// Kind of relation.
    pub kind: RelatedWorkKind,
    /// One line on the overlap with this pull request.
    pub overlap: String,
}

/// A question about the fit of the change at project scale.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct FitQuestion {
    /// The question, phrased as a question.
    pub question: String,
    /// What the question is about.
    pub topic: FitTopic,
    /// Lines read that motivate the question.
    pub evidence: Vec<Evidence>,
}
