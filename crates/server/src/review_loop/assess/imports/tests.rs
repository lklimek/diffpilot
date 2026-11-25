use super::*;
use contracts::agent::comment_report::ProposedDecisionKind;
use serde_json::json;

const POINT: &str = "https://github.com/acme/r/pull/12#discussion_r21";
const REPLY: &str = "https://github.com/acme/r/pull/12#discussion_r22";

fn authority() -> Authorities {
    Authorities {
        members: [(
            1,
            Principal {
                user: "u1".into(),
                login: "owner".into(),
            },
        )]
        .into(),
        relays: [(
            2,
            Some(Principal {
                user: "u1".into(),
                login: "owner".into(),
            }),
        )]
        .into(),
    }
}

fn statement(by: &str, kind: RecordedDecisionKind, quote: &str) -> RecordedDecision {
    RecordedDecision {
        kind,
        by: by.into(),
        on_behalf_of: None,
        comment_url: REPLY.into(),
        quote: quote.into(),
        target_pr: None,
        target_url: None,
    }
}

fn attempt(
    authority: &Authorities,
    source_id: u64,
    author_id: Option<u64>,
    d: &RecordedDecision,
    marker: Option<&str>,
    thread: &str,
) -> Option<ImportCandidate> {
    let data: PrData = serde_json::from_value(json!({
        "manifest":{"repo":"acme/r","pr":12,"head":"abc","base_ref":"main","base_sha":"def",
            "title":"Retry","body":"","pr_author":"alice","maintainers":["owner"],"bots":[]},
        "threads":{"threads":[{"id":"T21","comments":[
            {"author":{"login":"reviewer","databaseId":source_id},
             "body":"Bound the retry loop.", "url":POINT,"createdAt":"2026-11-24T00:00:00Z"},
            {"author":{"login":d.by,"databaseId":author_id},
             "body":d.quote,"relayMarker":marker,"url":REPLY,"createdAt":"2026-11-25T00:00:00Z"}
        ]}, {"id":"T51","comments":[{"author":{"login":"reviewer","databaseId":source_id},
            "body":"Bound the retry loop.", "url":POINT.replace("r21","r51"),
            "createdAt":"2026-11-23T00:00:00Z"}]}]},
        "review_comments":[],"reviews":[],"comments":[],"conventions":{},
        "ids":{"next_id":52,"items":[]},"scope":{"threads":["T21","T51"],"sources":[]}
    }))
    .unwrap();
    let url = if thread == "T21" {
        POINT.into()
    } else {
        POINT.replace("r21", "r51")
    };
    candidate(
        &data,
        authority,
        "payer",
        Point {
            url: &url,
            thread: Some(thread),
            quote: "Bound the retry loop.",
            written_at: None,
            shared_url: false,
        },
        d,
    )
}

fn reject(by: &str) -> RecordedDecision {
    statement(
        by,
        RecordedDecisionKind::Decision(ProposedDecisionKind::Reject),
        "We won't fix this issue.",
    )
}

#[test]
fn import_only_binding_authority() {
    let d = reject("outsider");
    assert!(attempt(&authority(), 3, Some(40), &d, None, "T21").is_none());
    assert!(
        attempt(&authority(), 3, Some(1), &d, None, "T21")
            .unwrap()
            .binding
    );
}

#[test]
fn member_matched_by_numeric_id_not_login() {
    let d = reject("new-owner-login");
    assert_eq!(
        attempt(&authority(), 3, Some(1), &d, None, "T21")
            .unwrap()
            .user_id,
        "u1"
    );
}

#[test]
fn renamed_login_squatter_has_no_authority() {
    let d = reject("owner");
    for id in [Some(41), None] {
        assert!(attempt(&authority(), 3, id, &d, None, "T21").is_none());
    }
}

const MARKER: &str = "<!-- maintainer-decision v1 kind=reject -->";

#[test]
fn linked_login_binds_only_with_structured_marker() {
    let d = reject("alice-ci");
    for marker in [None, Some("<!-- maintainer-decision v1 kind=todo -->")] {
        assert!(attempt(&authority(), 3, Some(2), &d, marker, "T21").is_none());
    }
    assert_eq!(
        attempt(&authority(), 3, Some(2), &d, Some(MARKER), "T21")
            .unwrap()
            .evidence
            .relay
            .as_deref(),
        Some("alice-ci")
    );
}

#[test]
fn linked_login_decides_for_its_member_only() {
    let mut d = reject("alice-ci");
    d.on_behalf_of = Some("other".into());
    assert!(attempt(&authority(), 3, Some(2), &d, Some(MARKER), "T21").is_none());
    d.on_behalf_of = Some("owner".into());
    assert_eq!(
        attempt(&authority(), 3, Some(2), &d, Some(MARKER), "T21")
            .unwrap()
            .user_id,
        "u1"
    );
}

#[test]
fn login_linked_by_two_members_imports_nothing() {
    let mut a = authority();
    a.add_relay(
        2,
        Principal {
            user: "u2".into(),
            login: "other".into(),
        },
    );
    assert!(attempt(&a, 3, Some(2), &reject("alice-ci"), Some(MARKER), "T21").is_none());
}

#[test]
fn linked_login_matched_by_resolved_id() {
    assert!(
        attempt(
            &authority(),
            3,
            Some(22),
            &reject("alice-ci"),
            Some(MARKER),
            "T21"
        )
        .is_none()
    );
    assert!(
        attempt(
            &authority(),
            3,
            Some(2),
            &reject("alice-ci-renamed"),
            Some(MARKER),
            "T21"
        )
        .is_some()
    );
}

#[test]
fn relay_prose_naming_member_does_not_bind() {
    let mut d = reject("alice-ci");
    d.quote = "Per owner, we won't fix this issue.".into();
    d.on_behalf_of = Some("owner".into());
    assert!(attempt(&authority(), 3, Some(2), &d, None, "T21").is_none());
}

#[test]
fn withdrawal_decides_only_authors_own_items() {
    let d = statement(
        "reviewer",
        RecordedDecisionKind::Withdrawn,
        "I withdraw my finding.",
    );
    assert!(attempt(&authority(), 3, Some(4), &d, None, "T21").is_none());
    let import = attempt(&authority(), 3, Some(3), &d, None, "T21").unwrap();
    assert_eq!(import.kind, DecisionKind::Reject);
    assert!(import.binding && import.author_withdrawal);
}

#[test]
fn import_without_cue_is_not_binding() {
    let mut d = reject("owner");
    d.quote = "Interesting observation.".into();
    assert!(
        !attempt(&authority(), 3, Some(1), &d, None, "T21")
            .unwrap()
            .binding
    );
}

#[test]
fn import_topic_guard_c21_c51() {
    let d = reject("owner");
    assert!(attempt(&authority(), 3, Some(1), &d, None, "T21").is_some());
    assert!(attempt(&authority(), 3, Some(1), &d, None, "T51").is_none());
}

#[tokio::test]
async fn linked_logins_of_another_tenant_do_not_bind() {
    use db::repo::DEFAULT_TENANT;
    use db::repo::users::GithubProfile;

    let state = crate::state::tests::test_state().await;
    db::repo::analyses::testing::seed_tenant(&state.db, "another")
        .await
        .unwrap();
    state
        .db
        .users()
        .upsert_member(
            "another",
            "u-foreign",
            &GithubProfile {
                github_id: 3,
                login: "foreign".into(),
                name: None,
                avatar_url: None,
            },
            Timestamp::now(),
        )
        .await
        .unwrap();
    state
        .db
        .dashboard()
        .save_settings(
            "another",
            "u-foreign",
            &contracts::api::dashboard::DashboardSettings {
                linked_logins: vec!["foreign-ci".into()],
                ..Default::default()
            },
            Timestamp::now(),
        )
        .await
        .unwrap();
    let raw: PrData = serde_json::from_value(json!({
        "manifest":{"repo":"acme/r","pr":12,"head":"abc","base_ref":"main","base_sha":"def",
            "title":"Retry","body":"","pr_author":"alice","maintainers":["foreign"],"bots":[],
            "delegates":["foreign-ci"]},
        "threads":[],"review_comments":[],"reviews":[],"comments":[],"conventions":{},
        "ids":{"next_id":1,"items":[]},"scope":{"threads":[],"sources":[]}
    }))
    .unwrap();
    let mut manifest = raw.manifest;
    let local = Authorities::read(&state, DEFAULT_TENANT, "u-foreign", &mut manifest)
        .await
        .unwrap();
    assert!(manifest.maintainers.is_empty());
    assert!(manifest.delegates.is_empty());
    assert!(attempt(&local, 4, Some(3), &reject("foreign"), Some(MARKER), "T21").is_none());
}

#[tokio::test]
async fn relay_resolution_is_cached_for_twenty_four_hours() {
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let mut config = crate::config::tests::test_config();
    config.github.api_url = url::Url::parse(&format!("{}/api", server.uri())).unwrap();
    let state = AppState::with_queue(config, db::repo::Db::in_memory().await.unwrap());
    let token = SecretString::from("fake-payer-token");
    let mount = |id| {
        Mock::given(method("GET"))
            .and(path("/api/users/alice-ci"))
            .and(header("Authorization", "Bearer fake-payer-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": id})))
            .expect(1)
    };
    mount(2).mount(&server).await;
    for _ in 0..2 {
        assert_eq!(
            state.relay_ids.resolve(&state, &token, "alice-ci").await,
            Some(2)
        );
    }
    server.verify().await;
    server.reset().await;
    state
        .relay_ids
        .0
        .lock()
        .unwrap()
        .get_mut("alice-ci")
        .unwrap()
        .0 -= Duration::from_hours(24);
    mount(3).mount(&server).await;
    assert_eq!(
        state.relay_ids.resolve(&state, &token, "alice-ci").await,
        Some(3)
    );
}
