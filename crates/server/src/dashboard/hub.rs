//! Per-user refresh registry behind the dashboard API and SSE; owned and filled by WP11-REFRESH. WP11-SCHED and
//! WP11-JUDGE land in later waves and extend it then.

/// In-process dashboard state shared by all handlers; cheap to clone.
#[derive(Clone, Debug, Default)]
pub struct DashboardHub {}
