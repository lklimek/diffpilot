use super::*;

use contracts::api::pr::{PrHistory, PrSummary};
use contracts::api::review_loop::WatchState;
use contracts::ids::{AnalysisId, CommitSha};
use db::repo::analyses::NewAnalysis;

const HISTORY: &str = "/api/v1/me/prs";

fn at(seconds: i64) -> Timestamp {
    Timestamp::from_second(seconds).unwrap()
}

async fn history(w: &World, uri: &str) -> PrHistory {
    serde_json::from_value(w.get(uri).await).expect("PR history")
}

async fn seed_pr(w: &World, tenant: &str, number: u32) -> i64 {
    let pr = PrSummary {
        repo: crate::github::review_loop::tests::pr482().repo,
        number,
        title: format!("Pull request {number}"),
        author: "author".into(),
        html_url: format!("https://github.com/acme/node-client/pull/{number}"),
        base_sha: CommitSha("a".repeat(40)),
        head_sha: CommitSha("b".repeat(40)),
    };
    let repo = w
        .state
        .db
        .repos()
        .upsert(tenant, &pr.repo, None)
        .await
        .unwrap();
    w.state
        .db
        .pull_requests()
        .upsert(tenant, repo.id, &pr, at(1))
        .await
        .unwrap()
}

async fn analyse(w: &World, pr: i64, user: &str, head: char, time: Timestamp) -> AnalysisId {
    let id = AnalysisId::new_random();
    w.state
        .db
        .analyses()
        .create(
            NewAnalysis {
                id,
                tenant_id: DEFAULT_TENANT.into(),
                pull_request_id: pr,
                base_sha: CommitSha("a".repeat(40)),
                head_sha: CommitSha(head.to_string().repeat(40)),
                language: Default::default(),
                created_by: user.into(),
            },
            time,
        )
        .await
        .unwrap();
    id
}

async fn watch(w: &World, tenant: &str, pr: i64, time: Timestamp) {
    w.state
        .db
        .review_loop()
        .watch(tenant, "u1", pr, 10, time)
        .await
        .unwrap();
}

async fn add_member(w: &World, tenant: &str, user: &str, github_id: i64) {
    w.state
        .db
        .users()
        .upsert_member(
            tenant,
            user,
            &GithubProfile {
                github_id,
                login: (if user == "u1" { "lklimek" } else { user }).into(),
                name: None,
                avatar_url: None,
            },
            at(1),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn pr_page_get_upserts_opening() {
    let w = world().await;
    w.get(PR).await;
    let first = history(&w, HISTORY).await.prs.remove(0).opened_at.unwrap();
    w.get(PR).await;
    let reopened = history(&w, HISTORY).await.prs;
    assert_eq!(reopened.len(), 1);
    assert_eq!(reopened[0].pr.number, 482);
    assert!(reopened[0].opened_at.unwrap() > first);
}

#[tokio::test]
async fn access_token_pr_page_read_records_no_opening() {
    let w = world().await;
    let pr = seed_pr(&w, DEFAULT_TENANT, 482).await;
    w.state
        .db
        .review_loop()
        .ensure_pr(DEFAULT_TENANT, pr, at(1))
        .await
        .unwrap();
    let (token, _) = crate::auth::access_tokens::stored_token(
        &w.state.db,
        "u1",
        vec![TokenScope::Read],
        Timestamp::now() + SignedDuration::from_hours(24),
    )
    .await;
    let get = |uri| {
        Request::get(uri)
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap()
    };
    let response = w.app.clone().oneshot(get(PR)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = w.app.clone().oneshot(get(HISTORY)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap(),
        json!({"prs": [], "next_cursor": null})
    );
    assert!(w.server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn openings_are_tenant_scoped() {
    let w = world().await;
    db::repo::analyses::testing::seed_tenant(&w.state.db, "other")
        .await
        .unwrap();
    add_member(&w, "other", "u1", 1).await;
    let foreign = seed_pr(&w, "other", 482).await;
    let local = seed_pr(&w, DEFAULT_TENANT, 482).await;
    assert_ne!(local, foreign);
    w.get(PR).await;
    let prs = w.state.db.pull_requests();
    assert!(
        prs.record_opening("other", "u1", local, at(1))
            .await
            .is_err()
    );
    assert!(
        prs.history("other", "u1", None, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        prs.history(DEFAULT_TENANT, "u1", None, 10).await.unwrap()[0].0,
        local
    );
}

#[tokio::test]
async fn me_prs_union_of_analyses_watches_openings() {
    let w = world().await;
    let analysis_only = seed_pr(&w, DEFAULT_TENANT, 480).await;
    analyse(&w, analysis_only, "u1", 'b', at(5)).await;
    let watch_only = seed_pr(&w, DEFAULT_TENANT, 481).await;
    watch(&w, DEFAULT_TENANT, watch_only, at(6)).await;
    w.get(PR).await;
    let all_sources = seed_pr(&w, DEFAULT_TENANT, 483).await;
    let prs = w.state.db.pull_requests();
    prs.record_opening(DEFAULT_TENANT, "u1", all_sources, at(7))
        .await
        .unwrap();
    watch(&w, DEFAULT_TENANT, all_sources, at(8)).await;
    analyse(&w, all_sources, "u1", 'b', at(9)).await;
    let latest = analyse(&w, all_sources, "u1", 'c', at(10)).await;
    let body = w.get(HISTORY).await;
    let rows = body["prs"].as_array().unwrap();
    let numbers: Vec<_> = rows
        .iter()
        .map(|row| row["pr"]["number"].as_u64().unwrap())
        .collect();
    assert_eq!(numbers, [482, 483, 481, 480]);
    assert!(rows[0].get("opened_at").is_some());
    assert!(rows[0].get("analysis").is_none() && rows[0].get("watch").is_none());
    assert_eq!(rows[1]["title"], "Pull request 483");
    assert_eq!(rows[1]["last_activity_at"], json!(at(10)));
    assert_eq!(rows[1]["opened_at"], json!(at(7)));
    assert_eq!(
        rows[1]["analysis"],
        json!({
            "id": latest, "status": "planning", "head_sha": "c".repeat(40), "head_moved": true,
        })
    );
    assert_eq!(
        rows[1]["watch"],
        json!({"state": "active", "since": at(8), "ended_at": null})
    );
    assert!(rows[2].get("watch").is_some() && rows[2].get("analysis").is_none());
    assert_eq!(rows[3]["analysis"]["head_moved"], false);
    assert!(rows[3].get("watch").is_none() && rows[3].get("opened_at").is_none());
}

#[tokio::test]
async fn me_prs_includes_ended_watches() {
    let w = world().await;
    let id = seed_pr(&w, DEFAULT_TENANT, 480).await;
    watch(&w, DEFAULT_TENANT, id, at(5)).await;
    w.state
        .db
        .review_loop()
        .set_watch_state("u1", id, WatchState::Ended, at(9))
        .await
        .unwrap();
    let rows = history(&w, HISTORY).await.prs;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].last_activity_at, at(9));
    let watch = rows[0].watch.as_ref().unwrap();
    assert_eq!(watch.state, WatchState::Ended);
    assert_eq!(watch.since, at(5));
    assert_eq!(watch.ended_at, Some(at(9)));
}

#[tokio::test]
async fn me_prs_newest_first_cursor_paging() {
    let w = world().await;
    for (number, time) in [(480, 9), (481, 10), (482, 5), (483, 10)] {
        let id = seed_pr(&w, DEFAULT_TENANT, number).await;
        w.state
            .db
            .pull_requests()
            .record_opening(DEFAULT_TENANT, "u1", id, at(time))
            .await
            .unwrap();
    }
    let mut numbers = Vec::new();
    let mut uri = format!("{HISTORY}?limit=1");
    for index in 0..4 {
        let page = history(&w, &uri).await;
        assert_eq!(page.prs.len(), 1);
        numbers.push(page.prs[0].pr.number);
        if index == 3 {
            assert!(page.next_cursor.is_none());
        } else {
            let cursor = page.next_cursor.expect("another page");
            uri = format!("{HISTORY}?limit=1&cursor={cursor}");
        }
    }
    assert_eq!(numbers, [483, 481, 480, 482]);
    assert_eq!(
        w.call("GET", &format!("{HISTORY}?cursor=invalid"), None)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn me_prs_tenant_scoped() {
    let w = world().await;
    add_member(&w, DEFAULT_TENANT, "u2", 2).await;
    let other_user = seed_pr(&w, DEFAULT_TENANT, 480).await;
    analyse(&w, other_user, "u2", 'b', at(7)).await;
    w.state
        .db
        .pull_requests()
        .record_opening(DEFAULT_TENANT, "u2", other_user, at(8))
        .await
        .unwrap();
    w.state
        .db
        .review_loop()
        .watch(DEFAULT_TENANT, "u2", other_user, 10, at(9))
        .await
        .unwrap();
    db::repo::analyses::testing::seed_tenant(&w.state.db, "other")
        .await
        .unwrap();
    let other_tenant = seed_pr(&w, "other", 481).await;
    add_member(&w, "other", "u1", 1).await;
    watch(&w, "other", other_tenant, at(10)).await;
    w.state
        .db
        .pull_requests()
        .record_opening("other", "u1", other_tenant, at(11))
        .await
        .unwrap();
    w.get(PR).await;
    let rows = history(&w, HISTORY).await.prs;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].pr.number, 482);
}

#[tokio::test]
async fn me_prs_no_longer_501() {
    let w = world().await;
    let anonymous = w
        .app
        .clone()
        .oneshot(Request::get(HISTORY).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        w.get(HISTORY).await,
        json!({"prs": [], "next_cursor": null})
    );
}
