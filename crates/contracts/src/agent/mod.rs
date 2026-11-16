//! Agent output per job kind. Field docs become schema descriptions the model reads.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod analyse_flow;
pub mod ask;
pub mod plan;
pub mod pr_context;
pub mod strict;

pub use strict::StrictSchemaError;

/// Kind of an agent job (brief §5 Job kinds).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    /// Partition the changed hunks into flows.
    Plan,
    /// Describe the pull request context.
    PrContext,
    /// Analyse one flow.
    AnalyseFlow,
    /// Answer a reviewer's question about a step.
    Ask,
}

impl JobKind {
    /// Every job kind, in pipeline order.
    pub const ALL: [JobKind; 4] = [Self::Plan, Self::PrContext, Self::AnalyseFlow, Self::Ask];

    /// The snake_case name used on the wire and in file names.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::PrContext => "pr_context",
            Self::AnalyseFlow => "analyse_flow",
            Self::Ask => "ask",
        }
    }
}

/// The output JSON Schema of a job kind, in the Codex strict subset.
pub fn output_schema(kind: JobKind) -> Result<Value, StrictSchemaError> {
    match kind {
        JobKind::Plan => strict::output_schema_for::<plan::PlanOutput>(),
        JobKind::PrContext => strict::output_schema_for::<pr_context::PrContextOutput>(),
        JobKind::AnalyseFlow => strict::output_schema_for::<analyse_flow::FlowAnalysisOutput>(),
        JobKind::Ask => strict::output_schema_for::<ask::AskOutput>(),
    }
}
