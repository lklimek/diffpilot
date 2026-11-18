//! MemCan's `list_todos` (memcan-server `serve.rs`, memcan-core `todo.rs` at dd00d97) over the TODOs seeded with
//! `POST /_fake/todos`: project required, one optional status, optional owner (trimmed; blank = all owners),
//! limit default 100 clamped to 1–200, order priority (high first) then `created_at` then id, `has_more` past
//! the limit.

use rmcp::ErrorData;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// MemCan's status vocabulary in its display order.
const STATUSES: [&str; 6] = [
    "pending",
    "in_progress",
    "blocked",
    "done",
    "postponed",
    "cancelled",
];

/// One TODO in MemCan's `TodoItem` shape.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Todo {
    /// UUID.
    pub id: String,
    /// Title.
    pub title: String,
    /// Description.
    pub description: Option<String>,
    /// Project.
    pub project: String,
    /// `high`, `medium` or `low`.
    pub priority: String,
    /// One of MemCan's statuses.
    pub status: String,
    /// Owner.
    pub owner: Option<String>,
    /// Ids of the TODOs blocking this one.
    #[serde(default)]
    pub blocked_by: Vec<String>,
    /// RFC 3339 creation time.
    pub created_at: String,
    /// RFC 3339 time of the first terminal state.
    pub completed_at: Option<String>,
}

/// Arguments of `list_todos`.
#[derive(Deserialize)]
pub(crate) struct ListParams {
    project: String,
    status: Option<String>,
    owner: Option<String>,
    limit: Option<u32>,
}

/// MemCan's `priority_rank`.
fn priority_rank(priority: &str) -> u8 {
    match priority {
        "high" => 0,
        "medium" => 1,
        "low" => 2,
        _ => 3,
    }
}

/// The `list_todos` answer, or MemCan's internal error for an unknown status.
pub(crate) fn list(todos: &[Todo], params: ListParams) -> Result<Value, ErrorData> {
    if let Some(status) = params.status.as_deref()
        && !STATUSES.contains(&status)
    {
        return Err(ErrorData::internal_error(
            format!(
                "list_todos failed: invalid status '{status}', must be one of: {}",
                STATUSES.join(", ")
            ),
            None,
        ));
    }
    let owner = params
        .owner
        .as_deref()
        .map(str::trim)
        .filter(|owner| !owner.is_empty());
    let limit = usize::try_from(params.limit.unwrap_or(100).clamp(1, 200)).unwrap_or(100);
    let mut found: Vec<&Todo> = todos
        .iter()
        .filter(|todo| todo.project == params.project)
        .filter(|todo| params.status.as_deref().is_none_or(|s| todo.status == s))
        .filter(|todo| owner.is_none_or(|o| todo.owner.as_deref() == Some(o)))
        .collect();
    found.sort_by(|a, b| {
        priority_rank(&a.priority)
            .cmp(&priority_rank(&b.priority))
            .then_with(|| a.created_at.cmp(&b.created_at))
            .then_with(|| a.id.cmp(&b.id))
    });
    let has_more = found.len() > limit;
    found.truncate(limit);
    Ok(json!({"todos": found, "has_more": has_more}))
}
