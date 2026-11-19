//! Viewed marks against a wiremock GitHub and the fixture repository of `acme/node-client#482`.

use serde_json::{Value, json};
use wiremock::ResponseTemplate;

use super::*;
use crate::github::viewed::{FILES_VIEWED, MARK_VIEWED, MAX_FILE_PAGES, UNMARK_VIEWED};
use crate::test_support::github::{HEAD, fake_github_sign_in, pr482 as fixture, spawn_fake_github};

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
    let read = read(&f.state, "u1", &f.summary).await.expect("read");
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
    let read = read(&f.state, "u1", &f.summary).await.expect("read");
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
    let read = read(&f.state, "u1", &f.summary).await.expect("read");
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

/// The GraphQL documents against fake-github: mark, read, unmark, and GitHub's answer for a file outside the pull
/// request.
#[tokio::test]
async fn the_documents_work_against_fake_github() {
    use crate::github::viewed as client;
    let base = spawn_fake_github(None).await;
    let token = SecretString::from(fake_github_sign_in(&base).await);
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
    // NOT_FOUND on the mutation field proves nothing (as for pending-review writes): the outcome stays open.
    assert!(matches!(error, GraphqlError::FieldFailed(_)), "{error:?}");
    assert!(matches!(classify(error), ViewedError::Github(_)));
}

/// One rule with pending-review writes: only an answer that proves nothing was applied is a refusal; an error
/// next to resolved data or an unexpected answer leaves the mark open ("reload to see your mark").
#[test]
fn only_answers_that_prove_nothing_was_applied_are_refusals() {
    use crate::github::graphql::GraphqlErrorType as Kind;
    let status = GraphqlError::Status;
    for refused in [
        GraphqlError::NotExecuted(Kind::Other),
        GraphqlError::NotExecuted(Kind::Forbidden),
        GraphqlError::FieldFailed(Kind::Forbidden),
        GraphqlError::RateLimited,
        status(reqwest::StatusCode::UNPROCESSABLE_ENTITY),
    ] {
        let mapped = classify(refused);
        assert!(matches!(mapped, ViewedError::Refused(_)), "{mapped:?}");
    }
    for open in [
        GraphqlError::Rejected(Kind::Forbidden),
        GraphqlError::Rejected(Kind::NotFound),
        GraphqlError::Rejected(Kind::Unprocessable),
        GraphqlError::FieldFailed(Kind::NotFound),
        GraphqlError::FieldFailed(Kind::Unprocessable),
        GraphqlError::NoData,
        status(reqwest::StatusCode::BAD_GATEWAY),
    ] {
        let mapped = classify(open);
        assert!(matches!(mapped, ViewedError::Github(_)), "{mapped:?}");
    }
}

/// GitHub names a renamed file by its new path; the old path is not a file of the pull request.
#[tokio::test]
async fn a_renamed_file_is_marked_by_its_new_path() {
    let f = fixture().await;
    f.head_is(HEAD, 1).await;
    f.mock(
        json!({"query": MARK_VIEWED, "variables": {"in": {"path": "src/rpc/retry.rs"}}}),
        ResponseTemplate::new(200)
            .set_body_json(json!({"data": {"markFileAsViewed": {"clientMutationId": null}}})),
        1,
    )
    .await;
    set(&f.state, "u1", &f.summary, "src/rpc/retry.rs", true)
        .await
        .expect("new path");
    let old = set(&f.state, "u1", &f.summary, "src/util/backoff.rs", true).await;
    assert!(matches!(old, Err(ViewedError::UnknownFile)), "{old:?}");
}

/// At most one Viewed write of a user runs per analysis; a second one meanwhile is refused before any GitHub call.
#[tokio::test]
async fn a_second_write_while_one_runs_is_refused() {
    let f = fixture().await;
    let head = json!({"data": {"repository": {"pullRequest": {"id": "PR_fixture482", "headRefOid": HEAD}}}});
    f.mock(
        json!({"query": crate::github::review::PULL_REQUEST_HEAD}),
        ResponseTemplate::new(200)
            .set_body_json(head)
            .set_delay(std::time::Duration::from_millis(300)),
        2,
    )
    .await;
    f.mock(
        json!({"query": MARK_VIEWED}),
        ResponseTemplate::new(200)
            .set_body_json(json!({"data": {"markFileAsViewed": {"clientMutationId": null}}})),
        2,
    )
    .await;
    let (first, second) = tokio::join!(set(&f.state, "u1", &f.summary, CLIENT, true), async {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        set(&f.state, "u1", &f.summary, CLIENT, true).await
    });
    first.expect("first");
    assert!(matches!(second, Err(ViewedError::Busy)), "{second:?}");
    set(&f.state, "u1", &f.summary, CLIENT, true)
        .await
        .expect("after the first ended");
}

/// A GitHub refusal of the read is visible to operators (warn, once per request, no token); the list still shows
/// the marks as not read.
#[tokio::test]
async fn a_refused_read_is_logged_at_warn() {
    let f = fixture().await;
    f.mock(
        json!({"query": FILES_VIEWED}),
        ResponseTemplate::new(200).set_body_json(
            json!({"errors": [{"type": "FORBIDDEN", "message": "Resource not accessible by integration"}]}),
        ),
        1,
    )
    .await;
    let capture = crate::test_support::logs::LogCapture::start();
    let read = read(&f.state, "u1", &f.summary).await.expect("read");
    assert_eq!(read.status, ViewedStatus::NotChecked);
    let logs = capture.text();
    let warned: Vec<&str> = logs
        .lines()
        .filter(|line| {
            line.contains("WARN") && line.contains("GitHub refused to read the Viewed marks")
        })
        .collect();
    assert_eq!(warned.len(), 1, "{logs}");
    assert!(!logs.contains(crate::review::tests::USER_TOKEN), "{logs}");
}

/// Without a GitHub sign-in the read is not "not checked": the user must sign in again.
#[tokio::test]
async fn a_read_without_a_sign_in_asks_to_sign_in() {
    let f = fixture().await;
    crate::auth::end_sign_in(&f.state, "u1")
        .await
        .expect("end sign-in");
    let read = read(&f.state, "u1", &f.summary).await;
    assert!(
        matches!(read, Err(UserTokenError::SignInRequired)),
        "{read:?}"
    );
}

/// A failed read of the pull request's head comes before the mutation: nothing was sent, so it is a read failure
/// with the mark unchanged, never "did not confirm the change".
#[tokio::test]
async fn a_failed_head_read_is_not_a_mutation_outcome() {
    let f = fixture().await;
    f.mock(
        json!({"query": crate::github::review::PULL_REQUEST_HEAD}),
        ResponseTemplate::new(502),
        1,
    )
    .await;
    let unread = set(&f.state, "u1", &f.summary, CLIENT, true).await;
    assert!(
        matches!(unread, Err(ViewedError::HeadUnread(_))),
        "{unread:?}"
    );
    assert!(mutations(&f.graphql_bodies().await).is_empty());
}
