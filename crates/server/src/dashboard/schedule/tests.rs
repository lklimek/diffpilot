//! Scheduler tests with an injected `now` and no wall-clock sleeps: the pure due rule (DST in Europe/Warsaw and
//! America/New_York, zone changes, catch-up, stagger), and [`tick`] against an in-memory database and a wiremock
//! GitHub (real time, like the refresh tests: sqlx's in-memory pool does not run under tokio's paused clock).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use contracts::api::dashboard::{DashboardSummary, RefreshFailureCode};
use contracts::time::Timestamp;
use db::repo::dashboard::{StartOutcome, StoredDashboardSettings};
use db::repo::users::GithubProfile;
use db::repo::{DEFAULT_TENANT, Db};
use http_body_util::BodyExt;
use jiff::SignedDuration;
use secrecy::SecretString;
use serde_json::json;
use tower::ServiceExt;
use url::Url;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::api;
use crate::auth::{end_sign_in, store_sign_in_tokens};
use crate::config::tests::test_config;
use crate::dashboard::hub::{Clock, DashboardHub};
use crate::dashboard::refresh::rank::NoopRanker;
use crate::github::oauth::TokenGrant;
use crate::test_support::logs::LogCapture;

const WARSAW: &str = "Europe/Warsaw";
const NEW_YORK: &str = "America/New_York";
const ACCESS_CANARY: &str = "ghu_scheduled_access_canary";
const REFRESH_CANARY: &str = "ghr_scheduled_refresh_canary";

fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> Timestamp {
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:00Z")
        .parse::<Timestamp>()
        .expect("time")
}

fn date(y: i16, mo: i8, d: i8) -> Date {
    Date::new(y, mo, d).expect("date")
}

fn schedule(zone: &str, time: &str) -> Schedule {
    Schedule::parse(zone, time).expect("schedule")
}

// ---------------------------------------------------------------------------------------------------------------
// The due rule

#[test]
fn no_zone_no_schedule() {
    let on = DashboardSettings {
        time_zone: Some(WARSAW.to_owned()),
        ..DashboardSettings::default()
    };
    assert_eq!(
        Schedule::of(&on),
        Some(schedule(WARSAW, "06:00")),
        "default 06:00, on"
    );
    let cases = [
        DashboardSettings {
            time_zone: None,
            ..on.clone()
        },
        DashboardSettings {
            schedule_enabled: false,
            ..on.clone()
        },
        DashboardSettings {
            time_zone: Some("Mars/Olympus".to_owned()),
            ..on.clone()
        },
        DashboardSettings {
            schedule_time: "six".to_owned(),
            ..on.clone()
        },
    ];
    for settings in cases {
        assert_eq!(Schedule::of(&settings), None, "{settings:?}");
    }
}

#[test]
fn schedule_runs_once_per_local_day() {
    let s = schedule(WARSAW, "06:00");
    let today = utc(2026, 11, 18, 5, 0);
    assert_eq!(
        s.due(utc(2026, 11, 18, 4, 59), Some(date(2026, 11, 17))),
        Due::Later { instant: today },
        "05:59 local: not yet"
    );
    assert_eq!(
        s.due(today, Some(date(2026, 11, 17))),
        Due::Now {
            on: date(2026, 11, 18),
            instant: today
        }
    );
    let tomorrow = utc(2026, 11, 19, 5, 0);
    for later in [today, utc(2026, 11, 18, 22, 59)] {
        assert_eq!(
            s.due(later, Some(date(2026, 11, 18))),
            Due::Later { instant: tomorrow },
            "recorded today: the next run is tomorrow"
        );
    }
    assert_eq!(
        s.due(tomorrow, Some(date(2026, 11, 18))),
        Due::Now {
            on: date(2026, 11, 19),
            instant: tomorrow
        }
    );
}

#[test]
fn dst_gap_runs_at_first_valid_instant() {
    // Warsaw 2026-03-29: 02:00 CET jumps to 03:00 CEST (01:00 UTC).
    let warsaw = schedule(WARSAW, "02:30");
    assert_eq!(warsaw.instant(date(2026, 3, 29)), utc(2026, 3, 29, 1, 0));
    assert_eq!(
        warsaw.due(utc(2026, 3, 29, 0, 59), Some(date(2026, 3, 28))),
        Due::Later {
            instant: utc(2026, 3, 29, 1, 0)
        }
    );
    assert!(matches!(
        warsaw.due(utc(2026, 3, 29, 1, 0), Some(date(2026, 3, 28))),
        Due::Now { on, .. } if on == date(2026, 3, 29)
    ));
    assert_eq!(
        warsaw.instant(date(2026, 3, 30)),
        utc(2026, 3, 30, 0, 30),
        "CEST"
    );
    // New York 2026-03-08: 02:00 EST jumps to 03:00 EDT (07:00 UTC).
    let new_york = schedule(NEW_YORK, "02:30");
    assert_eq!(new_york.instant(date(2026, 3, 8)), utc(2026, 3, 8, 7, 0));
    assert_eq!(
        new_york.instant(date(2026, 3, 7)),
        utc(2026, 3, 7, 7, 30),
        "EST"
    );
}

#[test]
fn dst_overlap_runs_once() {
    // Warsaw 2026-10-25: 02:30 happens at 00:30 UTC (CEST) and again at 01:30 UTC (CET).
    let warsaw = schedule(WARSAW, "02:30");
    let first = utc(2026, 10, 25, 0, 30);
    assert_eq!(warsaw.instant(date(2026, 10, 25)), first);
    assert_eq!(
        warsaw.due(first, Some(date(2026, 10, 24))),
        Due::Now {
            on: date(2026, 10, 25),
            instant: first
        }
    );
    assert_eq!(
        warsaw.due(utc(2026, 10, 25, 1, 30), Some(date(2026, 10, 25))),
        Due::Later {
            instant: utc(2026, 10, 26, 1, 30)
        },
        "the second 02:30 does not run again"
    );
    // New York 2026-11-01: 01:30 happens at 05:30 UTC (EDT) and again at 06:30 UTC (EST).
    let new_york = schedule(NEW_YORK, "01:30");
    let first = utc(2026, 11, 1, 5, 30);
    assert_eq!(new_york.instant(date(2026, 11, 1)), first);
    assert!(matches!(new_york.due(first, None), Due::Now { .. }));
    assert_eq!(
        new_york.due(utc(2026, 11, 1, 6, 30), Some(date(2026, 11, 1))),
        Due::Later {
            instant: utc(2026, 11, 2, 6, 30)
        }
    );
}

#[test]
fn missed_run_is_caught_up_on_the_same_local_date_only() {
    let s = schedule(WARSAW, "06:00");
    assert_eq!(
        s.due(utc(2026, 11, 18, 13, 0), Some(date(2026, 11, 16))),
        Due::Now {
            on: date(2026, 11, 18),
            instant: utc(2026, 11, 18, 5, 0)
        },
        "the server was down at 06:00: caught up for today"
    );
    assert_eq!(
        s.due(utc(2026, 11, 19, 2, 0), Some(date(2026, 11, 17))),
        Due::Later {
            instant: utc(2026, 11, 19, 5, 0)
        },
        "the missed 2026-11-18 is never caught up"
    );
}

#[test]
fn zone_change_keeps_one_run_per_local_date() {
    // Ran at 00:15 Warsaw on 2026-11-18 (2026-11-17 23:15 UTC), then moved to New York, where it is 2026-11-17.
    let new_york = schedule(NEW_YORK, "00:15");
    assert_eq!(
        new_york.due(utc(2026, 11, 17, 23, 30), Some(date(2026, 11, 18))),
        Due::Later {
            instant: utc(2026, 11, 19, 5, 15)
        },
        "a recorded date ahead of the new local date waits for the date after it"
    );
    // Ran at 06:00 New York (11:00 UTC), then moved to Tokyo, where it is 20:30 on the same date.
    let tokyo = schedule("Asia/Tokyo", "06:00");
    assert_eq!(
        tokyo.due(utc(2026, 11, 17, 11, 30), Some(date(2026, 11, 17))),
        Due::Later {
            instant: utc(2026, 11, 17, 21, 0)
        }
    );
    // Not run yet today in New York (05:00), moved to Warsaw (11:00): the Warsaw 06:00 is caught up.
    let warsaw = schedule(WARSAW, "06:00");
    assert_eq!(
        warsaw.due(utc(2026, 11, 17, 10, 0), Some(date(2026, 11, 16))),
        Due::Now {
            on: date(2026, 11, 17),
            instant: utc(2026, 11, 17, 5, 0)
        }
    );
}

#[test]
fn next_run_at_follows_the_due_rule_and_a_later_success() {
    let s = schedule(WARSAW, "06:00");
    let today = utc(2026, 11, 18, 5, 0);
    let tomorrow = utc(2026, 11, 19, 5, 0);
    assert_eq!(s.next_run_at(utc(2026, 11, 18, 4, 0), None, None), today);
    assert_eq!(
        s.next_run_at(utc(2026, 11, 18, 9, 0), Some(date(2026, 11, 17)), None),
        today,
        "overdue: about to be caught up"
    );
    assert_eq!(
        s.next_run_at(utc(2026, 11, 18, 9, 0), None, Some(utc(2026, 11, 18, 8, 0))),
        tomorrow,
        "a refresh after 06:00 makes today's run unnecessary"
    );
    assert_eq!(
        s.next_run_at(utc(2026, 11, 18, 9, 0), None, Some(utc(2026, 11, 18, 4, 0))),
        today,
        "a refresh before 06:00 does not"
    );
    assert_eq!(
        s.next_run_at(utc(2026, 11, 18, 9, 0), Some(date(2026, 11, 18)), None),
        tomorrow
    );
}

#[test]
fn offsets_are_stable_whole_minutes_below_ten() {
    let offsets: Vec<SignedDuration> = (0..200).map(|n| offset(&format!("user-{n}"))).collect();
    for (n, delta) in offsets.iter().enumerate() {
        assert!(
            (SignedDuration::ZERO..SignedDuration::from_mins(10)).contains(delta),
            "{delta}"
        );
        assert_eq!(delta.as_secs() % 60, 0);
        assert_eq!(*delta, offset(&format!("user-{n}")), "stable");
    }
    let distinct: std::collections::HashSet<_> = offsets.iter().collect();
    assert_eq!(distinct.len(), 10, "users are spread over all ten minutes");
}

#[test]
fn sleep_is_bounded_between_one_second_and_a_minute() {
    let now = utc(2026, 11, 18, 5, 0);
    assert_eq!(sleep_until(now, None), TICK);
    assert_eq!(
        sleep_until(now, Some(now - SignedDuration::from_mins(5))),
        MIN_SLEEP
    );
    assert_eq!(
        sleep_until(now, Some(now + SignedDuration::from_secs(30))),
        Duration::from_secs(30)
    );
    assert_eq!(
        sleep_until(now, Some(now + SignedDuration::from_hours(20))),
        TICK
    );
}

#[tokio::test]
async fn a_panicking_evaluation_is_contained() {
    let capture = LogCapture::start();
    assert_eq!(
        guarded(async { panic!("injected panic in one user's run") }).await,
        None::<()>
    );
    assert_eq!(guarded(async { 7 }).await, Some(7));
    assert!(capture.text().contains("panicked"), "{}", capture.text());
}

#[tokio::test]
async fn one_scheduler_per_process() {
    let state = AppState::with_queue(test_config(), Db::in_memory().await.expect("db"));
    assert!(spawn(state.clone()));
    assert!(!spawn(state.clone()), "a second scheduler is refused");
    let _ = state.stopping.send(true);
}

// ---------------------------------------------------------------------------------------------------------------
// tick() against the database and a wiremock GitHub

struct World {
    state: AppState,
    github: MockServer,
}

/// 2026-11-18 09:00 UTC (10:00 in Warsaw): the default 06:00 run of the day is overdue.
fn morning() -> Timestamp {
    utc(2026, 11, 18, 9, 0)
}

async fn world_at(now: Timestamp, scan_limit: Duration) -> World {
    let github = MockServer::start().await;
    let mut config = test_config();
    config.github.oauth_url = Url::parse(&github.uri()).expect("oauth url");
    config.github.api_url = Url::parse(&format!("{}/api", github.uri())).expect("api url");
    let mut state = AppState::with_queue(config, Db::in_memory().await.expect("db"));
    state.dashboard = DashboardHub::testing(
        Clock::starting_at(now),
        scan_limit,
        Arc::new(NoopRanker),
        Duration::ZERO,
    );
    // Searches answer with nothing, so a scan with a usable token ends with an empty snapshot.
    Mock::given(method("GET"))
        .and(path("/api/search/issues"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "total_count": 0,
            "incomplete_results": false,
            "items": []
        })))
        .mount(&github)
        .await;
    World { state, github }
}

async fn world() -> World {
    world_at(morning(), Duration::from_secs(3600)).await
}

/// Answers GitHub's refresh of any token with `bad_refresh_token`.
async fn reject_refreshes(world: &World) {
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": "bad_refresh_token",
            "error_description": "The refresh token passed is incorrect or expired."
        })))
        .mount(&world.github)
        .await;
}

/// The allowed user `user_id` with the schedule on (Warsaw, 06:00) and, if `access_expires_in` is given, a token
/// pair whose access token expires in that many seconds; no session.
async fn add_user(world: &World, user_id: &str, github_id: i64, access_expires_in: Option<i64>) {
    let login = format!("login-{github_id}");
    let state = &world.state;
    db::repo::tenants::testing::allow_login(&state.db, DEFAULT_TENANT, &login, github_id)
        .await
        .expect("allow");
    state
        .db
        .users()
        .upsert_member(
            DEFAULT_TENANT,
            user_id,
            &GithubProfile {
                github_id,
                login,
                name: None,
                avatar_url: None,
            },
            Timestamp::now(),
        )
        .await
        .expect("user");
    set_settings(world, user_id, |_| {}).await;
    if let Some(expires_in) = access_expires_in {
        let grant = TokenGrant {
            access_token: SecretString::from(ACCESS_CANARY),
            expires_in,
            refresh_token: SecretString::from(REFRESH_CANARY),
            refresh_token_expires_in: 15_638_400,
        };
        let lock = lock_user(user_id).await;
        store_sign_in_tokens(state, &lock, DEFAULT_TENANT, &grant, Timestamp::now())
            .await
            .expect("pair");
    }
}

async fn set_settings(world: &World, user_id: &str, change: impl FnOnce(&mut DashboardSettings)) {
    let mut settings = DashboardSettings {
        time_zone: Some(WARSAW.to_owned()),
        include_todos: false,
        ..DashboardSettings::default()
    };
    change(&mut settings);
    world
        .state
        .db
        .dashboard()
        .save_settings(DEFAULT_TENANT, user_id, &settings, Timestamp::now())
        .await
        .expect("settings");
}

async fn stored(world: &World, user_id: &str) -> StoredDashboardSettings {
    world
        .state
        .db
        .dashboard()
        .settings(DEFAULT_TENANT, user_id)
        .await
        .expect("settings")
        .expect("row")
}

async fn last_run(world: &World, user_id: &str) -> Option<ScheduledRunRecord> {
    stored(world, user_id).await.last_scheduled
}

async fn latest_refresh(world: &World, user_id: &str) -> Option<RefreshRecord> {
    world
        .state
        .db
        .dashboard()
        .latest_refresh(DEFAULT_TENANT, user_id)
        .await
        .expect("refresh")
}

async fn has_pair(world: &World, user_id: &str) -> bool {
    world
        .state
        .db
        .user_tokens()
        .get(user_id)
        .await
        .expect("pair")
        .is_some()
}

async fn tick_once(world: &World) -> Tick {
    tick(world.state.clone()).await.expect("tick")
}

/// Waits until the user's refresh task ended (a ranking runs after the scan the scheduler waited for).
async fn settle(world: &World, user_id: &str) {
    let Some(refresh) = latest_refresh(world, user_id).await else {
        return;
    };
    let ended = tokio::time::timeout(Duration::from_secs(20), async {
        while world.state.dashboard.is_running(refresh.id) {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(ended.is_ok(), "the refresh task never ended");
}

#[tokio::test]
async fn missed_run_caught_up_once_on_the_same_local_date() {
    let world = world().await;
    add_user(&world, "u1", 101, Some(28_800)).await;
    let capture = LogCapture::start();
    assert_eq!(tick_once(&world).await, Tick::Ran);
    let refresh = latest_refresh(&world, "u1").await.expect("started");
    assert_eq!(refresh.trigger, RefreshTrigger::Scheduled);
    assert_ne!(
        refresh.state,
        RefreshState::Scanning,
        "the scheduler waited for the scan"
    );
    assert_eq!(
        last_run(&world, "u1").await,
        Some(ScheduledRunRecord {
            on: date(2026, 11, 18),
            outcome: ScheduledOutcome::Started,
            reason: None
        })
    );
    settle(&world, "u1").await;
    let next = utc(2026, 11, 19, 5, 0) + offset("u1");
    assert_eq!(tick_once(&world).await, Tick::Idle { next: Some(next) });
    assert_eq!(
        latest_refresh(&world, "u1").await.map(|r| r.id),
        Some(refresh.id),
        "caught up once"
    );
    let logs = capture.text();
    assert!(
        !logs.contains(ACCESS_CANARY) && !logs.contains(REFRESH_CANARY),
        "{logs}"
    );
}

#[tokio::test]
async fn no_catch_up_after_a_later_manual_refresh() {
    let world = world().await;
    add_user(&world, "u1", 101, Some(28_800)).await;
    let dashboard = world.state.db.dashboard();
    let StartOutcome::Started { refresh, .. } = dashboard
        .start_refresh(
            DEFAULT_TENANT,
            "u1",
            RefreshTrigger::Manual,
            utc(2026, 11, 18, 6, 0),
        )
        .await
        .expect("manual")
    else {
        panic!("manual refresh not started");
    };
    dashboard
        .store_snapshot(
            DEFAULT_TENANT,
            "u1",
            refresh.id,
            &json!({}),
            utc(2026, 11, 18, 6, 1),
        )
        .await
        .expect("snapshot after 06:00 local");
    assert_eq!(tick_once(&world).await, Tick::Ran);
    assert_eq!(
        last_run(&world, "u1").await,
        Some(ScheduledRunRecord {
            on: date(2026, 11, 18),
            outcome: ScheduledOutcome::Skipped,
            reason: Some(REFRESHED_ALREADY.to_owned())
        })
    );
    assert_eq!(
        latest_refresh(&world, "u1").await.map(|r| r.id),
        Some(refresh.id),
        "no scheduled refresh"
    );
}

#[tokio::test]
async fn scheduled_run_skips_when_a_refresh_is_live() {
    let world = world().await;
    add_user(&world, "u1", 101, Some(28_800)).await;
    let StartOutcome::Started { refresh, .. } = world
        .state
        .db
        .dashboard()
        .start_refresh(
            DEFAULT_TENANT,
            "u1",
            RefreshTrigger::Manual,
            utc(2026, 11, 18, 8, 0),
        )
        .await
        .expect("manual")
    else {
        panic!("manual refresh not started");
    };
    assert_eq!(tick_once(&world).await, Tick::Ran);
    let run = last_run(&world, "u1").await.expect("recorded");
    assert_eq!(
        (run.outcome, run.reason.as_deref()),
        (ScheduledOutcome::Skipped, Some(ALREADY_RUNNING))
    );
    let latest = latest_refresh(&world, "u1").await.expect("refresh");
    assert_eq!(
        (latest.id, latest.state),
        (refresh.id, RefreshState::Scanning)
    );
}

#[tokio::test]
async fn rejected_github_refresh_records_needs_sign_in() {
    let world = world().await;
    reject_refreshes(&world).await;
    add_user(&world, "u1", 101, Some(-10)).await;
    let capture = LogCapture::start();
    assert_eq!(tick_once(&world).await, Tick::Ran);
    let refresh = latest_refresh(&world, "u1").await.expect("started");
    assert_eq!(refresh.state, RefreshState::Failed);
    assert_eq!(
        refresh.failure.map(|f| f.code),
        Some(RefreshFailureCode::SignInRequired)
    );
    assert_eq!(
        last_run(&world, "u1").await,
        Some(ScheduledRunRecord {
            on: date(2026, 11, 18),
            outcome: ScheduledOutcome::NeedsSignIn,
            reason: Some(SIGN_IN_AGAIN.to_owned())
        })
    );
    assert!(
        !has_pair(&world, "u1").await,
        "the rejected pair ends as in v1"
    );
    assert!(
        matches!(tick_once(&world).await, Tick::Idle { .. }),
        "no retry storm"
    );
    let logs = capture.text();
    assert!(
        !logs.contains(ACCESS_CANARY) && !logs.contains(REFRESH_CANARY),
        "{logs}"
    );

    // Without any pair nothing starts.
    add_user(&world, "u2", 102, None).await;
    assert_eq!(tick_once(&world).await, Tick::Ran);
    assert_eq!(latest_refresh(&world, "u2").await, None);
    let run = last_run(&world, "u2").await.expect("recorded");
    assert_eq!(run.outcome, ScheduledOutcome::NeedsSignIn);
}

#[tokio::test]
async fn failed_scheduled_scan_records_its_message() {
    let world = world_at(morning(), Duration::ZERO).await;
    add_user(&world, "u1", 101, Some(28_800)).await;
    assert_eq!(tick_once(&world).await, Tick::Ran);
    let refresh = latest_refresh(&world, "u1").await.expect("started");
    let failure = refresh.failure.expect("failed");
    assert_eq!(failure.code, RefreshFailureCode::TimedOut);
    let run = last_run(&world, "u1").await.expect("recorded");
    assert_eq!(
        (run.outcome, run.reason),
        (ScheduledOutcome::Failed, Some(failure.message))
    );
}

/// Two user ids with distinct offsets, the larger one non-zero.
fn staggered_users() -> (String, String) {
    let ids: Vec<String> = (0..50).map(|n| format!("user-{n}")).collect();
    let first = &ids[0];
    let second = ids
        .iter()
        .find(|id| offset(id) != offset(first))
        .expect("distinct offsets");
    let mut pair = [first.clone(), second.clone()];
    pair.sort_by_key(|id| offset(id));
    let [early, late] = pair;
    (early, late)
}

#[tokio::test]
async fn scheduled_users_run_one_at_a_time_in_offset_order() {
    let (early, late) = staggered_users();
    let world = world().await;
    // The later user is added first, so neither insertion nor id order explains the result.
    add_user(&world, &late, 102, Some(28_800)).await;
    add_user(&world, &early, 101, Some(28_800)).await;

    assert_eq!(tick_once(&world).await, Tick::Ran);
    let first = latest_refresh(&world, &early)
        .await
        .expect("earlier offset first");
    assert_ne!(first.state, RefreshState::Scanning);
    assert_eq!(latest_refresh(&world, &late).await, None, "one at a time");
    settle(&world, &early).await;

    assert_eq!(tick_once(&world).await, Tick::Ran);
    let second = latest_refresh(&world, &late)
        .await
        .expect("then the later one");
    let first_scanned = first
        .scanned_at
        .or(first.finished_at)
        .expect("first scan ended");
    assert!(second.started_at >= first_scanned, "{second:?} {first:?}");
    settle(&world, &late).await;
    assert!(matches!(tick_once(&world).await, Tick::Idle { .. }));
}

#[tokio::test]
async fn users_wait_for_their_offset_after_the_scheduled_instant() {
    let (_, late) = staggered_users();
    let instant = utc(2026, 11, 18, 5, 0);
    let world = world_at(instant, Duration::from_secs(3600)).await;
    add_user(&world, &late, 102, Some(28_800)).await;
    assert_eq!(
        tick_once(&world).await,
        Tick::Idle {
            next: Some(instant + offset(&late))
        }
    );
    assert_eq!(latest_refresh(&world, &late).await, None);
}

#[tokio::test]
async fn unlisted_user_is_not_scheduled() {
    let world = world().await;
    add_user(&world, "u1", 101, Some(28_800)).await;
    db::repo::tenants::testing::remove_login(&world.state.db, DEFAULT_TENANT, "login-101")
        .await
        .expect("remove");
    assert_eq!(tick_once(&world).await, Tick::Idle { next: None });
    assert_eq!(latest_refresh(&world, "u1").await, None, "never runs");
    assert_eq!(last_run(&world, "u1").await, None);
    assert!(!has_pair(&world, "u1").await, "the kept pair is deleted");
}

#[tokio::test]
async fn sign_out_everywhere_deletes_pair_and_stops_schedule() {
    let world = world().await;
    add_user(&world, "u1", 101, Some(28_800)).await;
    end_sign_in(&world.state, "u1")
        .await
        .expect("sign out everywhere");
    assert!(!has_pair(&world, "u1").await);
    assert_eq!(tick_once(&world).await, Tick::Ran);
    assert_eq!(
        latest_refresh(&world, "u1").await,
        None,
        "no refresh without a sign-in"
    );
    assert_eq!(
        last_run(&world, "u1").await.map(|run| run.outcome),
        Some(ScheduledOutcome::NeedsSignIn)
    );
}

#[tokio::test]
async fn pair_kept_without_session_while_schedule_on() {
    let world = world().await;
    add_user(&world, "u1", 101, Some(28_800)).await;
    world
        .state
        .db
        .dashboard()
        .record_scheduled_run(
            DEFAULT_TENANT,
            "u1",
            &ScheduledRunRecord {
                on: date(2026, 11, 18),
                outcome: ScheduledOutcome::Started,
                reason: None,
            },
        )
        .await
        .expect("ran today");
    assert!(matches!(tick_once(&world).await, Tick::Idle { .. }));
    assert!(has_pair(&world, "u1").await, "kept by the schedule");
    set_settings(&world, "u1", |s| s.schedule_enabled = false).await;
    assert_eq!(tick_once(&world).await, Tick::Idle { next: None });
    assert!(
        !has_pair(&world, "u1").await,
        "schedule off: deleted by the next expiry sweep"
    );
}

/// R1 reports the next run and the last run's instant, both resolved through a DST gap.
#[tokio::test]
async fn summary_reports_next_and_last_run_across_a_dst_gap() {
    let world = world_at(utc(2026, 3, 28, 12, 0), Duration::from_secs(3600)).await;
    add_user(&world, "u1", 101, None).await;
    set_settings(&world, "u1", |s| s.schedule_time = "02:30".to_owned()).await;
    world
        .state
        .db
        .dashboard()
        .record_scheduled_run(
            DEFAULT_TENANT,
            "u1",
            &ScheduledRunRecord {
                on: date(2026, 3, 29),
                outcome: ScheduledOutcome::NeedsSignIn,
                reason: Some(SIGN_IN_AGAIN.to_owned()),
            },
        )
        .await
        .expect("record");
    let token = "schedule-summary-session";
    let now = Timestamp::now();
    world
        .state
        .db
        .sessions()
        .create(
            &db::repo::sessions::NewSession {
                token_hash: &sha256(token.as_bytes()),
                tenant_id: DEFAULT_TENANT,
                user_id: "u1",
                csrf_token: "csrf",
                expires_at: now + SignedDuration::from_hours(1),
            },
            now,
        )
        .await
        .expect("session");
    let response = api::router(world.state.clone())
        .oneshot(
            Request::get("/api/v1/dashboard")
                .header("cookie", format!("__Host-sr_session={token}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let summary: DashboardSummary = serde_json::from_slice(&bytes).expect("summary");
    let last = summary.schedule.last_run.expect("last run");
    assert_eq!(
        last.at,
        utc(2026, 3, 29, 1, 0),
        "02:30 does not exist that day"
    );
    assert_eq!(last.outcome, ScheduledOutcome::NeedsSignIn);
    assert_eq!(
        summary.schedule.next_run_at,
        Some(utc(2026, 3, 30, 0, 30)),
        "recorded for 2026-03-29 (ahead of the local date): the next date's 02:30 CEST"
    );
}

// ---------------------------------------------------------------------------------------------------------------
// Review repros (branch review/v11-sched-repro)

const SANTIAGO: &str = "America/Santiago";

/// Santiago changes clocks at local midnight: 2026-09-06 00:00 (-04) jumps to 01:00 (-03); 2026-04-05 00:00 (-03)
/// falls back to 2026-04-04 23:00 (-04), so 23:xx of 2026-04-04 happens twice.
#[test]
fn review_santiago_midnight_dst_runs_once_per_local_date() {
    let midnight = schedule(SANTIAGO, "00:00");
    assert_eq!(
        midnight.instant(date(2026, 9, 6)),
        utc(2026, 9, 6, 4, 0),
        "gap: first valid instant"
    );
    assert_eq!(
        midnight.due(utc(2026, 9, 6, 3, 59), Some(date(2026, 9, 5))),
        Due::Later {
            instant: utc(2026, 9, 6, 4, 0)
        }
    );
    assert_eq!(
        midnight.due(utc(2026, 9, 6, 4, 0), Some(date(2026, 9, 5))),
        Due::Now {
            on: date(2026, 9, 6),
            instant: utc(2026, 9, 6, 4, 0)
        }
    );
    let late = schedule(SANTIAGO, "23:30");
    let first = utc(2026, 4, 5, 2, 30);
    assert_eq!(
        late.instant(date(2026, 4, 4)),
        first,
        "overlap: first occurrence"
    );
    assert_eq!(
        late.due(first, Some(date(2026, 4, 3))),
        Due::Now {
            on: date(2026, 4, 4),
            instant: first
        }
    );
    assert_eq!(
        late.due(utc(2026, 4, 5, 3, 30), Some(date(2026, 4, 4))),
        Due::Later {
            instant: utc(2026, 4, 6, 3, 30)
        },
        "the second 23:30 does not run again"
    );
}

/// Contract (module doc, plan §A.5): a failing user is tried at most once a day and the scheduler goes on with the
/// others. A persistent database error for the first due user must not keep the later user from running.
#[tokio::test]
async fn review_one_failing_user_does_not_block_the_others() {
    let (early, late) = staggered_users();
    let world = world().await;
    add_user(&world, &early, 101, Some(28_800)).await;
    add_user(&world, &late, 102, Some(28_800)).await;
    db::repo::tenants::testing::fail_scheduled_run_records(&world.state.db, &early)
        .await
        .expect("inject");
    for _ in 0..3 {
        let _ = tick(world.state.clone()).await;
    }
    assert!(
        latest_refresh(&world, &late).await.is_some() || last_run(&world, &late).await.is_some(),
        "the later user never ran: one user's persistent error starves every other user"
    );
}

// ---------------------------------------------------------------------------------------------------------------
// Review fixes: per-user isolation, stop, restart recovery, lapsed pair

#[tokio::test]
async fn unreadable_settings_row_does_not_block_the_others() {
    let world = world().await;
    add_user(&world, "u1", 101, Some(28_800)).await;
    add_user(&world, "u2", 102, Some(28_800)).await;
    db::repo::tenants::testing::corrupt_dashboard_settings(&world.state.db, "u1")
        .await
        .expect("corrupt");
    let capture = LogCapture::start();
    assert_eq!(tick_once(&world).await, Tick::Ran);
    assert!(latest_refresh(&world, "u2").await.is_some());
    assert!(
        capture.text().contains("could not be read"),
        "{}",
        capture.text()
    );
}

#[tokio::test]
async fn a_failing_user_is_given_up_for_the_day() {
    let (early, late) = staggered_users();
    let world = world().await;
    add_user(&world, &early, 101, Some(28_800)).await;
    add_user(&world, &late, 102, Some(28_800)).await;
    db::repo::tenants::testing::fail_scheduled_run_records(&world.state.db, &early)
        .await
        .expect("inject");
    assert_eq!(
        tick_once(&world).await,
        Tick::Ran,
        "the later user runs in the same evaluation"
    );
    assert!(latest_refresh(&world, &late).await.is_some());
    assert_eq!(latest_refresh(&world, &early).await, None);
    settle(&world, &late).await;
    assert!(
        matches!(tick_once(&world).await, Tick::Idle { .. }),
        "the failing user is not tried again today"
    );
}

#[tokio::test]
async fn stop_requested_starts_no_further_run() {
    let world = world().await;
    add_user(&world, "u1", 101, Some(28_800)).await;
    world.state.stopping.send_replace(true);
    assert!(matches!(tick_once(&world).await, Tick::Idle { .. }));
    assert_eq!(latest_refresh(&world, "u1").await, None);
    assert_eq!(last_run(&world, "u1").await, None);
}

async fn record_started_today(world: &World, user_id: &str) {
    world
        .state
        .db
        .dashboard()
        .record_scheduled_run(
            DEFAULT_TENANT,
            user_id,
            &ScheduledRunRecord {
                on: date(2026, 11, 18),
                outcome: ScheduledOutcome::Started,
                reason: None,
            },
        )
        .await
        .expect("claimed");
}

/// A run a restart interrupted (scan stopped, or claimed and never started) becomes `failed`; the day's catch-up
/// then decides again. A run whose scan ended stays `started`.
#[tokio::test]
async fn interrupted_run_is_failed_at_start_up_and_caught_up() {
    let world = world().await;
    add_user(&world, "u1", 101, Some(28_800)).await;
    add_user(&world, "u2", 102, Some(28_800)).await;
    add_user(&world, "u3", 103, Some(28_800)).await;
    let dashboard = world.state.db.dashboard();
    for user in ["u1", "u2", "u3"] {
        record_started_today(&world, user).await;
    }
    let StartOutcome::Started { refresh, .. } = dashboard
        .start_refresh(
            DEFAULT_TENANT,
            "u1",
            RefreshTrigger::Scheduled,
            utc(2026, 11, 18, 5, 9),
        )
        .await
        .expect("scheduled")
    else {
        panic!("not started");
    };
    let StartOutcome::Started { refresh: done, .. } = dashboard
        .start_refresh(
            DEFAULT_TENANT,
            "u3",
            RefreshTrigger::Scheduled,
            utc(2026, 11, 18, 5, 9),
        )
        .await
        .expect("scheduled")
    else {
        panic!("not started");
    };
    dashboard
        .store_snapshot(
            DEFAULT_TENANT,
            "u3",
            done.id,
            &json!({}),
            utc(2026, 11, 18, 5, 10),
        )
        .await
        .expect("scanned");
    // The restart: the refresh recovery runs first.
    crate::dashboard::refresh::recover(&world.state)
        .await
        .expect("refresh recovery");
    recover(world.state.clone())
        .await
        .expect("schedule recovery");
    for user in ["u1", "u2"] {
        let run = last_run(&world, user).await.expect("recorded");
        assert_eq!(
            (run.outcome, run.reason.as_deref()),
            (ScheduledOutcome::Failed, Some(INTERRUPTED)),
            "{user}"
        );
    }
    assert_eq!(
        last_run(&world, "u3").await.map(|run| run.outcome),
        Some(ScheduledOutcome::Started),
        "its scan ended before the restart"
    );
    let mut caught_up = Vec::new();
    while tick_once(&world).await == Tick::Ran {
        for user in ["u1", "u2"] {
            if latest_refresh(&world, user)
                .await
                .is_some_and(|r| r.id != refresh.id)
                && !caught_up.contains(&user)
            {
                caught_up.push(user);
                settle(&world, user).await;
            }
        }
    }
    caught_up.sort_unstable();
    assert_eq!(caught_up, ["u1", "u2"], "today's run is caught up again");
}

#[tokio::test]
async fn missing_pair_asks_to_sign_in_to_keep_the_refresh_running() {
    let world = world().await;
    add_user(&world, "u1", 101, None).await;
    assert_eq!(tick_once(&world).await, Tick::Ran);
    let run = last_run(&world, "u1").await.expect("recorded");
    assert_eq!(
        (run.outcome, run.reason.as_deref()),
        (
            ScheduledOutcome::NeedsSignIn,
            Some("Sign in again to keep the daily refresh running.")
        )
    );
    assert_eq!(latest_refresh(&world, "u1").await, None);
}

// ---------------------------------------------------------------------------------------------------------------
// A scheduled refresh ranks like a manual one (judge jobs, daily cap): the fake GitHub dashboard world and the
// production ranker.

mod ranked {
    use std::net::SocketAddr;
    use std::path::{Path, PathBuf};

    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use contracts::api::dashboard::{RankingState, RepositoryRule};
    use contracts::common::RepoRef;
    use contracts::job::spec::{CredentialKind, LlmCredentialRef};
    use db::repo::dashboard::JudgeCounts;
    use reqwest::header::{ACCEPT, LOCATION};
    use serde_json::Value;
    use sha2::{Digest, Sha256};

    use super::*;
    use crate::dashboard::judge::{DAILY_CAP, JudgeRanker};

    const VERIFIER: &str = "schedule-test-verifier-0123456789-0123456789-0123456789";
    const CALLBACK: &str = "https://review.example.test/auth/github/callback";

    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    async fn spawn_github() -> String {
        let config = fake_github::Config {
            fixture_dir: repo_root().join("fixtures/pr-482/github"),
            app_url: Url::parse("https://review.example.test").expect("url"),
            user_login: None,
            user_id: None,
            app_id: None,
            app_public_key_file: None,
            dashboard_fixture_dir: Some(repo_root().join("fixtures/dashboard/github")),
            dashboard_world_at: None,
        };
        let router = fake_github::router(&config).expect("fake GitHub");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr: SocketAddr = listener.local_addr().expect("addr");
        tokio::spawn(async move { axum::serve(listener, router).await });
        format!("http://{addr}")
    }

    /// The fake's OAuth web flow; returns a token pair the fake accepts.
    async fn sign_in(base: &str) -> TokenGrant {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("client");
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER.as_bytes()));
        let mut authorize = Url::parse(&format!("{base}/login/oauth/authorize")).expect("url");
        authorize.query_pairs_mut().extend_pairs([
            ("client_id", "fixture-app-client-id"),
            ("redirect_uri", CALLBACK),
            ("state", "s"),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
        ]);
        let response = http.get(authorize).send().await.expect("authorize");
        let location =
            Url::parse(response.headers()[LOCATION].to_str().expect("location")).expect("url");
        let code = location
            .query_pairs()
            .find(|(k, _)| k == "code")
            .expect("code")
            .1
            .into_owned();
        let body: Value = http
            .post(format!("{base}/login/oauth/access_token"))
            .header(ACCEPT, "application/json")
            .form(&[
                ("client_id", "fixture-app-client-id"),
                ("client_secret", "fixture-client-secret"),
                ("code", &code),
                ("redirect_uri", CALLBACK),
                ("code_verifier", VERIFIER),
            ])
            .send()
            .await
            .expect("token")
            .json()
            .await
            .expect("json");
        TokenGrant {
            access_token: SecretString::from(
                body["access_token"].as_str().expect("access").to_owned(),
            ),
            expires_in: body["expires_in"].as_i64().unwrap_or(28_800),
            refresh_token: SecretString::from(
                body["refresh_token"].as_str().expect("refresh").to_owned(),
            ),
            refresh_token_expires_in: body["refresh_token_expires_in"]
                .as_i64()
                .unwrap_or(15_811_200),
        }
    }

    fn rule(name: &str, alias: &str) -> RepositoryRule {
        RepositoryRule {
            repo: RepoRef {
                owner: "acme".to_owned(),
                name: name.to_owned(),
            },
            alias: Some(alias.to_owned()),
            watch_issues: false,
            ownership_policy: None,
            branch_patterns: Vec::new(),
        }
    }

    /// Alice of the fake dashboard world, signed in once (no session left), schedule on in Warsaw.
    async fn world() -> AppState {
        let github = spawn_github().await;
        let mut config = test_config();
        config.github.api_url = Url::parse(&github).expect("url");
        config.github.oauth_url = Url::parse(&github).expect("url");
        let db = Db::in_memory().await.expect("db");
        db.llm_credentials()
            .register(
                DEFAULT_TENANT,
                &LlmCredentialRef {
                    id: crate::pipeline::CREDENTIAL_ID.to_owned(),
                    kind: CredentialKind::CodexChatgpt,
                },
                true,
                Timestamp::now(),
            )
            .await
            .expect("credential");
        let mut state = AppState::with_queue(config, db);
        state.dashboard = DashboardHub::testing(
            Clock::starting_at(morning()),
            Duration::from_secs(3600),
            Arc::new(JudgeRanker),
            Duration::ZERO,
        );
        db::repo::tenants::testing::allow_login(&state.db, DEFAULT_TENANT, "alice", 4_820_300)
            .await
            .expect("allow");
        state
            .db
            .users()
            .upsert_member(
                DEFAULT_TENANT,
                "u1",
                &GithubProfile {
                    github_id: 4_820_300,
                    login: "alice".to_owned(),
                    name: None,
                    avatar_url: None,
                },
                Timestamp::now(),
            )
            .await
            .expect("user");
        let grant = sign_in(&github).await;
        let lock = lock_user("u1").await;
        store_sign_in_tokens(&state, &lock, DEFAULT_TENANT, &grant, Timestamp::now())
            .await
            .expect("pair");
        drop(lock);
        let settings = DashboardSettings {
            linked_logins: vec!["alice-helper".to_owned()],
            repositories: vec![
                rule("platform", "PLAT"),
                rule("wallet-ui", "WUI"),
                rule("node-client", "NC"),
            ],
            enumerate_owners: vec!["acme".to_owned()],
            include_todos: false,
            time_zone: Some(WARSAW.to_owned()),
            ..DashboardSettings::default()
        };
        state
            .db
            .dashboard()
            .save_settings(DEFAULT_TENANT, "u1", &settings, Timestamp::now())
            .await
            .expect("settings");
        state
    }

    /// Ticks once and waits until the scheduled refresh's ranking is past its queueing.
    async fn scheduled_ranking(state: &AppState) -> RefreshRecord {
        assert_eq!(tick(state.clone()).await.expect("tick"), Tick::Ran);
        let ranked = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let latest = state
                    .db
                    .dashboard()
                    .latest_refresh(DEFAULT_TENANT, "u1")
                    .await
                    .expect("refresh")
                    .expect("started");
                if latest
                    .ranking_state
                    .is_some_and(|s| s != RankingState::Running)
                    || latest.counts.left_provisional > 0
                    || !state
                        .db
                        .jobs()
                        .list_for_refresh(latest.id, &[])
                        .await
                        .expect("jobs")
                        .is_empty()
                {
                    return latest;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        ranked.expect("the ranking queued its work")
    }

    #[tokio::test]
    async fn scheduled_refresh_queues_judge_jobs() {
        let state = world().await;
        let refresh = scheduled_ranking(&state).await;
        assert_eq!(refresh.trigger, RefreshTrigger::Scheduled);
        assert!(refresh.has_snapshot, "{refresh:?}");
        let jobs = state
            .db
            .jobs()
            .list_for_refresh(refresh.id, &[])
            .await
            .expect("jobs");
        assert!(
            !jobs.is_empty(),
            "the scheduled refresh's items go to the judge"
        );
    }

    #[tokio::test]
    async fn scheduled_refresh_respects_the_daily_judgement_cap() {
        let state = world().await;
        // Earlier today, before the scheduled 06:00: a refresh that used the whole daily cap.
        let dashboard = state.db.dashboard();
        let StartOutcome::Started { refresh, .. } = dashboard
            .start_refresh(
                DEFAULT_TENANT,
                "u1",
                RefreshTrigger::Manual,
                utc(2026, 11, 18, 4, 0),
            )
            .await
            .expect("earlier")
        else {
            panic!("not started");
        };
        dashboard
            .store_snapshot(
                DEFAULT_TENANT,
                "u1",
                refresh.id,
                &json!({}),
                utc(2026, 11, 18, 4, 1),
            )
            .await
            .expect("snapshot");
        let reserved = dashboard
            .reserve_sends(
                DEFAULT_TENANT,
                "u1",
                refresh.id,
                0,
                DAILY_CAP,
                db::repo::dashboard::SendCaps {
                    per_refresh: DAILY_CAP,
                    per_day: DAILY_CAP,
                },
                utc(2026, 11, 18, 4, 1),
            )
            .await
            .expect("reserve");
        assert_eq!(reserved, db::repo::dashboard::SendReservation::Reserved);
        let _ = dashboard
            .finish_ranking(
                DEFAULT_TENANT,
                "u1",
                refresh.id,
                0,
                RankingState::Finished,
                JudgeCounts {
                    judged: DAILY_CAP,
                    reused: 0,
                    left_provisional: 0,
                },
                utc(2026, 11, 18, 4, 2),
            )
            .await
            .expect("finish");
        let scheduled = scheduled_ranking(&state).await;
        assert_ne!(
            scheduled.id, refresh.id,
            "the 06:00 run is not skipped by a 05:01 refresh"
        );
        let jobs = state
            .db
            .jobs()
            .list_for_refresh(scheduled.id, &[])
            .await
            .expect("jobs");
        assert!(
            jobs.is_empty(),
            "no judgement beyond the daily cap: {jobs:?}"
        );
        assert!(
            scheduled.counts.left_provisional > 0,
            "the scanned items stay provisional: {scheduled:?}"
        );
    }
}
