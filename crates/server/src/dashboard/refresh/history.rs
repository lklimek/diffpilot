//! Chart history (plan §C.1, OD14, OD16): pull requests the scanned logins opened, merged or closed and the reviews
//! they gave in the 12-week window, as dated events deduplicated across logins. The window starts at midnight of
//! the user's local date 12 weeks ago (IANA zone; UTC until one is stored); the web buckets the events.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use contracts::api::dashboard::{HistoryEvent, HistoryKind, ScanNote};
use contracts::common::RepoRef;
use contracts::time::Timestamp;
use jiff::civil::{Date, Time};
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Span};

use super::reread::PullRenames;
use crate::dashboard::plugin::PrKey;
use crate::dashboard::score::ScoreRules;
use crate::dashboard::snapshot::Snapshot;
use crate::dashboard::zone;
use crate::github::work::{
    AUTHORED_PAGES, AuthoredPulls, Fetched, HistoryPr, ItemRef, PullDetail, TokenSource,
    WorkClient, WorkError, repo_name,
};

/// The chart window.
const WINDOW_DAYS: i64 = 12 * 7;

/// First local date of the window and its first instant.
pub(super) fn window_start(now: Timestamp, zone: &TimeZone) -> (Date, Timestamp) {
    let today = now.to_zoned(zone.clone()).date();
    let first = today
        .checked_sub(Span::new().days(WINDOW_DAYS))
        .unwrap_or(today);
    (first, first_instant(first, zone).unwrap_or(now))
}

/// The first instant of local date `day`: midnight, or the end of a DST gap that starts at midnight.
fn first_instant(day: Date, zone: &TimeZone) -> Option<Timestamp> {
    let midnight = day.to_datetime(Time::midnight());
    // Gaps last at most a few hours; minute steps find the first local time that exists.
    (0..=6 * 60).find_map(|minutes| {
        zone::earliest(
            midnight
                .checked_add(SignedDuration::from_mins(minutes))
                .ok()?,
            zone,
        )
    })
}

/// Dated events, one per pull request and kind; for reviews the newest in the window.
#[derive(Default)]
struct Events(HashMap<(String, HistoryKind), Timestamp>);

impl Events {
    fn add(&mut self, kind: HistoryKind, item: &ItemRef, at: Timestamp, start: Timestamp) {
        if at < start {
            return;
        }
        let slot = self.0.entry((item.to_string(), kind)).or_insert(at);
        *slot = if kind == HistoryKind::ReviewGiven {
            (*slot).max(at)
        } else {
            (*slot).min(at)
        };
    }

    fn into_events(self) -> Vec<HistoryEvent> {
        let mut events: Vec<HistoryEvent> = self
            .0
            .into_iter()
            .map(|((item_id, kind), at)| HistoryEvent { kind, at, item_id })
            .collect();
        sort_events(&mut events);
        events
    }
}

fn sort_events(events: &mut [HistoryEvent]) {
    events.sort_by(|a, b| (a.at, &a.item_id, a.kind as u8).cmp(&(b.at, &b.item_id, b.kind as u8)));
}

struct Lifecycle<'a> {
    author: Option<&'a str>,
    state: &'a str,
    created_at: Timestamp,
    closed_at: Option<Timestamp>,
    merged_at: Option<Timestamp>,
}

impl<'a> From<&'a HistoryPr> for Lifecycle<'a> {
    fn from(pr: &'a HistoryPr) -> Self {
        Self {
            author: pr.author.as_deref(),
            state: &pr.state,
            created_at: pr.created_at,
            closed_at: pr.closed_at,
            merged_at: pr.merged_at,
        }
    }
}

impl<'a> From<&'a PullDetail> for Lifecycle<'a> {
    fn from(pr: &'a PullDetail) -> Self {
        Self {
            author: pr.author.as_ref().and_then(|a| a.login.as_deref()),
            state: &pr.state,
            created_at: pr.created_at,
            closed_at: pr.closed_at,
            merged_at: pr.merged_at,
        }
    }
}

impl Lifecycle<'_> {
    fn authored_by_scanned_login(&self, rules: &ScoreRules) -> bool {
        self.author.is_some_and(|login| rules.is_scanned(login))
    }

    fn events(&self) -> impl Iterator<Item = (HistoryKind, Timestamp)> {
        let terminal = match self.state {
            "MERGED" => self.merged_at.map(|at| (HistoryKind::PrMerged, at)),
            "CLOSED" => self.closed_at.map(|at| (HistoryKind::PrClosedUnmerged, at)),
            _ => None,
        };
        std::iter::once((HistoryKind::PrOpened, self.created_at)).chain(terminal)
    }
}

/// Replaces authored lifecycle events from the same pull details used to update open items.
pub(super) fn apply(
    rules: &ScoreRules,
    stored: &mut Snapshot,
    found: &BTreeMap<ItemRef, PullDetail>,
    renames: &PullRenames,
    now: Timestamp,
) -> bool {
    if found.is_empty() {
        return false;
    }
    let zone = stored
        .settings
        .time_zone
        .as_deref()
        .and_then(zone::parse)
        .unwrap_or(TimeZone::UTC);
    let (_, start) = window_start(now, &zone);
    let mut updates: HashMap<_, Vec<_>> = found
        .iter()
        .map(|(item, detail)| {
            let repo = repo_name(&item.repo);
            let item_id = if let Some(rename) = renames.get(item) {
                rename.current.to_string()
            } else {
                let name = detail
                    .base_repository
                    .as_ref()
                    .map(|base| base.name_with_owner.as_str())
                    .filter(|name| name.eq_ignore_ascii_case(&repo))
                    .unwrap_or(&repo);
                format!("{name}#{}", item.number)
            };
            let lifecycle = Lifecycle::from(detail);
            let authored = lifecycle.authored_by_scanned_login(rules);
            let events = lifecycle
                .events()
                .filter(|(_, at)| authored && *at >= start)
                .map(|(kind, at)| HistoryEvent {
                    kind,
                    at,
                    item_id: item_id.clone(),
                })
                .collect();
            (item_id.to_ascii_lowercase(), events)
        })
        .collect();
    for old in renames.values().flat_map(|rename| &rename.previous) {
        updates
            .entry(old.to_string().to_ascii_lowercase())
            .or_default();
    }
    let mut changed = false;
    stored.history.retain(|event| {
        if event.kind == HistoryKind::ReviewGiven {
            return true;
        }
        let Some(events) = updates.get_mut(&event.item_id.to_ascii_lowercase()) else {
            return true;
        };
        if let Some(index) = events.iter().position(|next| next == event) {
            events.swap_remove(index);
            true
        } else {
            changed = true;
            false
        }
    });
    let kept = stored.history.len();
    stored.history.extend(updates.into_values().flatten());
    changed |= stored.history.len() != kept;
    if changed {
        sort_events(&mut stored.history);
    }
    changed
}

/// Reconciles the scan's open membership and chart against the same latest observed state.
pub(super) fn reconcile(
    rules: &ScoreRules,
    pulls: &mut BTreeMap<ItemRef, PullDetail>,
    terminal: &BTreeMap<ItemRef, PullDetail>,
    history: &BTreeMap<ItemRef, HistoryPr>,
    start: Timestamp,
) -> Vec<HistoryEvent> {
    let mut events = Events::default();
    from_details(rules, history, start, &mut events);
    let states: HashMap<_, _> = history
        .iter()
        .map(|(item, detail)| (PrKey::new(&item.repo, item.number), detail.state.as_str()))
        .collect();
    let mut reconcile = |item: &ItemRef, detail: &PullDetail| {
        if let Some(state) = states.get(&PrKey::new(&item.repo, item.number)) {
            return *state == "OPEN";
        }
        let lifecycle = Lifecycle::from(detail);
        if lifecycle.authored_by_scanned_login(rules) {
            for (kind, at) in lifecycle.events() {
                events.add(kind, item, at, start);
            }
        }
        lifecycle.state == "OPEN"
    };
    for (item, detail) in terminal {
        // Terminal Q2 answers can supply chart facts but do not carry open ranking context.
        reconcile(item, detail);
    }
    pulls.retain(|item, detail| reconcile(item, detail));
    events.into_events()
}

/// Events of pull requests read by search and Q13.
fn from_details(
    rules: &ScoreRules,
    details: &BTreeMap<ItemRef, HistoryPr>,
    start: Timestamp,
    events: &mut Events,
) {
    for (item, pr) in details {
        let lifecycle = Lifecycle::from(pr);
        if lifecycle.authored_by_scanned_login(rules) {
            for (kind, at) in lifecycle.events() {
                events.add(kind, item, at, start);
            }
            // Reviews count on other people's pull requests only: GitHub lists an author's thread replies as
            // reviews, and a scanned login reviewing another's pull request is the user reviewing their own work.
            continue;
        }
        for (login, times) in &pr.reviews {
            if rules.is_scanned(login) {
                for at in times {
                    events.add(HistoryKind::ReviewGiven, item, *at, start);
                }
            }
        }
    }
}

fn skipped(what: String, error: &WorkError) -> ScanNote {
    ScanNote {
        text: format!("{what} could not be read ({error}); the charts miss it"),
        item: None,
    }
}

/// Where the history of the logins search refuses is read.
pub(super) struct Refused<'a> {
    /// The settings' repository rows.
    pub repos: &'a [RepoRef],
    /// The owners to enumerate.
    pub owners: &'a [String],
}

/// Reads the history of `logins` since `first` (local date) / `start`. A login search refuses is read from the
/// pull requests of the configured and enumerated repositories (whoever merged them; Q16) plus the pull requests
/// its public events name (its reviews elsewhere; Q14). Errors that end the refresh are returned; others become
/// notes. Lifecycle facts read in Q13 take precedence over Q16.
pub(super) async fn collect<T: TokenSource>(
    client: &WorkClient<T>,
    logins: &[String],
    refused_from: &Refused<'_>,
    first: Date,
    start: Timestamp,
) -> Result<Fetched<HistoryPr>, WorkError> {
    let mut notes = Vec::new();
    let mut searched: BTreeSet<ItemRef> = BTreeSet::new();
    let mut from_feeds: BTreeSet<ItemRef> = BTreeSet::new();
    let mut refused: Vec<(String, FeedReach)> = Vec::new();
    for login in logins {
        match client.search_history(login, first).await {
            Ok(found) => {
                notes.extend(found.notes);
                searched.extend(found.opened);
                searched.extend(found.closed);
                searched.extend(found.reviewed);
            }
            Err(WorkError::Unsearchable) => match client.user_events(login).await {
                Ok(feed) => {
                    // An event only names a pull request; its details tell authorship and review times.
                    from_feeds.extend(
                        feed.events
                            .into_iter()
                            .filter(|e| e.created_at >= start)
                            .map(|e| e.item),
                    );
                    let reach = match feed.reaches_back_to {
                        Some(at) if at > start => FeedReach::Since(at),
                        _ => FeedReach::Whole,
                    };
                    refused.push((login.clone(), reach));
                }
                Err(error) if error.ends_refresh() => return Err(error),
                Err(error) => {
                    notes.push(skipped(format!("The public events of {login}"), &error));
                    refused.push((login.clone(), FeedReach::Unread));
                }
            },
            Err(error) if error.ends_refresh() => return Err(error),
            Err(error) => notes.push(skipped(format!("The history of {login}"), &error)),
        }
    }
    let mut listed = BTreeMap::new();
    if !refused.is_empty() {
        let authors: Vec<String> = refused.iter().map(|(login, _)| login.clone()).collect();
        let mut found = client
            .authored_pull_requests(refused_from.repos, refused_from.owners, &authors, start)
            .await?;
        notes.append(&mut found.notes);
        for (login, reach) in &refused {
            notes.push(refused_note(login, &found, *reach));
        }
        for (repo, since) in &found.cut_short {
            notes.push(ScanNote {
                text: format!(
                    "Pull requests of {} updated before {} were not read (more than {} in the window): the charts \
                     may miss some of {}",
                    repo_name(repo),
                    utc_date(*since),
                    AUTHORED_PAGES * 100,
                    authors.join(", ")
                ),
                item: None,
            });
        }
        listed = found.found;
    }
    // A pull request the repository listing already holds needs no detail read, unless search found it (the
    // reviews of searchable logins come with the details).
    let pulls: Vec<ItemRef> = searched
        .iter()
        .cloned()
        .chain(
            from_feeds
                .into_iter()
                .filter(|item| !listed.contains_key(item)),
        )
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let fetched = client.history_details(&pulls, logins).await?;
    notes.extend(fetched.notes);
    let mut seen = BTreeSet::new();
    let found = fetched
        .found
        .into_iter()
        .chain(listed)
        .filter(|(item, _)| seen.insert(PrKey::new(&item.repo, item.number)))
        .collect();
    Ok(Fetched { found, notes })
}

/// The UTC date of `at` (scan notes name days, not instants).
fn utc_date(at: Timestamp) -> Date {
    at.to_zoned(TimeZone::UTC).date()
}

/// How much of a refused login's public events the refresh read.
#[derive(Clone, Copy)]
enum FeedReach {
    /// Every event since the window start.
    Whole,
    /// Cut at the feed's cap: events before this time are unknown.
    Since(Timestamp),
    /// Not read.
    Unread,
}

/// What the charts know of a login search refuses, and how to complete them.
fn refused_note(login: &str, listed: &AuthoredPulls, reach: FeedReach) -> ScanNote {
    let elsewhere = match reach {
        FeedReach::Whole => "come from its public events".to_owned(),
        FeedReach::Since(at) => format!(
            "come from its public events, which reach back to {} only",
            utc_date(at)
        ),
        FeedReach::Unread => "are missing (its public events could not be read)".to_owned(),
    };
    let unreadable = if listed.unreadable.is_empty() {
        String::new()
    } else {
        format!(
            " ({} could not be read with your access)",
            listed
                .unreadable
                .iter()
                .map(repo_name)
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    ScanNote {
        text: format!(
            "GitHub search refuses {login}: the charts count its pull requests in {} repositories of your \
             settings{unreadable}; its reviews and its pull requests in other repositories {elsewhere} and may be \
             incomplete. To count them, add those repositories in Settings → Dashboard, or their owners under \
             \"When GitHub search refuses an account\".",
            listed.repositories.len()
        ),
        item: None,
    }
}

#[cfg(test)]
mod tests {
    use contracts::api::dashboard::DashboardSettings;

    use super::*;

    fn at(text: &str) -> Timestamp {
        text.parse::<Timestamp>().expect("time")
    }

    fn item(number: u32) -> ItemRef {
        ItemRef::parse("acme/platform", number).expect("item")
    }

    #[test]
    fn window_starts_at_local_midnight_twelve_weeks_back() {
        let now = "2026-11-18T09:00:00Z".parse::<Timestamp>().expect("now");
        let warsaw = zone::parse("Europe/Warsaw").expect("zone");
        let (first, start) = window_start(now, &warsaw);
        assert_eq!(first, Date::new(2026, 8, 26).expect("date"));
        // Warsaw is UTC+2 on 26 August (summer time).
        assert_eq!(start, at("2026-08-25T22:00:00Z"));
        let late = "2026-11-18T23:30:00Z".parse::<Timestamp>().expect("now");
        assert_eq!(
            window_start(late, &warsaw).0,
            Date::new(2026, 8, 27).expect("date"),
            "23:30 UTC is already the 19th in Warsaw"
        );
    }

    /// Chile starts DST at local midnight (2026-09-06 00:00 does not exist in America/Santiago): the window
    /// starts at that date's first instant, never at `now`.
    #[test]
    fn window_start_survives_a_midnight_dst_gap() {
        let now = at("2026-11-29T15:00:00Z");
        let santiago = zone::parse("America/Santiago").expect("zone");
        let (first, start) = window_start(now, &santiago);
        assert_eq!(first, Date::new(2026, 9, 6).expect("date"));
        assert!(start < now, "window start fell back to now: {start}");
        assert_eq!(
            start,
            at("2026-09-06T04:00:00Z"),
            "01:00 -03 is the first instant"
        );
    }

    #[test]
    fn details_give_one_event_per_pull_request_and_kind_for_scanned_logins() {
        let settings = DashboardSettings {
            linked_logins: vec!["alice-helper".to_owned()],
            ..DashboardSettings::default()
        };
        let rules = ScoreRules::new("alice", &settings);
        let start = at("2026-08-25T22:00:00Z");
        let details = BTreeMap::from([
            (
                item(1190),
                HistoryPr {
                    state: "MERGED".to_owned(),
                    created_at: at("2026-11-02T10:00:00Z"),
                    closed_at: Some(at("2026-11-10T10:00:00Z")),
                    merged_at: Some(at("2026-11-10T10:00:00Z")),
                    author: Some("alice".to_owned()),
                    reviews: BTreeMap::new(),
                },
            ),
            (
                item(1195),
                HistoryPr {
                    state: "OPEN".to_owned(),
                    created_at: at("2026-11-01T10:00:00Z"),
                    closed_at: None,
                    merged_at: None,
                    author: Some("renata-s".to_owned()),
                    reviews: BTreeMap::from([
                        (
                            "alice".to_owned(),
                            vec![at("2026-11-03T10:00:00Z"), at("2026-11-12T10:00:00Z")],
                        ),
                        ("alice-helper".to_owned(), vec![at("2026-11-05T10:00:00Z")]),
                    ]),
                },
            ),
            (
                item(1100),
                HistoryPr {
                    state: "CLOSED".to_owned(),
                    created_at: at("2026-07-01T10:00:00Z"),
                    closed_at: Some(at("2026-07-02T10:00:00Z")),
                    merged_at: None,
                    author: Some("alice".to_owned()),
                    reviews: BTreeMap::new(),
                },
            ),
        ]);
        let mut events = Events::default();
        from_details(&rules, &details, start, &mut events);
        let events: Vec<(HistoryKind, String, Timestamp)> = events
            .into_events()
            .into_iter()
            .map(|e| (e.kind, e.item_id, e.at))
            .collect();
        assert_eq!(
            events,
            [
                (
                    HistoryKind::PrOpened,
                    "acme/platform#1190".to_owned(),
                    at("2026-11-02T10:00:00Z")
                ),
                (
                    HistoryKind::PrMerged,
                    "acme/platform#1190".to_owned(),
                    at("2026-11-10T10:00:00Z")
                ),
                (
                    HistoryKind::ReviewGiven,
                    "acme/platform#1195".to_owned(),
                    at("2026-11-12T10:00:00Z")
                ),
            ]
        );
    }

    /// A review counts only on someone else's pull request (reference `stats.py` `classify_pr`: "reviews of other
    /// people's PRs"). GitHub lists an author's thread replies as `COMMENTED` reviews by the author
    /// (dashpay/platform#5683: four by its author Claudius-Maginificent), and a scanned login reviewing a linked
    /// login's pull request is the user reviewing their own work.
    #[test]
    fn reviews_of_pull_requests_a_scanned_login_authored_are_not_reviews_given() {
        let settings = DashboardSettings {
            linked_logins: vec!["alice-helper".to_owned()],
            ..DashboardSettings::default()
        };
        let rules = ScoreRules::new("alice", &settings);
        let start = at("2026-08-25T22:00:00Z");
        let own = |author: &str, reviewer: &str| HistoryPr {
            state: "OPEN".to_owned(),
            created_at: at("2026-11-02T10:00:00Z"),
            closed_at: None,
            merged_at: None,
            author: Some(author.to_owned()),
            reviews: BTreeMap::from([(reviewer.to_owned(), vec![at("2026-11-03T10:00:00Z")])]),
        };
        let details = BTreeMap::from([
            (item(5683), own("alice-helper", "alice-helper")),
            (item(5684), own("alice-helper", "alice")),
        ]);
        let mut events = Events::default();
        from_details(&rules, &details, start, &mut events);
        let reviews: Vec<String> = events
            .into_events()
            .into_iter()
            .filter(|e| e.kind == HistoryKind::ReviewGiven)
            .map(|e| e.item_id)
            .collect();
        assert!(reviews.is_empty(), "self-reviews counted: {reviews:?}");
    }
}
