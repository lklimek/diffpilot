//! Feedback on the LLM analysis (stored locally and written to MemCan).

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::common::LineSide;
use crate::ids::{AnnotationId, FeedbackId, StepId};

/// Body of `POST /api/v1/analyses/{id}/feedback`; one entry per user and target (later posts replace it).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct FeedbackRequest {
    /// What the feedback is about.
    pub target: FeedbackTarget,
    /// The value.
    pub value: UserFeedbackValue,
    /// Optional note.
    pub note: Option<String>,
}

/// What a feedback entry is about.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "target", rename_all = "snake_case")]
pub enum FeedbackTarget {
    /// A step, its assessment included.
    Step {
        /// The step.
        step_id: StepId,
    },
    /// An annotation.
    Annotation {
        /// The annotation.
        annotation_id: AnnotationId,
    },
    /// A plain changed line (used by `missed_by_llm`).
    Line {
        /// Repository-relative path on that side.
        path: String,
        /// `base` for a removed line.
        side: LineSide,
        /// Line on that side.
        line: u32,
    },
}

/// Feedback values a reviewer can choose.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum UserFeedbackValue {
    /// The analysis is correct.
    Correct,
    /// The analysis is wrong.
    Wrong,
    /// The analysis is irrelevant.
    Irrelevant,
    /// The risk is rated too high (calibration example).
    Overrated,
    /// The risk is rated too low (calibration example).
    Underrated,
}

/// Every stored feedback value (brief §3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackValue {
    /// The analysis is correct.
    Correct,
    /// The analysis is wrong.
    Wrong,
    /// The analysis is irrelevant.
    Irrelevant,
    /// The risk is rated too high.
    Overrated,
    /// The risk is rated too low.
    Underrated,
    /// The reviewer commented on a changed line without annotation (recorded automatically).
    MissedByLlm,
}

impl From<UserFeedbackValue> for FeedbackValue {
    fn from(value: UserFeedbackValue) -> Self {
        match value {
            UserFeedbackValue::Correct => Self::Correct,
            UserFeedbackValue::Wrong => Self::Wrong,
            UserFeedbackValue::Irrelevant => Self::Irrelevant,
            UserFeedbackValue::Overrated => Self::Overrated,
            UserFeedbackValue::Underrated => Self::Underrated,
        }
    }
}

/// A stored feedback entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Feedback {
    /// Feedback id.
    pub id: FeedbackId,
    /// What it is about.
    pub target: FeedbackTarget,
    /// The value.
    pub value: FeedbackValue,
    /// Optional note.
    pub note: Option<String>,
    /// When it was recorded.
    pub created_at: DateTime<Utc>,
}
