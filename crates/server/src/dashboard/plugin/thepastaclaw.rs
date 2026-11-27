//! The thepastaclaw status plug-in (ADR-022 §10): the review-system's public status file (`status.json` on GitHub
//! Pages), read with `If-None-Match`. A pull request in `live.active` gets "reviewing (<phase>, step n of m)", one in
//! `live.queued` "queued (<reason>)"; neither gets a Ready to merge opinion. The change mark follows the run and
//! the head, so a finished round (the pull request leaves `live.active`) is a change. All of it is untrusted data.

use std::collections::HashMap;

use contracts::api::dashboard::Fact;
use contracts::time::Timestamp;
use futures::future::BoxFuture;
use reqwest::Client;
use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::Value;
use tokio::sync::Mutex;
use url::Url;

use super::fetch::{self, Answer};
use super::{DashboardPlugin, PluginData, PluginError, Polled, PrKey, PrStatement};
use crate::github::work::ItemRef;

/// Characters of a head commit shown.
const SHORT_SHA: usize = 7;

/// thepastaclaw's status file at a URL.
#[derive(Debug)]
pub struct Thepastaclaw {
    status: Url,
    home: Url,
    http: Client,
    cache: Mutex<Cache>,
}

#[derive(Debug, Default)]
struct Cache {
    etag: Option<String>,
    /// Records of the last data left out as unreadable (logged when the count changes).
    unreadable: usize,
}

#[derive(Debug, Deserialize)]
struct StatusBody {
    data_as_of: Timestamp,
    live: Live,
}

#[derive(Debug, Deserialize)]
struct Live {
    active: Vec<Value>,
    queued: Vec<Value>,
}

/// The fields of a `live.active` run a statement uses.
#[derive(Debug, Deserialize)]
struct Run {
    repo: String,
    number: u32,
    id: Option<u64>,
    sha: Option<String>,
    started_at: Option<Timestamp>,
    phase: Option<String>,
    steps: Option<Vec<IgnoredAny>>,
    upcoming: Option<Vec<IgnoredAny>>,
}

/// The fields of a `live.queued` entry a statement uses.
#[derive(Debug, Deserialize)]
struct Queued {
    repo: String,
    number: u32,
    sha: Option<String>,
    reason: Option<String>,
    eligible_at: Option<Timestamp>,
    position: Option<u32>,
}

impl Thepastaclaw {
    /// The status file at `status`; its home page is the directory above the file's (`review-system/`).
    pub fn new(status: Url) -> Result<Self, reqwest::Error> {
        let home = status.join("../").unwrap_or_else(|_| status.clone());
        Ok(Self {
            status,
            home,
            http: fetch::client()?,
            cache: Mutex::default(),
        })
    }

    async fn read(&self) -> Result<Polled, PluginError> {
        let mut cache = self.cache.lock().await;
        let answer =
            fetch::get_json::<StatusBody>(&self.http, self.status.clone(), cache.etag.as_deref())
                .await?;
        let (etag, body) = match answer {
            Answer::NotModified => return Ok(Polled::Unchanged),
            Answer::Body { etag, body } => (etag, body),
        };
        cache.etag = etag;
        let (data, unreadable) = self.data(&body);
        if unreadable != cache.unreadable {
            if unreadable > 0 {
                tracing::warn!(
                    unreadable,
                    total = body.live.active.len() + body.live.queued.len(),
                    "thepastaclaw status records of an unexpected shape or repository name are left out; check the status file's schema_version"
                );
            } else {
                tracing::info!("thepastaclaw status records are all readable again");
            }
            cache.unreadable = unreadable;
        }
        Ok(Polled::Data(data))
    }

    /// The statements of `body`, and how many records were left out as unreadable (shape or repository name).
    fn data(&self, body: &StatusBody) -> (PluginData, usize) {
        let mut unreadable = 0;
        let mut runs: HashMap<PrKey, Run> = HashMap::new();
        for value in &body.live.active {
            match record::<Run>(value, |r| (&r.repo, r.number)) {
                Some((key, run)) => {
                    runs.entry(key).or_insert(run);
                }
                None => unreadable += 1,
            }
        }
        let mut queue: HashMap<PrKey, Queued> = HashMap::new();
        for value in &body.live.queued {
            match record::<Queued>(value, |q| (&q.repo, q.number)) {
                Some((key, queued)) => {
                    queue.entry(key).or_insert(queued);
                }
                None => unreadable += 1,
            }
        }
        let mut statements: HashMap<PrKey, PrStatement> = runs
            .iter()
            .map(|(key, run)| (key.clone(), self.reviewing(run)))
            .collect();
        for (key, queued) in &queue {
            match statements.get_mut(key) {
                Some(reviewing) => {
                    reviewing.details.push(again(queued));
                    reviewing.mark = format!("{} + {}", reviewing.mark, queued_mark(queued));
                }
                None => {
                    statements.insert(key.clone(), self.waiting(queued));
                }
            }
        }
        (
            PluginData {
                as_of: body.data_as_of,
                prs: statements,
            },
            unreadable,
        )
    }

    fn reviewing(&self, run: &Run) -> PrStatement {
        let step = match (&run.steps, &run.upcoming) {
            (Some(done), upcoming) if !done.is_empty() => Some(format!(
                "step {} of {}",
                done.len(),
                done.len() + upcoming.as_ref().map_or(0, Vec::len)
            )),
            _ => None,
        };
        let within: Vec<&str> = run
            .phase
            .iter()
            .map(String::as_str)
            .chain(step.as_deref())
            .collect();
        let summary = if within.is_empty() {
            "reviewing".to_owned()
        } else {
            format!("reviewing ({})", within.join(", "))
        };
        let mut facts = Vec::new();
        if let Some(started_at) = run.started_at {
            facts.push(fact("Since", &clock(started_at)));
        }
        if let Some(head) = short(run.sha.as_deref()) {
            facts.push(fact("Head", &head));
        }
        let mut url = self.home.clone();
        if let Some(id) = run.id {
            url.set_fragment(Some(&format!("run={id}")));
        }
        PrStatement {
            url: url.to_string(),
            summary: Some(summary),
            details: Vec::new(),
            facts,
            actions: Vec::new(),
            ready_to_merge: None,
            whose_move: None,
            github_updated_at: None,
            mark: format!("reviewing {:?} {:?}", run.id, run.sha),
        }
    }

    fn waiting(&self, queued: &Queued) -> PrStatement {
        let mut facts = Vec::new();
        if let Some(eligible_at) = queued.eligible_at {
            facts.push(fact("Eligible", &clock(eligible_at)));
        }
        if let Some(position) = queued.position {
            facts.push(fact("Position", &position.to_string()));
        }
        if let Some(head) = short(queued.sha.as_deref()) {
            facts.push(fact("Head", &head));
        }
        let mut url = self.home.clone();
        url.set_fragment(Some("queue"));
        PrStatement {
            url: url.to_string(),
            summary: Some(match &queued.reason {
                Some(reason) => format!("queued ({reason})"),
                None => "queued".to_owned(),
            }),
            details: Vec::new(),
            facts,
            actions: Vec::new(),
            ready_to_merge: None,
            whose_move: None,
            github_updated_at: None,
            mark: queued_mark(queued),
        }
    }
}

/// `value` read as `T` with its validated pull request; `None` for a record of another shape or repository name.
fn record<T: for<'de> Deserialize<'de>>(
    value: &Value,
    item: impl Fn(&T) -> (&String, u32),
) -> Option<(PrKey, T)> {
    let parsed = serde_json::from_value::<T>(value.clone()).ok()?;
    let (repo, number) = item(&parsed);
    let item = repo
        .split_once('/')
        .and_then(|(owner, name)| ItemRef::new(owner, name, number))?;
    Some((PrKey::new(&item.repo, item.number), parsed))
}

/// The detail on a reviewed pull request that is queued again (a newer head).
fn again(queued: &Queued) -> String {
    let mut text = "queued again".to_owned();
    if let Some(head) = short(queued.sha.as_deref()) {
        text.push_str(&format!(" for head {head}"));
    }
    if let Some(reason) = &queued.reason {
        text.push_str(&format!(" ({reason})"));
    }
    text
}

fn queued_mark(queued: &Queued) -> String {
    format!("queued {:?}", queued.sha)
}

/// The first characters of a hexadecimal commit id; `None` for anything else.
fn short(sha: Option<&str>) -> Option<String> {
    let sha = sha?;
    (sha.len() >= SHORT_SHA && sha.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| sha[..SHORT_SHA].to_owned())
}

/// A time as UTC text (the browser formats only the data time; facts are plain text).
fn clock(at: Timestamp) -> String {
    at.get().strftime("%Y-%m-%d %H:%M UTC").to_string()
}

fn fact(label: &str, value: &str) -> Fact {
    Fact {
        label: label.to_owned(),
        value: value.to_owned(),
    }
}

/// The plug-in's id.
pub const ID: &str = "thepastaclaw";

impl DashboardPlugin for Thepastaclaw {
    fn id(&self) -> &'static str {
        ID
    }

    fn name(&self) -> &'static str {
        "thepastaclaw"
    }

    fn home(&self) -> &Url {
        &self.home
    }

    fn poll(&self) -> BoxFuture<'_, Result<Polled, PluginError>> {
        Box::pin(self.read())
    }
}
