//! The open-page rules of plan §M: presence (visits and their end), and the step per tick (F1 nothing while a refresh
//! is live or rate-limited, F2 automatic scan, F3 fact check).

use contracts::api::dashboard::{RefreshFailure, RefreshFailureCode, RefreshState, RefreshTrigger};
use contracts::ids::RefreshId;
use db::repo::dashboard::{JudgeCounts, RefreshProgress, RefreshRecord};

use super::*;

fn at(minutes: i64) -> Timestamp {
    "2026-11-23T10:00:00Z".parse::<Timestamp>().expect("time") + SignedDuration::from_mins(minutes)
}

fn alice() -> Scope {
    Scope {
        tenant_id: "default".to_owned(),
        user_id: "u1".to_owned(),
    }
}

fn refresh(state: RefreshState, started: i64) -> RefreshRecord {
    RefreshRecord {
        id: RefreshId::new_random(),
        user_id: "u1".to_owned(),
        trigger: RefreshTrigger::Manual,
        state,
        progress: RefreshProgress::default(),
        log: Vec::new(),
        failure: None,
        ranking_state: None,
        ranking_attempt: 0,
        counts: JudgeCounts::default(),
        has_snapshot: matches!(state, RefreshState::Ranking | RefreshState::Finished),
        started_at: at(started),
        scanned_at: Some(at(started + 1)),
        finished_at: None,
    }
}

/// A visit begins at the first read and lasts while reads come at most [`OPEN_FOR`] apart; after a longer gap the
/// next read begins a new visit; a user not read for longer is no longer open.
#[test]
fn a_visit_lasts_while_reads_come_and_ends_without_them() {
    let readers = Readers::default();
    let scope = alice();
    assert!(readers.open(at(0)).is_empty());
    readers.mark_open(&scope, at(0));
    readers.mark_open(&scope, at(2));
    assert_eq!(readers.open(at(4)), [(scope.clone(), at(0))], "one visit");
    assert!(readers.open(at(6)).is_empty(), "no read for 4 min: closed");
    readers.mark_open(&scope, at(10));
    assert_eq!(
        readers.open(at(10)),
        [(scope.clone(), at(10))],
        "a new visit"
    );
}

/// The last fact check counts for the snapshot it checked only.
#[test]
fn a_fact_check_counts_for_its_snapshot() {
    let readers = Readers::default();
    let (checked, newer) = (RefreshId::new_random(), RefreshId::new_random());
    readers.checked(&alice(), checked, at(3));
    assert_eq!(readers.checked_at(&alice(), checked), Some(at(3)));
    assert_eq!(readers.checked_at(&alice(), newer), None);
}

/// F1: a live refresh or a rate limit not reset yet stops everything; F2: no refresh, the newest started 30 min ago,
/// or before this visit and 15 min ago, starts a scan (whatever its outcome); F3: facts read 5 min ago are checked.
#[test]
fn each_tick_takes_the_first_step_that_applies() {
    let visit = at(0);
    let (now, recent) = (at(10), Some(at(9)));
    assert_eq!(step(now, visit, None, None), Step::Scan, "never refreshed");
    for live in [RefreshState::Scanning, RefreshState::Ranking] {
        assert_eq!(
            step(at(60), visit, Some(&refresh(live, 0)), None),
            Step::Nothing,
            "{live:?}"
        );
    }
    let mut limited = refresh(RefreshState::Failed, -60);
    limited.failure = Some(RefreshFailure {
        code: RefreshFailureCode::GithubRateLimited,
        message: "limited".into(),
        retry_at: Some(at(20)),
    });
    assert_eq!(step(now, visit, Some(&limited), recent), Step::Nothing);
    assert_eq!(
        step(at(21), visit, Some(&limited), recent),
        Step::Scan,
        "reset"
    );

    let finished = |started| refresh(RefreshState::Finished, started);
    assert_eq!(
        step(now, visit, Some(&finished(-10)), recent),
        Step::Scan,
        "on opening"
    );
    assert_eq!(
        step(now, visit, Some(&finished(-4)), recent),
        Step::Nothing,
        "fresh on opening"
    );
    assert_eq!(
        step(at(31), visit, Some(&finished(1)), Some(at(28))),
        Step::Scan,
        "every 30 min"
    );
    assert_eq!(
        step(at(30), visit, Some(&finished(1)), Some(at(28))),
        Step::Nothing
    );
    assert_eq!(
        step(at(30), visit, Some(&finished(1)), Some(at(25))),
        Step::CheckFacts,
        "facts 5 min old"
    );
    assert_eq!(
        step(at(30), visit, Some(&finished(1)), None),
        Step::CheckFacts
    );
    let failed = refresh(RefreshState::Failed, 5);
    assert_eq!(
        step(at(20), visit, Some(&failed), None),
        Step::CheckFacts,
        "a failed scan is tried again after 30 min only"
    );
    assert_eq!(step(at(35), visit, Some(&failed), None), Step::Scan);
}
