//! Output of the `ask` job: the answer to a reviewer's question.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::common::Evidence;

/// An LLM answer to a question about a step.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AskOutput {
    /// The answer; a suspected defect is stated as a risk scenario saying what was not checked.
    pub answer: String,
    /// Lines read that support the answer.
    pub evidence: Vec<Evidence>,
}
