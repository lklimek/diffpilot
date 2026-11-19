//! Ask: private questions about a step, answered in the asker's working session for this PR.

use crate::time::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::agent::ask::AskOutput;
use crate::ids::{QuestionId, StepId};

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

/// A private question and its LLM answer, pinned to the step; returned only to its asker.
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
    /// The step.
    pub step_id: StepId,
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
