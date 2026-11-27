//! Frozen assessment passes, independent method results and owner-decision comparison.

use crate::api::review_loop::{DecisionTarget, ReviewDecisionKind};
use crate::ids::{CommentItemId, CommitSha, DecisionId, JobId, PassId, UserId};
use crate::job::result::Usage;
use crate::job::spec::ModelChoice;
use crate::time::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// An assessment adapter; native outputs retain their own validation contracts.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case")]
pub enum AssessmentMethod {
    /// Host-supplied claudius 4.2 report.
    #[default]
    ClaudiusHost,
    /// Existing strict prompt v4 output.
    PromptV4,
}

impl AssessmentMethod {
    /// The other adapter for a paired pass.
    pub const fn other(self) -> Self {
        match self {
            Self::ClaudiusHost => Self::PromptV4,
            Self::PromptV4 => Self::ClaudiusHost,
        }
    }
}

/// Per-user settings; changing them alone never schedules work.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct AssessmentConfig {
    /// Only this method can change visible items or effects.
    #[serde(default)]
    pub primary_method: AssessmentMethod,
    /// Explicit owner/repo#number selections, empty by default.
    #[serde(default)]
    pub ab_prs: Vec<String>,
    /// Optional UTC deadline, absent by default.
    #[serde(default)]
    pub ab_until: Option<Timestamp>,
}

impl AssessmentConfig {
    /// Freeze the pair selection at admission, with an exclusive deadline.
    pub fn paired(&self, pr: &str, now: Timestamp) -> bool {
        self.ab_prs
            .iter()
            .any(|selected| selected.eq_ignore_ascii_case(pr))
            || self.ab_until.is_some_and(|until| now < until)
    }
}

/// Durable method identity carried by both job input and result; attempt is the enclosing job's fence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AssessmentRunRef {
    /// Logical pass shared by an A/B pair and all its retries.
    pub pass_id: PassId,
    /// Independent adapter.
    pub method: AssessmentMethod,
}

/// Non-secret immutable snapshot reference; includes source versions, scope and standing decisions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AssessmentInput {
    /// owner/repo#number.
    pub pr: String,
    /// Stable PR database key.
    #[ts(type = "number")]
    pub pull_request_id: i64,
    /// Assessed head.
    pub head: CommitSha,
    /// Base used by both adapters.
    pub base: CommitSha,
    /// SHA-256 of the frozen input, including source versions and standing decisions.
    pub input_hash: String,
    /// SHA-256 of the frozen scope.
    pub scope_hash: String,
    /// Immutable host-owned non-secret snapshot reference, never model-supplied.
    pub snapshot_ref: String,
    /// Identical model and effort for both adapters.
    pub model: ModelChoice,
}

/// One admitted logical pass; retries keep its original day and configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AssessmentPass {
    /// Pass identity.
    pub id: PassId,
    /// Payer and quota owner.
    pub payer: UserId,
    /// Frozen input.
    pub input: AssessmentInput,
    /// Frozen user configuration.
    pub config: AssessmentConfig,
    /// Whether both adapters are scheduled.
    pub paired: bool,
    /// Original UTC admission day, YYYY-MM-DD.
    pub day_utc: String,
    /// Original admission time.
    pub admitted_at: Timestamp,
}

/// Safe quota status; unrelated jobs are unaffected.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AssessmentQuota {
    /// Logical passes still available today, out of 80.
    pub remaining: u32,
    /// Next UTC midnight.
    pub resets_at: Timestamp,
}

/// Pre-admission rejection consumes no slot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS, thiserror::Error)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum AssessmentAdmissionFailure {
    /// Daily logical-pass cap reached.
    #[error("the daily assessment limit has been reached until {resets_at}")]
    AssessmentDailyLimit {
        /// Next UTC midnight.
        resets_at: Timestamp,
    },
    /// No linked, unpaused credential is available.
    #[error("a usable linked credential is required")]
    CredentialMissing,
}

/// Method execution status, independent of per-row completeness.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum AssessmentRunStatus {
    /// Waiting for the container.
    Queued,
    /// An attempt is running.
    Running,
    /// Accepted result.
    Succeeded,
    /// Failed method; the other method is not promoted.
    Failed,
    /// Cancelled method.
    Cancelled,
}

/// Persisted method summary; usage and duration include failed attempts once each.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AssessmentMethodRun {
    /// False when any started attempt has unavailable metering; usage then reports only known totals.
    pub usage_complete: bool,
    /// Commit actually reported by the adapter; a mismatch stays comparison-only.
    pub reported_head: Option<CommitSha>,
    /// Frozen method identity.
    pub reference: AssessmentRunRef,
    /// One job whose attempts execute this method.
    pub job_id: JobId,
    /// Version of the adapter.
    pub version: String,
    /// Current state.
    pub status: AssessmentRunStatus,
    /// Current attempt fence.
    pub attempt: u32,
    /// The only accepted successful attempt, if any.
    pub accepted_attempt: Option<u32>,
    /// Billable usage from all attempts, deduplicated by job/attempt.
    pub usage: Usage,
    /// Total duration in milliseconds across attempts.
    #[ts(type = "number")]
    pub duration_ms: u64,
    /// Scrubbed failure kind, never upstream response text.
    pub failure: Option<crate::job::result::JobFailureKind>,
}

/// A nullable common verdict never inferred from prose.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum AssessmentVerdict {
    /// Valid at the frozen head.
    Valid,
    /// Invalid concern.
    Invalid,
    /// Already fixed at the frozen head.
    AlreadyFixed,
    /// Outside the PR's scope.
    OutOfScope,
}

/// Distinct completeness/alignment states; null verdict and proposal remain independent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ComparisonItemStatus {
    /// Attributed reply/status evidence; never a scored point or a source-consumption marker.
    Context,
    /// Row aligned to a common identity.
    Matched,
    /// Expected row absent.
    MissingRow,
    /// Direct duplicate of a canonical comparison key.
    Duplicate,
    /// No unambiguous common identity.
    Unmatched,
    /// Method failed, so the row is unavailable.
    MethodFailed,
    /// Report head differs from the frozen head; excluded from scoring.
    HeadMismatch,
}

/// Frozen source identity; quote and version are required for body-point alignment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AssessmentSource {
    /// Inline thread identity.
    Inline {
        /// GitHub node ID.
        thread_id: String,
        /// Frozen thread body/version hash, preventing decision reuse after edits.
        version: String,
    },
    /// Point grounded in one source version.
    Body {
        /// Original report source kind.
        source_kind: crate::api::review_loop::CommentSourceKind,
        /// Source URL.
        url: String,
        /// Frozen body SHA-256.
        version: String,
        /// Verbatim grounded quote.
        quote: String,
    },
    /// A standalone agent comment supplied by DiffPilot.
    Api {
        /// Item assigned to this submission.
        item: CommentItemId,
        /// Frozen comment body hash.
        version: String,
    },
}

/// Pair only the same frozen source, with a known point identity or an exact body quote.
pub fn sources_align(left: &AssessmentSource, right: &AssessmentSource, known_item: bool) -> bool {
    match (left, right) {
        (
            AssessmentSource::Inline {
                thread_id: a,
                version: av,
            },
            AssessmentSource::Inline {
                thread_id: b,
                version: bv,
            },
        ) => a == b && av == bv,
        (
            AssessmentSource::Body {
                source_kind: a,
                url: au,
                version: av,
                quote: aq,
            },
            AssessmentSource::Body {
                source_kind: b,
                url: bu,
                version: bv,
                quote: bq,
            },
        ) => a == b && au == bu && av == bv && (known_item || aq == bq),
        (
            AssessmentSource::Api {
                item: a,
                version: av,
            },
            AssessmentSource::Api {
                item: b,
                version: bv,
            },
        ) => a == b && av == bv,
        _ => false,
    }
}

/// Common method-local comparison row; secondary IDs never allocate primary items.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AssessmentComparisonItem {
    /// Model-reported reply-chain evidence, separate from actual diffpilot owner truth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub recorded_decision: Option<crate::agent::comment_report::RecordedDecision>,
    /// Stable comparison identity inside this pass.
    pub comparison_key: String,
    /// Adapter-local ID, distinct from a primary item ID.
    pub local_id: String,
    /// Adapter and pass.
    pub run: AssessmentRunRef,
    /// Adapter version.
    pub method_version: String,
    /// Frozen source identity.
    pub source: AssessmentSource,
    /// Existing primary item, only for an unambiguous alignment.
    pub primary_item_id: Option<CommentItemId>,
    /// Frozen normalized primary point hash, distinct from the full source version.
    pub primary_item_hash: Option<String>,
    /// Alignment/completeness state.
    pub status: ComparisonItemStatus,
    /// Canonical comparison key for a duplicate.
    pub duplicate_of: Option<String>,
    /// Normalized verdict, absent when unavailable.
    pub verdict: Option<AssessmentVerdict>,
    /// Native verdict preserved exactly.
    pub native_verdict: Option<String>,
    /// Machine proposal; absence is distinct from a missing row or verdict.
    pub proposal: Option<crate::agent::comment_report::ProposedDecision>,
    /// Actual diffpilot owner decision reference as of the report cutoff.
    pub owner_decision: Option<AssessmentOwnerDecision>,
}

/// Actual owner choice, separate from inferred/relayed report evidence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AssessmentOwnerDecision {
    /// Durable diffpilot decision ID.
    pub id: DecisionId,
    /// Revision retained in append-only history.
    pub revision: u32,
    /// Actual deciding user.
    pub owner: UserId,
    /// Actual decision time.
    pub at: Timestamp,
    /// Actual chosen kind.
    pub kind: ReviewDecisionKind,
    /// Actual target.
    pub target: Option<DecisionTarget>,
}
