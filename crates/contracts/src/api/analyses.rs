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
    /// Whether this reader has saved review state (including explicitly empty state).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub has_my_state: Option<bool>,
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
    Planning {
        /// The current stage; absent when no job of the analysis is queued, running or paused.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        stage: Option<AnalysisStage>,
    },
    /// Analysing flows.
    Running {
        /// Flows analysed.
        done: u32,
        /// Flows in total.
        total: u32,
        /// The current stage; absent when no job of the analysis is queued, running or paused.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        stage: Option<AnalysisStage>,
    },
    /// Waiting for the Codex usage limit to reset.
    Paused {
        /// When it resumes, if known.
        resumes_at: Option<DateTime<Utc>>,
        /// Flows analysed before the pause.
        done: u32,
        /// Flows in total; null while planning (the number of flows is not known yet).
        total: Option<u32>,
        /// The stage that resumes; absent when no job of the analysis is queued, running or paused.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        stage: Option<AnalysisStage>,
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

/// The stage of the analysis's live job (the queued, running or paused job of its current round); only facts the
/// server knows, no estimate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AnalysisStage {
    /// What the job does.
    pub kind: StageKind,
    /// The flow analysed (`flow` only); absent when the stored plan does not list it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub flow: Option<StageFlow>,
    /// When the server handed the job to the runner; null while the job waits for the runner.
    pub started_at: Option<DateTime<Utc>>,
}

/// Kind of an analysis stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum StageKind {
    /// Planning the flows.
    Plan,
    /// Describing the pull request context.
    PrContext,
    /// Analysing one flow.
    Flow,
}

/// The flow of a `flow` stage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct StageFlow {
    /// 1-based position of the flow in the stored plan (the order flows are analysed in).
    pub index: u32,
    /// Flow title.
    pub title: String,
}

/// A flow named in a state message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct FlowRef {
    /// Flow id.
    pub id: FlowId,
    /// Flow title.
    pub title: String,
}
