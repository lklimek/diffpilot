//! Analyses and their progress.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::api::pr::PrSummary;
use crate::common::{LlmLanguage, PrRef};
use crate::ids::{AnalysisId, CommitSha, FlowId};

/// Body of `POST /api/v1/analyses`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CreateAnalysisRequest {
    /// The pull request.
    pub pr: PrRef,
    /// The head the user saw; a different current head is a conflict.
    pub head_sha: CommitSha,
}

/// An analysis and its state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AnalysisSummary {
    /// Analysis id.
    pub id: AnalysisId,
    /// The pull request as analysed (its head is the analysis head).
    pub pr: PrSummary,
    /// Language of the LLM texts.
    pub language: LlmLanguage,
    /// State and progress.
    pub state: AnalysisState,
    /// When the analysis was requested.
    pub created_at: DateTime<Utc>,
    /// When it finished, failed or was cancelled.
    pub finished_at: Option<DateTime<Utc>>,
}

/// State of an analysis (brief §4.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AnalysisState {
    /// Planning flows; the number of flows is not known yet.
    Planning,
    /// Analysing flows.
    Running {
        /// Flows analysed.
        done: u32,
        /// Flows in total.
        total: u32,
    },
    /// Waiting for the Codex usage limit to reset.
    Paused {
        /// When it resumes, if known.
        resumes_at: Option<DateTime<Utc>>,
        /// Flows analysed before the pause.
        done: u32,
        /// Flows in total; null while planning (the number of flows is not known yet).
        total: Option<u32>,
    },
    /// Failed; Retry is offered.
    Failed {
        /// The flow that failed, or null when it failed before any flow existed.
        flow: Option<FlowRef>,
        /// Plain description of what failed.
        message: String,
    },
    /// Every flow analysed; the review can open.
    Finished {
        /// Flows in total.
        total: u32,
    },
    /// Cancelled; partial results discarded.
    Cancelled,
}

/// A flow named in a state message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct FlowRef {
    /// Flow id.
    pub id: FlowId,
    /// Flow title.
    pub title: String,
}
