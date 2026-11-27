//! Dashboard plug-ins (ADR-022): external, read-only sources of statements about pull requests. The host polls
//! them at most once per [`Timing::interval`] server-wide and only while a user reads the dashboard or any user
//! watches a pull request ([`PluginHost::set_watching`]), keeps their data in memory, serves the current statements
//! to the view ([`Overlay`]) and reports which pull requests changed (the change signal behind the targeted
//! re-read, `refresh::reread`).

mod fetch;
pub mod pr_hygiene;
pub mod thepastaclaw;

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use contracts::api::dashboard::{Fact, PluginNote, PluginState, PluginStatus};
use contracts::common::RepoRef;
use contracts::time::Timestamp;
use futures::future::BoxFuture;
use jiff::SignedDuration;
use tokio::sync::{Notify, watch};
use tokio::time::Instant;
use url::Url;

use crate::config::Config;
use crate::state::AppState;

/// Characters kept of a summary or a detail.
const TEXT_CHARS: usize = 300;
/// Details kept per statement.
const DETAILS_KEPT: usize = 64;
/// Facts kept per statement.
const FACTS_KEPT: usize = 6;
/// Logins kept of those owing a review.
const OWED_KEPT: usize = 10;
/// Characters kept of a fact's label or value.
const FACT_CHARS: usize = 100;

/// A dashboard plug-in: one external source of statements about pull requests.
pub trait DashboardPlugin: Send + Sync + fmt::Debug {
    /// Stable, URL-safe id, e.g. `pr-hygiene`.
    fn id(&self) -> &'static str;
    /// Name shown before every statement, e.g. `PR Hygiene`.
    fn name(&self) -> &'static str;
    /// The source's home page.
    fn home(&self) -> &Url;
    /// Reads the source once.
    fn poll(&self) -> BoxFuture<'_, Result<Polled, PluginError>>;
    /// Whether its statements can hold a pull request back from Ready to merge.
    fn holds_ready_to_merge(&self) -> bool {
        false
    }
}

/// The answer of one poll.
#[derive(Clone, Debug, PartialEq)]
pub enum Polled {
    /// The source says nothing changed since the previous successful poll (e.g. HTTP 304).
    Unchanged,
    /// The source's current data.
    Data(PluginData),
}

/// What a plug-in says now.
#[derive(Clone, Debug, PartialEq)]
pub struct PluginData {
    /// When the source produced the data (its clock).
    pub as_of: Timestamp,
    /// Statements by pull request; a pull request without one is not covered.
    pub prs: HashMap<PrKey, PrStatement>,
}

/// A pull request, compared ignoring the ASCII case of owner and name.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PrKey {
    owner: String,
    name: String,
    number: u32,
}

impl PrKey {
    /// The key of `repo#number`.
    pub fn new(repo: &RepoRef, number: u32) -> Self {
        Self {
            owner: repo.owner.to_ascii_lowercase(),
            name: repo.name.to_ascii_lowercase(),
            number,
        }
    }
}

/// A plug-in's statement about one pull request.
#[derive(Clone, Debug, PartialEq)]
pub struct PrStatement {
    /// Evidence: the source's record of the pull request (`http`/`https`).
    pub url: String,
    /// Headline.
    pub summary: Option<String>,
    /// Further points.
    pub details: Vec<String>,
    /// Labelled values.
    pub facts: Vec<Fact>,
    /// Ordered actionable statements for Comments.
    pub actions: Vec<String>,
    /// Opinion on Ready to merge; `None`: no opinion.
    pub ready_to_merge: Option<Readiness>,
    /// Whose move it is (Waiting on others, K15 W3b); `None`: no opinion.
    pub whose_move: Option<Move>,
    /// GitHub's PR update time observed by the source; absent without GitHub clock evidence.
    pub github_updated_at: Option<Timestamp>,
    /// Changes whenever the pull request's state at the source changes (time-only values excluded).
    pub mark: String,
}

/// A plug-in's opinion on Ready to merge (K17).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Readiness {
    /// The source lists it as ready to merge as well.
    Agrees,
    /// The source holds it back, for `reason`.
    Holds {
        /// Plain text.
        reason: String,
    },
}

/// Whose move a plug-in says a pull request is (K15 W3b).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Move {
    /// The author's: it does not wait on others.
    Author,
    /// The reviewers' or maintainers': it waits on people.
    People {
        /// GitHub logins the source says owe a review; empty when it names none.
        owed: Vec<String>,
        /// Plain text.
        reason: String,
    },
}

/// A poll failed; the plug-in's previous data ages out.
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    /// No answer (connection, timeout, TLS).
    #[error("the plug-in's source could not be reached")]
    Http(#[source] reqwest::Error),
    /// An answer other than 2xx or 304.
    #[error("the plug-in's source answered HTTP {0}")]
    Status(u16),
    /// The answer was longer than the limit.
    #[error("the plug-in's source answered with more than {limit} bytes")]
    TooLarge {
        /// Bytes accepted.
        limit: usize,
    },
    /// The answer was not JSON of the expected shape.
    #[error("the plug-in's source answered with data of an unexpected shape")]
    Malformed(#[source] serde_json::Error),
    /// 304 to a request sent without a validator.
    #[error("the plug-in's source answered 304 to a first read")]
    UnexpectedNotModified,
}

impl PluginError {
    /// Kind of failure, for logging changes only.
    fn kind(&self) -> String {
        match self {
            Self::Http(_) => "unreachable".to_owned(),
            Self::Status(status) => format!("http {status}"),
            Self::TooLarge { .. } => "too large".to_owned(),
            Self::Malformed(_) | Self::UnexpectedNotModified => "malformed".to_owned(),
        }
    }
}

/// Cadence and freshness of the host.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// Shortest time between two polls of the host.
    pub interval: Duration,
    /// Data is shown while its last successful poll is at most this old.
    pub stale_after: Duration,
    /// Data is shown while its `as_of` is at most this old.
    pub max_age: SignedDuration,
    /// A dashboard read keeps its reader active this long.
    pub active_for: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(60),
            stale_after: Duration::from_secs(5 * 60),
            max_age: SignedDuration::from_hours(1),
            active_for: Duration::from_secs(3 * 60),
        }
    }
}

/// The configured plug-ins and what they said; cheap to clone.
#[derive(Clone, Default)]
pub struct PluginHost {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    slots: Vec<Slot>,
    timing: Timing,
    /// Active readers (tenant, user) and their last dashboard read.
    active: Mutex<HashMap<(String, String), Instant>>,
    /// Some user watches a pull request.
    watching: AtomicBool,
    /// Woken when a reader becomes active or a first watch appears.
    woken: Notify,
}

struct Slot {
    plugin: Arc<dyn DashboardPlugin>,
    state: Mutex<SlotState>,
}

#[derive(Default)]
struct SlotState {
    data: Option<Arc<PluginData>>,
    last_success: Option<Instant>,
    /// Failure kind of the last poll; `None` after a success.
    failing: Option<String>,
    /// Whether the data was shown at the last poll (logged when it changes).
    shown: bool,
}

impl fmt::Debug for PluginHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PluginHost")
            .field(
                "plugins",
                &self
                    .inner
                    .slots
                    .iter()
                    .map(|s| s.plugin.id())
                    .collect::<Vec<_>>(),
            )
            .field("timing", &self.inner.timing)
            .finish_non_exhaustive()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl PluginHost {
    /// The plug-ins `config` enables (none when no plug-in variable is set).
    pub fn from_config(config: &Config) -> Self {
        let mut plugins: Vec<Arc<dyn DashboardPlugin>> = Vec::new();
        if let Some(base) = &config.pr_hygiene_url {
            match pr_hygiene::PrHygiene::new(base.clone()) {
                Ok(plugin) => plugins.push(Arc::new(plugin)),
                Err(error) => {
                    tracing::error!(%error, "the PR Hygiene plug-in could not be set up; it stays off")
                }
            }
        }
        if let Some(status) = &config.thepastaclaw_status_url {
            match thepastaclaw::Thepastaclaw::new(status.clone()) {
                Ok(plugin) => plugins.push(Arc::new(plugin)),
                Err(error) => {
                    tracing::error!(%error, "the thepastaclaw status plug-in could not be set up; it stays off")
                }
            }
        }
        Self::new(plugins, Timing::default())
    }

    /// A host of `plugins` with `timing`.
    pub fn new(plugins: Vec<Arc<dyn DashboardPlugin>>, timing: Timing) -> Self {
        Self {
            inner: Arc::new(Inner {
                slots: plugins
                    .into_iter()
                    .map(|plugin| Slot {
                        plugin,
                        state: Mutex::default(),
                    })
                    .collect(),
                timing,
                active: Mutex::default(),
                watching: AtomicBool::new(false),
                woken: Notify::new(),
            }),
        }
    }

    /// No plug-in is configured.
    pub fn is_empty(&self) -> bool {
        self.inner.slots.is_empty()
    }

    /// Cadence and freshness.
    pub fn timing(&self) -> Timing {
        self.inner.timing
    }

    /// Records a dashboard read of the user; a reader who was not active wakes the poller.
    pub fn mark_active(&self, tenant_id: &str, user_id: &str) {
        if self.is_empty() {
            return;
        }
        let now = Instant::now();
        let active_for = self.inner.timing.active_for;
        let previous =
            lock(&self.inner.active).insert((tenant_id.to_owned(), user_id.to_owned()), now);
        if previous.is_none_or(|at| now.duration_since(at) > active_for) {
            self.inner.woken.notify_one();
        }
    }

    /// The users who read the dashboard within [`Timing::active_for`]; forgets the others.
    pub fn active_users(&self) -> Vec<(String, String)> {
        let active_for = self.inner.timing.active_for;
        let mut active = lock(&self.inner.active);
        active.retain(|_, at| at.elapsed() <= active_for);
        let mut users: Vec<_> = active.keys().cloned().collect();
        users.sort();
        users
    }

    /// Records whether any user watches a pull request (the watch list's presence source; none until it reports);
    /// a first watch wakes the poller.
    pub fn set_watching(&self, watching: bool) {
        if self.is_empty() {
            return;
        }
        let was = self.inner.watching.swap(watching, Ordering::SeqCst);
        if watching && !was {
            self.inner.woken.notify_one();
        }
    }

    /// Some user watches a pull request.
    pub fn is_watching(&self) -> bool {
        self.inner.watching.load(Ordering::SeqCst)
    }

    /// Whether the poller has a reason to poll: an active reader or a watched pull request.
    fn present(&self) -> bool {
        self.is_watching() || !self.active_users().is_empty()
    }

    /// Waits until a reader becomes active or a first watch appears (returns at once when either may have happened
    /// since the last wait).
    pub async fn woken(&self) {
        self.inner.woken.notified().await;
    }

    fn fresh(&self, state: &SlotState, now: Timestamp) -> Option<Arc<PluginData>> {
        let timing = self.inner.timing;
        let data = state.data.as_ref()?;
        let recent = state
            .last_success
            .is_some_and(|at| at.elapsed() <= timing.stale_after);
        let young = now.duration_since(data.as_of) <= timing.max_age;
        (recent && young).then(|| Arc::clone(data))
    }

    /// The statements shown at `now`: those of plug-ins whose data is fresh.
    pub fn overlay(&self, now: Timestamp) -> Overlay {
        Overlay {
            now: Some(now),
            sources: self
                .inner
                .slots
                .iter()
                .filter_map(|slot| {
                    let data = self.fresh(&lock(&slot.state), now)?;
                    Some(Source {
                        id: slot.plugin.id(),
                        name: slot.plugin.name(),
                        data,
                    })
                })
                .collect(),
        }
    }

    /// The configured plug-ins and whether their statements are shown at `now`. Data that only aged while nobody
    /// was active (no failure since the last success, `as_of` within [`Timing::max_age`]) is `waiting`: the next
    /// read refreshes it; `stale` means the source failed or its data is too old.
    pub fn statuses(&self, now: Timestamp) -> Vec<PluginStatus> {
        self.inner
            .slots
            .iter()
            .map(|slot| {
                let state = lock(&slot.state);
                PluginStatus {
                    plugin: slot.plugin.id().to_owned(),
                    source: slot.plugin.name().to_owned(),
                    url: slot.plugin.home().to_string(),
                    state: if self.fresh(&state, now).is_some() {
                        PluginState::Current
                    } else if state.failing.is_none()
                        && state.data.as_ref().is_none_or(|data| {
                            now.duration_since(data.as_of) <= self.inner.timing.max_age
                        })
                    {
                        PluginState::Waiting
                    } else {
                        PluginState::Stale
                    },
                    as_of: state.data.as_ref().map(|d| d.as_of),
                    holds_ready_to_merge: slot.plugin.holds_ready_to_merge(),
                }
            })
            .collect()
    }

    /// Polls every plug-in once; returns the pull requests whose statement was added, changed or removed since the
    /// previous data (none on a plug-in's first data). Failures are logged once per change of failure kind.
    pub async fn poll_once(&self, now: Timestamp) -> HashSet<PrKey> {
        let mut changed = HashSet::new();
        for slot in &self.inner.slots {
            let polled = slot.plugin.poll().await;
            let mut state = lock(&slot.state);
            let id = slot.plugin.id();
            match polled {
                Ok(polled) => {
                    if let Some(kind) = state.failing.take() {
                        tracing::info!(plugin = id, failure = %kind, "dashboard plug-in answers again");
                    }
                    state.last_success = Some(Instant::now());
                    if let Polled::Data(data) = polled {
                        let data = bounded(data, slot.plugin.home());
                        if let Some(previous) = &state.data {
                            changed.extend(differences(previous, &data));
                        }
                        state.data = Some(Arc::new(data));
                    }
                }
                Err(error) => {
                    let kind = error.kind();
                    if state.failing.as_ref() != Some(&kind) {
                        tracing::warn!(plugin = id, %error, error_chain = ?error,
                            "dashboard plug-in poll failed; its statements are hidden once they are older than 5 minutes");
                    }
                    state.failing = Some(kind);
                }
            }
            let shown = self.fresh(&state, now).is_some();
            if shown != state.shown {
                state.shown = shown;
                if shown {
                    tracing::info!(plugin = id, "dashboard plug-in statements are shown");
                } else {
                    tracing::info!(
                        plugin = id,
                        "dashboard plug-in data is stale; its statements are hidden"
                    );
                }
            }
        }
        changed
    }
}

/// Pull requests whose statement differs between `previous` and `next`.
fn differences(previous: &PluginData, next: &PluginData) -> impl Iterator<Item = PrKey> {
    let mut keys: Vec<PrKey> = previous
        .prs
        .iter()
        .filter(|(key, old)| next.prs.get(*key).is_none_or(|new| new.mark != old.mark))
        .map(|(key, _)| key.clone())
        .collect();
    keys.extend(
        next.prs
            .keys()
            .filter(|key| !previous.prs.contains_key(*key))
            .cloned(),
    );
    keys.into_iter()
}

/// Invisible format characters that can reorder or hide displayed text: bidi embeddings, overrides, isolates and
/// marks, zero-width characters, the soft hyphen and the byte order mark (Unicode category Cf).
fn is_format(c: char) -> bool {
    matches!(c,
        '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206F}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}')
}

/// A GitHub login: letters, digits and `-`, at most 39.
fn is_login(text: &str) -> bool {
    (1..=39).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// `text` without control and format characters, cut to `max` characters.
fn plain(text: &str, max: usize) -> String {
    text.chars()
        .filter(|c| !is_format(*c))
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// The data with every external text bounded (ADR-022 §7) and evidence links that are not `http(s)` replaced by
/// the plug-in's home page.
fn bounded(mut data: PluginData, home: &Url) -> PluginData {
    for statement in data.prs.values_mut() {
        statement.summary = statement
            .summary
            .as_deref()
            .map(|s| plain(s, TEXT_CHARS))
            .filter(|s| !s.is_empty());
        statement.details = statement
            .details
            .iter()
            .map(|d| plain(d, TEXT_CHARS))
            .filter(|d| !d.is_empty())
            .take(DETAILS_KEPT)
            .collect();
        statement.facts = statement
            .facts
            .iter()
            .take(FACTS_KEPT)
            .map(|f| Fact {
                label: plain(&f.label, FACT_CHARS),
                value: plain(&f.value, FACT_CHARS),
            })
            .collect();
        if let Some(Readiness::Holds { reason }) = &mut statement.ready_to_merge {
            *reason = plain(reason, TEXT_CHARS);
        }
        if let Some(Move::People { owed, reason }) = &mut statement.whose_move {
            *reason = plain(reason, TEXT_CHARS);
            let mut kept: Vec<String> = Vec::new();
            for login in owed.iter().filter(|l| is_login(l)) {
                if kept.len() < OWED_KEPT && !kept.iter().any(|k| k.eq_ignore_ascii_case(login)) {
                    kept.push(login.clone());
                }
            }
            *owed = kept;
        }
        let web =
            Url::parse(&statement.url).is_ok_and(|url| matches!(url.scheme(), "http" | "https"));
        if !web {
            statement.url = home.to_string();
        }
    }
    data
}

/// The statements shown now, by plug-in in configuration order.
#[derive(Clone, Debug, Default)]
pub struct Overlay {
    now: Option<Timestamp>,
    sources: Vec<Source>,
}

#[derive(Clone, Debug)]
struct Source {
    id: &'static str,
    name: &'static str,
    data: Arc<PluginData>,
}

/// A plug-in's hold on a pull request Ready to merge would list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hold {
    /// Plug-in id.
    pub plugin: String,
    /// Plug-in name.
    pub source: String,
    /// Its reason.
    pub reason: String,
    /// Evidence link.
    pub url: String,
}

/// A plug-in's statement of whose move a pull request is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveStatement {
    /// Plug-in id.
    pub plugin: String,
    /// Plug-in name.
    pub source: String,
    /// Evidence link.
    pub url: String,
    /// When the source data was generated, independent of subsequent successful polls.
    pub as_of: Timestamp,
    /// GitHub's PR update time observed by the source.
    pub github_updated_at: Option<Timestamp>,
    /// Whose move.
    pub whose: Move,
}

impl Overlay {
    fn statements(
        &self,
        repo: &RepoRef,
        number: u32,
    ) -> impl Iterator<Item = (&Source, &PrStatement)> {
        let key = PrKey::new(repo, number);
        self.sources
            .iter()
            .filter_map(move |source| Some((source, source.data.prs.get(&key)?)))
    }

    /// The notes about `repo#number`.
    pub fn notes(&self, repo: &RepoRef, number: u32) -> Vec<PluginNote> {
        self.statements(repo, number)
            .map(|(source, statement)| PluginNote {
                plugin: source.id.to_owned(),
                source: source.name.to_owned(),
                url: statement.url.clone(),
                as_of: source.data.as_of,
                summary: statement.summary.clone(),
                details: statement.details.clone(),
                facts: statement.facts.clone(),
                actions: statement.actions.clone(),
            })
            .collect()
    }

    /// The first plug-in that says whose move `repo#number` is (K15 W3b).
    pub fn whose_move(&self, repo: &RepoRef, number: u32) -> Option<MoveStatement> {
        self.statements(repo, number)
            .filter(|(source, _)| self.now.is_some_and(|now| source.data.as_of <= now))
            .find_map(|(source, statement)| {
                Some(MoveStatement {
                    plugin: source.id.to_owned(),
                    source: source.name.to_owned(),
                    url: statement.url.clone(),
                    as_of: source.data.as_of,
                    github_updated_at: statement.github_updated_at,
                    whose: statement.whose_move.clone()?,
                })
            })
    }

    /// The first plug-in holding `repo#number` back from Ready to merge.
    pub fn hold(&self, repo: &RepoRef, number: u32) -> Option<Hold> {
        self.statements(repo, number)
            .find_map(|(source, statement)| match &statement.ready_to_merge {
                Some(Readiness::Holds { reason }) => Some(Hold {
                    plugin: source.id.to_owned(),
                    source: source.name.to_owned(),
                    reason: reason.clone(),
                    url: statement.url.clone(),
                }),
                Some(Readiness::Agrees) | None => None,
            })
    }
}

/// Starts the poller when a plug-in is configured: while a user is active or watches a pull request it polls every
/// plug-in at most once per [`Timing::interval`] and re-reads changed pull requests of active users; with nobody
/// present it waits and sends nothing. Stops with the server.
pub fn spawn(state: AppState) -> bool {
    if state.plugins.is_empty() {
        return false;
    }
    let host = state.plugins.clone();
    let stopping = state.stopping.subscribe();
    let clock = state.dashboard.clone();
    tokio::spawn(host.run(
        stopping,
        move || clock.now(),
        move |changed| {
            let state = state.clone();
            async move {
                for (tenant_id, user_id) in state.plugins.active_users() {
                    let scope = super::refresh::Scope { tenant_id, user_id };
                    super::refresh::reread::changed_prs(&state, &scope, &changed).await;
                }
                crate::review_loop::watch::plugin_changed(&state, &changed).await;
            }
        },
    ));
    true
}

impl PluginHost {
    /// The poller loop: waits while nobody is active or watching, polls, hands a non-empty change signal to
    /// `on_change`, sleeps [`Timing::interval`]; ends once `stopping` turns true.
    async fn run<F, Fut>(
        self,
        mut stopping: watch::Receiver<bool>,
        now: impl Fn() -> Timestamp,
        mut on_change: F,
    ) where
        F: FnMut(HashSet<PrKey>) -> Fut,
        Fut: Future<Output = ()>,
    {
        tracing::info!(plugins = ?self, "dashboard plug-in poller started");
        loop {
            while !self.present() {
                tokio::select! {
                    () = self.woken() => {}
                    _ = stopping.wait_for(|stop| *stop) => return,
                }
            }
            let changed = self.poll_once(now()).await;
            if !changed.is_empty() {
                on_change(changed).await;
            }
            tokio::select! {
                () = tokio::time::sleep(self.timing().interval) => {}
                _ = stopping.wait_for(|stop| *stop) => return,
            }
        }
    }
}

#[cfg(test)]
mod tests;
