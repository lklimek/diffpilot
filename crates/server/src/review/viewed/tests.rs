//! Viewed marks against a wiremock GitHub and the fixture repository of `acme/node-client#482`.

use serde_json::{Value, json};
use wiremock::ResponseTemplate;

use super::*;
use crate::github::viewed::{FILES_VIEWED, MARK_VIEWED, MAX_FILE_PAGES, UNMARK_VIEWED};
use crate::review::comments::github_tests::{HEAD, fixture};

const CLIENT: &str = "src/rpc/client.rs";

fn files_page(nodes: Value, next: Option<&str>) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"data": {"repository": {"pullRequest": {
        "id": "PR_fixture482", "headRefOid": HEAD,
        "files": {"pageInfo": {"hasNextPage": next.is_some(), "endCursor": next}, "nodes": nodes}}}}}))
}

fn refused(field: &str, kind: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"data": {field: null},
        "errors": [{"type": kind, "path": [field], "message": "m"}]}))
}

fn mutations(bodies: &[Value]) -> Vec<(String, Value)> {
    bodies
        .iter()
        .filter_map(|body| {
            let query = body["query"].as_str()?;
            query
                .starts_with("mutation")
                .then(|| (query.to_owned(), body["variables"]["in"].clone()))
        })
        .collect()
}

#[tokio::test]
async fn read_maps_every_state_and_the_head() {
    let f = fixture().await;
    let nodes = json!([
        {"path": CLIENT, "viewerViewedState": "VIEWED"},
        {"path": "src/rpc/retry.rs", "viewerViewedState": "DISMISSED"},
        {"path": "src/rpc/metrics.rs", "viewerViewedState": "UNVIEWED"},
        {"path": "src/odd.rs", "viewerViewedState": "SOMETHING_NEW"}
    ]);
    f.mock(json!({"query": FILES_VIEWED}), files_page(nodes, None), 1)
        .await;
    let read = read(&f.state, "u1", &f.summary).await;
    assert_eq!(read.status, ViewedStatus::Checked);
    assert_eq!(read.head, Some(CommitSha(HEAD.to_owned())));
    assert_eq!(
        read.files.into_iter().collect::<Vec<_>>(),
        [
            ("src/rpc/client.rs".to_owned(), FileViewedState::Viewed),
            ("src/rpc/metrics.rs".to_owned(), FileViewedState::Unviewed),
            ("src/rpc/retry.rs".to_owned(), FileViewedState::Dismissed),
        ],
        "an unknown state is not read, never assumed"
    );
}

#[tokio::test]
async fn read_follows_pages_up_to_the_cap() {
    let f = fixture().await;
    let page = |n: usize| json!([{"path": format!("f{n}.rs"), "viewerViewedState": "VIEWED"}]);
    f.mock(
        json!({"query": FILES_VIEWED, "variables": {"after": null}}),
        files_page(page(0), Some("c1")),
        1,
    )
    .await;
    for n in 1..MAX_FILE_PAGES {
        f.mock(
            json!({"query": FILES_VIEWED, "variables": {"after": format!("c{n}")}}),
            files_page(page(n), Some(&format!("c{}", n + 1))),
            1,
        )
        .await;
    }
    let read = read(&f.state, "u1", &f.summary).await;
    assert_eq!(read.status, ViewedStatus::Partial);
    assert_eq!(read.files.len(), MAX_FILE_PAGES);
}

#[tokio::test]
async fn an_unanswered_read_is_not_checked() {
    let f = fixture().await;
    f.mock(
        json!({"query": FILES_VIEWED}),
        ResponseTemplate::new(502),
        1,
    )
    .await;
    let read = read(&f.state, "u1", &f.summary).await;
    assert_eq!(
        read,
        ViewedRead {
            files: BTreeMap::new(),
            status: ViewedStatus::NotChecked,
            head: None
        }
    );
}

#[tokio::test]
async fn marking_and_unmarking_send_exactly_the_mutation_with_the_node_id() {
    let f = fixture().await;
    f.head_is(HEAD, 2).await;
    let ok = |field: &str| {
        ResponseTemplate::new(200)
            .set_body_json(json!({"data": {field: {"clientMutationId": null}}}))
    };
    f.mock(json!({"query": MARK_VIEWED}), ok("markFileAsViewed"), 1)
        .await;
    f.mock(json!({"query": UNMARK_VIEWED}), ok("unmarkFileAsViewed"), 1)
        .await;
    set(&f.state, "u1", &f.summary, CLIENT, true)
        .await
        .expect("marked");
    set(&f.state, "u1", &f.summary, CLIENT, false)
        .await
        .expect("unmarked");
    let input = json!({"pullRequestId": "PR_fixture482", "path": CLIENT});
    assert_eq!(
        mutations(&f.graphql_bodies().await),
        [
            (MARK_VIEWED.to_owned(), input.clone()),
            (UNMARK_VIEWED.to_owned(), input)
        ]
    );
}

#[tokio::test]
async fn a_moved_head_or_a_path_outside_the_pull_request_changes_nothing() {
    let f = fixture().await;
    f.head_is("1111111111111111111111111111111111111111", 1)
        .await;
    let moved = set(&f.state, "u1", &f.summary, CLIENT, true).await;
    assert!(
        matches!(moved, Err(ViewedError::PrHeadChanged)),
        "{moved:?}"
    );
    let outside = set(&f.state, "u1", &f.summary, "src/not-in-the-pr.rs", true).await;
    assert!(
        matches!(outside, Err(ViewedError::UnknownFile)),
        "{outside:?}"
    );
    assert!(mutations(&f.graphql_bodies().await).is_empty());
}

/// U1: a token GitHub does not let mark files gets a typed refusal (the UI then offers "Mark it on GitHub").
#[tokio::test]
async fn a_refusal_is_typed_and_a_missing_answer_is_not_a_refusal() {
    let f = fixture().await;
    f.head_is(HEAD, 3).await;
    f.mock(
        json!({"query": MARK_VIEWED}),
        refused("markFileAsViewed", "FORBIDDEN"),
        1,
    )
    .await;
    f.mock(
        json!({"query": UNMARK_VIEWED}),
        ResponseTemplate::new(502),
        1,
    )
    .await;
    let forbidden = set(&f.state, "u1", &f.summary, CLIENT, true).await;
    assert!(
        matches!(forbidden, Err(ViewedError::Refused(_))),
        "{forbidden:?}"
    );
    let unanswered = set(&f.state, "u1", &f.summary, CLIENT, false).await;
    assert!(
        matches!(unanswered, Err(ViewedError::Github(_))),
        "{unanswered:?}"
    );
    f.server.reset().await;
    f.head_is(HEAD, 1).await;
    f.mock(
        json!({"query": MARK_VIEWED}),
        ResponseTemplate::new(200).set_body_json(json!({"data": {"markFileAsViewed": null}})),
        1,
    )
    .await;
    let empty = set(&f.state, "u1", &f.summary, CLIENT, true).await;
    assert!(matches!(empty, Err(ViewedError::NoPayload)), "{empty:?}");
}

#[tokio::test]
async fn a_rejected_token_is_marked_for_refresh() {
    let f = fixture().await;
    f.mock(
        json!({"query": crate::github::review::PULL_REQUEST_HEAD}),
        ResponseTemplate::new(401),
        1,
    )
    .await;
    let rejected = set(&f.state, "u1", &f.summary, CLIENT, true).await;
    assert!(
        rejected.as_ref().is_err_and(ViewedError::unauthorized),
        "{rejected:?}"
    );
    let stored = f
        .state
        .db
        .user_tokens()
        .get("u1")
        .await
        .expect("tokens")
        .expect("stored");
    assert!(
        stored.tokens.access_expires_at <= contracts::time::Timestamp::now(),
        "the next use refreshes the pair"
    );
}

/// The GraphQL documents against fake-github: mark, read, unmark, and a refusal for a file outside the pull request.
#[tokio::test]
async fn the_documents_work_against_fake_github() {
    use crate::github::viewed as client;
    let base = crate::github::work::tests::spawn_fake().await;
    let token = SecretString::from(crate::github::work::tests::sign_in(&base).await);
    let mut config = crate::config::tests::test_config().github;
    config.api_url = url::Url::parse(&base).expect("url");
    let pr = PrRef {
        repo: contracts::common::RepoRef {
            owner: "acme".to_owned(),
            name: "node-client".to_owned(),
        },
        number: 482,
    };
    let read = client::read(&config, &token, &pr).await.expect("read");
    assert_eq!(read.pull_request_id, "PR_fixture482");
    assert!(!read.incomplete);
    assert_eq!(read.files.get(CLIENT), Some(&FileViewedState::Unviewed));
    client::set(&config, &token, &read.pull_request_id, CLIENT, true)
        .await
        .expect("marked");
    let read = client::read(&config, &token, &pr).await.expect("read");
    assert_eq!(read.files.get(CLIENT), Some(&FileViewedState::Viewed));
    client::set(&config, &token, &read.pull_request_id, CLIENT, false)
        .await
        .expect("unmarked");
    let read = client::read(&config, &token, &pr).await.expect("read");
    assert_eq!(read.files.get(CLIENT), Some(&FileViewedState::Unviewed));
    let outside = client::set(&config, &token, &read.pull_request_id, "src/none.rs", true).await;
    let Err(ViewedWriteError::Graphql(error)) = outside else {
        panic!("{outside:?}")
    };
    assert!(matches!(classify(error), ViewedError::Refused(_)));
}
