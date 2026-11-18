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

use crate::dashboard::score::ScoreRules;
use crate::dashboard::zone;
use crate::github::work::{
    AUTHORED_PAGES, Fetched, HistoryPr, ItemRef, TokenSource, WorkClient, WorkError, repo_name,
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
        let rank = |kind: HistoryKind| kind as u8;
        events.sort_by(|a, b| {
            (a.at, &a.item_id, rank(a.kind)).cmp(&(b.at, &b.item_id, rank(b.kind)))
        });
        events
    }
}

/// Events of pull requests read by search and Q13.
fn from_details(
    rules: &ScoreRules,
    details: &BTreeMap<ItemRef, HistoryPr>,
    start: Timestamp,
    events: &mut Events,
) {
    for (item, pr) in details {
        if pr.author.as_deref().is_some_and(|a| rules.is_scanned(a)) {
            events.add(HistoryKind::PrOpened, item, pr.created_at, start);
            match (pr.merged_at, pr.closed_at) {
                (Some(merged), _) => events.add(HistoryKind::PrMerged, item, merged, start),
                (None, Some(closed)) => {
                    events.add(HistoryKind::PrClosedUnmerged, item, closed, start);
                }
                (None, None) => {}
            }
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
/// notes.
pub(super) async fn collect<T: TokenSource>(
    client: &WorkClient<T>,
    rules: &ScoreRules,
    logins: &[String],
    refused_from: &Refused<'_>,
    first: Date,
    start: Timestamp,
) -> Result<(Vec<HistoryEvent>, Vec<ScanNote>), WorkError> {
    let mut events = Events::default();
    let mut notes = Vec::new();
    let mut searched: BTreeSet<ItemRef> = BTreeSet::new();
    let mut from_feeds: BTreeSet<ItemRef> = BTreeSet::new();
    let mut refused: Vec<(String, Option<Timestamp>)> = Vec::new();
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
                    for event in feed.events.into_iter().filter(|e| e.created_at >= start) {
                        // The login's own review event is its review; other events only name a pull request.
                        if event.kind == "PullRequestReviewEvent" {
                            events.add(
                                HistoryKind::ReviewGiven,
                                &event.item,
                                event.created_at,
                                start,
                            );
                        }
                        from_feeds.insert(event.item);
                    }
                    refused.push((login.clone(), feed.reaches_back_to.filter(|at| *at > start)));
                }
                Err(error) if error.ends_refresh() => return Err(error),
                Err(error) => {
                    notes.push(skipped(format!("The public events of {login}"), &error));
                    refused.push((login.clone(), Some(start)));
                }
            },
            Err(error) if error.ends_refresh() => return Err(error),
            Err(error) => notes.push(skipped(format!("The history of {login}"), &error)),
        }
    }
    let mut listed = BTreeMap::new();
    if !refused.is_empty() {
        let authors: Vec<String> = refused.iter().map(|(login, _)| login.clone()).collect();
        let found = client
            .authored_pull_requests(refused_from.repos, refused_from.owners, &authors, start)
            .await?;
        notes.extend(found.notes);
        for (login, feed_since) in &refused {
            notes.push(refused_note(login, found.repositories.len(), *feed_since));
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
    let Fetched {
        mut found,
        notes: detail_notes,
    } = client.history_details(&pulls, logins).await?;
    notes.extend(detail_notes);
    for (item, pr) in listed {
        found.entry(item).or_insert(pr);
    }
    from_details(rules, &found, start, &mut events);
    Ok((events.into_events(), notes))
}

/// The UTC date of `at` (scan notes name days, not instants).
fn utc_date(at: Timestamp) -> Date {
    at.to_zoned(TimeZone::UTC).date()
}

/// What the charts know of a login search refuses.
fn refused_note(login: &str, repositories: usize, feed_since: Option<Timestamp>) -> ScanNote {
    let elsewhere = match feed_since {
        Some(at) => format!(
            "its reviews and its pull requests elsewhere come from its public events, which reach back to {} only",
            utc_date(at)
        ),
        None => {
            "its reviews and its pull requests elsewhere come from its public events".to_owned()
        }
    };
    ScanNote {
        text: format!(
            "GitHub search refuses {login}: its pull requests are read from {repositories} configured or enumerated \
             repositories; {elsewhere}"
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
}
