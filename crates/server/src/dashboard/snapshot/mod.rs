//! The stored snapshot document of a refresh (scan facts, not scores) and what R1/R1b serve from it: the items are
//! scored when read, with the judgements present then and at the snapshot's scan time, so the order changes only
//! when judgements land (plan §D "one snapshot document per refresh").

mod page;
mod view;

use contracts::api::dashboard::{
    DashboardSettings, HistoryEvent, KindCounts, ScanNote, TodosState, TodosStatus,
};
use contracts::time::Timestamp;
use db::error::DbError;
use db::repo::dashboard::RefreshRecord;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::score::{IssueFacts, PrFacts, TodoFacts};
use super::settings;
use crate::state::AppState;

pub use page::{CursorError, NO_SNAPSHOT, etag, etag_matches, page, revision};
pub use view::{View, ViewItem, build};

/// Version of [`Snapshot`]; a stored document of another version reads as no snapshot (refresh again).
pub const SNAPSHOT_VERSION: u32 = 1;

/// One refresh's scan, as stored (`dashboard_refreshes.snapshot`). Untrusted text is capped at scan time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Unanswered mentions from the scan, independent of scoring.
    #[serde(default)]
    pub mentions: Vec<contracts::api::dashboard::Mention>,
    /// Structured gaps in mention reads for this snapshot.
    #[serde(default)]
    pub mention_coverage: contracts::api::dashboard::MentionCoverage,
    /// [`SNAPSHOT_VERSION`].
    pub version: u32,
    /// The signed-in login at scan time.
    pub own_login: String,
    /// The settings the scan used (display ids and rules stay those of the scan).
    pub settings: DashboardSettings,
    /// When the scan finished; scores are computed at this time.
    pub scanned_at: Timestamp,
    /// Open pull requests.
    pub pull_requests: Vec<StoredPr>,
    /// Open issues.
    pub issues: Vec<StoredIssue>,
    /// Open MemCan TODOs (read now, or kept from the previous snapshot).
    pub todos: Vec<StoredTodo>,
    /// State of the TODO rows.
    pub todos_state: TodosState,
    /// Chart events of the 12-week window.
    pub history: Vec<HistoryEvent>,
    /// What the scan missed or approximated.
    pub notes: Vec<ScanNote>,
    /// Configured repositories and owners the token cannot see.
    pub unreachable: Vec<String>,
    /// What the scan read before exclusion.
    pub checked: KindCounts,
    /// The pull requests carry GitHub's merge state and the user's write access; false in snapshots stored before
    /// they were recorded.
    #[serde(default)]
    pub merge_states_recorded: bool,
    /// When pull requests of this snapshot were last re-read after a plug-in's change signal (ADR-022); absent
    /// while none was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prs_reread_at: Option<Timestamp>,
    /// TODO rows hidden because the current settings hide TODOs (read time only, never stored).
    #[serde(skip)]
    pub todos_hidden: bool,
}

/// A pull request of the snapshot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredPr {
    /// Scoring facts.
    pub facts: PrFacts,
    /// From a private repository.
    pub private: bool,
    /// Head commit at scan time (review links compare it with the analysed head).
    pub head_sha: String,
    /// Opened (the awaiting fallback when the request time is unknown).
    pub created_at: Timestamp,
    /// Failing build, test and lint checks.
    pub ci_red: Vec<String>,
    /// Running build, test and lint checks.
    pub ci_pending: Vec<String>,
    /// Issues it closes (`closingIssuesReferences`, first 5); empty in a snapshot stored before they were kept.
    #[serde(default)]
    pub linked_issues: Vec<StoredLinkedIssue>,
}

/// An issue a pull request closes. The scan reads no repository for it: it is shown under the pull request's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredLinkedIssue {
    /// Number.
    pub number: u32,
    /// Title (capped like other titles).
    pub title: String,
}

/// An issue of the snapshot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredIssue {
    /// Scoring facts.
    pub facts: IssueFacts,
    /// From a private repository, as far as the scan saw (pull request details tell a repository's privacy).
    pub private: bool,
    /// Comment count.
    pub comments: u32,
    /// Opened; absent in snapshots stored before it was recorded.
    #[serde(default)]
    pub opened_at: Option<Timestamp>,
}

/// A MemCan TODO of the snapshot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredTodo {
    /// Scoring facts (`changed_at` = the change time of plan §G).
    pub facts: TodoFacts,
    /// MemCan project.
    pub project: String,
    /// Description, plain text, at most 500 characters.
    pub description: Option<String>,
    /// Owner.
    pub owner: Option<String>,
    /// Number of blocking TODOs.
    pub blocked_by: usize,
    /// Change key (plan §G) to find the change time in the next refresh.
    pub change_key: String,
}

/// The user's current snapshot with its refresh; `None` before the first scan, and for a document of an unknown
/// version or shape (logged; the next refresh stores a readable one). TODO rows are left out while the user's
/// current settings hide TODOs (T19), whatever the scan stored.
pub async fn load(
    state: &AppState,
    tenant_id: &str,
    user_id: &str,
) -> Result<Option<(RefreshRecord, Snapshot)>, DbError> {
    let Some((record, document)) = state
        .db
        .dashboard()
        .latest_snapshot::<Value>(tenant_id, user_id)
        .await?
    else {
        return Ok(None);
    };
    let Some(mut snapshot) = readable(document) else {
        return Ok(None);
    };
    if !settings::effective(&state.db, tenant_id, user_id)
        .await?
        .include_todos
    {
        hide_todos(&mut snapshot);
    }
    let hidden = state
        .db
        .dashboard()
        .hidden_mentions(tenant_id, user_id)
        .await?;
    snapshot
        .mentions
        .retain(|mention| !hidden.contains(&mention.id));
    Ok(Some((record, snapshot)))
}

/// Drops the TODO rows, as a scan with TODOs off stores them.
fn hide_todos(snapshot: &mut Snapshot) {
    snapshot.todos_hidden = true;
    snapshot.todos.clear();
    snapshot.checked.todo = 0;
    snapshot.todos_state = TodosState {
        status: TodosStatus::Off,
        read_at: None,
        failed_at: None,
    };
}

/// The snapshot in `document`, if it has this version and shape.
pub(crate) fn readable(document: Value) -> Option<Snapshot> {
    let version = document.get("version").and_then(Value::as_u64);
    if version != Some(u64::from(SNAPSHOT_VERSION)) {
        tracing::warn!(
            ?version,
            "stored dashboard snapshot has another version; shown as none until the next refresh"
        );
        return None;
    }
    serde_json::from_value(document)
        .inspect_err(|error| {
            tracing::warn!(%error, "stored dashboard snapshot is unreadable; shown as none until the next refresh");
        })
        .ok()
}

/// The configured repositories and owners the token could not see at the latest scan (Settings marks).
pub async fn unreachable(
    state: &AppState,
    tenant_id: &str,
    user_id: &str,
) -> Result<Vec<String>, DbError> {
    Ok(load(state, tenant_id, user_id)
        .await?
        .map(|(_, snapshot)| snapshot.unreachable)
        .unwrap_or_default())
}
