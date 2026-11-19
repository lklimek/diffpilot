//! Ask: private questions about a step, lines or a file, a flow or the whole PR, with follow-ups in threads, answered
//! in the asker's working session for this PR.

use crate::time::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::agent::ask::AskOutput;
use crate::common::{LineRange, LineSide};
use crate::ids::{FlowId, QuestionId, StepId};

/// Body of `POST /api/v1/analyses/{id}/steps/{step_id}/questions`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AskRequest {
    /// The question, at most `AskRequest::MAX_TEXT_CHARS` characters.
    pub text: String,
}

impl AskRequest {
    /// Longest question accepted, in characters.
    pub const MAX_TEXT_CHARS: usize = 2000;
}

/// Body of `POST /api/v1/analyses/{id}/questions`: a question with what it is about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AnchoredAskRequest {
    /// The question, at most `AskRequest::MAX_TEXT_CHARS` characters.
    pub text: String,
    /// What the question is about; a follow-up repeats its thread's anchor.
    pub anchor: AskAnchor,
    /// A follow-up: a question of the thread it continues (the thread's first question is stored); absent for a new
    /// thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub parent_id: Option<QuestionId>,
}

/// What a question is about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AskAnchor {
    /// A step of the Tour, optionally narrowed to some of its lines.
    Step {
        /// The step.
        step_id: StepId,
        /// Lines inside the step, on its side and file; null for the whole step.
        lines: Option<LineRange>,
    },
    /// Lines of a file of the pull request that lie in no single step (or a whole file).
    Lines {
        /// Repository-relative path on `side`.
        path: String,
        /// Commit the lines refer to.
        side: LineSide,
        /// The lines, at most `AskAnchor::MAX_LINES`; null for the whole file.
        range: Option<LineRange>,
    },
    /// An analysed flow as a whole.
    Flow {
        /// The flow.
        flow_id: FlowId,
    },
    /// The whole pull request.
    Pr,
}

impl AskAnchor {
    /// Longest line range of an anchor, in lines: the longest range the API serves.
    pub const MAX_LINES: u32 = crate::api::code::RangeQuery::MAX_LINES;

    /// The step of a step anchor.
    pub fn step_id(&self) -> Option<&StepId> {
        match self {
            Self::Step { step_id, .. } => Some(step_id),
            Self::Lines { .. } | Self::Flow { .. } | Self::Pr => None,
        }
    }
}

/// A private question and its LLM answer, pinned to its anchor; returned only to its asker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AskQuestion {
    /// Question id.
    pub id: QuestionId,
    /// GitHub login of the asker; absent only in older stored payloads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub asked_by: Option<String>,
    /// Always true in current HTTP/SSE responses; absent only in older payloads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub mine: Option<bool>,
    /// What the question is about.
    pub anchor: AskAnchor,
    /// The step of a step anchor; absent for other anchors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub step_id: Option<StepId>,
    /// A follow-up: the first question of its thread; absent for a first question.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub parent_id: Option<QuestionId>,
    /// The question.
    pub text: String,
    /// State.
    pub state: AskState,
    /// The LLM answer once answered.
    pub answer: Option<AskOutput>,
    /// Plain description when failed.
    pub failure: Option<String>,
    /// When it was asked.
    pub created_at: Timestamp,
    /// While `pending` and waiting for the LLM usage limit to reset: when its job is queued again; absent
    /// otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub resumes_at: Option<Timestamp>,
}

/// State of a question.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum AskState {
    /// Queued.
    Pending,
    /// The agent is working on it.
    Answering,
    /// Answered.
    Answered,
    /// Failed.
    Failed,
}
