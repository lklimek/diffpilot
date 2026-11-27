use super::*;

const SUBMIT: &str = "/api/v1/prs/acme/node-client/482/comments/submissions";
const TEXT: &str = "The retry can exceed the operation deadline.";

async fn submit(w: &World, token: &str, value: &Value) -> (StatusCode, Value) {
    let response = w
        .app
        .clone()
        .oneshot(
            Request::post(SUBMIT)
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(value.to_string()))
                .expect("request"),
        )
        .await
        .expect("infallible");
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn submission(target: Value) -> Value {
    json!({"submission_key": uuid::Uuid::new_v4(), "body": TEXT, "target": target})
}

#[tokio::test]
async fn agent_comment_submission_is_scoped_idempotent_and_survives_refresh() {
    let w = world().await;
    let (reader, _) = feed::agent(&w, &[TokenScope::Read]).await;
    let (executor, _) = feed::agent(&w, &feed::EXECUTE).await;
    let (writer, principal) =
        feed::agent(&w, &[TokenScope::Read, TokenScope::ReviewLoopSubmit]).await;
    let value = submission(json!({"kind": "standalone"}));
    for token in [reader, executor] {
        let (status, _) = submit(&w, &token, &value).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
    let (left, right) = tokio::join!(submit(&w, &writer, &value), submit(&w, &writer, &value));
    assert!(
        matches!(
            (left.0, right.0),
            (StatusCode::CREATED, StatusCode::OK) | (StatusCode::OK, StatusCode::CREATED)
        ),
        "{left:?} / {right:?}"
    );
    assert_eq!(left.1, right.1);
    let item = left.1["item_no"].as_u64().expect("item number");
    assert!(
        left.1["url"]
            .as_str()
            .unwrap()
            .ends_with(&format!("/pr/acme/node-client/482/comments?item=C{item}"))
    );

    w.age_read().await;
    w.server.reset().await;
    let (status, replay) = submit(&w, &writer, &value).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay, left.1);
    assert!(w.server.received_requests().await.unwrap().is_empty());
    serve(&w.server, &[]).await;
    let mut conflicting = value;
    conflicting["body"] = json!("A different comment");
    assert_eq!(
        submit(&w, &writer, &conflicting).await.0,
        StatusCode::CONFLICT
    );
    assert!(
        w.state
            .db
            .review_loop()
            .live_passes()
            .await
            .unwrap()
            .is_empty()
    );
    w.age_read().await;
    assert_eq!(
        w.post(&format!("{PR}/comments/read"), json!({})).await.0,
        StatusCode::ACCEPTED
    );
    let items: Vec<_> = w
        .items()
        .await
        .into_iter()
        .filter(|item| item["origin"] == "api")
        .collect();
    assert_eq!(items.len(), 1);
    let item = &items[0];
    assert_eq!(item["no"], left.1["item_no"]);
    assert_eq!(item["body"], TEXT);
    assert_eq!(item["author"], format!("agent {}", principal.token_id));
    assert_eq!(item["status"], "not_assessed");
    assert!(item["gone_at"].is_null());
    assert!(item["decisions"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn agent_annotation_on_a_pr_comment_without_a_card_stays_read_only() {
    use wiremock::matchers::body_partial_json;

    let w = world().await;
    let (writer, _) = feed::agent(&w, &[TokenScope::Read, TokenScope::ReviewLoopSubmit]).await;
    let root = "https://github.com/acme/node-client/pull/482#issuecomment-5000000102";
    let value = submission(json!({"kind": "github", "comment": "IC_fixture482_s1"}));
    let (status, receipt) = submit(&w, &writer, &value).await;
    assert_eq!(status, StatusCode::CREATED, "{receipt}");
    assert!(receipt["item_no"].is_null());
    let before = w.items().await;
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .and(body_partial_json(
            json!({"variables": {"id": "IC_fixture482_s1"}}),
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data": {"node": {
                "__typename": "IssueComment", "url": root, "body": "A PR-level status report",
                "author": {"login": "github-actions"}, "createdAt": "2026-11-27T10:00:00Z",
                "pullRequest": {"number": 482, "title": "Retry RPC calls",
                    "repository": {"nameWithOwner": "acme/node-client", "isPrivate": false}}
            }}})),
        )
        .with_priority(1)
        .mount(&w.server)
        .await;
    let detail = w
        .get(&format!("{PR}/comments/threads/issue%3AIC_fixture482_s1"))
        .await;
    assert!(detail["item_no"].is_null(), "{detail}");
    assert_eq!(detail["pr_level"], true);
    assert_eq!(detail["comments"][0]["body"], "A PR-level status report");
    assert_eq!(detail["agent_annotations"][0]["target_url"], root);
    assert_eq!(detail["agent_annotations"][0]["body"], TEXT);
    assert_eq!(w.items().await, before);
}

#[tokio::test]
async fn agent_annotations_keep_the_exact_github_target_and_do_not_add_queue_items() {
    let w = world().await;
    let (writer, _) = feed::agent(&w, &[TokenScope::Read, TokenScope::ReviewLoopSubmit]).await;
    let target = "https://github.com/acme/node-client/pull/482#discussion_r3000000201";
    let value = submission(json!({"kind": "github", "comment": target}));
    let mut other_pr = value.clone();
    other_pr["target"]["comment"] = json!(target.replace("/482#", "/483#"));
    assert_eq!(
        submit(&w, &writer, &other_pr).await.0,
        StatusCode::NOT_FOUND
    );
    let (status, receipt) = submit(&w, &writer, &value).await;
    assert_eq!(status, StatusCode::CREATED, "{receipt}");
    assert!(
        receipt["url"]
            .as_str()
            .unwrap()
            .contains("PRRT_fixture482_l2")
    );
    let (status, replay) = submit(&w, &writer, &value).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay, receipt);
    let item = w.item(L2).await;
    assert_eq!(item["origin"], "github");
    assert_eq!(item["revision"], 1);
    let annotations = item["agent_annotations"].as_array().unwrap();
    assert_eq!(annotations.len(), 1);
    assert_eq!(annotations[0]["target_url"], target);
    assert_eq!(annotations[0]["body"], TEXT);
    assert!(w.items().await.iter().all(|item| item["origin"] != "api"));
}

#[tokio::test]
async fn agent_annotations_find_replies_after_the_first_page() {
    use wiremock::matchers::body_partial_json;

    let w = world().await;
    serve(
        &w.server,
        &[("review-threads.json", &|answer| {
            let nodes = answer["data"]["repository"]["pullRequest"]["reviewThreads"]["nodes"]
                .as_array_mut()
                .unwrap();
            let thread = nodes
                .iter_mut()
                .find(|thread| thread["id"] == "PRRT_fixture482_l2")
                .unwrap();
            thread["comments"]["totalCount"] = json!(51);
        })],
    )
    .await;
    let root = "https://github.com/acme/node-client/pull/482#discussion_r3000000201";
    let last = "https://github.com/acme/node-client/pull/482#discussion_r3000000299";
    Mock::given(method("GET"))
        .and(path(
            "/api/repos/acme/node-client/pulls/comments/3000000299",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "pull_request_url": "https://api.github.com/repos/acme/node-client/pulls/482",
            "in_reply_to_id": 3000000201_u64
        })))
        .mount(&w.server)
        .await;
    let id = "PRRC_deep_reply";
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .and(body_partial_json(json!({"variables": {"id": id}})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {"node": {
            "__typename": "PullRequestReviewComment", "url": last, "state": "SUBMITTED", "replyTo": {"url": root},
            "pullRequest": {"number": 482, "title": "Retry calls",
                "repository": {"nameWithOwner": "acme/node-client", "isPrivate": false}}
        }}})))
        .with_priority(1)
        .mount(&w.server)
        .await;
    for (after, next, url) in [(None, Some("last"), root), (Some("last"), None, last)] {
        Mock::given(method("POST"))
            .and(path("/api/graphql"))
            .and(body_partial_json(json!({"variables": {"id": "PRRT_fixture482_l2", "after": after}})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {"node": {
                "__typename":"PullRequestReviewThread", "path":"src/rpc/client.rs",
                "pullRequest":{"number":482,"title":"Retry calls",
                    "repository":{"nameWithOwner":"acme/node-client","isPrivate":false}},
                "comments":{"pageInfo":{"hasNextPage":next.is_some(),"endCursor":next},
                    "nodes":[{"state":"SUBMITTED","body":"Original","url":url,"author":{"login":"bob"},
                        "createdAt":"2026-11-27T10:00:00Z"}]}
            }}})))
            .with_priority(1)
            .mount(&w.server)
            .await;
    }
    let (writer, _) = feed::agent(&w, &[TokenScope::Read, TokenScope::ReviewLoopSubmit]).await;
    for selector in [last, id] {
        let (status, receipt) = submit(
            &w,
            &writer,
            &submission(json!({"kind":"github","comment":selector})),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{receipt}");
        assert!(
            receipt["url"]
                .as_str()
                .unwrap()
                .contains("PRRT_fixture482_l2")
        );
    }
    let card = w.item(L2).await;
    let annotations = card["agent_annotations"].as_array().unwrap();
    assert_eq!(annotations.len(), 2);
    assert!(
        annotations
            .iter()
            .all(|annotation| annotation["target_url"] == last)
    );
}
