//! CI classification (skill `classify_ci`): build, test and lint checks apart from approval and policy gates.

use chrono::{DateTime, Utc};

use super::facts::CiEffective;
use super::text;

/// One context of a head commit's `statusCheckRollup`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckContext {
    /// A GitHub Actions or app check run.
    CheckRun {
        /// Job name.
        name: String,
        /// Workflow name, when the run belongs to one.
        workflow: Option<String>,
        /// GraphQL `status` (`COMPLETED`, `IN_PROGRESS`, `QUEUED`, …).
        status: Option<String>,
        /// GraphQL `conclusion` (`SUCCESS`, `FAILURE`, …); null while running.
        conclusion: Option<String>,
        /// Start time; null for a queued re-run (counts as the newest).
        started_at: Option<DateTime<Utc>>,
    },
    /// A commit status.
    StatusContext {
        /// Context name.
        context: String,
        /// GraphQL `state` (`SUCCESS`, `FAILURE`, `ERROR`, `PENDING`, `EXPECTED`).
        state: String,
        /// Creation time.
        created_at: Option<DateTime<Utc>>,
    },
}

/// A head commit's check rollup.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CheckRollup {
    /// Contexts as read.
    pub contexts: Vec<CheckContext>,
    /// More contexts exist than were read.
    pub truncated: bool,
}

/// Result of [`classify_ci`] (skill `ciEffective`, `ciRedChecks`, `ciPendingChecks`, `ciPolicyNotPassing`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiSummary {
    /// Effective state.
    pub effective: CiEffective,
    /// Failing build, test or lint checks.
    pub red: Vec<String>,
    /// Running build, test or lint checks.
    pub pending: Vec<String>,
    /// Policy gates not passing, as `name (STATE)`.
    pub policy_not_passing: Vec<String>,
}

const CHECK_FAILED: [&str; 5] = [
    "FAILURE",
    "TIMED_OUT",
    "CANCELLED",
    "ACTION_REQUIRED",
    "STARTUP_FAILURE",
];

/// Latest run per (workflow, check) wins; policy gates (K12) never make CI red; `None` = no rollup.
pub fn classify_ci(rollup: Option<&CheckRollup>) -> CiSummary {
    let mut out = CiSummary {
        effective: CiEffective::None,
        red: vec![],
        pending: vec![],
        policy_not_passing: vec![],
    };
    let Some(rollup) = rollup else {
        return out;
    };
    // Insertion-ordered (workflow, name) -> (recency, context); a queued re-run (no time) is the newest.
    type Recency = (bool, Option<DateTime<Utc>>);
    let mut latest: Vec<((String, String), Recency, &CheckContext)> = Vec::new();
    for context in &rollup.contexts {
        let (key, when) = match context {
            CheckContext::CheckRun {
                name,
                workflow,
                started_at,
                ..
            } => (
                (workflow.clone().unwrap_or_default(), name.clone()),
                *started_at,
            ),
            CheckContext::StatusContext {
                context,
                created_at,
                ..
            } => ((String::new(), context.clone()), *created_at),
        };
        let when = (when.is_none(), when);
        match latest.iter_mut().find(|(k, ..)| *k == key) {
            Some(slot) if when >= slot.1 => *slot = (key, when, context),
            Some(_) => {}
            None => latest.push((key, when, context)),
        }
    }
    let shown = |(workflow, name): &(String, String)| {
        let twins = latest.iter().filter(|((_, n), ..)| n == name).count();
        if twins > 1 && !workflow.is_empty() {
            format!("{workflow} / {name}")
        } else {
            name.clone()
        }
    };
    let mut rows: Vec<(String, &(String, String), &CheckContext)> = latest
        .iter()
        .map(|(key, _, context)| (shown(key), key, *context))
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    for (label, (_, name), context) in rows {
        let (state, failed, pending) = match context {
            CheckContext::StatusContext { state, .. } => (
                state.clone(),
                matches!(state.as_str(), "FAILURE" | "ERROR"),
                matches!(state.as_str(), "PENDING" | "EXPECTED"),
            ),
            CheckContext::CheckRun {
                status, conclusion, ..
            } => (
                conclusion
                    .clone()
                    .or_else(|| status.clone())
                    .unwrap_or_else(|| "UNKNOWN".into()),
                conclusion
                    .as_deref()
                    .is_some_and(|c| CHECK_FAILED.contains(&c)),
                status.as_deref() != Some("COMPLETED"),
            ),
        };
        if !(failed || pending) {
            continue;
        }
        if text::is_policy_check(name) {
            out.policy_not_passing.push(format!("{label} ({state})"));
        } else if failed {
            out.red.push(label);
        } else {
            out.pending.push(label);
        }
    }
    out.effective = if !out.red.is_empty() {
        CiEffective::Red
    } else if !out.pending.is_empty() || rollup.contexts.is_empty() {
        CiEffective::Pending
    } else if rollup.truncated {
        CiEffective::Unknown
    } else {
        CiEffective::Green
    };
    out
}
