//! Ask: questions about a step answered by an agent job that resumes the analysis's session.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::agent::ask::AskOutput;
use crate::ids::{QuestionId, StepId};

/// Body of `POST /api/v1/analyses/{id}/steps/{step_id}/questions`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AskRequest {
    /// The question.
    pub text: String,
}

/// A question and its LLM answer, pinned to the step.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AskQuestion {
    /// Question id.
    pub id: QuestionId,
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
    pub created_at: DateTime<Utc>,
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
