use contracts::api::access_tokens::AccessTokenId;
use contracts::api::dashboard::CommentTally;
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
         INSERT INTO tenant_allowed_logins (tenant_id, login, github_id, bound_at)
           VALUES ('default', 'alice', 1, 't'), ('default', 'bob', 2, 't');
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
        proposed_decision: None,
        evidence_text: None,
        recorded_decision: None,
        job: JobId::new_random(),
        head: head(),
        assessed_at: at(5),
        reused_since: None,
        validity: Validity::Valid,
        evidence: vec![],
        pre_existing: Some(false),
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
    rl.add_assessment(T, item.id, &assessment("second"), "fp2")
        .await
        .expect("second again");
    let latest = rl
        .latest_assessment(item.id)
        .await
        .expect("latest")
        .expect("some");
    assert_eq!(latest.fingerprint, "fp2");
    assert_eq!(latest.assessment.necessity.reason, "second");
    let revised = rl
        .item_by_no(PR, item.no)
        .await
        .expect("read")
        .expect("item");
    assert_eq!(
        revised.revision,
        item.revision + 2,
        "each new recommendation changes the item, the same one again does not"
    );
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
    assert_eq!(
        rl.report(T, effects[1].id, &a, &done("k3"), lease, later)
            .await
            .expect("report"),
        ReportOutcomeRecord::LeaseExpired,
        "after the feed task returned it, the former claimer is still told its lease ended"
    );
    assert_eq!(
        rl.report(T, effects[1].id, &b, &done("k3"), lease, later)
            .await
            .expect("report"),
        ReportOutcomeRecord::NotClaimer
    );

    // Another agent takes it, releases it, takes it again and finishes.
    assert!(matches!(
        rl.claim(decision.id, &b, &all, lease, later)
            .await
            .expect("claim"),
        ClaimOutcome::Claimed(_)
    ));
    assert_eq!(
        rl.report(T, effects[1].id, &a, &done("k4"), lease, later)
            .await
            .expect("report"),
        ReportOutcomeRecord::NotClaimer,
        "another agent's claim is not the lapsed one's"
    );
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
    for effect in [effects[1].id, effects[2].id] {
        assert_eq!(
            rl.report(T, effect, &b, &done("b-done"), lease, later)
                .await
                .expect("report"),
            ReportOutcomeRecord::Recorded,
            "an idempotency key is scoped per effect"
        );
    }
    assert_eq!(
        rl.report(T, fix, &a, &done("k5"), lease, later)
            .await
            .expect("report"),
        ReportOutcomeRecord::Ended
    );
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

/// Plan §2.3: `done` waits for the `after` effect; a failed effect cancels what waits for it once no agent holds it.
#[tokio::test]
async fn effects_after_a_failed_one_are_cancelled_and_done_waits_its_turn() {
    let db = db().await;
    let rl = db.review_loop();
    let item = new_item(&db, "dedupe-chain").await;
    let decision = decide_one(&db, fix_decision(&item), at(10)).await;
    let a = AccessTokenId("agentAAA".into());
    let lease = SignedDuration::from_mins(30);
    let all = [EffectKind::Fix, EffectKind::Reply, EffectKind::Resolve];
    rl.claim(decision.id, &a, &all, lease, at(11))
        .await
        .expect("claim");
    let report = |outcome, key: &str| ReportRequest {
        outcome,
        reference: Some("https://github.com/acme/node-client/pull/482#discussion_r1".into()),
        message: Some("the fix does not build".into()),
        idempotency_key: key.into(),
    };
    let [fix, reply, resolve] = [0, 1, 2].map(|i| decision.effects[i].id);
    assert_eq!(
        rl.report(
            T,
            reply,
            &a,
            &report(ReportOutcome::Done, "k1"),
            lease,
            at(12)
        )
        .await
        .expect("report"),
        ReportOutcomeRecord::Waiting
    );
    assert_eq!(
        rl.report(
            T,
            fix,
            &a,
            &report(ReportOutcome::Failed, "k2"),
            lease,
            at(13)
        )
        .await
        .expect("report"),
        ReportOutcomeRecord::Recorded
    );
    assert_eq!(
        rl.report(
            T,
            resolve,
            &a,
            &report(ReportOutcome::Done, "k3"),
            lease,
            at(14)
        )
        .await
        .expect("report"),
        ReportOutcomeRecord::Waiting,
        "what waits on a failed fix is never done"
    );
    assert_eq!(
        rl.report(
            T,
            reply,
            &a,
            &report(ReportOutcome::Released, "k4"),
            lease,
            at(14)
        )
        .await
        .expect("report"),
        ReportOutcomeRecord::Recorded
    );
    let effects = rl.item_decisions(item.id).await.expect("decisions")[0]
        .decision
        .effects
        .clone();
    assert_eq!(
        effects
            .iter()
            .map(|effect| effect.state)
            .collect::<Vec<_>>(),
        [
            EffectState::Failed,
            EffectState::Cancelled,
            EffectState::Cancelled
        ]
    );
    assert!(effects[1].claim.is_none() && effects[2].claim.is_none());
    assert!(
        rl.claimable(T, &all, None, at(14))
            .await
            .expect("feed")
            .is_empty()
    );
}

/// RQ3: an item the owner reopened stays asked while an agent claims, reports on or lets lapse the earlier
/// decision, and what the agent had not claimed is never offered again.
#[tokio::test]
async fn a_reopened_item_stays_asked_through_reports_and_lease_ends() {
    let db = db().await;
    let rl = db.review_loop();
    let item = new_item(&db, "dedupe-reask").await;
    let decision = decide_one(&db, fix_decision(&item), at(10)).await;
    let a = AccessTokenId("agentAAA".into());
    let lease = SignedDuration::from_mins(30);
    rl.claim(
        decision.id,
        &a,
        &[EffectKind::Fix, EffectKind::Reply, EffectKind::Resolve],
        lease,
        at(11),
    )
    .await
    .expect("claim");
    let item = rl
        .item_by_no(PR, item.no)
        .await
        .expect("read")
        .expect("item");
    assert_eq!(item.state, ItemState::Executing);
    let ActionOutcome::Done(reopened) = rl
        .reopen(item.id, item.revision, at(12))
        .await
        .expect("reopen")
    else {
        panic!("reopen refused");
    };
    assert_eq!(reopened.state, ItemState::Reopened);
    let done = ReportRequest {
        outcome: ReportOutcome::Done,
        reference: Some("0123456789abcdef".into()),
        message: None,
        idempotency_key: "k1".into(),
    };
    assert_eq!(
        rl.report(T, decision.effects[0].id, &a, &done, lease, at(13))
            .await
            .expect("report"),
        ReportOutcomeRecord::Recorded
    );
    let later = at(13) + lease + SignedDuration::from_secs(1);
    rl.expire_leases(later).await.expect("expire");
    let item = rl
        .item_by_no(PR, item.no)
        .await
        .expect("read")
        .expect("item");
    assert_eq!(item.state, ItemState::Reopened, "the owner is still asked");
    let effects = rl.item_decisions(item.id).await.expect("decisions")[0]
        .decision
        .effects
        .clone();
    assert_eq!(
        effects
            .iter()
            .map(|effect| effect.state)
            .collect::<Vec<_>>(),
        [
            EffectState::Done,
            EffectState::Cancelled,
            EffectState::Cancelled
        ],
        "what the lapsed claim held is not offered again"
    );
}

/// Plan §2.1: an item a complete read no longer produces is marked gone once (its row stays) and is no longer gone
/// when a read produces it again; PR-level questions are never gone.
#[tokio::test]
async fn items_no_longer_read_are_marked_gone_and_kept() {
    let db = db().await;
    let rl = db.review_loop();
    let kept = new_item(&db, "dedupe-kept").await;
    let left = new_item(&db, "dedupe-left").await;
    let mut question = observed("pr:3:merge", "Merge now?");
    question.kind = CommentItemKind::PrQuestion;
    rl.upsert_item(T, PR, &question, ItemState::Undecided, at(1))
        .await
        .expect("question");
    assert_eq!(
        rl.mark_gone(PR, &["dedupe-kept"], at(5))
            .await
            .expect("gone"),
        [left.no]
    );
    let gone = rl
        .item_by_no(PR, left.no)
        .await
        .expect("read")
        .expect("kept row");
    assert_eq!(
        (gone.gone_at, gone.revision, gone.state),
        (Some(at(5)), left.revision + 1, ItemState::Undecided)
    );
    assert!(
        rl.mark_gone(PR, &["dedupe-kept"], at(6))
            .await
            .expect("again")
            .is_empty(),
        "gone once"
    );
    let still = rl
        .item_by_no(PR, kept.no)
        .await
        .expect("read")
        .expect("item");
    assert_eq!((still.gone_at, still.revision), (None, kept.revision));
    let UpsertOutcome::Changed(back) = rl
        .upsert_item(
            T,
            PR,
            &observed("dedupe-left", "body"),
            ItemState::Undecided,
            at(7),
        )
        .await
        .expect("back")
    else {
        panic!("a returning item changes");
    };
    assert_eq!((back.gone_at, back.revision), (None, gone.revision + 1));
}

/// My work's comment counts (R1b `WorkItem.comments`): read PRs only, no PR-level questions or gone items; waiting
/// as the queue counts it, without items a standing "merge now" decision hands to its follow-up PR.
#[tokio::test]
async fn comment_tallies_count_comments_and_the_waiting_ones_per_read_pull_request_and_name_watches()
 {
    let db = db().await;
    let rl = db.review_loop();
    let waiting = new_item(&db, "dedupe-waiting").await;
    let decided = new_item(&db, "dedupe-decided").await;
    let gone = new_item(&db, "dedupe-gone").await;
    let followed = new_item(&db, "dedupe-followed").await;
    decide_one(&db, fix_decision(&decided), at(2)).await;
    rl.mark_gone(
        PR,
        &["dedupe-waiting", "dedupe-decided", "dedupe-followed"],
        at(3),
    )
    .await
    .expect("gone");
    let mut question = observed("pr:3:merge", "Merge now?");
    question.kind = CommentItemKind::PrQuestion;
    let UpsertOutcome::New(question) = rl
        .upsert_item(T, PR, &question, ItemState::Undecided, at(4))
        .await
        .expect("question")
    else {
        panic!("a new question");
    };
    let merge = decide_one(
        &db,
        NewDecision {
            kind: DecisionKind::MergeWithFollowUps,
            reply_text: None,
            comment: None,
            effects: vec![NewEffect {
                seq: 1,
                after: None,
                kind: EffectKind::FollowUpPr,
                executor: Executor::Agent,
                params: EffectParams {
                    item_nos: vec![followed.no],
                    ..EffectParams::default()
                },
            }],
            ..fix_decision(&question)
        },
        at(5),
    )
    .await;
    let acme = |owner: &str, name: &str, number| {
        (
            RepoRef {
                owner: owner.into(),
                name: name.into(),
            },
            number,
        )
    };
    let wanted = [
        acme("acme", "node-client", 501),
        acme("ACME", "Node-Client", 482),
        acme("acme", "other", 482),
        acme("acme", "node-client", 490),
    ];
    assert!(
        rl.comment_tallies(T, "u1", &wanted)
            .await
            .expect("tallies")
            .is_empty(),
        "no PR read successfully or watched yet"
    );
    db.review_loop()
        .ensure_pr(T, 2, at(6))
        .await
        .expect("pr 490");
    rl.watch(T, "u1", 2, 20, at(6)).await.expect("watch 490");
    rl.record_read(
        PR,
        &ReadResult::Read {
            snapshot: serde_json::json!({}),
            round_count: 3,
        },
        at(6),
    )
    .await
    .expect("read");
    let tallies = rl.comment_tallies(T, "u1", &wanted).await.expect("tallies");
    assert_eq!(
        tallies,
        [
            PrCommentTally {
                index: 1,
                counts: Some(CommentTally {
                    total: 3,
                    needs_you: 1,
                }),
                watched: false,
            },
            // Watched, its comments never read (the watch's first read failed).
            PrCommentTally {
                index: 3,
                counts: None,
                watched: true,
            },
        ],
        "C{} waits; C{} is decided, C{} gone, C{} followed up",
        waiting.no,
        decided.no,
        gone.no,
        followed.no
    );
    let question = rl
        .item_by_no(PR, question.no)
        .await
        .expect("read")
        .expect("question");
    rl.undo(T, question.id, question.revision, "u1", at(7))
        .await
        .expect("undo");
    assert_eq!(
        rl.comment_tallies(T, "u1", &wanted).await.expect("tallies")[0]
            .counts
            .map(|counts| counts.needs_you),
        Some(2),
        "an undone merge decision {} hands nothing over",
        merge.id
    );
    assert_eq!(
        rl.comment_tallies(T, "u2", &wanted)
            .await
            .expect("tallies")
            .iter()
            .map(|tally| (tally.index, tally.watched))
            .collect::<Vec<_>>(),
        [(1, false)],
        "another user's watch is not this user's"
    );
    rl.unwatch("u1", 2).await.expect("unwatch");
    assert_eq!(
        rl.comment_tallies(T, "u1", &wanted)
            .await
            .expect("tallies")
            .len(),
        1,
        "an unwatched PR without a read is not listed"
    );
    assert!(
        rl.comment_tallies("other", "u1", &wanted)
            .await
            .expect("tallies")
            .is_empty(),
        "another tenant reads nothing"
    );
}

/// Plan §2.4: an answer landing on an undecided item asks it back; other states keep theirs.
#[tokio::test]
async fn an_answer_asks_an_undecided_item_back() {
    let db = db().await;
    let rl = db.review_loop();
    let item = new_item(&db, "dedupe-asked").await;
    assert!(rl.asked_back(item.id, at(5)).await.expect("asked back"));
    let asked = rl
        .item_by_no(PR, item.no)
        .await
        .expect("read")
        .expect("item");
    assert_eq!(asked.state, ItemState::AskedBack);
    assert_eq!(asked.revision, item.revision + 1);
    assert!(!rl.asked_back(item.id, at(6)).await.expect("again"));
}

#[tokio::test]
async fn carry_events_are_logged_and_counted() {
    let db = db().await;
    let rl = db.review_loop();
    let source = new_item(&db, "dedupe-a").await;
    let repeat = new_item(&db, "dedupe-b").await;
    let decision = decide_one(&db, fix_decision(&source), at(10)).await;
    let evidence = CarryEvidence {
        quote: None,
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
    let pending = rl
        .carry_events(T, Some(PR), None, 10)
        .await
        .expect("events");
    assert_eq!(pending[0].effects_pending, 3);
    let asked = NewCarryEvent {
        item: source.id,
        source_decision: decision.id,
        kind: CarryKind::AskedAgain,
        reason: Some(ReaskReason::BotEdited),
        evidence: CarryEvidence {
            quote: None,
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
    assert_eq!(
        events[1].effects_pending, 0,
        "asking the source again cancelled its decision's unclaimed effects"
    );
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
            effects_pending: 0,
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
        .put_channel(
            T,
            "u1",
            ChannelKind::Ntfy,
            &input(SecretUpdate::Keep {
                seen: Some((ChannelKind::Ntfy, "https://ntfy.example".into())),
            }),
        )
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

/// A kept secret stays only on the target the caller checked the request against: a secret stored or moved
/// meanwhile makes the save a conflict, and nothing changes.
#[tokio::test]
async fn a_kept_secret_stays_only_on_the_target_it_was_checked_against() {
    let db = db().await;
    let rl = db.review_loop();
    let input = |url: &str, secret| ChannelInput {
        url: url.into(),
        topic: Some("diffpilot-alice".into()),
        secret,
        events: vec![UserEventKind::QuestionsReady],
        enabled: true,
    };
    let good = "https://ntfy.example";
    rl.put_channel(
        T,
        "u1",
        ChannelKind::Ntfy,
        &input(good, SecretUpdate::Set(vec![7, 7, 7])),
    )
    .await
    .expect("create");
    for seen in [
        None,
        Some((ChannelKind::Ntfy, "https://stale.example".to_owned())),
    ] {
        let saved = rl
            .put_channel(
                T,
                "u1",
                ChannelKind::Ntfy,
                &input("https://elsewhere.example", SecretUpdate::Keep { seen }),
            )
            .await;
        assert!(matches!(saved, Err(DbError::Conflict)), "{saved:?}");
    }
    let channel = rl.channels("u1").await.expect("channels").remove(0);
    assert_eq!(
        (channel.url.as_str(), channel.secret),
        (good, Some(vec![7, 7, 7]))
    );
    let kept = rl
        .put_channel(
            T,
            "u1",
            ChannelKind::Ntfy,
            &input(
                "https://ntfy.example/other",
                SecretUpdate::Keep {
                    seen: Some((ChannelKind::Ntfy, good.into())),
                },
            ),
        )
        .await
        .expect("checked target");
    assert_eq!(kept.secret, Some(vec![7, 7, 7]));

    let token = AccessTokenId("agentAAA".into());
    let push = |kind, url: &str, secret| PushInput {
        kind,
        url: url.into(),
        topic: (kind == ChannelKind::Ntfy).then(|| "agent-topic".to_owned()),
        secret,
    };
    rl.put_push_target(
        T,
        &token,
        &push(
            ChannelKind::Webhook,
            "https://hooks.example/a",
            SecretUpdate::Set(vec![1, 2, 3]),
        ),
        at(1),
    )
    .await
    .expect("push target");
    for seen in [
        None,
        Some((ChannelKind::Ntfy, "https://hooks.example/a".to_owned())),
    ] {
        let saved = rl
            .put_push_target(
                T,
                &token,
                &push(
                    ChannelKind::Webhook,
                    "https://attacker.example/",
                    SecretUpdate::Keep { seen },
                ),
                at(2),
            )
            .await;
        assert!(matches!(saved, Err(DbError::Conflict)), "{saved:?}");
    }
    let kept = rl
        .put_push_target(
            T,
            &token,
            &push(
                ChannelKind::Webhook,
                "https://hooks.example/b",
                SecretUpdate::Keep {
                    seen: Some((ChannelKind::Webhook, "https://hooks.example/a".into())),
                },
            ),
            at(3),
        )
        .await
        .expect("checked push target");
    assert_eq!(kept.push_secret, Some(vec![1, 2, 3]));
    assert_eq!(
        kept.push.map(|(_, url, _)| url).as_deref(),
        Some("https://hooks.example/b")
    );
}

/// Questions ready for a round are written once per watcher: the marker, the event and its deliveries together.
#[tokio::test]
async fn questions_ready_is_written_once_per_user_pull_request_and_round() {
    let db = db().await;
    let rl = db.review_loop();
    rl.watch(T, "u1", PR, 20, at(0)).await.expect("watch");
    rl.put_channel(
        T,
        "u1",
        ChannelKind::Ntfy,
        &ChannelInput {
            url: "https://ntfy.example".into(),
            topic: Some("t".into()),
            secret: SecretUpdate::Keep { seen: None },
            events: vec![UserEventKind::QuestionsReady],
            enabled: true,
        },
    )
    .await
    .expect("channel");
    let event = NewUserEvent {
        user_id: "u1".into(),
        kind: UserEventKind::QuestionsReady,
        pull_request_id: Some(PR),
        count: 2,
        text: "acme/node-client #482: 2 questions ready".into(),
        url: Some("/pr/acme/node-client/482/comments".into()),
    };
    let written = [
        rl.questions_ready(T, &event, "round:3", at(1))
            .await
            .expect("first"),
        rl.questions_ready(T, &event, "round:3", at(2))
            .await
            .expect("again"),
        rl.questions_ready(T, &event, "round:4", at(3))
            .await
            .expect("next round"),
    ];
    assert_eq!(written, [true, false, true]);
    assert_eq!(
        rl.user_events("u1", None, 10).await.expect("events").len(),
        2
    );
    assert_eq!(rl.due_deliveries(at(3), 10).await.expect("due").len(), 2);
    assert_eq!(
        rl.watches("u1").await.expect("watches")[0]
            .notified_for
            .as_deref(),
        Some("round:4")
    );
    assert!(
        !rl.questions_ready(
            T,
            &NewUserEvent {
                user_id: "u2".into(),
                ..event
            },
            "round:3",
            at(4)
        )
        .await
        .expect("not watching"),
        "only a watcher is told"
    );
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

#[tokio::test]
async fn open_feed_tracks_claimable_decisions_with_their_kinds_and_owners() {
    let db = db().await;
    let rl = db.review_loop();
    let item = new_item(&db, "dedupe-a").await;
    let decision = decide_one(&db, fix_decision(&item), at(10)).await;
    let all = [EffectKind::Fix, EffectKind::Reply, EffectKind::Resolve];
    let open = rl.open_feed(at(11)).await.expect("open");
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].decision, decision.id);
    assert_eq!(open[0].tenant_id, T);
    assert_eq!(open[0].pr.number, 482);
    assert_eq!(open[0].kinds, all);

    let lease = SignedDuration::from_mins(30);
    let a = AccessTokenId("agentAAA".into());
    rl.claim(decision.id, &a, &all, lease, at(12))
        .await
        .expect("claim");
    assert!(rl.open_feed(at(13)).await.expect("claimed").is_empty());
    assert_eq!(
        rl.open_feed(at(12) + lease + SignedDuration::from_secs(1))
            .await
            .expect("expired")
            .len(),
        1,
        "a lapsed lease is claimable again"
    );

    let owner = rl
        .decision_owner(decision.id)
        .await
        .expect("owner")
        .expect("known");
    assert_eq!((owner.tenant_id.as_str(), owner.pull_request_id), (T, PR));
    let by_effect = rl
        .effect_owner(decision.effects[1].id)
        .await
        .expect("owner")
        .expect("known");
    assert_eq!(by_effect, owner);
    assert_eq!(
        rl.decision_owner(contracts::ids::DecisionId::new_random())
            .await
            .expect("unknown"),
        None
    );
}

#[tokio::test]
async fn event_deliveries_are_queued_with_the_event_per_subscribed_channel() {
    let db = db().await;
    let rl = db.review_loop();
    let channel = |events: Vec<UserEventKind>, enabled| ChannelInput {
        url: "https://ntfy.example".into(),
        topic: Some("t".into()),
        secret: SecretUpdate::Keep { seen: None },
        events,
        enabled,
    };
    let ntfy = rl
        .put_channel(
            T,
            "u1",
            ChannelKind::Ntfy,
            &channel(vec![UserEventKind::QuestionsReady], true),
        )
        .await
        .expect("ntfy");
    rl.put_channel(
        T,
        "u1",
        ChannelKind::Webhook,
        &channel(vec![UserEventKind::RoundExecuted], true),
    )
    .await
    .expect("webhook");
    rl.put_channel(
        T,
        "u2",
        ChannelKind::Ntfy,
        &channel(vec![UserEventKind::QuestionsReady], false),
    )
    .await
    .expect("off");
    let event = |user: &str, kind, pull_request_id| NewUserEvent {
        user_id: user.into(),
        kind,
        pull_request_id,
        count: 5,
        text: "acme/node-client #482: 5 questions ready".into(),
        url: Some("https://review.example/pr/acme/node-client/482/comments".into()),
    };
    let ready = rl
        .add_user_event(
            T,
            &event("u1", UserEventKind::QuestionsReady, Some(PR)),
            at(5),
        )
        .await
        .expect("ready");
    rl.add_user_event(T, &event("u1", UserEventKind::Test, None), at(6))
        .await
        .expect("test");
    rl.add_user_event(
        T,
        &event("u2", UserEventKind::QuestionsReady, Some(PR)),
        at(7),
    )
    .await
    .expect("other user");
    // Due at once and however late the delivery task looks (no age cut-off); only u1's ntfy channel takes
    // questions_ready, tests and disabled channels are left out.
    let due = rl
        .due_deliveries(at(5) + SignedDuration::from_hours(3), 10)
        .await
        .expect("due");
    assert_eq!(
        due.iter().map(|d| d.target.clone()).collect::<Vec<_>>(),
        [DeliveryTarget::Channel {
            event: ready,
            channel: ntfy.id
        }]
    );
    // A channel set up later does not receive earlier events.
    rl.put_channel(
        T,
        "u2",
        ChannelKind::Webhook,
        &channel(vec![UserEventKind::QuestionsReady], true),
    )
    .await
    .expect("later channel");
    assert_eq!(rl.due_deliveries(at(8), 10).await.expect("due").len(), 1);
    rl.postpone_delivery(due[0].id, at(100))
        .await
        .expect("postpone");
    assert!(
        rl.due_deliveries(at(99), 10)
            .await
            .expect("later")
            .is_empty()
    );
    assert_eq!(
        rl.due_deliveries(at(100), 10).await.expect("due")[0].attempts,
        0
    );

    let read = rl.user_event(ready).await.expect("read").expect("known");
    assert_eq!(read.kind, UserEventKind::QuestionsReady);
    assert_eq!(read.pr.map(|pr| pr.number), Some(482));
    assert_eq!(
        rl.channel(ntfy.id).await.expect("channel").map(|c| c.kind),
        Some(ChannelKind::Ntfy)
    );
    assert_eq!(rl.rounds_through(PR).await.expect("none"), None);

    let token = AccessTokenId("agentAAA".into());
    rl.subscribe(T, &token, &[EffectKind::Reply], at(1))
        .await
        .expect("subscribe");
    assert!(
        rl.subscription(&token, at(2))
            .await
            .expect("subscription")
            .is_some()
    );
    sqlx::query("UPDATE access_tokens SET revoked_at = 't' WHERE id = 'agentAAA'")
        .execute(&db.pool)
        .await
        .expect("revoke");
    assert!(
        rl.subscription(&token, at(3))
            .await
            .expect("revoked")
            .is_none()
    );
}

#[tokio::test]
async fn subscriptions_of_users_off_the_allow_list_are_absent() {
    let db = db().await;
    let rl = db.review_loop();
    let token = AccessTokenId("agentAAA".into());
    rl.subscribe(T, &token, &[EffectKind::Reply], at(1))
        .await
        .expect("subscribe");
    assert_eq!(rl.subscriptions(T, at(2)).await.expect("listed").len(), 1);
    assert_eq!(
        rl.subscriptions(T, at(2)).await.expect("listed")[0].user_id,
        "u1"
    );
    crate::repo::tenants::testing::remove_login(&db, T, "alice")
        .await
        .expect("remove");
    assert!(rl.subscriptions(T, at(3)).await.expect("listed").is_empty());
    assert!(
        rl.subscription(&token, at(3))
            .await
            .expect("read")
            .is_none(),
        "no push goes to a removed user's target"
    );
}

#[tokio::test]
async fn a_round_is_announced_once_per_user() {
    let db = db().await;
    let rl = db.review_loop();
    let item = new_item(&db, "dedupe-a").await;
    let decision = decide_one(&db, fix_decision(&item), at(10)).await;
    assert!(rl.unannounced_rounds().await.expect("none").is_empty());
    let a = AccessTokenId("agentAAA".into());
    let all = [EffectKind::Fix, EffectKind::Reply, EffectKind::Resolve];
    let lease = SignedDuration::from_mins(30);
    rl.claim(decision.id, &a, &all, lease, at(11))
        .await
        .expect("claim");
    let done = ReportRequest {
        outcome: ReportOutcome::Done,
        reference: Some("9f3c2e1a0b".into()),
        message: None,
        idempotency_key: "k1".into(),
    };
    rl.report(T, decision.effects[0].id, &a, &done, lease, at(12))
        .await
        .expect("report");
    assert_eq!(rl.unannounced_rounds().await.expect("one"), [PR]);
    let event = |user: &str| NewUserEvent {
        user_id: user.into(),
        kind: UserEventKind::RoundExecuted,
        pull_request_id: Some(PR),
        count: 1,
        text: "acme/node-client #482: 1 decision executed".into(),
        url: Some("/pr/acme/node-client/482/comments".into()),
    };
    assert_eq!(
        rl.add_round_events(T, &[event("u1")], at(12), at(13))
            .await
            .expect("stored"),
        1
    );
    assert_eq!(
        rl.add_round_events(T, &[event("u1"), event("u2")], at(12), at(14))
            .await
            .expect("again"),
        1,
        "u1 was told about this round already"
    );
    assert_eq!(rl.rounds_through(PR).await.expect("through"), Some(at(12)));
    assert!(rl.unannounced_rounds().await.expect("none").is_empty());
    assert_eq!(
        rl.user_events("u1", None, 10).await.expect("events").len(),
        1
    );
}

#[tokio::test]
async fn send_slots_are_limited_per_target_within_the_window() {
    let db = db().await;
    let rl = db.review_loop();
    let hour = SignedDuration::from_hours(1);
    for second in 0..3 {
        assert_eq!(
            rl.take_send_slot(T, "channel:u1/ntfy", 3, hour, at(second))
                .await
                .expect("slot"),
            Ok(())
        );
    }
    assert_eq!(
        rl.take_send_slot(T, "channel:u1/ntfy", 3, hour, at(60))
            .await
            .expect("slot"),
        Err(at(0) + hour)
    );
    assert_eq!(
        rl.take_send_slot(T, "push:agentAAA", 3, hour, at(60))
            .await
            .expect("other target"),
        Ok(())
    );
    assert_eq!(
        rl.take_send_slot(T, "channel:u1/ntfy", 3, hour, at(0) + hour)
            .await
            .expect("slot"),
        Ok(()),
        "the first message left the window"
    );
}

/// RQ4, RQ5, RQ11: decisions are planned on the state of the transaction that stores them; a plan that refuses
/// stores nothing.
#[tokio::test]
async fn decisions_are_planned_on_the_state_they_are_stored_with() {
    let db = db().await;
    let rl = db.review_loop();
    let item = new_item(&db, "planned").await;
    let stored = rl
        .decide_planned(T, PR, at(5), |view| {
            assert_eq!(view.row.map(|row| row.pull_request_id), Some(PR));
            assert_eq!(
                view.items.iter().map(|item| item.no).collect::<Vec<_>>(),
                [item.no]
            );
            assert!(view.decisions.is_empty());
            Ok::<_, DbError>(vec![fix_decision(&view.items[0])])
        })
        .await
        .expect("decide");
    assert!(
        matches!(&stored, DecideOutcome::Stored(decisions) if decisions.len() == 1),
        "{stored:?}"
    );
    let refused = rl
        .decide_planned(T, PR, at(6), |view| {
            assert_eq!(view.decisions.len(), 1, "the decision just stored");
            Err::<Vec<NewDecision>, _>(DbError::Conflict)
        })
        .await;
    assert!(matches!(refused, Err(DbError::Conflict)), "{refused:?}");
    assert_eq!(
        rl.item_decisions(item.id).await.expect("decisions").len(),
        1
    );
}

/// RQ3: a decided item raised again is stored as read and asked again in one transaction: the owner's question
/// and the cancellation of the unclaimed earlier effects never wait for a second write.
#[tokio::test]
async fn an_item_raised_again_is_stored_and_asked_again_together() {
    let db = db().await;
    let rl = db.review_loop();
    let item = new_item(&db, "objection").await;
    let decision = decide_one(&db, fix_decision(&item), at(10)).await;
    let mut resolved = observed("objection", "body");
    resolved.github.resolved = Some(true);
    resolved.outside = Some(OutsideResolution {
        by: "alice".into(),
        seen_at: at(11),
    });
    rl.upsert_item(T, PR, &resolved, ItemState::Undecided, at(11))
        .await
        .expect("outside");
    let UpsertOutcome::Changed(asked) = rl
        .upsert_asking_again(
            T,
            PR,
            &observed("objection", "body"),
            ItemState::Executing,
            at(12),
        )
        .await
        .expect("asked again")
    else {
        panic!("expected a change");
    };
    assert_eq!(asked.state, ItemState::Reopened);
    assert_eq!(asked.outside, None);
    assert_eq!(asked.github.resolved, Some(false));
    assert_eq!(
        effect_states(&db, &decision).await,
        [EffectState::Cancelled; 3]
    );
}

/// Plan §2.3, RQ3: a claim whose lease ended is settled before the decision is claimed again: what waits on a
/// failed effect is cancelled and a reopened item's work is not handed out, even before the expiry sweep ran.
#[tokio::test]
async fn an_ended_lease_is_settled_before_the_decision_is_claimed_again() {
    let db = db().await;
    let rl = db.review_loop();
    let (a, b) = (
        AccessTokenId("agentAAA".into()),
        AccessTokenId("agentBBB".into()),
    );
    let lease = SignedDuration::from_mins(30);
    let all = [EffectKind::Fix, EffectKind::Reply, EffectKind::Resolve];

    let item = new_item(&db, "lapsed-after-failure").await;
    let failed = decide_one(&db, fix_decision(&item), at(10)).await;
    rl.claim(failed.id, &a, &all, lease, at(11))
        .await
        .expect("claim");
    let failure = ReportRequest {
        outcome: ReportOutcome::Failed,
        reference: None,
        message: Some("the fix does not build".into()),
        idempotency_key: "k1".into(),
    };
    rl.report(T, failed.effects[0].id, &a, &failure, lease, at(12))
        .await
        .expect("report");
    let ended = at(12) + lease + SignedDuration::from_secs(1);
    assert_eq!(
        rl.claim(failed.id, &b, &all, lease, ended)
            .await
            .expect("claim"),
        ClaimOutcome::NothingOpen,
        "a reply and resolve after a failed fix are never handed out"
    );
    assert_eq!(
        effect_states(&db, &failed).await,
        [
            EffectState::Failed,
            EffectState::Cancelled,
            EffectState::Cancelled
        ]
    );

    let item = new_item(&db, "lapsed-after-reopen").await;
    let reopened = decide_one(&db, fix_decision(&item), at(10)).await;
    rl.claim(reopened.id, &a, &all, lease, at(11))
        .await
        .expect("claim");
    let item = rl
        .item_by_no(PR, item.no)
        .await
        .expect("read")
        .expect("item");
    rl.reopen(item.id, item.revision, at(12))
        .await
        .expect("reopen");
    let ended = at(11) + lease + SignedDuration::from_secs(1);
    assert_eq!(
        rl.claim(reopened.id, &a, &all, lease, ended)
            .await
            .expect("claim"),
        ClaimOutcome::NothingOpen,
        "the owner is reconsidering the item"
    );
    assert_eq!(
        effect_states(&db, &reopened).await,
        [EffectState::Cancelled; 3]
    );

    let item = new_item(&db, "lapsed-healthy").await;
    let healthy = decide_one(&db, fix_decision(&item), at(10)).await;
    rl.claim(healthy.id, &a, &all, lease, at(11))
        .await
        .expect("claim");
    let ended = at(11) + lease + SignedDuration::from_secs(1);
    assert_eq!(
        rl.claim(healthy.id, &b, &all, lease, ended)
            .await
            .expect("claim"),
        ClaimOutcome::Claimed(ended + lease),
        "another agent takes over work whose lease ended"
    );
}

/// The states of the decision's effects, in order.
async fn effect_states(
    db: &Db,
    decision: &contracts::api::review_loop::Decision,
) -> Vec<EffectState> {
    db.review_loop()
        .feed_decision(decision.id)
        .await
        .expect("fetch")
        .expect("some")
        .decision
        .effects
        .iter()
        .map(|effect| effect.state)
        .collect()
}
