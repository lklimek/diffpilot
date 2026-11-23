//! Agent output per job kind. Field docs become schema descriptions the model reads.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod analyse_flow;
pub mod ask;
pub mod judge_work;
pub mod plan;
pub mod pr_context;
pub mod strict;
pub mod verify_comments;

pub use strict::StrictSchemaError;

/// The claudius skills the shared prompt rules require (analysis, repair and Ask jobs).
pub const ANALYSIS_SKILLS: [&str; 1] = ["severity"];

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
    /// Judge dashboard items (no pull request, ADR-017).
    JudgeWork,
    /// Assess a pull request's review comments, or answer a question about one (ADR-023).
    VerifyComments,
}

impl JobKind {
    /// Every job kind, in pipeline order, then the dashboard's and the review loop's.
    pub const ALL: [JobKind; 6] = [
        Self::Plan,
        Self::PrContext,
        Self::AnalyseFlow,
        Self::Ask,
        Self::JudgeWork,
        Self::VerifyComments,
    ];

    /// The claudius skills a job of this kind is given and needs (ADR-002); the runner gives only skills its
    /// `SR_RUNNER_SKILLS` allows. A `verify_comments` prompt states what it needs itself (ADR-002 amendment
    /// 2026-11-23).
    pub fn required_skills(self) -> &'static [&'static str] {
        match self {
            Self::Plan | Self::PrContext | Self::AnalyseFlow | Self::Ask => &ANALYSIS_SKILLS,
            Self::JudgeWork | Self::VerifyComments => &[],
        }
    }

    /// The snake_case name used on the wire and in file names.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::PrContext => "pr_context",
            Self::AnalyseFlow => "analyse_flow",
            Self::Ask => "ask",
            Self::JudgeWork => "judge_work",
            Self::VerifyComments => "verify_comments",
        }
    }
}

/// The output JSON Schema of a job kind, in the Codex strict subset.
pub fn output_schema(kind: JobKind) -> Result<Value, StrictSchemaError> {
    match kind {
        JobKind::Plan => strict::output_schema_for::<plan::PlanOutput>(),
        JobKind::PrContext => strict::output_schema_for::<pr_context::PrContextOutput>(),
        JobKind::AnalyseFlow => {
            let mut schema = strict::output_schema_for::<analyse_flow::FlowAnalysisOutput>()?;
            // The strict subset requires every property of an object. A second closed shape lets low
            // assessments omit the detailed fields rather than emitting six null placeholders.
            if let Some(assessment) =
                schema.pointer_mut("/properties/steps/items/properties/assessment")
            {
                let detailed = assessment.clone();
                *assessment = serde_json::json!({"anyOf": [detailed, {
                    "type":"object", "additionalProperties":false,
                    "properties":{"risk":{"type":"string","enum":["low"]},"reason":{"type":"string"}},
                    "required":["risk","reason"]
                }]});
            }
            Ok(schema)
        }
        JobKind::Ask => strict::output_schema_for::<ask::AskOutput>(),
        JobKind::JudgeWork => judge_work::output_schema(),
        JobKind::VerifyComments => {
            strict::output_schema_for::<verify_comments::VerifyCommentsOutput>()
        }
    }
}
