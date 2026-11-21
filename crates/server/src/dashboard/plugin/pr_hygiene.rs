//! The PR Hygiene plug-in (ADR-022 §9): the public JSON API of the PR Hygiene service (`/api/v1/prs`,
//! `/api/v1/stages`), read with `If-None-Match`. Governed pull requests of repositories that are not stale get a
//! statement: the engine's next action, its blockers, stage, whose move and lateness, and a hold on Ready to merge
//! unless the engine state is `ready-to-merge`. All of it is untrusted data.

use std::collections::HashMap;
use std::time::Duration;

use contracts::api::dashboard::Fact;
use contracts::time::Timestamp;
use futures::future::BoxFuture;
use reqwest::header::{ACCEPT, ETAG, IF_NONE_MATCH};
use reqwest::{Client, StatusCode, redirect};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use url::Url;

use super::{DashboardPlugin, PluginData, PluginError, Polled, PrKey, PrStatement, Readiness};
use crate::github::work::ItemRef;

/// Bound of one request.
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest answer read.
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
/// The engine state of a pull request the engine lists as ready to merge.
const READY_TO_MERGE: &str = "ready-to-merge";
/// Values of a record that change with time alone, left out of its change mark.
const TIME_ONLY: [&str; 2] = ["lateness", "idle"];

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
    #[serde(default)]
    blockers: Vec<String>,
    lateness: Option<String>,
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

/// One request's answer.
enum Answer<T> {
    NotModified,
    Body { etag: Option<String>, body: T },
}

impl PrHygiene {
    /// The service at `base` (its root; a missing trailing slash is added).
    pub fn new(mut base: Url) -> Result<Self, reqwest::Error> {
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        let http = Client::builder()
            .redirect(redirect::Policy::none())
            .timeout(HTTP_TIMEOUT)
            .user_agent("diffpilot")
            .build()?;
        Ok(Self {
            base,
            http,
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
        let mut request = self
            .http
            .get(self.endpoint(path))
            .header(ACCEPT, "application/json");
        if let Some(etag) = etag {
            request = request.header(IF_NONE_MATCH, etag);
        }
        let mut response = request.send().await.map_err(PluginError::Http)?;
        let status = response.status();
        if status == StatusCode::NOT_MODIFIED {
            return if etag.is_some() {
                Ok(Answer::NotModified)
            } else {
                Err(PluginError::UnexpectedNotModified)
            };
        }
        if !status.is_success() {
            return Err(PluginError::Status(status.as_u16()));
        }
        let etag = response
            .headers()
            .get(ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(PluginError::Http)? {
            if bytes.len() + chunk.len() > MAX_BODY_BYTES {
                return Err(PluginError::TooLarge {
                    limit: MAX_BODY_BYTES,
                });
            }
            bytes.extend_from_slice(&chunk);
        }
        let body = serde_json::from_slice(&bytes).map_err(PluginError::Malformed)?;
        Ok(Answer::Body { etag, body })
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
        let (data, unreadable) = self.data(&prs_body, &stages_body);
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
            if let Some(statement) = self.statement(&item, &record, value, &owners) {
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
    ) -> Option<PrStatement> {
        let engine_state = record.engine_state.as_deref()?;
        let mut facts = Vec::new();
        if let Some(stage) = &record.stage {
            facts.push(fact("Stage", stage));
            if let Some(owner) = owners.get(stage.as_str()) {
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
        let ready_to_merge = if engine_state == READY_TO_MERGE {
            Readiness::Agrees
        } else {
            Readiness::Holds {
                reason: record
                    .next_action
                    .clone()
                    .or_else(|| record.stage.as_ref().map(|s| format!("Stage: {s}")))
                    .unwrap_or_else(|| format!("Engine state: {engine_state}")),
            }
        };
        Some(PrStatement {
            url: self
                .endpoint(&format!(
                    "api/v1/prs/{}/{}/{}",
                    item.repo.owner, item.repo.name, item.number
                ))
                .to_string(),
            summary: record.next_action.clone(),
            details: record.blockers.clone(),
            facts,
            ready_to_merge: Some(ready_to_merge),
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

impl DashboardPlugin for PrHygiene {
    fn id(&self) -> &'static str {
        "pr-hygiene"
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
}
