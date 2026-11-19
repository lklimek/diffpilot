//! Server-sent events of `GET /api/v1/analyses/{id}/events` (`docs/contracts/sse-events.md`).

use crate::time::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::api::analyses::AnalysisSummary;
use crate::api::questions::AskQuestion;

/// One SSE event; the SSE `event` field is the `event` tag, `data` is this JSON.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "event", rename_all = "snake_case")]
#[expect(
    clippy::large_enum_variant,
    reason = "infrequent SSE snapshots do not justify another allocation per broadcast clone"
)]
pub enum AnalysisEvent {
    /// Full analysis snapshot; sent on connect and on every change.
    Analysis {
        /// The analysis.
        summary: AnalysisSummary,
    },
    /// A question changed state or got its answer.
    Question {
        /// The question.
        question: AskQuestion,
    },
    /// Sent every 15 s while the stream is open, so a silent but open stream is told from a dead one.
    Heartbeat {
        /// Server time of sending.
        at: Timestamp,
    },
}

impl AnalysisEvent {
    /// The SSE event name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Analysis { .. } => "analysis",
            Self::Question { .. } => "question",
            Self::Heartbeat { .. } => "heartbeat",
        }
    }
}
