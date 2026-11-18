//! Chart history (plan §C.1, OD14, OD16): pull requests the scanned logins opened, merged or closed and the reviews
//! they gave in the 12-week window, as dated events deduplicated across logins. The window starts at midnight of
//! the user's local date 12 weeks ago (IANA zone; UTC until one is stored); the web buckets the events.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{DateTime, Days, NaiveDate, TimeDelta, Utc};
use chrono_tz::Tz;
use contracts::api::dashboard::{HistoryEvent, HistoryKind, ScanNote};

use crate::dashboard::score::ScoreRules;
use crate::github::work::{
    Fetched, HistoryPr, ItemRef, TokenSource, UserEvent, WorkClient, WorkError,
};

/// The chart window.
const WINDOW_DAYS: u64 = 12 * 7;

/// First local date of the window and its first instant.
pub(super) fn window_start(now: DateTime<Utc>, zone: Tz) -> (NaiveDate, DateTime<Utc>) {
    let today = now.with_timezone(&zone).date_naive();
    let first = today
        .checked_sub_days(Days::new(WINDOW_DAYS))
        .unwrap_or(today);
    (first, first_instant(first, zone).unwrap_or(now))
}

/// The first instant of local date `day`: midnight, or the end of a DST gap that starts at midnight.
fn first_instant(day: NaiveDate, zone: Tz) -> Option<DateTime<Utc>> {
    let midnight = day.and_hms_opt(0, 0, 0)?;
    // Gaps last at most a few hours; minute steps find the first local time that exists.
    (0..=6 * 60).find_map(|minutes| {
        (midnight + TimeDelta::minutes(minutes))
            .and_local_timezone(zone)
            .earliest()
            .map(|at| at.with_timezone(&Utc))
    })
}

/// Dated events, one per pull request and kind; for reviews the newest in the window.
#[derive(Default)]
struct Events(HashMap<(String, HistoryKind), DateTime<Utc>>);

impl Events {
    fn add(&mut self, kind: HistoryKind, item: &ItemRef, at: DateTime<Utc>, start: DateTime<Utc>) {
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
    start: DateTime<Utc>,
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

/// Events of a login's public event feed (Q14, the fallback for a login search refuses). The actor of an
/// `opened` event is the author; a closing event counts only for a pull request a scanned login authored.
fn from_user_events(
    rules: &ScoreRules,
    feed: &[UserEvent],
    start: DateTime<Utc>,
    events: &mut Events,
) {
    for event in feed {
        let own = event
            .author
            .as_deref()
            .is_some_and(|author| rules.is_scanned(author));
        let kind = match (event.kind.as_str(), event.action.as_deref(), event.merged) {
            ("PullRequestEvent", Some("opened"), _) => HistoryKind::PrOpened,
            ("PullRequestEvent", Some("closed"), Some(true)) if own => HistoryKind::PrMerged,
            ("PullRequestEvent", Some("closed"), _) if own => HistoryKind::PrClosedUnmerged,
            ("PullRequestReviewEvent", ..) => HistoryKind::ReviewGiven,
            _ => continue,
        };
        events.add(kind, &event.item, event.created_at, start);
    }
}

fn skipped(what: String, error: &WorkError) -> ScanNote {
    ScanNote {
        text: format!("{what} could not be read ({error}); the charts miss it"),
        item: None,
    }
}

/// Reads the history of `logins` since `first` (local date) / `start`. Errors that end the refresh are returned;
/// others become notes.
pub(super) async fn collect<T: TokenSource>(
    client: &WorkClient<T>,
    rules: &ScoreRules,
    logins: &[String],
    first: NaiveDate,
    start: DateTime<Utc>,
) -> Result<(Vec<HistoryEvent>, Vec<ScanNote>), WorkError> {
    let mut events = Events::default();
    let mut notes = Vec::new();
    let mut pulls: BTreeSet<ItemRef> = BTreeSet::new();
    for login in logins {
        match client.search_history(login, first).await {
            Ok(found) => {
                notes.extend(found.notes);
                pulls.extend(found.opened);
                pulls.extend(found.closed);
                pulls.extend(found.reviewed);
            }
            Err(WorkError::Unsearchable) => match client.user_events(login).await {
                Ok(feed) => {
                    from_user_events(rules, &feed, start, &mut events);
                    notes.push(ScanNote {
                        text: format!(
                            "GitHub search refuses {login}: its chart history comes from its last 300 public \
                             events and may be incomplete"
                        ),
                        item: None,
                    });
                }
                Err(error) if error.ends_refresh() => return Err(error),
                Err(error) => notes.push(skipped(format!("The history of {login}"), &error)),
            },
            Err(error) if error.ends_refresh() => return Err(error),
            Err(error) => notes.push(skipped(format!("The history of {login}"), &error)),
        }
    }
    let pulls: Vec<ItemRef> = pulls.into_iter().collect();
    let Fetched {
        found,
        notes: detail_notes,
    } = client.history_details(&pulls, logins).await?;
    notes.extend(detail_notes);
    from_details(rules, &found, start, &mut events);
    Ok((events.into_events(), notes))
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use contracts::api::dashboard::DashboardSettings;

    use super::*;

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .expect("time")
            .with_timezone(&Utc)
    }

    fn item(number: u32) -> ItemRef {
        ItemRef::parse("acme/platform", number).expect("item")
    }

    #[test]
    fn window_starts_at_local_midnight_twelve_weeks_back() {
        let now = Utc
            .with_ymd_and_hms(2026, 11, 18, 9, 0, 0)
            .single()
            .expect("now");
        let (first, start) = window_start(now, chrono_tz::Europe::Warsaw);
        assert_eq!(first, NaiveDate::from_ymd_opt(2026, 8, 26).expect("date"));
        // Warsaw is UTC+2 on 26 August (summer time).
        assert_eq!(start, at("2026-08-25T22:00:00Z"));
        let late = Utc
            .with_ymd_and_hms(2026, 11, 18, 23, 30, 0)
            .single()
            .expect("now");
        assert_eq!(
            window_start(late, chrono_tz::Europe::Warsaw).0,
            NaiveDate::from_ymd_opt(2026, 8, 27).expect("date"),
            "23:30 UTC is already the 19th in Warsaw"
        );
    }

    /// Chile starts DST at local midnight (2026-09-06 00:00 does not exist in America/Santiago): the window
    /// starts at that date's first instant, never at `now`.
    #[test]
    fn window_start_survives_a_midnight_dst_gap() {
        let now = at("2026-11-29T15:00:00Z");
        let (first, start) = window_start(now, chrono_tz::America::Santiago);
        assert_eq!(first, NaiveDate::from_ymd_opt(2026, 9, 6).expect("date"));
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
        let event = |number, action: &str, merged, author: Option<&str>, when| UserEvent {
            kind: "PullRequestEvent".to_owned(),
            item: item(number),
            action: Some(action.to_owned()),
            merged,
            author: author.map(str::to_owned),
            created_at: at(when),
        };
        let feed = [
            event(1190, "opened", None, None, "2026-11-02T11:00:00Z"),
            // A scanned login closed someone else's PR, and a closing event without its author: neither counts.
            event(
                1300,
                "closed",
                Some(true),
                Some("renata-s"),
                "2026-11-05T09:00:00Z",
            ),
            event(1301, "closed", Some(false), None, "2026-11-05T10:00:00Z"),
        ];
        from_user_events(&rules, &feed, start, &mut events);
        let events: Vec<(HistoryKind, String, DateTime<Utc>)> = events
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
