//! Read-only MemCan TODO reader of the dashboard (plan §G, spike S11-4): one connection per refresh, then
//! `list_todos` per project, sequentially; no other MemCan tool is ever called. Open statuses are kept
//! client-side; a project reporting `has_more` is re-read once per open status; an over-size answer is retried
//! once with a smaller limit. Problems with one project become scan notes; an unavailable MemCan, or a read longer
//! than [`READ_BUDGET`], stops the read and never fails the refresh.
//!
//! TODO text is untrusted (agents write it from external content) and later enters the judge prompt as data:
//! every text field is cleaned of control and format characters (bidi overrides included) and capped here.

use std::collections::HashSet;
use std::time::Duration;

use contracts::time::Timestamp;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::client::{MemcanClient, MemcanError};
use crate::config::MemcanConfig;
use crate::dashboard::score::TodoStatus;
use crate::secrets::sha256;

/// The only MemCan tool this module calls.
const LIST_TODOS: &str = "list_todos";
/// TODOs asked for per call.
const LIMIT: u32 = 100;
/// TODOs asked for when the answer at [`LIMIT`] was too large.
const SMALL_LIMIT: u32 = 25;
/// Longest whole TODO read of a refresh (all projects).
const READ_BUDGET: Duration = Duration::from_secs(120);
/// Longest description kept, in characters.
const MAX_DESCRIPTION_CHARS: usize = 500;
/// Longest title kept, in characters.
const MAX_TITLE_CHARS: usize = 200;
/// Longest owner and priority kept, in characters.
const MAX_SHORT_CHARS: usize = 100;
/// Most blocking TODO ids kept.
const MAX_BLOCKED_BY: usize = 50;
/// MemCan statuses of open TODOs, in the order of the per-status reads.
const OPEN_STATUSES: [(&str, TodoStatus); 4] = [
    ("pending", TodoStatus::Pending),
    ("in_progress", TodoStatus::InProgress),
    ("blocked", TodoStatus::Blocked),
    ("postponed", TodoStatus::Postponed),
];

/// One open MemCan TODO (plan §G item mapping).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Todo {
    /// MemCan id (item key `memcan~<uuid>`, link `<SR_MEMCAN_UI_URL>/tasks/<uuid>`).
    pub id: Uuid,
    /// Title, at most 200 characters (untrusted text, see the module documentation).
    pub title: String,
    /// Description as plain text, at most 500 characters (untrusted); none when MemCan has none.
    pub description: Option<String>,
    /// The project it was read from.
    pub project: String,
    /// MemCan priority (`high`, `medium`, `low`), cleaned, at most 100 characters.
    pub priority: String,
    /// Status.
    pub status: TodoStatus,
    /// Owner, at most 100 characters; none when MemCan has none.
    pub owner: Option<String>,
    /// Ids (UUIDs) of the TODOs blocking it, at most 50.
    pub blocked_by: Vec<String>,
    /// Creation time.
    pub created_at: Timestamp,
    /// SHA-256 hex of status, priority, title, owner, `blocked_by` and the whole description (MemCan has no
    /// `updated_at`); stored with the snapshot to find the change time in the next refresh.
    pub change_key: String,
}

impl Todo {
    /// Change time (R1b `updated_since`, "changed" marker): `created_at` on first sight, the previous change time
    /// while the change key is the same, else `scanned_at` of this refresh. `previous` = the change key and
    /// change time in the previous snapshot.
    pub fn changed_at(
        &self,
        previous: Option<(&str, Timestamp)>,
        scanned_at: Timestamp,
    ) -> Timestamp {
        match previous {
            None => self.created_at,
            Some((key, at)) if key == self.change_key => at,
            Some(_) => scanned_at,
        }
    }
}

/// Outcome of reading the TODOs of a refresh.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TodosRead {
    /// Every project was read (some perhaps with notes).
    Read {
        /// Open TODOs in project order, then MemCan's order (priority, then age).
        todos: Vec<Todo>,
        /// Scan notes (plain text), one per problem.
        notes: Vec<String>,
    },
    /// MemCan was unreachable, refused the token or did not answer in time: nothing of this read is kept, and the
    /// refresh shows the previous snapshot's TODOs as not refreshed.
    Unavailable {
        /// Scan note (plain text).
        note: String,
    },
}

/// Reads the open TODOs of `projects` (the effective list of the user's settings); call it only when MemCan is
/// configured and the user shows TODOs. Never fails: problems are notes, an unavailable MemCan is
/// [`TodosRead::Unavailable`].
pub async fn read(config: &MemcanConfig, projects: &[String]) -> TodosRead {
    let client = match MemcanClient::connect(config).await {
        Ok(client) => client,
        Err(error) => return unavailable(&error),
    };
    let client = &client;
    read_with(projects, READ_BUDGET, |arguments| {
        client.call(LIST_TODOS, arguments)
    })
    .await
}

/// [`read`] over `list`, which carries out one `list_todos` call with the given arguments, within `budget`.
/// `list` returns a future that does not borrow `list` itself (no async closure), so [`read`]'s future is `Send`
/// and runs in a spawned task.
async fn read_with<F, L>(projects: &[String], budget: Duration, mut list: L) -> TodosRead
where
    L: FnMut(Value) -> F,
    F: Future<Output = Result<String, MemcanError>>,
{
    let mut todos = Vec::new();
    let mut notes = Vec::new();
    let all = async {
        for project in projects {
            todos.extend(read_project(project, &mut list, &mut notes).await?);
        }
        Ok(())
    };
    match tokio::time::timeout(budget, all).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return unavailable(&error),
        Err(_) => return unavailable(&MemcanError::Timeout),
    }
    tracing::debug!(
        projects = projects.len(),
        todos = todos.len(),
        notes = notes.len(),
        "MemCan TODOs read"
    );
    TodosRead::Read { todos, notes }
}

fn unavailable(error: &MemcanError) -> TodosRead {
    tracing::warn!(%error, "MemCan TODOs not refreshed: MemCan is unavailable");
    let note = match error {
        MemcanError::Timeout => "MemCan did not answer in time: TODOs not refreshed.",
        _ => "MemCan unreachable or token rejected (check SR_MEMCAN_TOKEN): TODOs not refreshed.",
    };
    TodosRead::Unavailable {
        note: note.to_owned(),
    }
}

/// One `list_todos` answer.
#[derive(Deserialize)]
struct Page {
    todos: Vec<Value>,
    has_more: bool,
}

/// A TODO as MemCan answers it (S11-4 I1); `status` is read separately.
#[derive(Deserialize)]
struct Raw {
    id: Uuid,
    title: String,
    description: Option<String>,
    priority: String,
    owner: Option<String>,
    #[serde(default)]
    blocked_by: Vec<String>,
    created_at: Timestamp,
}

/// The open TODOs of one project. `Err` only when MemCan is unavailable; other problems are added to `notes`.
async fn read_project<F, L>(
    project: &str,
    list: &mut L,
    notes: &mut Vec<String>,
) -> Result<Vec<Todo>, MemcanError>
where
    L: FnMut(Value) -> F,
    F: Future<Output = Result<String, MemcanError>>,
{
    let Some((page, _)) = fetch(project, None, list, notes).await? else {
        return Ok(Vec::new());
    };
    let pages = if page.has_more {
        let mut pages = Vec::new();
        for (status, _) in OPEN_STATUSES {
            if let Some((page, limit)) = fetch(project, Some(status), list, notes).await? {
                if page.has_more {
                    notes.push(format!(
                        "MemCan project {project}: more than {limit} {} TODOs; the first {limit} are shown.",
                        status.replace('_', " ")
                    ));
                }
                pages.push(page);
            }
        }
        pages
    } else {
        vec![page]
    };
    let mut unreadable = 0_usize;
    let mut todos = Vec::new();
    // A TODO whose status changed between the per-status reads is in two pages: the first wins.
    let mut seen = HashSet::new();
    for value in pages.into_iter().flat_map(|page| page.todos) {
        let Some(status) = open_status(&value) else {
            continue;
        };
        match serde_json::from_value::<Raw>(value) {
            Ok(raw) if seen.insert(raw.id) => todos.push(todo(project, status, raw)),
            Ok(_) => {}
            Err(_) => unreadable += 1,
        }
    }
    if unreadable > 0 {
        tracing::warn!(
            project,
            unreadable,
            "MemCan TODOs with an unexpected shape left out"
        );
        notes.push(format!(
            "MemCan project {project}: {unreadable} TODO(s) could not be read and are left out."
        ));
    }
    Ok(todos)
}

/// One page of `project` (optionally of one status) with the limit it was read with; an over-size answer is
/// asked for once more with [`SMALL_LIMIT`]. `None` (and a note) when the page cannot be had.
async fn fetch<F, L>(
    project: &str,
    status: Option<&str>,
    list: &mut L,
    notes: &mut Vec<String>,
) -> Result<Option<(Page, u32)>, MemcanError>
where
    L: FnMut(Value) -> F,
    F: Future<Output = Result<String, MemcanError>>,
{
    let mut limit = LIMIT;
    let mut answer = list(arguments(project, status, limit)).await;
    if matches!(answer, Err(MemcanError::InvalidResponse { .. })) {
        tracing::debug!(
            project,
            ?status,
            "MemCan TODO answer too large; asking again with a smaller limit"
        );
        limit = SMALL_LIMIT;
        answer = list(arguments(project, status, limit)).await;
    }
    let scope = match status {
        Some(status) => format!("its {} TODOs", status.replace('_', " ")),
        None => "its TODOs".to_owned(),
    };
    let error = match answer {
        Ok(text) => match serde_json::from_str::<Page>(&text) {
            Ok(page) => return Ok(Some((page, limit))),
            Err(_) => None,
        },
        Err(error) if error.is_unavailable() => return Err(error),
        Err(error) => Some(error),
    };
    let note = match &error {
        None => {
            format!("MemCan project {project}: the answer could not be read; {scope} are left out.")
        }
        Some(MemcanError::InvalidResponse { .. }) => {
            format!("MemCan project {project}: {scope} are too large to read and are left out.")
        }
        Some(_) => {
            format!("MemCan project {project}: MemCan refused the TODO list; {scope} are left out.")
        }
    };
    // The error is scrubbed of the token and the MemCan address (`client`).
    tracing::warn!(
        project,
        ?status,
        ?error,
        "MemCan TODO list of a project not read"
    );
    notes.push(note);
    Ok(None)
}

fn arguments(project: &str, status: Option<&str>, limit: u32) -> Value {
    match status {
        Some(status) => json!({"project": project, "status": status, "limit": limit}),
        None => json!({"project": project, "limit": limit}),
    }
}

/// MemCan name of an open status.
fn status_name(status: TodoStatus) -> &'static str {
    OPEN_STATUSES
        .iter()
        .find(|(_, open)| *open == status)
        .map_or("", |(name, _)| name)
}

/// The open status of a TODO answer; `None` for closed (`done`, `cancelled`) and unknown statuses.
fn open_status(value: &Value) -> Option<TodoStatus> {
    let status = value.get("status")?.as_str()?;
    OPEN_STATUSES
        .iter()
        .find(|(name, _)| *name == status)
        .map(|(_, status)| *status)
}

fn todo(project: &str, status: TodoStatus, raw: Raw) -> Todo {
    let changed = json!([
        status_name(status),
        raw.priority,
        raw.title,
        raw.owner,
        raw.blocked_by,
        raw.description
    ]);
    Todo {
        id: raw.id,
        title: clean(&raw.title, false, MAX_TITLE_CHARS),
        description: raw
            .description
            .as_deref()
            .map(|text| clean(text, true, MAX_DESCRIPTION_CHARS)),
        project: project.to_owned(),
        priority: clean(&raw.priority, false, MAX_SHORT_CHARS),
        status,
        owner: raw
            .owner
            .as_deref()
            .map(|text| clean(text, false, MAX_SHORT_CHARS)),
        blocked_by: raw
            .blocked_by
            .iter()
            .filter(|id| Uuid::parse_str(id).is_ok())
            .take(MAX_BLOCKED_BY)
            .cloned()
            .collect(),
        created_at: raw.created_at,
        change_key: sha256(changed.to_string().as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    }
}

/// `text` without control characters (line breaks and tabs kept when `multiline`) and format characters
/// (bidi overrides and isolates, zero-width characters), cut to `max_chars`.
fn clean(text: &str, multiline: bool, max_chars: usize) -> String {
    text.chars()
        .filter(|&c| !(c.is_control() && !(multiline && matches!(c, '\n' | '\t'))) && !is_format(c))
        .take(max_chars)
        .collect()
}

/// Unicode general category Cf (format), Unicode 16.
fn is_format(c: char) -> bool {
    matches!(
        c,
        '\u{ad}'
            | '\u{600}'..='\u{605}'
            | '\u{61c}'
            | '\u{6dd}'
            | '\u{70f}'
            | '\u{890}'..='\u{891}'
            | '\u{8e2}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{110bd}'
            | '\u{110cd}'
            | '\u{13430}'..='\u{1343f}'
            | '\u{1bca0}'..='\u{1bca3}'
            | '\u{1d173}'..='\u{1d17a}'
            | '\u{e0001}'
            | '\u{e0020}'..='\u{e007f}'
    )
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;

    use super::*;
    use crate::memcan::fake::{FakeMemcan, Mode, TOKEN};
    use crate::test_support::logs::LogCapture;

    /// Scripted `list_todos`: answers in order, records the arguments.
    struct Script {
        answers: RefCell<VecDeque<Result<String, MemcanError>>>,
        calls: RefCell<Vec<Value>>,
        /// Time each call takes.
        delay: Option<Duration>,
    }

    impl Script {
        fn new(answers: Vec<Result<String, MemcanError>>) -> Self {
            Self {
                answers: RefCell::new(answers.into()),
                calls: RefCell::default(),
                delay: None,
            }
        }

        async fn read(&self, projects: &[&str]) -> TodosRead {
            let projects: Vec<String> = projects.iter().map(|p| (*p).to_owned()).collect();
            read_with(&projects, READ_BUDGET, |arguments| async move {
                self.calls.borrow_mut().push(arguments);
                if let Some(delay) = self.delay {
                    tokio::time::sleep(delay).await;
                }
                self.answers
                    .borrow_mut()
                    .pop_front()
                    .expect("an answer for every call")
            })
            .await
        }

        fn calls(&self) -> Vec<Value> {
            self.calls.borrow().clone()
        }
    }

    fn raw(id: u32, status: &str) -> Value {
        json!({
            "id": format!("00000000-0000-4000-8000-{id:012}"),
            "title": format!("TODO {id}"),
            "description": "synthetic",
            "project": "answer-project",
            "priority": "medium",
            "status": status,
            "owner": null,
            "blocked_by": [],
            "created_at": "2026-11-13T09:00:00+00:00",
            "completed_at": null,
        })
    }

    fn page(todos: Vec<Value>, has_more: bool) -> Result<String, MemcanError> {
        Ok(json!({"todos": todos, "has_more": has_more}).to_string())
    }

    fn too_large() -> Result<String, MemcanError> {
        Err(MemcanError::InvalidResponse { tool: LIST_TODOS })
    }

    /// The numbers `raw` put into the ids.
    fn ids(read: &TodosRead) -> Vec<u32> {
        match read {
            TodosRead::Read { todos, .. } => todos
                .iter()
                .map(|t| t.id.to_string()[24..].parse().expect("number"))
                .collect(),
            TodosRead::Unavailable { .. } => panic!("unavailable: {read:?}"),
        }
    }

    fn notes(read: &TodosRead) -> Vec<String> {
        match read {
            TodosRead::Read { notes, .. } => notes.clone(),
            TodosRead::Unavailable { note } => vec![note.clone()],
        }
    }

    fn at(time: &str) -> Timestamp {
        time.parse::<Timestamp>().expect("time")
    }

    #[tokio::test]
    async fn read_runs_in_a_spawned_task() {
        let fake = FakeMemcan::start();
        let config = fake.config();
        let projects = vec!["platform".to_owned()];
        let read = tokio::spawn(async move { read(&config, &projects).await })
            .await
            .expect("task");
        assert!(matches!(read, TodosRead::Read { .. }), "{read:?}");
    }

    #[tokio::test]
    async fn todos_are_read_per_project_with_limit_100_keeping_open_statuses() {
        let script = Script::new(vec![
            page(
                vec![
                    raw(1, "in_progress"),
                    raw(2, "done"),
                    raw(3, "cancelled"),
                    raw(4, "pending"),
                ],
                false,
            ),
            page(vec![raw(5, "blocked"), raw(6, "postponed")], false),
        ]);
        let read = script.read(&["platform", "consensus"]).await;
        assert_eq!(
            script.calls(),
            [
                json!({"project": "platform", "limit": 100}),
                json!({"project": "consensus", "limit": 100}),
            ]
        );
        assert_eq!(ids(&read), [1, 4, 5, 6]);
        assert!(notes(&read).is_empty());
        let TodosRead::Read { todos, .. } = read else {
            unreachable!()
        };
        assert_eq!(todos[0].project, "platform", "the project asked for");
        assert_eq!(todos[0].status, TodoStatus::InProgress);
        assert_eq!(todos[2].project, "consensus");
        assert_eq!(todos[2].status, TodoStatus::Blocked);
        assert_eq!(todos[0].created_at, at("2026-11-13T09:00:00Z"));
    }

    #[tokio::test]
    async fn description_is_plain_text_of_at_most_500_characters() {
        let mut long = raw(1, "pending");
        long["description"] = json!(format!("a\u{7}b\n{}", "é".repeat(600)));
        let mut none = raw(2, "pending");
        none["description"] = Value::Null;
        let script = Script::new(vec![page(vec![long, none], false)]);
        let TodosRead::Read { todos, .. } = script.read(&["p"]).await else {
            panic!("unavailable")
        };
        let description = todos[0].description.as_deref().expect("description");
        assert_eq!(description.chars().count(), 500);
        assert!(description.starts_with("ab\né"), "{description}");
        assert_eq!(todos[1].description, None);
    }

    #[tokio::test]
    async fn todos_list_falls_back_per_status_when_has_more() {
        let script = Script::new(vec![
            page(vec![raw(1, "pending"), raw(2, "done")], true),
            page(vec![raw(11, "pending")], false),
            page(vec![raw(12, "in_progress")], true),
            page(vec![], false),
            page(vec![raw(14, "postponed")], false),
            page(vec![raw(21, "pending")], false),
        ]);
        let read = script.read(&["big", "small"]).await;
        assert_eq!(
            script.calls(),
            [
                json!({"project": "big", "limit": 100}),
                json!({"project": "big", "status": "pending", "limit": 100}),
                json!({"project": "big", "status": "in_progress", "limit": 100}),
                json!({"project": "big", "status": "blocked", "limit": 100}),
                json!({"project": "big", "status": "postponed", "limit": 100}),
                json!({"project": "small", "limit": 100}),
            ]
        );
        assert_eq!(
            ids(&read),
            [11, 12, 14, 21],
            "the per-status pages replace the first"
        );
        assert_eq!(
            notes(&read),
            ["MemCan project big: more than 100 in progress TODOs; the first 100 are shown."]
        );
    }

    #[tokio::test]
    async fn a_todo_in_two_status_pages_is_kept_once() {
        let mut moved = raw(12, "in_progress");
        moved["title"] = json!("moved meanwhile");
        let script = Script::new(vec![
            page(vec![raw(1, "pending")], true),
            page(vec![raw(12, "pending")], false),
            page(vec![moved], false),
            page(vec![], false),
            page(vec![], false),
        ]);
        let read = script.read(&["p"]).await;
        assert_eq!(ids(&read), [12], "first occurrence wins");
        let TodosRead::Read { todos, .. } = read else {
            unreachable!()
        };
        assert_eq!(todos[0].status, TodoStatus::Pending);
    }

    #[tokio::test]
    async fn every_text_field_is_cleaned_and_capped() {
        let mut dirty = raw(1, "pending");
        dirty["title"] = json!(format!(
            "a\u{202e}b\u{2066}c\u{200b}d\ne\u{7}{}",
            "t".repeat(300)
        ));
        dirty["description"] = json!("x\u{202a}y\u{2069}z\u{feff}\nnext\tline");
        dirty["owner"] = json!(format!("o\u{202d}w\n{}", "n".repeat(200)));
        dirty["priority"] = json!(format!("hi\u{200f}gh{}", "h".repeat(200)));
        let blockers: Vec<String> = (0..60)
            .map(|i| format!("00000000-0000-4000-8000-{i:012}"))
            .chain(["not-an-id".to_owned()])
            .collect();
        dirty["blocked_by"] = json!(blockers);
        let script = Script::new(vec![page(vec![dirty], false)]);
        let TodosRead::Read { todos, .. } = script.read(&["p"]).await else {
            panic!("unavailable")
        };
        let todo = &todos[0];
        assert!(todo.title.starts_with("abcde"), "{}", todo.title);
        assert_eq!(todo.title.chars().count(), 200);
        assert_eq!(todo.description.as_deref(), Some("xyz\nnext\tline"));
        let owner = todo.owner.as_deref().expect("owner");
        assert!(owner.starts_with("own"), "{owner}");
        assert_eq!(owner.chars().count(), 100);
        assert!(todo.priority.starts_with("high"), "{}", todo.priority);
        assert_eq!(todo.priority.chars().count(), 100);
        assert_eq!(todo.blocked_by.len(), 50);
        assert!(todo.blocked_by.iter().all(|id| Uuid::parse_str(id).is_ok()));
    }

    #[tokio::test]
    async fn oversized_answer_is_asked_again_with_limit_25_then_noted_too_large() {
        let script = Script::new(vec![
            too_large(),
            page(vec![raw(1, "pending")], false),
            too_large(),
            too_large(),
            page(vec![raw(3, "pending")], false),
        ]);
        let read = script.read(&["a", "b", "c"]).await;
        assert_eq!(
            script.calls(),
            [
                json!({"project": "a", "limit": 100}),
                json!({"project": "a", "limit": 25}),
                json!({"project": "b", "limit": 100}),
                json!({"project": "b", "limit": 25}),
                json!({"project": "c", "limit": 100}),
            ]
        );
        assert_eq!(ids(&read), [1, 3]);
        assert_eq!(
            notes(&read),
            ["MemCan project b: its TODOs are too large to read and are left out."]
        );
    }

    #[tokio::test]
    async fn small_limit_page_with_more_falls_back_per_status_and_notes_truncation() {
        let script = Script::new(vec![
            too_large(),
            page(vec![raw(1, "pending")], true),
            too_large(),
            page(vec![raw(11, "pending")], true),
            page(vec![], false),
            page(vec![], false),
            page(vec![], false),
        ]);
        let read = script.read(&["p"]).await;
        assert_eq!(script.calls().len(), 7);
        assert_eq!(
            script.calls()[3],
            json!({"project": "p", "status": "pending", "limit": 25})
        );
        assert_eq!(ids(&read), [11]);
        assert_eq!(
            notes(&read),
            ["MemCan project p: more than 25 pending TODOs; the first 25 are shown."]
        );
    }

    #[tokio::test]
    async fn rejected_or_unreadable_answers_are_notes_for_that_project_only() {
        let script = Script::new(vec![
            Err(MemcanError::Rejected {
                tool: LIST_TODOS,
                code: Some(-32603),
                detail: "list_todos failed".to_owned(),
            }),
            Ok("<html>busy".to_owned()),
            page(
                vec![
                    raw(3, "pending"),
                    json!({"status": "pending", "id": "not-a-uuid"}),
                ],
                false,
            ),
        ]);
        let read = script.read(&["a", "b", "c"]).await;
        assert_eq!(ids(&read), [3]);
        assert_eq!(
            notes(&read),
            [
                "MemCan project a: MemCan refused the TODO list; its TODOs are left out.",
                "MemCan project b: the answer could not be read; its TODOs are left out.",
                "MemCan project c: 1 TODO(s) could not be read and are left out.",
            ]
        );
    }

    #[tokio::test]
    async fn unavailable_memcan_stops_the_read_and_keeps_nothing() {
        let unreachable = Script::new(vec![
            page(vec![raw(1, "pending")], false),
            Err(MemcanError::Unreachable {
                detail: "connection refused".to_owned(),
            }),
        ]);
        let read = unreachable.read(&["a", "b", "c"]).await;
        assert_eq!(unreachable.calls().len(), 2, "project c is not asked for");
        assert_eq!(
            read,
            TodosRead::Unavailable {
                note: "MemCan unreachable or token rejected (check SR_MEMCAN_TOKEN): TODOs not refreshed."
                    .to_owned()
            }
        );
        let slow = Script::new(vec![too_large(), Err(MemcanError::Timeout)]);
        assert_eq!(
            slow.read(&["a", "b"]).await,
            TodosRead::Unavailable {
                note: "MemCan did not answer in time: TODOs not refreshed.".to_owned()
            }
        );
        assert_eq!(slow.calls().len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn the_whole_read_stops_after_its_time_budget() {
        let mut slow = Script::new(
            (0..10)
                .map(|i| page(vec![raw(i, "pending")], false))
                .collect(),
        );
        slow.delay = Some(Duration::from_secs(50));
        assert_eq!(
            slow.read(&["a", "b", "c", "d", "e"]).await,
            TodosRead::Unavailable {
                note: "MemCan did not answer in time: TODOs not refreshed.".to_owned()
            }
        );
        assert_eq!(slow.calls().len(), 3, "no call starts after 120 s");
    }

    #[tokio::test]
    async fn no_projects_no_calls() {
        let script = Script::new(Vec::new());
        assert_eq!(
            script.read(&[]).await,
            TodosRead::Read {
                todos: Vec::new(),
                notes: Vec::new()
            }
        );
        assert!(script.calls().is_empty());
    }

    #[tokio::test]
    async fn todo_change_time_from_change_key() {
        let base = raw(1, "pending");
        let read_one = async |value: Value| {
            let script = Script::new(vec![page(vec![value], false)]);
            match script.read(&["p"]).await {
                TodosRead::Read { mut todos, .. } => todos.remove(0),
                TodosRead::Unavailable { .. } => panic!("unavailable"),
            }
        };
        let todo = read_one(base.clone()).await;
        let created = at("2026-11-13T09:00:00Z");
        let previous = at("2026-11-15T08:00:00Z");
        let scanned = at("2026-11-18T08:12:00Z");
        assert_eq!(todo.changed_at(None, scanned), created, "first sight");
        assert_eq!(
            todo.changed_at(Some((&todo.change_key, previous)), scanned),
            previous,
            "unchanged"
        );
        assert_eq!(
            todo.changed_at(Some(("another key", previous)), scanned),
            scanned,
            "changed since the previous snapshot"
        );
        for (field, value) in [
            ("status", json!("blocked")),
            ("priority", json!("high")),
            ("title", json!("renamed")),
            ("owner", json!("alice")),
            (
                "blocked_by",
                json!(["00000000-0000-4000-8000-000000000009"]),
            ),
            ("description", json!("edited")),
        ] {
            let mut changed = base.clone();
            changed[field] = value;
            assert_ne!(
                read_one(changed).await.change_key,
                todo.change_key,
                "{field} is part of the change key"
            );
        }
        let mut tail = base.clone();
        tail["description"] = json!(format!("{}x", "a".repeat(600)));
        let mut other_tail = base.clone();
        other_tail["description"] = json!(format!("{}y", "a".repeat(600)));
        assert_ne!(
            read_one(tail).await.change_key,
            read_one(other_tail).await.change_key,
            "the whole description counts, not the 500 characters kept"
        );
        for (field, value) in [
            ("created_at", json!("2026-11-14T09:00:00+00:00")),
            ("completed_at", json!("2026-11-14T09:00:00+00:00")),
            ("project", json!("elsewhere")),
        ] {
            let mut same = base.clone();
            same[field] = value;
            assert_eq!(
                read_one(same).await.change_key,
                todo.change_key,
                "{field} is not part of the change key"
            );
        }
    }

    #[tokio::test]
    async fn only_list_todos_is_called_and_failures_never_show_the_token() {
        let capture = LogCapture::start();
        let fake = FakeMemcan::start();
        // The test fake knows no `list_todos`: MemCan's invalid-params answer, a note per project.
        let answered = read(&fake.config(), &["a".to_owned(), "b".to_owned()]).await;
        assert_eq!(
            notes(&answered),
            [
                "MemCan project a: MemCan refused the TODO list; its TODOs are left out.",
                "MemCan project b: MemCan refused the TODO list; its TODOs are left out.",
            ]
        );
        let tools: Vec<String> = fake.calls().into_iter().map(|(tool, _)| tool).collect();
        assert_eq!(
            tools,
            ["list_todos", "list_todos"],
            "read-only: nothing else"
        );

        let refused = read(
            &fake.config_with_token("wrong-token-CANARY-0815"),
            &["a".to_owned()],
        )
        .await;
        assert_eq!(
            notes(&refused),
            ["MemCan unreachable or token rejected (check SR_MEMCAN_TOKEN): TODOs not refreshed."]
        );
        fake.set_mode(Mode::HangAll);
        let hung = read(&fake.config(), &["a".to_owned()]).await;
        assert!(matches!(hung, TodosRead::Unavailable { .. }), "{hung:?}");
        let logs = capture.text();
        for leak in [TOKEN, "wrong-token-CANARY-0815", "mcp-url-CANARY"] {
            assert!(!logs.contains(leak), "{leak} in the logs: {logs}");
            assert!(
                !format!("{answered:?}{refused:?}{hung:?}").contains(leak),
                "{leak} in a note"
            );
        }
        assert!(
            fake.calls().iter().all(|(tool, _)| tool == "list_todos"),
            "{:?}",
            fake.calls()
        );
    }
}
