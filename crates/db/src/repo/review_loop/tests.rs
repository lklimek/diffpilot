use contracts::api::access_tokens::AccessTokenId;
use contracts::api::review_loop::{
    AuthorKind, CarryEvidence, CarryKind, ChannelKind, CommentAssessment, CommentItemKind,
    DecisionKind, DecisionVia, DeliveryOutcome, EffectKind, EffectParams, EffectState, Executor,
    Gate, GateMark, GateSource, ItemGithubFacts, ItemLocation, ItemState, Necessity,
    NecessityVerdict, OutsideResolution, ReaskReason, Recommendation, ReportOutcome, UserEventKind,
    Validity, WatchState,
};
use contracts::common::{LineRange, RepoRef};
use contracts::ids::{CommitSha, JobId};
use contracts::time::Timestamp;
use jiff::SignedDuration;

use super::*;
use crate::repo::DEFAULT_TENANT;

const T: &str = DEFAULT_TENANT;
const PR: i64 = 1;

fn at(second: i64) -> Timestamp {
    Timestamp::from_second(1_795_000_000 + second).expect("time")
}

fn head() -> CommitSha {
    CommitSha("52d2da140191a9d47becf6a446310be803c007fc".into())
}

async fn db() -> Db {
    let db = Db::in_memory().await.expect("in-memory database");
    sqlx::raw_sql(
        "INSERT INTO users (id, github_id, login, created_at, updated_at)
           VALUES ('u1', 1, 'alice', 't', 't'), ('u2', 2, 'bob', 't', 't');
         INSERT INTO repos (id, tenant_id, owner, name) VALUES (1, 'default', 'acme', 'node-client');
         INSERT INTO pull_requests (id, tenant_id, repo_id, number, title, author, html_url, base_sha, head_sha,
             fetched_at)
           VALUES (1, 'default', 1, 482, 'feat(rpc): retry', 'alice', 'u', 'b', 'h', 't'),
                  (2, 'default', 1, 490, 'fix(rpc): cap', 'alice', 'u', 'b', 'h', 't'),
                  (3, 'default', 1, 501, 'docs: retry', 'alice', 'u', 'b', 'h', 't');
         INSERT INTO access_tokens (id, tenant_id, user_id, name, token_hash, scopes, created_at, expires_at)
           VALUES ('agentAAA', 'default', 'u1', 'laptop agent', zeroblob(32), '[\"review_loop_execute\"]', 't',
                   '2999-01-01T00:00:00.000000Z'),
                  ('agentBBB', 'default', 'u1', 'ci agent', randomblob(32), '[\"review_loop_execute\"]', 't',
                   '2999-01-01T00:00:00.000000Z');",
    )
    .execute(&db.pool)
    .await
    .expect("seed");
    db.review_loop().ensure_pr(T, PR, at(0)).await.expect("pr");
    db
}

fn observed(key: &str, body: &str) -> ObservedItem {
    ObservedItem {
        kind: CommentItemKind::Thread,
        stable_key: key.into(),
        author: "thepastaclaw".into(),
        author_kind: AuthorKind::Bot,
        url: Some(format!(
            "https://github.com/acme/node-client/pull/482#{key}"
        )),
        location: Some(ItemLocation {
            path: "src/rpc/client.rs".into(),
            lines: Some(LineRange {
                start_line: 96,
                end_line: 99,
            }),
        }),
        title: "Do not retry a call that may have been applied".into(),
        body: body.into(),
        body_hash: format!("hash:{body}"),
        head: head(),
        round: Some(1),
        bot_severity: Some("Blocking".into()),
        github: ItemGithubFacts {
            resolved: Some(false),
            resolved_by: None,
            outdated: Some(false),
            bot_status: None,
            edited_after_decision: false,
            replies: 0,
            marker: Some(key.into()),
        },
        outside: None,
    }
}

async fn new_item(db: &Db, key: &str) -> ItemRecord {
    match db
        .review_loop()
        .upsert_item(T, PR, &observed(key, "body"), ItemState::Undecided, at(1))
        .await
        .expect("upsert")
    {
        UpsertOutcome::New(item) => item,
        other => panic!("expected a new item: {other:?}"),
    }
}

fn fix_decision(item: &ItemRecord) -> NewDecision {
    NewDecision {
        item: item.id,
        item_revision: item.revision,
        user_id: "u1".into(),
        kind: DecisionKind::FixHere,
        reason: None,
        target: None,
        r#override: None,
        resolve_ticked: true,
        reply_text: Some("Per maintainer decision, fixed.".into()),
        comment: Some("keep the test small".into()),
        recommendation: None,
        via: DecisionVia::Question,
        carried_from: None,
        head: head(),
        effects: vec![
            NewEffect {
                seq: 1,
                after: None,
                kind: EffectKind::Fix,
                executor: Executor::Agent,
                params: EffectParams::default(),
            },
            NewEffect {
                seq: 2,
                after: Some(1),
                kind: EffectKind::Reply,
                executor: Executor::Agent,
                params: EffectParams {
                    text: Some("Per maintainer decision, fixed.".into()),
                    ..EffectParams::default()
                },
            },
            NewEffect {
                seq: 3,
                after: Some(2),
                kind: EffectKind::Resolve,
                executor: Executor::Agent,
                params: EffectParams::default(),
            },
        ],
    }
}

async fn decide_one(
    db: &Db,
    decision: NewDecision,
    now: Timestamp,
) -> contracts::api::review_loop::Decision {
    match db
        .review_loop()
        .decide(T, &[decision], now)
        .await
        .expect("decide")
    {
        DecideOutcome::Stored(mut stored) => stored.pop().expect("one"),
        DecideOutcome::ItemChanged(nos) => panic!("item changed: {nos:?}"),
    }
}

#[tokio::test]
async fn items_are_numbered_once_and_revised_only_on_change() {
    let db = db().await;
    let rl = db.review_loop();
    let first = new_item(&db, "dedupe-a").await;
    let second = new_item(&db, "dedupe-b").await;
    assert_eq!((first.no, second.no), (1, 2));
    assert_eq!(first.revision, 1);
    let same = rl
        .upsert_item(
            T,
            PR,
            &observed("dedupe-a", "body"),
            ItemState::Undecided,
            at(2),
        )
        .await
        .expect("upsert");
    assert!(matches!(same, UpsertOutcome::Unchanged(ref item) if item.revision == 1));
    let edited = rl
        .upsert_item(
            T,
            PR,
            &observed("dedupe-a", "edited"),
            ItemState::Undecided,
            at(3),
        )
        .await
        .expect("upsert");
    let UpsertOutcome::Changed(edited) = edited else {
        panic!("expected a change: {edited:?}");
    };
    assert_eq!(
        (edited.no, edited.revision, edited.body_hash.as_str()),
        (1, 2, "hash:edited")
    );
    let mut resolved = observed("dedupe-a", "edited");
    resolved.outside = Some(OutsideResolution {
        by: "carol".into(),
        seen_at: at(4),
    });
    let UpsertOutcome::Changed(outside) = rl
        .upsert_item(T, PR, &resolved, ItemState::Undecided, at(4))
        .await
        .expect("upsert")
    else {
        panic!("expected a change");
    };
    assert_eq!(outside.state, ItemState::Outside);
    let UpsertOutcome::Changed(reopened) = rl
        .upsert_item(
            T,
            PR,
            &observed("dedupe-a", "edited"),
            ItemState::Undecided,
            at(5),
        )
        .await
        .expect("upsert")
    else {
        panic!("expected a change");
    };
    assert_eq!(
        (reopened.state, reopened.revision),
        (ItemState::Undecided, 4)
    );
    assert_eq!(rl.items(PR).await.expect("items").len(), 2);
    assert_eq!(
        rl.item_by_no(PR, 2)
            .await
            .expect("item")
            .map(|item| item.id),
        Some(second.id)
    );
    assert!(matches!(
        rl.set_item_state(first.id, 1, ItemState::AskedBack, at(6))
            .await,
        Err(DbError::Conflict)
    ));
    let gated = rl
        .set_gates(
            first.id,
            4,
            &[GateMark {
                gate: Gate::Funds,
                source: GateSource::Owner,
            }],
            at(6),
        )
        .await
        .expect("gates");
    assert_eq!((gated.revision, gated.gates.len()), (5, 1));
}

#[tokio::test]
async fn read_failure_keeps_the_last_good_read() {
    let db = db().await;
    let rl = db.review_loop();
    rl.record_read(
        PR,
        &ReadResult::Read {
            snapshot: serde_json::json!({"threads": 8}),
            round_count: 3,
        },
        at(10),
    )
    .await
    .expect("read");
    rl.record_read(
        PR,
        &ReadResult::Failed(contracts::api::review_loop::ReadFailure::RateLimited),
        at(20),
    )
    .await
    .expect("failure");
    let state = rl.pr_state(PR).await.expect("state").expect("row");
    assert_eq!(state.read_at, Some(at(10)));
    assert_eq!(state.round_count, 3);
    assert_eq!(
        state.read_failure,
        Some((
            contracts::api::review_loop::ReadFailure::RateLimited,
            at(20)
        ))
    );
    assert_eq!(
        state.github_snapshot,
        Some(serde_json::json!({"threads": 8}))
    );
    assert!(matches!(
        rl.record_read(
            99,
            &ReadResult::Failed(contracts::api::review_loop::ReadFailure::Unavailable),
            at(30)
        )
        .await,
        Err(DbError::NotFound)
    ));
}

#[tokio::test]
async fn assessments_keep_their_fingerprint_and_newest_wins() {
    let db = db().await;
    let rl = db.review_loop();
    let item = new_item(&db, "dedupe-a").await;
    let assessment = |reason: &str| CommentAssessment {
        job: JobId::new_random(),
        head: head(),
        assessed_at: at(5),
        reused_since: None,
        validity: Validity::Valid,
        evidence: vec![],
        pre_existing: false,
        category: "correctness".into(),
        necessity: Necessity {
            verdict: NecessityVerdict::In,
            reason: reason.into(),
            better_pr: None,
        },
        gates: vec![],
        elsewhere: vec![],
        fix_sketch: None,
        fix_size: None,
        reply_drafts: vec![],
        recommendation: Recommendation {
            kind: DecisionKind::FixHere,
            reason: reason.into(),
            target: None,
            reason_class: "real bug".into(),
        },
        code: None,
        prompt_version: 1,
        skills_commit: Some("0123456789abcdef0123456789abcdef01234567".into()),
    };
    rl.add_assessment(T, item.id, &assessment("first"), "fp1")
        .await
        .expect("first");
    rl.add_assessment(T, item.id, &assessment("second"), "fp2")
        .await
        .expect("second");
    let latest = rl
        .latest_assessment(item.id)
        .await
        .expect("latest")
        .expect("some");
    assert_eq!(latest.fingerprint, "fp2");
    assert_eq!(latest.assessment.necessity.reason, "second");
    assert_eq!(
        latest.assessment.skills_commit.as_deref(),
        Some("0123456789abcdef0123456789abcdef01234567")
    );
    // An assessment stored before SK1 has no skills commit and still reads.
    sqlx::query("UPDATE comment_assessments SET body = json_remove(body, '$.skills_commit')")
        .execute(&db.pool)
        .await
        .expect("old body");
    let old = rl
        .latest_assessment(item.id)
        .await
        .expect("latest")
        .expect("some");
    assert_eq!(old.assessment.skills_commit, None);
}

#[tokio::test]
async fn one_live_pass_per_pull_request() {
    let db = db().await;
    let rl = db.review_loop();
    let pass = NewPass {
        pull_request_id: PR,
        user_id: Some("u1".into()),
        cause: PassCause::Opened,
        head: head(),
        sent: Vec::new(),
        question: None,
    };
    let PassStart::Started(first) = rl.start_pass(T, &pass, at(0)).await.expect("start") else {
        panic!("expected a start");
    };
    assert_eq!(
        rl.start_pass(T, &pass, at(1)).await.expect("busy"),
        PassStart::Busy(first)
    );
    rl.pass_running(first, JobId::new_random())
        .await
        .expect("running");
    rl.finish_pass(first, true, at(60)).await.expect("finish");
    assert!(matches!(
        rl.start_pass(T, &pass, at(61)).await.expect("again"),
        PassStart::Started(_)
    ));
    assert_eq!(
        rl.pass_usage("u1", at(0), PR).await.expect("usage"),
        (2, Some(at(60)))
    );
    assert_eq!(rl.pass_usage("u1", at(1), PR).await.expect("usage").0, 1);
}

#[tokio::test]
async fn decisions_need_the_current_revision_and_supersede_the_previous() {
    let db = db().await;
    let rl = db.review_loop();
    let item = new_item(&db, "dedupe-a").await;
    let other = new_item(&db, "dedupe-b").await;
    let first = decide_one(&db, fix_decision(&item), at(10)).await;
    assert_eq!(first.item_no, 1);
    assert_eq!(first.decided_by, "alice");
    assert_eq!(first.comment.as_deref(), Some("keep the test small"));
    let states: Vec<_> = first.effects.iter().map(|effect| effect.state).collect();
    assert_eq!(
        states,
        [
            EffectState::Open,
            EffectState::Waiting,
            EffectState::Waiting
        ]
    );
    let after = rl.item_by_no(PR, 1).await.expect("item").expect("some");
    assert_eq!(after.state, ItemState::Executing);
    assert!(after.revision > item.revision);

    // A stale revision stores nothing, also for the other rows of the batch.
    let stale = rl
        .decide(T, &[fix_decision(&other), fix_decision(&item)], at(11))
        .await
        .expect("decide");
    assert_eq!(stale, DecideOutcome::ItemChanged(vec![1]));
    assert!(rl.item_decisions(other.id).await.expect("none").is_empty());

    let mut reject = fix_decision(&after);
    reject.kind = DecisionKind::Reject;
    reject.reason = Some("Out of scope.".into());
    reject.effects.clear();
    let second = decide_one(&db, reject, at(12)).await;
    assert_eq!(second.supersedes, Some(first.id));
    let history = rl.item_decisions(item.id).await.expect("history");
    assert_eq!(history.len(), 2);
    assert!(
        history[0]
            .decision
            .effects
            .iter()
            .all(|effect| effect.state == EffectState::Cancelled)
    );
    assert_eq!(history[0].body_hash, "hash:body");
    assert_eq!(
        history[0].lines,
        Some(LineRange {
            start_line: 96,
            end_line: 99
        })
    );
    let decided = rl.item_by_no(PR, 1).await.expect("item").expect("some");
    assert_eq!(decided.state, ItemState::Decided);
    assert_eq!(rl.pr_decisions(PR).await.expect("pr").len(), 2);

    let update = sqlx::query("UPDATE comment_decisions SET comment = 'changed'")
        .execute(&db.pool)
        .await;
    assert!(update.is_err(), "decisions are append-only");
}

#[tokio::test]
async fn claims_hold_a_lease_and_reports_come_from_the_claimer() {
    let db = db().await;
    let rl = db.review_loop();
    let item = new_item(&db, "dedupe-a").await;
    let decision = decide_one(&db, fix_decision(&item), at(10)).await;
    let a = AccessTokenId("agentAAA".into());
    let b = AccessTokenId("agentBBB".into());
    let lease = SignedDuration::from_mins(30);
    let all = [EffectKind::Fix, EffectKind::Reply, EffectKind::Resolve];
    let repo = RepoRef {
        owner: "ACME".into(),
        name: "node-client".into(),
    };

    let feed = rl
        .claimable(T, &all, Some(&repo), at(11))
        .await
        .expect("feed");
    assert_eq!(feed.len(), 1);
    assert_eq!(feed[0].pr.number, 482);
    assert_eq!(feed[0].item.no, 1);
    assert!(
        rl.claimable(T, &[EffectKind::Reply, EffectKind::Resolve], None, at(11))
            .await
            .expect("feed")
            .is_empty(),
        "fix is outside these capabilities"
    );
    assert_eq!(
        rl.claim(decision.id, &a, &[EffectKind::Reply], lease, at(11))
            .await
            .expect("claim"),
        ClaimOutcome::CapabilityMissing
    );
    assert_eq!(
        rl.claim(decision.id, &a, &all, lease, at(12))
            .await
            .expect("claim"),
        ClaimOutcome::Claimed(at(12) + lease)
    );
    assert_eq!(
        rl.claim(decision.id, &b, &all, lease, at(13))
            .await
            .expect("claim"),
        ClaimOutcome::Taken
    );
    assert!(
        rl.claimable(T, &all, None, at(13))
            .await
            .expect("feed")
            .is_empty()
    );

    let fix = decision.effects[0].id;
    let done = |key: &str| ReportRequest {
        outcome: ReportOutcome::Done,
        reference: Some("9f3c2e1a0b".into()),
        message: None,
        idempotency_key: key.into(),
    };
    assert_eq!(
        rl.report(T, fix, &b, &done("k1"), lease, at(14))
            .await
            .expect("report"),
        ReportOutcomeRecord::NotClaimer
    );
    assert_eq!(
        rl.report(T, fix, &a, &done("k1"), lease, at(14))
            .await
            .expect("report"),
        ReportOutcomeRecord::Recorded
    );
    assert_eq!(
        rl.report(T, fix, &a, &done("k1"), lease, at(15))
            .await
            .expect("report"),
        ReportOutcomeRecord::Duplicate
    );
    let effects = rl.item_decisions(item.id).await.expect("decisions")[0]
        .decision
        .effects
        .clone();
    assert_eq!(effects[0].state, EffectState::Done);
    assert_eq!(effects[0].reports.len(), 1);
    assert_eq!(effects[0].reports[0].token_name, "laptop agent");
    let renewed = effects[1].claim.clone().expect("claimed");
    assert_eq!(
        (renewed.token_name.as_str(), renewed.lease_until),
        ("laptop agent", at(14) + lease)
    );

    // The lease ends: the rest returns to the feed and the old claimer's report is refused.
    let later = at(14) + lease + SignedDuration::from_secs(1);
    assert_eq!(
        rl.report(T, effects[1].id, &a, &done("k2"), lease, later)
            .await
            .expect("report"),
        ReportOutcomeRecord::LeaseExpired
    );
    assert_eq!(
        rl.expire_leases(later).await.expect("expire"),
        vec![decision.id]
    );
    let effects = rl.item_decisions(item.id).await.expect("decisions")[0]
        .decision
        .effects
        .clone();
    assert_eq!(effects[1].state, EffectState::Open, "its fix is done");
    assert_eq!(effects[2].state, EffectState::Waiting);
    assert!(effects[1].claim.is_none());

    // Another agent takes it, releases it, takes it again and finishes.
    assert!(matches!(
        rl.claim(decision.id, &b, &all, lease, later)
            .await
            .expect("claim"),
        ClaimOutcome::Claimed(_)
    ));
    let release = ReportRequest {
        outcome: ReportOutcome::Released,
        reference: None,
        message: None,
        idempotency_key: "r1".into(),
    };
    assert_eq!(
        rl.report(T, effects[1].id, &b, &release, lease, later)
            .await
            .expect("release"),
        ReportOutcomeRecord::Recorded
    );
    assert_eq!(
        rl.claimable(T, &all, None, later)
            .await
            .expect("feed")
            .len(),
        1
    );
    rl.claim(decision.id, &b, &all, lease, later)
        .await
        .expect("claim");
    for (effect, key) in [(effects[1].id, "b2"), (effects[2].id, "b3")] {
        assert_eq!(
            rl.report(T, effect, &b, &done(key), lease, later)
                .await
                .expect("report"),
            ReportOutcomeRecord::Recorded
        );
    }
    let item = rl.item_by_no(PR, 1).await.expect("item").expect("some");
    assert_eq!(item.state, ItemState::Done);
    assert_eq!(
        rl.claim(decision.id, &a, &all, lease, later)
            .await
            .expect("claim"),
        ClaimOutcome::NothingOpen
    );
    let fetched = rl
        .feed_decision(decision.id)
        .await
        .expect("fetch")
        .expect("some");
    assert_eq!(fetched.decision.id, decision.id);
}

#[tokio::test]
async fn carry_events_are_logged_and_counted() {
    let db = db().await;
    let rl = db.review_loop();
    let source = new_item(&db, "dedupe-a").await;
    let repeat = new_item(&db, "dedupe-b").await;
    let decision = decide_one(&db, fix_decision(&source), at(10)).await;
    let evidence = CarryEvidence {
        url: repeat.url.clone(),
        body_hash_before: None,
        body_hash_after: None,
        commit: None,
        lines: None,
    };
    let carried = NewCarryEvent {
        item: repeat.id,
        source_decision: decision.id,
        kind: CarryKind::Carried,
        reason: None,
        evidence: evidence.clone(),
    };
    rl.record_carry(T, &carried, at(20)).await.expect("carry");
    let asked = NewCarryEvent {
        item: source.id,
        source_decision: decision.id,
        kind: CarryKind::AskedAgain,
        reason: Some(ReaskReason::BotEdited),
        evidence: CarryEvidence {
            body_hash_before: Some("hash:body".into()),
            body_hash_after: Some("hash:edited".into()),
            ..evidence
        },
    };
    rl.record_carry(T, &asked, at(30)).await.expect("re-ask");
    let mut wrong = asked.clone();
    wrong.reason = None;
    assert!(matches!(
        rl.record_carry(T, &wrong, at(31)).await,
        Err(DbError::Conflict)
    ));

    let events = rl
        .carry_events(T, Some(PR), None, 10)
        .await
        .expect("events");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].kind, CarryKind::AskedAgain);
    assert_eq!((events[1].item_no, events[1].source_item_no), (2, 1));
    assert_eq!(events[1].source_kind, DecisionKind::FixHere);
    assert_eq!(events[1].effects_pending, 3);
    let page = rl
        .carry_events(T, None, Some((events[0].at, events[0].id)), 10)
        .await
        .expect("page");
    assert_eq!(page.len(), 1);
    let counts = rl.carry_counts(T, None).await.expect("counts");
    assert_eq!(counts.len(), 1);
    assert_eq!(
        counts[0].1,
        contracts::api::review_loop::CarryCounts {
            carried: 1,
            asked_again_bot_edited: 1,
            asked_again_code_changed: 0,
            effects_pending: 1,
        }
    );
    let repeat = rl.item_by_no(PR, 2).await.expect("item").expect("some");
    assert_eq!(
        (repeat.state, repeat.repeat_of),
        (ItemState::Repeat, Some(source.id))
    );
    let source = rl.item_by_no(PR, 1).await.expect("item").expect("some");
    assert_eq!(
        (source.state, source.repeat_of),
        (ItemState::Reopened, None)
    );
}

#[tokio::test]
async fn watches_are_limited_and_ended_ones_do_not_count() {
    let db = db().await;
    let rl = db.review_loop();
    assert_eq!(
        rl.watch(T, "u1", 1, 2, at(0)).await.expect("watch"),
        WatchOutcome::Watching
    );
    assert_eq!(
        rl.watch(T, "u1", 1, 2, at(1)).await.expect("again"),
        WatchOutcome::Watching
    );
    rl.watch(T, "u1", 2, 2, at(2)).await.expect("second");
    assert_eq!(
        rl.watch(T, "u1", 3, 2, at(3)).await.expect("third"),
        WatchOutcome::LimitReached
    );
    rl.set_watch_state("u1", 2, WatchState::Ended, at(4))
        .await
        .expect("end");
    assert_eq!(
        rl.watch(T, "u1", 3, 2, at(5)).await.expect("third"),
        WatchOutcome::Watching
    );
    rl.record_probe("u1", 1, &serde_json::json!({"threads": 8}), at(6))
        .await
        .expect("probe");
    rl.record_auto_pass("u1", 1, 5, at(7)).await.expect("pass");
    let watches = rl.watches("u1").await.expect("watches");
    assert_eq!(watches.len(), 3);
    let first = watches
        .iter()
        .find(|watch| watch.pull_request_id == 1)
        .expect("482");
    assert_eq!(first.pr.number, 482);
    assert_eq!(first.since, at(0));
    assert_eq!(first.last_probe_at, Some(at(6)));
    assert_eq!(
        first.last_auto_pass,
        Some(contracts::api::review_loop::AutoPass {
            at: at(7),
            questions: 5
        })
    );
    let ended = watches
        .iter()
        .find(|watch| watch.pull_request_id == 2)
        .expect("490");
    assert_eq!(
        (ended.state, ended.ended_at),
        (WatchState::Ended, Some(at(4)))
    );
    assert_eq!(rl.live_watches().await.expect("live").len(), 2);
    assert!(rl.unwatch("u1", 1).await.expect("unwatch"));
    assert!(!rl.unwatch("u1", 1).await.expect("gone"));
}

#[tokio::test]
async fn a_due_pass_keeps_its_first_time_and_watching_again_clears_watch_state() {
    let db = db().await;
    let rl = db.review_loop();
    rl.watch(T, "u1", 1, 20, at(0)).await.expect("watch");
    rl.set_pass_due("u1", 1, Some(at(1))).await.expect("due");
    rl.set_pass_due("u1", 1, Some(at(2)))
        .await
        .expect("due again");
    rl.set_notified("u1", 1, "round:2").await.expect("notified");
    let watch = &rl.watches("u1").await.expect("watches")[0];
    assert_eq!(
        (watch.pass_due_at, watch.notified_for.as_deref()),
        (Some(at(1)), Some("round:2")),
        "the first due time stays: the bot-round wait counts from it"
    );
    rl.set_pass_due("u1", 1, None).await.expect("clear");
    assert_eq!(
        rl.watches("u1").await.expect("watches")[0].pass_due_at,
        None
    );
    rl.set_pass_due("u1", 1, Some(at(3))).await.expect("due");
    rl.set_watch_state("u1", 1, WatchState::Ended, at(4))
        .await
        .expect("end");
    rl.watch(T, "u1", 1, 20, at(5)).await.expect("again");
    let watch = &rl.watches("u1").await.expect("watches")[0];
    assert_eq!(
        (
            watch.pass_due_at,
            watch.notified_for.as_deref(),
            watch.state
        ),
        (None, None, WatchState::Active)
    );
    assert!(matches!(
        rl.set_pass_due("u2", 1, Some(at(6))).await,
        Err(DbError::NotFound)
    ));
    assert_eq!(rl.charged_passes("u1", at(0)).await.expect("passes"), 0);
}

#[tokio::test]
async fn user_events_are_read_after_a_time() {
    let db = db().await;
    let rl = db.review_loop();
    let event = |kind, pull_request_id, count| NewUserEvent {
        user_id: "u1".into(),
        kind,
        pull_request_id,
        count,
        text: "acme/node-client #482: 5 questions ready".into(),
        url: Some("/pr/acme/node-client/482/comments".into()),
    };
    rl.add_user_event(T, &event(UserEventKind::QuestionsReady, Some(PR), 5), at(1))
        .await
        .expect("event");
    rl.add_user_event(T, &event(UserEventKind::Test, None, 0), at(2))
        .await
        .expect("test");
    assert!(
        rl.add_user_event(T, &event(UserEventKind::Test, Some(PR), 0), at(3))
            .await
            .is_err(),
        "a test event names no PR"
    );
    let all = rl.user_events("u1", None, 10).await.expect("events");
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].pr.as_ref().map(|pr| pr.number), Some(482));
    assert_eq!(all[0].count, 5);
    let later = rl.user_events("u1", Some(at(1)), 10).await.expect("later");
    assert_eq!(later.len(), 1);
    assert_eq!(later[0].kind, UserEventKind::Test);
    assert!(
        rl.user_events("u2", None, 10)
            .await
            .expect("other")
            .is_empty()
    );
}

#[tokio::test]
async fn channel_secrets_stay_ciphertext_and_health_changes_once() {
    let db = db().await;
    let rl = db.review_loop();
    let input = |secret| ChannelInput {
        url: "https://ntfy.example".into(),
        topic: Some("diffpilot-alice".into()),
        secret,
        events: vec![UserEventKind::QuestionsReady],
        enabled: true,
    };
    let created = rl
        .put_channel(
            T,
            "u1",
            ChannelKind::Ntfy,
            &input(SecretUpdate::Set(vec![7, 7, 7])),
        )
        .await
        .expect("create");
    let kept = rl
        .put_channel(T, "u1", ChannelKind::Ntfy, &input(SecretUpdate::Keep))
        .await
        .expect("keep");
    assert_eq!(kept.id, created.id);
    assert_eq!(kept.secret, Some(vec![7, 7, 7]));
    assert!(!format!("{kept:?}").contains("7, 7, 7"));
    let cleared = rl
        .put_channel(T, "u1", ChannelKind::Ntfy, &input(SecretUpdate::Clear))
        .await
        .expect("clear");
    assert_eq!(cleared.secret, None);

    let mut outcomes = Vec::new();
    for (outcome, second) in [
        (DeliveryOutcome::Failed, 10),
        (DeliveryOutcome::Failed, 20),
        (DeliveryOutcome::Delivered, 30),
    ] {
        outcomes.push(
            rl.record_channel_delivery(
                created.id,
                UserEventKind::QuestionsReady,
                outcome,
                Some("connection refused (3 attempts)"),
                at(second),
            )
            .await
            .expect("record"),
        );
    }
    assert_eq!(
        outcomes,
        [
            FailureTransition::TurnedFailing,
            FailureTransition::Unchanged,
            FailureTransition::Recovered
        ]
    );
    let channel = rl.channels("u1").await.expect("channels").remove(0);
    assert_eq!(channel.delivery.failing_since, None);
    assert_eq!(
        channel.delivery.last.map(|last| (last.outcome, last.at)),
        Some((DeliveryOutcome::Delivered, at(30)))
    );
    assert!(
        rl.delete_channel("u1", ChannelKind::Ntfy)
            .await
            .expect("delete")
    );
    assert!(rl.channels("u1").await.expect("none").is_empty());
}

#[tokio::test]
async fn push_targets_and_deliveries() {
    let db = db().await;
    let rl = db.review_loop();
    let token = AccessTokenId("agentAAA".into());
    rl.subscribe(T, &token, &[EffectKind::Reply], at(0))
        .await
        .expect("subscribe");
    let target = rl
        .put_push_target(
            T,
            &token,
            &PushInput {
                kind: ChannelKind::Webhook,
                url: "http://homeassistant.lan/api/webhook/x".into(),
                topic: None,
                secret: SecretUpdate::Set(vec![1, 2, 3]),
            },
            at(1),
        )
        .await
        .expect("push target");
    assert_eq!(target.capabilities, [EffectKind::Reply]);
    assert!(!format!("{target:?}").contains("1, 2, 3"));
    let subscriptions = rl.subscriptions(T, at(2)).await.expect("subscriptions");
    assert_eq!(subscriptions.len(), 1);
    assert_eq!(subscriptions[0].token_name, "laptop agent");

    let item = new_item(&db, "dedupe-a").await;
    let decision = decide_one(&db, fix_decision(&item), at(3)).await;
    let push = DeliveryTarget::Push {
        decision: decision.id,
        token: token.clone(),
    };
    assert!(rl.enqueue_delivery(T, &push, at(4)).await.expect("enqueue"));
    assert!(!rl.enqueue_delivery(T, &push, at(5)).await.expect("once"));
    let due = rl.due_deliveries(at(4), 10).await.expect("due");
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].target, push);
    rl.record_attempt(
        due[0].id,
        DeliveryOutcome::Failed,
        Some(at(64)),
        Some(503),
        Some("service unavailable"),
        at(4),
    )
    .await
    .expect("retry later");
    assert!(
        rl.due_deliveries(at(5), 10)
            .await
            .expect("not yet")
            .is_empty()
    );
    let due = rl.due_deliveries(at(64), 10).await.expect("due again");
    assert_eq!(due[0].attempts, 1);
    rl.record_attempt(
        due[0].id,
        DeliveryOutcome::Delivered,
        None,
        Some(200),
        None,
        at(64),
    )
    .await
    .expect("delivered");
    assert!(
        rl.due_deliveries(at(999), 10)
            .await
            .expect("done")
            .is_empty()
    );
    assert!(matches!(
        rl.record_attempt(
            due[0].id,
            DeliveryOutcome::Delivered,
            None,
            None,
            None,
            at(65)
        )
        .await,
        Err(DbError::NotFound)
    ));
    assert_eq!(
        rl.record_push_delivery(&token, DeliveryOutcome::Failed, Some("timeout"), at(70))
            .await
            .expect("health"),
        FailureTransition::TurnedFailing
    );
    assert!(rl.delete_push_target(&token).await.expect("delete"));
    assert!(!rl.delete_push_target(&token).await.expect("gone"));
}

fn names<T: serde::Serialize>(values: impl IntoIterator<Item = T>) -> String {
    let quoted: Vec<String> = values
        .into_iter()
        .map(|value| format!("'{}'", wire(value).expect("wire name")))
        .collect();
    format!("IN ({})", quoted.join(", "))
}

/// The CHECK lists of migration 0017 mirror the contract enums (`contracts::api::review_loop`).
#[tokio::test]
async fn check_lists_mirror_the_review_loop_enums() {
    use contracts::api::review_loop::{
        ChannelKind as C, CommentItemKind as K, DecisionKind as D, DecisionVia as V,
        EffectKind as E, EffectState as S, ItemState as I, ReadFailure as F, UserEventKind as U,
        WatchState as W,
    };
    let db = Db::in_memory().await.expect("db");
    let cases = [
        (
            "comment_prs",
            names([F::RateLimited, F::Unavailable, F::SignInRequired]),
        ),
        (
            "comment_items",
            names([K::Thread, K::ReviewBody, K::PrComment, K::PrQuestion]),
        ),
        (
            "comment_items",
            names([AuthorKind::Human, AuthorKind::Bot, AuthorKind::Diffpilot]),
        ),
        (
            "comment_items",
            names([
                I::Assessing,
                I::Undecided,
                I::AskedBack,
                I::Decided,
                I::Executing,
                I::Done,
                I::Repeat,
                I::Reopened,
                I::Outside,
            ]),
        ),
        (
            "comment_passes",
            names([
                PassCause::Opened,
                PassCause::Read,
                PassCause::AssessAgain,
                PassCause::Question,
                PassCause::Watch,
            ]),
        ),
        (
            "comment_decisions",
            names([
                D::Resolve,
                D::FixHere,
                D::Reject,
                D::HandledInPr,
                D::FollowUpPr,
                D::UpstreamIssue,
                D::Todo,
                D::MergeWithFollowUps,
                D::AnotherRound,
                D::DismissStaleReviews,
                D::KeepReviews,
            ]),
        ),
        (
            "comment_decisions",
            names([V::Question, V::Group, V::Bulk, V::Table, V::Carried]),
        ),
        (
            "comment_effects",
            names([
                E::Fix,
                E::Reply,
                E::Resolve,
                E::PrComment,
                E::FollowUpPr,
                E::UpstreamIssue,
                E::Todo,
                E::DismissReview,
            ]),
        ),
        (
            "comment_effects",
            names([Executor::Agent, Executor::DiffpilotBot]),
        ),
        (
            "comment_effects",
            names([
                S::Waiting,
                S::Open,
                S::Claimed,
                S::Done,
                S::Failed,
                S::Cancelled,
            ]),
        ),
        (
            "comment_effect_reports",
            names([
                ReportOutcome::Done,
                ReportOutcome::Failed,
                ReportOutcome::Released,
            ]),
        ),
        (
            "comment_carry_events",
            names([CarryKind::Carried, CarryKind::AskedAgain]),
        ),
        (
            "comment_carry_events",
            names([ReaskReason::BotEdited, ReaskReason::CodeChanged]),
        ),
        ("agent_subscriptions", names([C::Ntfy, C::Webhook])),
        (
            "agent_subscriptions",
            names([DeliveryOutcome::Delivered, DeliveryOutcome::Failed]),
        ),
        (
            "pr_watches",
            names([
                W::Active,
                W::WaitingForBotRound,
                W::JobRunning,
                W::LimitReached,
                W::Paused,
                W::Ended,
            ]),
        ),
        (
            "user_events",
            names([U::QuestionsReady, U::RoundExecuted, U::Test]),
        ),
        ("notification_channels", names([C::Ntfy, C::Webhook])),
        (
            "notification_channels",
            names([U::QuestionsReady, U::RoundExecuted, U::Test]),
        ),
        (
            "notification_channels",
            names([DeliveryOutcome::Delivered, DeliveryOutcome::Failed]),
        ),
    ];
    for (table, list) in cases {
        let sql: String =
            sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?")
                .bind(table)
                .fetch_one(&db.pool)
                .await
                .expect("table");
        let flat = sql.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(flat.contains(&list), "{table}: expected {list}");
    }
}

/// A pass keeps what it sent and the question it answers; a pass charged to nobody is allowed; live passes list.
#[tokio::test]
async fn pass_records_its_items_question_and_charged_user() {
    let db = db().await;
    let rl = db.review_loop();
    let item = new_item(&db, "dedupe-p").await;
    let sent = vec![SentItem {
        item: item.id,
        no: item.no,
        fingerprint: "fp".into(),
    }];
    let pass = NewPass {
        pull_request_id: PR,
        user_id: None,
        cause: PassCause::Read,
        head: head(),
        sent: sent.clone(),
        question: None,
    };
    let PassStart::Started(id) = rl.start_pass(T, &pass, at(0)).await.expect("start") else {
        panic!("expected a start");
    };
    let record = rl.pass(id).await.expect("pass");
    assert_eq!(
        (record.user_id, record.state, record.sent, record.job_id),
        (None, PassState::Queued, sent, None)
    );
    assert_eq!(rl.live_passes().await.expect("live").len(), 1);
    let job = JobId::new_random();
    rl.pass_running(id, job).await.expect("running");
    assert_eq!(rl.pass(id).await.expect("pass").job_id, Some(job));
    rl.finish_pass(id, true, at(5)).await.expect("finish");
    assert!(rl.live_passes().await.expect("live").is_empty());
    assert_eq!(rl.item(item.id).await.expect("item").id, item.id);
}

/// Without an analysis a comment-item question belongs to the pull request and the asker: listed for them only,
/// waiting for a question pass, counted per item once answered, never offered to the analysis Ask followers.
#[tokio::test]
async fn comment_item_question_without_an_analysis() {
    use crate::repo::questions::{Asked, NewPrQuestion, PrQuestionLimits, QuestionUpdate};
    use contracts::agent::ask::AskOutput;
    use contracts::api::questions::AskAnchor;
    use contracts::ids::QuestionId;
    let db = db().await;
    let item = new_item(&db, "dedupe-q").await;
    let limits = PrQuestionLimits {
        per_pull_request: 2,
        per_user: 10,
        user_window_start: at(0),
    };
    let question = |pr: i64, user: &str| NewPrQuestion {
        id: QuestionId::new_random(),
        pull_request_id: pr,
        item: item.id,
        parent_id: None,
        user_id: user.into(),
        text: "Is this still true?".into(),
    };
    assert!(matches!(
        db.questions()
            .create_for_pr(question(2, "u1"), limits, at(1))
            .await,
        Err(DbError::NotFound)
    ));
    let Asked::Stored(stored) = db
        .questions()
        .create_for_pr(question(PR, "u1"), limits, at(1))
        .await
        .expect("stored")
    else {
        panic!("expected a stored question");
    };
    assert_eq!(stored.analysis_id, None);
    assert_eq!(stored.pull_request_id, Some(PR));
    assert_eq!(
        stored.question.anchor,
        AskAnchor::CommentItem { item: item.id }
    );
    db.questions()
        .create_for_pr(question(PR, "u1"), limits, at(2))
        .await
        .expect("second");
    assert!(matches!(
        db.questions()
            .create_for_pr(question(PR, "u1"), limits, at(3))
            .await,
        Ok(Asked::AnalysisLimit)
    ));
    let questions = db.questions();
    assert_eq!(
        questions
            .list_for_pull_request(PR, "u1", at(4))
            .await
            .expect("list")
            .len(),
        2
    );
    assert!(
        questions
            .list_for_pull_request(PR, "u2", at(4))
            .await
            .expect("list")
            .is_empty()
    );
    assert!(
        questions
            .unanswered(at(4))
            .await
            .expect("unanswered")
            .is_empty()
    );
    let next = questions
        .next_unsent_for_pr(PR, at(4))
        .await
        .expect("next")
        .expect("one");
    assert_eq!(next.question.id, stored.question.id);
    assert_eq!(
        questions.pull_requests_with_unsent().await.expect("prs"),
        vec![PR]
    );
    questions
        .update(
            next.question.id,
            &QuestionUpdate::Answered(AskOutput {
                answer: "Yes.".into(),
                evidence: Vec::new(),
            }),
            at(5),
        )
        .await
        .expect("answered");
    assert_eq!(
        questions.answers_per_item(PR, "u1").await.expect("counts"),
        vec![(item.id, 1)]
    );
}

#[tokio::test]
async fn undo_cancels_unclaimed_effects_and_is_refused_once_one_is_claimed() {
    let db = db().await;
    let rl = db.review_loop();
    let item = new_item(&db, "dedupe-undo").await;
    let decision = decide_one(&db, fix_decision(&item), at(10)).await;
    let item = rl
        .item_by_no(PR, item.no)
        .await
        .expect("read")
        .expect("item");
    assert_eq!(
        rl.undo(T, item.id, item.revision + 1, "u1", at(11))
            .await
            .expect("undo"),
        ActionOutcome::ItemChanged
    );
    let ActionOutcome::Done(undone) = rl
        .undo(T, item.id, item.revision, "u1", at(11))
        .await
        .expect("undo")
    else {
        panic!("undo refused");
    };
    assert_eq!(undone.state, ItemState::Undecided);
    let stored = rl.item_decisions(item.id).await.expect("decisions");
    assert_eq!(stored.len(), 1, "decisions stay append-only");
    assert_eq!(stored[0].decision.undone_at, Some(at(11)));
    assert!(
        stored[0]
            .decision
            .effects
            .iter()
            .all(|effect| effect.state == EffectState::Cancelled)
    );
    assert_eq!(
        rl.undo(T, item.id, undone.revision, "u1", at(12))
            .await
            .expect("undo"),
        ActionOutcome::NoDecision,
        "an undone decision cannot be undone again"
    );
    assert!(
        rl.claimable(
            T,
            &[EffectKind::Fix, EffectKind::Reply, EffectKind::Resolve],
            None,
            at(12)
        )
        .await
        .expect("feed")
        .is_empty(),
        "an undone decision is not in the feed"
    );

    let again = decide_one(&db, fix_decision(&undone), at(13)).await;
    assert_eq!(again.supersedes, Some(decision.id));
    let token = AccessTokenId("agentAAA".into());
    let all = [EffectKind::Fix, EffectKind::Reply, EffectKind::Resolve];
    rl.claim(
        again.id,
        &token,
        &all,
        SignedDuration::from_mins(30),
        at(14),
    )
    .await
    .expect("claim");
    let item = rl
        .item_by_no(PR, item.no)
        .await
        .expect("read")
        .expect("item");
    assert_eq!(
        rl.undo(T, item.id, item.revision, "u1", at(15))
            .await
            .expect("undo"),
        ActionOutcome::EffectClaimed
    );
}

#[tokio::test]
async fn reopen_asks_again_and_skips_are_per_reader_and_revision() {
    let db = db().await;
    let rl = db.review_loop();
    let item = new_item(&db, "dedupe-reopen").await;
    assert_eq!(
        rl.reopen(item.id, item.revision, at(5))
            .await
            .expect("reopen"),
        ActionOutcome::NoDecision
    );
    decide_one(&db, fix_decision(&item), at(10)).await;
    let item = rl
        .item_by_no(PR, item.no)
        .await
        .expect("read")
        .expect("item");
    let ActionOutcome::Done(reopened) = rl
        .reopen(item.id, item.revision, at(11))
        .await
        .expect("reopen")
    else {
        panic!("reopen refused");
    };
    assert_eq!(reopened.state, ItemState::Reopened);
    let stored = rl.item_decisions(item.id).await.expect("decisions");
    assert!(
        stored[0]
            .decision
            .effects
            .iter()
            .all(|effect| effect.state == EffectState::Cancelled),
        "unclaimed effects of a reopened item are cancelled"
    );

    rl.skip(T, "u1", item.id, 3, at(12)).await.expect("skip");
    rl.skip(T, "u1", item.id, reopened.revision, at(13))
        .await
        .expect("skip again");
    assert_eq!(
        rl.skips("u1", PR).await.expect("skips"),
        vec![(item.id, reopened.revision)]
    );
    assert!(rl.skips("u2", PR).await.expect("skips").is_empty());
    assert!(rl.latest_pass(PR).await.expect("pass").is_none());
    let decision = rl.item_decisions(item.id).await.expect("decisions")[0]
        .decision
        .id;
    assert_eq!(rl.decision_user(decision).await.expect("user"), "u1");
}
