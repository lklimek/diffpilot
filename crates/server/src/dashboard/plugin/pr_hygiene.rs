//! The PR Hygiene plug-in (ADR-022 §9): the public JSON API of the PR Hygiene service (`/api/v1/prs`,
//! `/api/v1/stages`), read with `If-None-Match`. Governed pull requests of repositories that are not stale get a
//! statement: the engine's next action, its blockers, stage, whose move and lateness, a hold on Ready to merge
//! unless the engine state is `ready-to-merge`, and whose move it is for Waiting on others (the stage's owner; the
//! `asks` approvers owe a review). All of it is untrusted data.

mod context;

use std::collections::HashMap;
use std::time::Duration;

use contracts::api::dashboard::Fact;
use contracts::time::Timestamp;
use futures::future::BoxFuture;
use futures::{StreamExt, stream};
use reqwest::Client;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tokio::time::{Instant, timeout_at};
use url::Url;

use super::fetch::{self, Answer};
use super::{
    DashboardPlugin, Move, PluginData, PluginError, Polled, PrKey, PrStatement, Readiness,
};
use crate::github::work::ItemRef;

/// The engine state of a pull request the engine lists as ready to merge.
const READY_TO_MERGE: &str = "ready-to-merge";
/// Stage owners whose stage waits on people other than the author (`/api/v1/stages`).
const PEOPLE_OWNERS: [&str; 2] = ["reviewers", "maintainers"];
/// Values of a record that change with time alone, left out of its change mark.
const TIME_ONLY: [&str; 2] = ["lateness", "idle"];
/// Optional history must leave time for the aggregate notes and the other plug-ins.
const HISTORY_BUDGET: Duration = Duration::from_secs(2);

/// The PR Hygiene service at a base URL.
#[derive(Debug)]
pub struct PrHygiene {
    base: Url,
    http: Client,
    cache: Mutex<Cache>,
}

/// The last answers and their validators.
#[derive(Debug, Default)]
struct Cache {
    prs: Option<(String, PrsBody)>,
    stages: Option<(String, StagesBody)>,
    /// Records of the last data left out as unreadable (logged when the count changes).
    unreadable: usize,
    /// Per-PR history, reused until its aggregate record changes.
    details: HashMap<PrKey, (String, DetailBody)>,
}

#[derive(Clone, Debug, Deserialize)]
struct PrsBody {
    generated_at: Timestamp,
    prs: Vec<Value>,
    #[serde(default)]
    stale_repos: Vec<String>,
}

/// The fields of a `/prs` record a statement uses.
#[derive(Debug, Deserialize)]
struct PrRecord {
    repo: String,
    number: u32,
    engine_state: Option<String>,
    stage: Option<String>,
    next_action: Option<String>,
    lateness: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct DetailBody {
    generated_at: Timestamp,
    pr: DetailPr,
    stage_changes: Vec<Value>,
    stale: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct DetailPr {
    repo: String,
    number: u32,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct StagesBody {
    stages: Vec<StageRecord>,
}

#[derive(Clone, Debug, Deserialize)]
struct StageRecord {
    stage: String,
    owner: Option<String>,
}

impl PrHygiene {
    /// The service at `base` (its root; a missing trailing slash is added).
    pub fn new(mut base: Url) -> Result<Self, reqwest::Error> {
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        Ok(Self {
            base,
            http: fetch::client()?,
            cache: Mutex::default(),
        })
    }

    fn endpoint(&self, path: &str) -> Url {
        // `path` is relative (constants and validated owner, name and number); joining it to the base cannot fail.
        self.base.join(path).unwrap_or_else(|_| self.base.clone())
    }

    async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        etag: Option<&str>,
    ) -> Result<Answer<T>, PluginError> {
        fetch::get_json(&self.http, self.endpoint(path), etag).await
    }

    async fn read(&self) -> Result<Polled, PluginError> {
        let mut cache = self.cache.lock().await;
        let prs_etag = cache.prs.as_ref().map(|(etag, _)| etag.as_str());
        let stages_etag = cache.stages.as_ref().map(|(etag, _)| etag.as_str());
        let (prs, stages) = tokio::try_join!(
            self.get::<PrsBody>("api/v1/prs", prs_etag),
            self.get::<StagesBody>("api/v1/stages", stages_etag),
        )?;
        let (prs_body, stages_body) = match (prs, stages) {
            (Answer::Body { etag, body }, stages) => {
                let stages = match stages {
                    Answer::Body { etag, body } => {
                        cache.stages = etag.map(|etag| (etag, body.clone()));
                        body
                    }
                    Answer::NotModified => cached(&cache.stages)?,
                };
                cache.prs = etag.map(|etag| (etag, body.clone()));
                (body, stages)
            }
            (Answer::NotModified, Answer::Body { etag, body }) => {
                cache.stages = etag.map(|etag| (etag, body.clone()));
                (cached(&cache.prs)?, body)
            }
            (Answer::NotModified, Answer::NotModified) => return Ok(Polled::Unchanged),
        };
        let (mut data, unreadable) = self.data(&prs_body, &stages_body);
        self.history(&mut data, &mut cache).await;
        if unreadable != cache.unreadable {
            if unreadable > 0 {
                tracing::warn!(
                    unreadable,
                    total = prs_body.prs.len(),
                    "PR Hygiene records of an unexpected shape or repository name are left out; check the service's API version"
                );
            } else {
                tracing::info!("PR Hygiene records are all readable again");
            }
            cache.unreadable = unreadable;
        }
        Ok(Polled::Data(data))
    }

    /// History is absent from `/prs`; read it only for changed governed records, at most four at once.
    async fn history(&self, data: &mut PluginData, cache: &mut Cache) {
        cache.details.retain(|key, _| data.prs.contains_key(key));
        let pending: Vec<_> = data
            .prs
            .iter()
            .filter_map(|(key, statement)| {
                if cache
                    .details
                    .get(key)
                    .is_some_and(|(mark, _)| mark == &statement.mark)
                {
                    return None;
                }
                Some((key.clone(), statement.mark.clone()))
            })
            .collect();
        let deadline = Instant::now() + HISTORY_BUDGET;
        let mut results = stream::iter(pending)
            .map(|(key, mark)| async move {
                let path = format!("api/v1/prs/{}/{}/{}", key.owner, key.name, key.number);
                (key, mark, self.get::<DetailBody>(&path, None).await)
            })
            .buffer_unordered(4);
        while let Ok(Some((key, mark, result))) = timeout_at(deadline, results.next()).await {
            if let Ok(Answer::Body { body, .. }) = result
                && body
                    .pr
                    .repo
                    .eq_ignore_ascii_case(&format!("{}/{}", key.owner, key.name))
                && body.pr.number == key.number
            {
                cache.details.insert(key, (mark, body));
            }
        }
        for (key, statement) in &mut data.prs {
            if let Some((mark, detail)) = cache.details.get(key)
                && *mark == statement.mark
            {
                statement
                    .details
                    .extend(context::history(&detail.stage_changes));
                statement.details.push(format!(
                    "Stale: {}",
                    if detail.stale { "Yes" } else { "No" }
                ));
                statement
                    .details
                    .push(format!("Data generated: {}", detail.generated_at));
            } else {
                statement
                    .details
                    .push("Stage history: unavailable".to_owned());
            }
        }
    }

    /// The statements of `prs`, and how many records were left out as unreadable (shape or repository name).
    fn data(&self, prs: &PrsBody, stages: &StagesBody) -> (PluginData, usize) {
        let owners: HashMap<&str, &str> = stages
            .stages
            .iter()
            .filter_map(|s| Some((s.stage.as_str(), s.owner.as_deref()?)))
            .collect();
        let stale: Vec<String> = prs
            .stale_repos
            .iter()
            .map(|r| r.to_ascii_lowercase())
            .collect();
        let mut statements = HashMap::new();
        let mut unreadable = 0;
        for value in &prs.prs {
            let Ok(record) = serde_json::from_value::<PrRecord>(value.clone()) else {
                unreadable += 1;
                continue;
            };
            if stale.contains(&record.repo.to_ascii_lowercase()) {
                continue;
            }
            let Some(item) = record
                .repo
                .split_once('/')
                .and_then(|(owner, name)| ItemRef::new(owner, name, record.number))
            else {
                unreadable += 1;
                continue;
            };
            if let Some(statement) =
                self.statement(&item, &record, value, &owners, prs.generated_at)
            {
                statements.insert(PrKey::new(&item.repo, item.number), statement);
            }
        }
        (
            PluginData {
                as_of: prs.generated_at,
                prs: statements,
            },
            unreadable,
        )
    }

    /// The statement on a governed pull request; `None` for an ungoverned one (no engine state).
    fn statement(
        &self,
        item: &ItemRef,
        record: &PrRecord,
        value: &Value,
        owners: &HashMap<&str, &str>,
        as_of: Timestamp,
    ) -> Option<PrStatement> {
        let engine_state = record.engine_state.as_deref()?;
        let mut facts = Vec::new();
        let mut owner = None;
        if let Some(stage) = &record.stage {
            facts.push(fact("Stage", &context::readable(stage)));
            owner = owners.get(stage.as_str()).copied();
            if let Some(owner) = owner {
                facts.push(fact("Whose move", owner));
            }
        }
        if let Some(lateness) = record.lateness.as_deref() {
            let text = match lateness {
                "ok" => "on time",
                "late" => "late",
                "very-late" => "very late",
                other => other,
            };
            facts.push(fact("Lateness", text));
        }
        facts.push(fact("Engine state", &context::readable(engine_state)));
        facts.extend(context::timing(value, as_of));
        let reason = record
            .next_action
            .clone()
            .or_else(|| {
                record
                    .stage
                    .as_ref()
                    .map(|s| format!("Stage: {}", context::readable(s)))
            })
            .unwrap_or_else(|| format!("Engine state: {}", context::readable(engine_state)));
        let whose_move = match owner {
            // A configuration error's blocked stage names maintainers, but establishes no review to wait for.
            _ if engine_state == "configuration-error" => None,
            Some("author") => Some(Move::Author),
            Some(owner) if PEOPLE_OWNERS.contains(&owner) => Some(Move::People {
                owed: approvers(value),
                reason: reason.clone(),
            }),
            _ => None,
        };
        let ready_to_merge = if engine_state == READY_TO_MERGE {
            Readiness::Agrees
        } else {
            Readiness::Holds { reason }
        };
        Some(PrStatement {
            url: self
                .endpoint(&format!(
                    "api/v1/prs/{}/{}/{}",
                    item.repo.owner, item.repo.name, item.number
                ))
                .to_string(),
            summary: record.next_action.clone(),
            details: context::details(value),
            facts,
            ready_to_merge: Some(ready_to_merge),
            whose_move,
            github_updated_at: value
                .get("updated_at")
                .and_then(Value::as_str)
                .and_then(|at| at.parse().ok()),
            mark: mark(value),
        })
    }
}

fn cached<T: Clone>(entry: &Option<(String, T)>) -> Result<T, PluginError> {
    entry
        .as_ref()
        .map(|(_, body)| body.clone())
        .ok_or(PluginError::UnexpectedNotModified)
}

fn fact(label: &str, value: &str) -> Fact {
    Fact {
        label: label.to_owned(),
        value: value.to_owned(),
    }
}

/// The logins of every `asks[].approvers` of a record, in order; an entry of another shape is skipped.
fn approvers(value: &Value) -> Vec<String> {
    value
        .get("asks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|ask| ask.get("approvers").and_then(Value::as_array))
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// Hash of the record without its time-only values.
fn mark(value: &Value) -> String {
    let mut value = value.clone();
    if let Some(object) = value.as_object_mut() {
        for key in TIME_ONLY {
            object.remove(key);
        }
    }
    Sha256::digest(value.to_string().as_bytes())
        .iter()
        .take(16)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The plug-in's id.
pub const ID: &str = "pr-hygiene";

impl DashboardPlugin for PrHygiene {
    fn id(&self) -> &'static str {
        ID
    }

    fn name(&self) -> &'static str {
        "PR Hygiene"
    }

    fn home(&self) -> &Url {
        &self.base
    }

    fn poll(&self) -> BoxFuture<'_, Result<Polled, PluginError>> {
        Box::pin(self.read())
    }

    fn holds_ready_to_merge(&self) -> bool {
        true
    }
}
