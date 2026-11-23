//! Settings and read-only paired comparison HTTP contracts; routes are implemented by W5/W6.

use crate::assessment::{
    AssessmentComparisonItem, AssessmentConfig, AssessmentMethodRun, AssessmentPass,
    AssessmentQuota,
};
use crate::time::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// GET /settings/comment-assessment; PUT uses AssessmentConfig and never schedules work itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AssessmentSettings {
    /// Effective per-user configuration.
    pub config: AssessmentConfig,
    /// Safe remaining/reset status.
    pub quota: AssessmentQuota,
}

/// GET …/comments/assessment-comparison, scoped to payer and PR.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AssessmentComparison {
    /// Frozen logical pass.
    pub pass: AssessmentPass,
    /// Owner revisions are selected as of this cutoff.
    pub cutoff: Timestamp,
    /// Independent method state and metering.
    pub runs: Vec<AssessmentMethodRun>,
    /// Includes unmatched, missing, duplicate and failed rows explicitly.
    pub items: Vec<AssessmentComparisonItem>,
}
