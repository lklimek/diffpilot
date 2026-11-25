use std::sync::Mutex;

use axum::{Json, Router, extract::State, routing::post};
use db::repo::github::{ChangeKind, Effect, NewDelivery, Repository, WebhookChange};
use tokio::time::Instant;

use super::*;
use crate::auth::{lock_user, store_sign_in_tokens};
use crate::github::oauth::TokenGrant;
use crate::webhooks::dispatch::Dispatcher;

#[derive(Clone)]
struct Github {
    queries: Arc<Mutex<Vec<String>>>,
    reviewer: &'static str,
    cost: u32,
    remaining: u32,
}

async fn answer(State(github): State<Github>, Json(body): Json<Value>) -> Json<Value> {
    let document = body["query"].as_str().unwrap();
    github.queries.lock().unwrap().push(document.to_owned());
    if !document.contains("a0: repository") {
        return Json(json!({"errors": [{"type": "NOT_FOUND", "message": "no review fixture"}]}));
    }
    let mut data = json!({"rateLimit": {
        "cost": github.cost, "remaining": github.remaining, "resetAt": "2099-01-01T00:00:00Z"
    }});
    let fixtures: Vec<Value> = serde_json::from_str(include_str!(
        "../../../../../../fixtures/dashboard/github/pull-requests.json"
    ))
    .unwrap();
    for (i, field) in document.split("repository(owner:").skip(1).enumerate() {
        let (lookup, rest) = field.split_once("(number: ").unwrap();
        let number = rest.split_once(')').unwrap().0.parse::<u32>().unwrap();
        let value = if lookup.ends_with("pullRequest") {
            if document.contains("reviewThreads { totalCount }") {
                data[format!("a{i}")] = json!({"pullRequest": {"state": "OPEN", "headRefOid": "a".repeat(40),
                    "updatedAt": "2026-11-18T08:55:00Z", "reviewThreads": {"totalCount": 0},
                    "comments": {"totalCount": 0}, "reviews": {"nodes": []}}});
                continue;
            }
            let mut pr = fixtures[0].clone();
            pr["number"] = json!(number);
            pr["baseRepository"] = json!({"nameWithOwner": "acme/wallet-ui", "isPrivate": false,
                "defaultBranchRef": {"name": "main"}, "viewerPermission": "WRITE"});
            pr["isDraft"] = json!(false);
            pr["isCrossRepository"] = json!(false);
            for field in ["assignees", "reviews", "comments", "reviewThreads"] {
                pr[field] = json!({"nodes": []});
            }
            pr["title"] = json!(format!("Changed pull request {number}"));
            pr["url"] = json!(format!("https://github.com/acme/wallet-ui/pull/{number}"));
            pr["author"] = json!({"__typename": "User", "login": "mira-k"});
            pr["reviewRequests"] = json!({"totalCount": 1, "nodes": [
                {"requestedReviewer": {"__typename": "User", "login": github.reviewer}}
            ]});
            json!({"pullRequest": pr})
        } else {
            json!({"issue": {"state": "OPEN", "title": "Changed issue", "body": "new body",
                "author": {"login":"bob"}, "createdAt": "2026-11-01T00:00:00Z",
                "updatedAt": "2026-11-18T08:55:00Z", "labels": {"nodes": [{"name":"bug"}]},
                "assignees": {"nodes": [{"login":"alice"}]}, "comments": {"totalCount": 7},
                "milestone": null, "repository": {"isPrivate": true} }})
        };
        data[format!("a{i}")] = value;
    }
    Json(json!({"data": data}))
}

async fn setup(reviewer: &'static str, cost: u32, remaining: u32) -> (Harness, Github) {
    let github = Github {
        queries: Arc::default(),
        reviewer,
        cost,
        remaining,
    };
    let app = Router::new()
        .route("/graphql", post(answer))
        .with_state(github.clone());
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", socket.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(socket, app).await.unwrap() });
    let h = harness_with_url(&url).await;
    h.db()
        .dashboard()
        .save_settings(
            DEFAULT_TENANT,
            "u1",
            &DashboardSettings {
                enumerate_owners: vec!["acme".into()],
                ..DashboardSettings::default()
            },
            now(),
        )
        .await
        .unwrap();
    for user in ["u1", "u2"] {
        let lock = lock_user(user).await;
        store_sign_in_tokens(
            &h.state,
            &lock,
            DEFAULT_TENANT,
            &TokenGrant {
                access_token: format!("fake-{user}").into(),
                expires_in: 3600,
                refresh_token: format!("fake-refresh-{user}").into(),
                refresh_token_expires_in: 86400,
            },
            Timestamp::now(),
        )
        .await
        .unwrap();
    }
    (h, github)
}

async fn seed(h: &Harness, user: &str, snapshot: &Snapshot) -> RefreshId {
    let id = h.stored(user, snapshot).await;
    let updated = h
        .db()
        .dashboard()
        .finish_ranking(
            DEFAULT_TENANT,
            user,
            id,
            0,
            RankingState::Finished,
            JudgeCounts::default(),
            now(),
        )
        .await
        .unwrap();
    assert_eq!(updated, RankingUpdate::Applied);
    id
}

async fn read(h: &Harness, user: &str) -> Snapshot {
    h.db()
        .dashboard()
        .latest_snapshot(DEFAULT_TENANT, user)
        .await
        .unwrap()
        .unwrap()
        .1
}

fn signal(kind: ChangeKind, repository: RepoRef, number: Option<u32>) -> WebhookChange {
    WebhookChange {
        kind,
        repository: Repository {
            name: repository,
            id: None,
        },
        head_repository: None,
        head_sha: None,
        number,
        action: Some("edited".into()),
        installation_id: Some(42),
        candidate_login: None,
        base_repository: None,
    }
}

fn candidate(number: u32) -> WebhookChange {
    let mut change = signal(ChangeKind::PullRequest, repo("wallet-ui"), Some(number));
    change.action = Some("review_requested".into());
    change.candidate_login = Some("alice".into());
    change
}

async fn enqueue(h: &Harness, dispatcher: &mut Dispatcher, at: Instant, change: WebhookChange) {
    let id = Uuid::new_v4();
    let hash = id.simple().to_string().repeat(2);
    let github = h.db().github();
    github
        .record(
            &NewDelivery {
                id,
                hash: &hash,
                event: "pull_request",
                action: change.action.as_deref(),
                installation_id: Some(42),
                effect: Effect::Signal(&change.repository, std::slice::from_ref(&change)),
            },
            now(),
        )
        .await
        .unwrap();
    let delivery = github
        .next_pending_excluding(now(), &dispatcher.in_flight())
        .await
        .unwrap()
        .unwrap();
    dispatcher.enqueue(&h.state, delivery, at).await.unwrap();
}

async fn flush(h: &Harness, dispatcher: &mut Dispatcher, at: Instant) {
    dispatcher
        .flush(&h.state, at, Timestamp::now())
        .await
        .unwrap();
}

const WINDOW: Duration = Duration::from_secs(10);

#[tokio::test]
async fn pr_event_rereads_only_affected_users() {
    let (h, github) = setup("alice", 1, 4000).await;
    seed(&h, "u1", &snapshot(prs(1), vec![], vec![])).await;
    seed(
        &h,
        "u2",
        &snapshot(vec![pr(2, "Unrelated")], vec![], vec![]),
    )
    .await;
    let mut d = Dispatcher::default();
    let start = Instant::now();
    enqueue(
        &h,
        &mut d,
        start,
        signal(ChangeKind::PullRequest, repo("wallet-ui"), Some(1)),
    )
    .await;
    flush(&h, &mut d, start + WINDOW).await;
    assert_eq!(
        read(&h, "u1").await.pull_requests[0].facts.title,
        "Changed pull request 1"
    );
    assert_eq!(
        read(&h, "u2").await.pull_requests[0].facts.title,
        "Unrelated"
    );
    assert_eq!(github.queries.lock().unwrap().len(), 1);
}

async fn fork_check(owner: &str) -> usize {
    let (h, github) = setup("alice", 1, 4000).await;
    let mut snap = snapshot(prs(1), vec![], vec![]);
    snap.pull_requests[0].head_repository = Some(RepoRef {
        owner: "fork".into(),
        name: "wallet-ui".into(),
    });
    seed(&h, "u1", &snap).await;
    let mut foreign = snap.clone();
    foreign.pull_requests[0].facts.repo.owner = owner.into();
    foreign.pull_requests[0].head_repository = None;
    foreign.pull_requests[0].head_sha = "b".repeat(40);
    seed(&h, "u2", &foreign).await;
    let mut d = Dispatcher::default();
    let start = Instant::now();
    let mut check = signal(
        ChangeKind::Check,
        RepoRef {
            owner: owner.into(),
            name: "wallet-ui".into(),
        },
        None,
    );
    check.head_sha = Some("a".repeat(40));
    enqueue(&h, &mut d, start, check).await;
    flush(&h, &mut d, start + WINDOW).await;
    github.queries.lock().unwrap().len()
}

#[tokio::test]
async fn check_suite_on_fork_maps_by_repository_and_sha() {
    assert_eq!(fork_check("fork").await, 1);
}

#[tokio::test]
async fn same_sha_in_foreign_repo_matches_nothing() {
    assert_eq!(fork_check("unrelated").await, 0);
}

#[tokio::test]
async fn issue_event_rereads_issue() {
    let (h, _) = setup("alice", 1, 4000).await;
    seed(
        &h,
        "u1",
        &snapshot(vec![], vec![issue(1, "Old"), issue(2, "Unchanged")], vec![]),
    )
    .await;
    let mut d = Dispatcher::default();
    let start = Instant::now();
    enqueue(
        &h,
        &mut d,
        start,
        signal(ChangeKind::Issue, repo("wallet-ui"), Some(1)),
    )
    .await;
    flush(&h, &mut d, start + WINDOW).await;
    let stored = read(&h, "u1").await;
    assert_eq!(stored.issues[0].facts.title, "Changed issue");
    assert_eq!(stored.issues[0].comments, 7);
    assert_eq!(stored.issues[1].facts.title, "Unchanged");
}

#[tokio::test]
async fn burst_of_events_one_reread_per_user() {
    let (h, github) = setup("alice", 1, 4000).await;
    seed(&h, "u1", &snapshot(prs(2), vec![], vec![])).await;
    let mut d = Dispatcher::default();
    let start = Instant::now();
    for number in [1, 2, 1, 2] {
        enqueue(
            &h,
            &mut d,
            start,
            signal(ChangeKind::PullRequest, repo("wallet-ui"), Some(number)),
        )
        .await;
    }
    flush(&h, &mut d, start + WINDOW).await;
    let queries = github.queries.lock().unwrap();
    assert_eq!(queries.len(), 1);
    assert_eq!(queries[0].matches("pullRequest(number:").count(), 2);
}

#[tokio::test]
async fn steady_stream_does_not_starve_window() {
    let (h, github) = setup("alice", 1, 4000).await;
    seed(&h, "u1", &snapshot(prs(1), vec![], vec![])).await;
    let mut d = Dispatcher::default();
    let start = Instant::now();
    for seconds in 0..10 {
        let at = start + Duration::from_secs(seconds);
        enqueue(
            &h,
            &mut d,
            at,
            signal(ChangeKind::PullRequest, repo("wallet-ui"), Some(1)),
        )
        .await;
        flush(&h, &mut d, at).await;
    }
    assert!(github.queries.lock().unwrap().is_empty());
    flush(&h, &mut d, start + WINDOW).await;
    assert_eq!(github.queries.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn event_reads_stop_at_hourly_budget() {
    for (cost, remaining) in [(1000, 4000), (1, 999)] {
        let (h, github) = setup("alice", cost, remaining).await;
        seed(&h, "u1", &snapshot(prs(1), vec![], vec![])).await;
        seed(&h, "u2", &snapshot(vec![pr(2, "Old")], vec![], vec![])).await;
        let mut d = Dispatcher::default();
        let start = Instant::now();
        for (offset, number) in [1, 1, 2, 1].into_iter().enumerate() {
            let at = start + WINDOW * offset as u32;
            enqueue(
                &h,
                &mut d,
                at,
                signal(ChangeKind::PullRequest, repo("wallet-ui"), Some(number)),
            )
            .await;
            flush(&h, &mut d, at + WINDOW).await;
        }
        let count = github.queries.lock().unwrap().len();
        assert_eq!(count, 2);
        let at = start + Duration::from_secs(3611);
        enqueue(
            &h,
            &mut d,
            at,
            signal(ChangeKind::PullRequest, repo("wallet-ui"), Some(1)),
        )
        .await;
        flush(&h, &mut d, at + WINDOW).await;
        assert_eq!(
            github.queries.lock().unwrap().len(),
            if remaining >= 1000 { 3 } else { 2 }
        );
    }
}

#[tokio::test]
async fn review_requested_adds_and_judges_new_item() {
    let (h, _) = setup("alice", 1, 4000).await;
    let id = seed(&h, "u1", &snapshot(vec![], vec![], vec![])).await;
    let mut d = Dispatcher::default();
    let start = Instant::now();
    use axum::body::Body;
    use axum::http::StatusCode;
    use tower::ServiceExt;

    let body = json!({
        "action": "review_requested",
        "repository": {"id": 99, "full_name": "acme/wallet-ui"},
        "installation": {"id": 42},
        "number": 1,
        "pull_request": {"number": 1},
        "requested_reviewer": {"login": "Alice"}
    });
    let guid = Uuid::new_v4();
    let (parts, body) = fake_github::signed_webhook(
        "/webhooks/github",
        h.state.config.github.app_id,
        "pull_request",
        guid,
        body.to_string().as_bytes(),
        b"synthetic-webhook-secret-long-enough",
    )
    .unwrap()
    .into_parts();
    let hooks = crate::webhooks::Webhooks::from_app(&h.state);
    let response = crate::api::webhook_router(hooks)
        .oneshot(axum::http::Request::from_parts(parts, Body::from(body)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let delivery = h
        .db()
        .github()
        .next_pending(Timestamp::now())
        .await
        .unwrap()
        .unwrap();
    d.enqueue(&h.state, delivery, start).await.unwrap();
    flush(&h, &mut d, start + WINDOW).await;
    assert!(
        h.db()
            .github()
            .next_pending(Timestamp::now())
            .await
            .unwrap()
            .is_none()
    );
    let (_, input) = h.serve(|i| succeeded(neutral(i))).await;
    assert_eq!(input.items[0].key, key(1));
    assert_eq!(h.ranked("u1", id).await.judged, 1);
    assert_eq!(
        read(&h, "u1").await.pull_requests[0].facts.relations,
        [Relation::ReviewRequested]
    );
}

#[tokio::test]
async fn new_item_relation_from_reread_not_event() {
    let (h, github) = setup("somebody-else", 1, 4000).await;
    let id = seed(&h, "u1", &snapshot(vec![], vec![], vec![])).await;
    let mut d = Dispatcher::default();
    let start = Instant::now();
    enqueue(&h, &mut d, start, candidate(1)).await;
    flush(&h, &mut d, start + WINDOW).await;
    assert_eq!(github.queries.lock().unwrap().len(), 1);
    assert!(read(&h, "u1").await.pull_requests.is_empty());
    assert!(h.jobs(id).await.is_empty());
}

#[tokio::test]
async fn event_never_submits_rejudge_of_existing_item() {
    let (h, _) = setup("alice", 1, 4000).await;
    let (id, _) = h.rank_all("u1", &snapshot(prs(1), vec![], vec![])).await;
    let mut d = Dispatcher::default();
    let start = Instant::now();
    enqueue(&h, &mut d, start, candidate(1)).await;
    enqueue(&h, &mut d, start, candidate(2)).await;
    flush(&h, &mut d, start + WINDOW).await;
    let (_, input) = h.serve(|i| succeeded(neutral(i))).await;
    assert_eq!(
        input.items.iter().map(|item| &item.key).collect::<Vec<_>>(),
        [&key(2)]
    );
    h.ranked("u1", id).await;
    let snap = read(&h, "u1").await;
    assert_eq!(snap.pull_requests[0].facts.title, "Changed pull request 1");
    assert_eq!(h.jobs(id).await.len(), 2);
    let stored = h.judgements("u1", &snap).await;
    assert_eq!(stored[&key(1)].judgement.freshness, Freshness::Inherited);
    assert!(!stored[&key(1)].rejudge_requested);
}

#[tokio::test]
async fn new_item_at_zero_room_submits_no_job() {
    let (h, _) = setup("alice", 1, 4000).await;
    for batch in 0..3 {
        let pulls = (100 + batch * 48..100 + (batch + 1) * 48)
            .map(|n| pr(n, "Filler"))
            .collect();
        h.rank_all("u1", &snapshot(pulls, vec![], vec![])).await;
    }
    let id = seed(&h, "u1", &snapshot(vec![], vec![], vec![])).await;
    let mut d = Dispatcher::default();
    let start = Instant::now();
    enqueue(&h, &mut d, start, candidate(1)).await;
    flush(&h, &mut d, start + WINDOW).await;
    let status = h.ranked("u1", id).await;
    assert_eq!(status.left_provisional, 1);
    assert!(h.jobs(id).await.is_empty());
    assert_eq!(read(&h, "u1").await.pull_requests.len(), 1);
}

#[tokio::test]
async fn at_most_20_event_items_per_day() {
    let (h, _) = setup("alice", 1, 4000).await;
    let id = seed(&h, "u1", &snapshot(vec![], vec![], vec![])).await;
    let mut d = Dispatcher::default();
    let start = Instant::now();
    for number in 1..=21 {
        enqueue(&h, &mut d, start, candidate(number)).await;
    }
    flush(&h, &mut d, start + WINDOW).await;
    let (sent, _) = h
        .serve_until_ended("u1", id, |i| succeeded(neutral(i)))
        .await;
    assert_eq!(sent, 20);
    assert_eq!(read(&h, "u1").await.pull_requests.len(), 20);

    // A completed Refresh and a fresh dispatcher must preserve the day's count.
    seed(&h, "u1", &snapshot(vec![], vec![], vec![])).await;
    let mut d = Dispatcher::default();
    enqueue(&h, &mut d, start + WINDOW, candidate(22)).await;
    flush(&h, &mut d, start + WINDOW * 2).await;
    assert!(read(&h, "u1").await.pull_requests.is_empty());
}

#[tokio::test]
async fn watched_pr_probe_at_most_once_per_minute() {
    use crate::review_loop::watch::WATCH_LIMIT;
    use contracts::api::pr::PrSummary;
    use contracts::ids::CommitSha;

    let (h, github) = setup("alice", 1, 4000).await;
    let repo = repo("wallet-ui");
    let wall = Timestamp::now();
    let row = h
        .db()
        .repos()
        .upsert(DEFAULT_TENANT, &repo, Some(99))
        .await
        .unwrap();
    let pr_id = h
        .db()
        .pull_requests()
        .upsert(
            DEFAULT_TENANT,
            row.id,
            &PrSummary {
                repo: repo.clone(),
                number: 1,
                title: "Watched".into(),
                author: "bob".into(),
                html_url: "https://github.com/acme/wallet-ui/pull/1".into(),
                head_sha: CommitSha("a".repeat(40)),
                base_sha: CommitSha("b".repeat(40)),
            },
            wall,
        )
        .await
        .unwrap();
    for user in ["u1", "u2"] {
        h.db()
            .review_loop()
            .watch(DEFAULT_TENANT, user, pr_id, WATCH_LIMIT, wall)
            .await
            .unwrap();
        h.db()
            .review_loop()
            .record_probe(
                user,
                pr_id,
                &json!({
                    "head": "a".repeat(40), "updated_at": "2026-11-18T08:55:00Z",
                    "threads": 0, "comments": 0, "last_review": null,
                }),
                wall - SignedDuration::from_mins(1),
            )
            .await
            .unwrap();
    }
    h.db()
        .github()
        .record_installation_lookup(&repo, Some((42, false)), wall)
        .await
        .unwrap();
    let mut d = Dispatcher::default();
    let start = Instant::now();
    for (offset, kind) in [
        (0, ChangeKind::PullRequest),
        (15, ChangeKind::PullRequestReview),
    ] {
        let at = start + Duration::from_secs(offset);
        let mut change = signal(kind, repo.clone(), Some(1));
        change.action = Some(
            if offset == 0 {
                "synchronize"
            } else {
                "resolved"
            }
            .into(),
        );
        enqueue(&h, &mut d, at, change).await;
        d.flush(
            &h.state,
            at + WINDOW,
            wall + SignedDuration::from_secs(offset as i64 + 10),
        )
        .await
        .unwrap();
        assert_eq!(github.queries.lock().unwrap().len(), 1);
    }
    d.flush(
        &h.state,
        start + Duration::from_secs(71),
        wall + SignedDuration::from_secs(71),
    )
    .await
    .unwrap();
    let queries = github.queries.lock().unwrap();
    assert_eq!(
        queries
            .iter()
            .filter(|query| query.contains("reviewThreads { totalCount }"))
            .count(),
        2
    );
    assert!(
        queries
            .iter()
            .any(|query| !query.contains("a0: repository")),
        "resolving a thread must reach the full watch read even when the probe matches"
    );
}

#[tokio::test]
async fn event_item_waits_behind_dedicated_rejudge() {
    let (h, _) = setup("alice", 1, 4000).await;
    let mut snap = snapshot(prs(1), vec![], vec![]);
    let (id, _) = h.rank_all("u1", &snap).await;
    snap.pull_requests[0].facts.title = "Facts changed".into();
    h.db()
        .dashboard()
        .replace_current_snapshot(DEFAULT_TENANT, "u1", id, &snap)
        .await
        .unwrap();
    refresh::rejudge(&h.state, &scope("u1"), &key(1))
        .await
        .unwrap();
    let first = h.claim().await;
    let input = JudgeWorkInput::from_prompt(&first.spec.prompt).unwrap();
    assert_eq!(input.items[0].key, key(1));

    let mut d = Dispatcher::default();
    let start = Instant::now();
    enqueue(&h, &mut d, start, candidate(2)).await;
    flush(&h, &mut d, start + WINDOW).await;
    assert!(read(&h, "u1").await.event_ranking_pending);
    h.complete(&first, succeeded(neutral(&input))).await;
    let (next, input) = h.serve(|i| succeeded(neutral(i))).await;
    assert_eq!(input.items[0].key, key(2));
    assert!(next.round > first.round);
    h.ranked("u1", id).await;
    assert!(!read(&h, "u1").await.event_ranking_pending);
}
