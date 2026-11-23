//! Owned read-side DTOs for claudius 4.2 host-supplied reports; no upstream schema is vendored.

use crate::api::review_loop::FixSize;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

/// Host report envelope; rows are decoded individually so a malformed row can be dropped.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CommentReport {
    /// Accepted versions are 4.2.x.
    pub schema_version: String,
    /// Must be comment_check.
    pub report_type: String,
    /// Host provenance, matched against the server's input.
    pub metadata: ReportMetadata,
    /// Independently parsed assessment rows; unknown report fields are ignored.
    #[ts(type = "unknown[]")]
    pub comment_assessments: Vec<Value>,
}

impl CommentReport {
    /// Whether the envelope uses the supported report version and host-supplied mode.
    pub fn supported(&self) -> bool {
        self.schema_version
            .strip_prefix("4.2.")
            .is_some_and(|patch| !patch.is_empty() && patch.bytes().all(|b| b.is_ascii_digit()))
            && self.report_type == "comment_check"
            && self.metadata.input_mode == "host_supplied"
    }
}

/// Required host-supplied provenance; other metadata is ignored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ReportMetadata {
    /// Host-supplied is the only accepted mode.
    pub input_mode: String,
    /// GitHub PR number.
    pub pr_number: u32,
    /// Commit assessed; a differing head is labelled rather than discarded.
    pub commit: String,
}

/// A row's source role in the upstream report.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// An inline review thread.
    Inline,
    /// A point in a submitted review body.
    ReviewBody,
    /// A point in a PR comment.
    PrComment,
    /// Reply context only.
    Reply,
    /// Status context only.
    Status,
}

/// The upstream assessment, before mapping into the API's validity and necessity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ReportAssessment {
    /// Valid at the assessed commit.
    Valid,
    /// Already fixed at that commit.
    AlreadyFixed,
    /// Valid concern, outside this PR's scope.
    OutOfScope,
    /// Invalid concern.
    Wrong,
}

/// Proposed decision kind; independent of PR-level decisions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ProposedDecisionKind {
    /// Resolve as fixed.
    Resolve,
    /// Fix in this PR.
    FixHere,
    /// Reject with a reason.
    Reject,
    /// Existing PR handles it.
    HandledInPr,
    /// Follow-up PR handles it.
    FollowUpPr,
    /// Upstream issue handles it.
    UpstreamIssue,
    /// Record a TODO.
    Todo,
}

/// Machine-readable proposed action; target semantics are validated by the server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ProposedDecision {
    /// Decision kind.
    pub kind: ProposedDecisionKind,
    /// Fix commit, if supplied.
    pub fixed_in: Option<String>,
    /// Target PR number.
    pub target_pr: Option<u32>,
    /// Target issue or PR URL.
    pub target_url: Option<String>,
    /// Follow-up title.
    pub title: Option<String>,
}

/// Recorded decision kind, including deferral and withdrawal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum RecordedDecisionKind {
    /// Maintainer deferred this point.
    Deferred,
    /// Source author withdrew this point.
    Withdrawn,
    /// Ordinary item decision.
    #[serde(untagged)]
    Decision(ProposedDecisionKind),
}

/// Attributed prior decision; server verifies authority and grounding before treating it as a fact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct RecordedDecision {
    /// Recorded decision.
    pub kind: RecordedDecisionKind,
    /// GitHub author login.
    pub by: String,
    /// Link to the decision's comment.
    pub comment_url: String,
    /// Verbatim decision quote, at most 600 characters.
    pub quote: String,
    /// PR target, when supplied.
    pub target_pr: Option<u32>,
    /// URL target, when supplied.
    pub target_url: Option<String>,
}

/// An upstream gate; only the four blocker gates map into stored gate items.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ReportGate {
    /// Upstream gate name, allowing gates beyond the four stored ones.
    pub gate: String,
    /// Gate evidence, labelled LLM.
    pub evidence: String,
}

/// Fields consumed from one upstream assessment; unknown fields, including severity floats, are discarded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ReportRow {
    /// Upstream CMT identifier.
    pub id: String,
    /// Source author.
    pub reviewer: String,
    /// Source role.
    pub source_kind: SourceKind,
    /// Source URL, checked against the host scope.
    pub comment_url: String,
    /// Required for an inline row.
    pub thread_id: Option<String>,
    /// Required for a body or PR-comment point; server checks a distinct verbatim span.
    pub source_quote: Option<String>,
    /// Problem description.
    pub description: String,
    /// Validity assessment; absent on context rows.
    pub assessment: Option<ReportAssessment>,
    /// Reasoning text.
    pub reasoning: String,
    /// Evidence text, labelled LLM.
    pub evidence: Option<String>,
    /// Human-readable proposed action.
    pub proposed_action: String,
    /// Machine decision, required on assessed host rows.
    pub proposed_decision: Option<ProposedDecision>,
    /// Earlier attributed decision.
    pub recorded_decision: Option<RecordedDecision>,
    /// Optional location in file:lines notation.
    pub location: Option<String>,
    /// Draft reply, passed through the server's wording guard.
    pub proposed_reply: Option<String>,
    /// Whether the problem exists on the base.
    pub pre_existing: Option<bool>,
    /// Category slug.
    pub category: Option<String>,
    /// Fix size.
    pub fix_size: Option<FixSize>,
    /// Named gates.
    #[serde(default)]
    pub gates: Vec<ReportGate>,
    /// Threads grouped with this row, intersected with host scope.
    #[serde(default)]
    pub merged_thread_ids: Vec<String>,
    /// Additional source links, subject to the same-source and author rules.
    #[serde(default)]
    pub merged_urls: Vec<String>,
}
