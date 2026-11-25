//! Provider-reported usage of retained finished jobs.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Token sums; cached input is included in input, not additional to it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct UsageTotals {
    /// Input tokens, including cached input.
    #[ts(type = "number")]
    pub input_tokens: u64,
    /// Cached input tokens.
    #[ts(type = "number")]
    pub cached_input_tokens: u64,
    /// Output tokens.
    #[ts(type = "number")]
    pub output_tokens: u64,
    /// Unknown: the retained job-result contract does not store reasoning tokens separately.
    #[ts(type = "number | null")]
    pub reasoning_output_tokens: Option<u64>,
    /// Finished jobs with a provider usage report, including Ask and all analysis rounds.
    #[ts(type = "number")]
    pub jobs: u64,
}

/// Usage windows in the server's local calendar, scoped to the current tenant.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct UsageWindows {
    /// From local midnight through now; null without reports.
    pub today: Option<UsageTotals>,
    /// Today and the previous six local calendar days; null without reports.
    pub last_7_days: Option<UsageTotals>,
    /// From the start of the server-local calendar month through now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub this_month: Option<UsageTotals>,
}
