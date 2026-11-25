//! Per-reader LLM activity on HTTP and the user event stream.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::job::result::LlmCallOutcome;
use crate::time::Timestamp;

/// Running and queued jobs of the reader's tenant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct LlmActivity {
    /// Jobs currently running.
    pub running: Vec<LlmJob>,
    /// Jobs awaiting execution or paused.
    pub queued: Vec<LlmJob>,
    /// End of the reader's own provider pause, if known.
    pub paused_until: Option<Timestamp>,
    /// The reader's latest provider call.
    pub last_call: Option<LlmCallOutcome>,
}

/// A job as visible to the reader.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "owner", rename_all = "snake_case")]
pub enum LlmJob {
    /// A job paid for by the reader.
    #[serde(rename = "self")]
    Own {
        /// The job's id.
        id: crate::ids::JobId,
        /// The job kind.
        kind: LlmJobKind,
        /// The subject and app-relative link, when available.
        subject: Option<JobSubject>,
        /// When the job started running.
        since: Option<Timestamp>,
        /// When the job was queued.
        queued_at: Option<Timestamp>,
        /// Why the job is waiting.
        waits_for: Option<WaitReason>,
    },
    /// Another user's job in this tenant.
    Other {
        /// Opaque index local to this response.
        id: String,
        /// The job kind.
        kind: LlmJobKind,
    },
}

/// Kind of LLM work as shown in the activity list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum LlmJobKind {
    /// PR planning and context.
    Analysis,
    /// Flow analysis.
    FlowAnalysis,
    /// Dashboard ranking or re-judgement.
    Ranking,
    /// Comment assessment.
    CommentAssessment,
    /// An Ask question.
    Ask,
}

/// Subject of a job paid for by the reader.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct JobSubject {
    /// User-facing subject.
    pub label: String,
    /// App-relative URL built by the server.
    pub href: String,
}

/// What must happen before the reader's job can run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WaitReason {
    /// A runner slot must become available.
    Runner,
    /// The reader's provider usage limit must reset.
    UsageLimit {
        /// When execution can resume.
        until: Timestamp,
    },
    /// An analysis is ahead of the ranking.
    AnalysisRunning,
}
