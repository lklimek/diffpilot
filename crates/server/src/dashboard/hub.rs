//! Per-user refresh registry behind the dashboard API and SSE; filled by WP11-REFRESH.

/// In-process dashboard state shared by all handlers; cheap to clone.
#[derive(Clone, Debug, Default)]
pub struct DashboardHub {}
