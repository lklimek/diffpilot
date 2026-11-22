//! Review-loop contract (ADR-023, `docs/review-workflow-plan.md` §6, §7): routes and their auth, the execute
//! scope, wire forms, masked secrets and the push payload's closed field set.
#![expect(clippy::expect_used, reason = "test helpers outside #[test] functions")]

use std::collections::BTreeSet;

use contracts::api::access_tokens::TokenScope;
use contracts::api::review_loop::{
    AgentPushPayload, DecideRequest, Decision, DecisionBatchRequest, DecisionKind, DecisionTarget,
    DecisionVia, FeedEvent, Gate, NotificationChannel, NotificationSettings, Override, PushTarget,
    PutChannelRequest, PutPushTargetRequest, SecretChange, WebhookPayload,
};
use contracts::common::{PrRef, RepoRef};
use contracts::ids::{CommitSha, DecisionId};
use contracts::routes::{self, HttpMethod, Listener, ROUTES, Route, RouteAuth};
use contracts::time::Timestamp;
use schemars::JsonSchema;
use serde_json::{Value, json};
use uuid::Uuid;

const READ: RouteAuth = RouteAuth::SessionOrToken(TokenScope::Read);
const EXECUTE: RouteAuth = RouteAuth::TokenOnly(TokenScope::ReviewLoopExecute);
const PR: &str = "/api/v1/prs/{owner}/{name}/{number}";

#[test]
fn review_loop_routes_follow_the_plan() {
    use HttpMethod::{Delete, Get, Post, Put};
    use RouteAuth::{Session, SessionCsrf};
    let pr = |rest: &str| format!("{PR}{rest}");
    let expected: Vec<(Route, HttpMethod, String, RouteAuth)> = vec![
        (routes::GET_PR_PAGE, Get, pr(""), READ),
        (routes::GET_COMMENTS, Get, pr("/comments"), READ),
        (
            routes::READ_COMMENTS,
            Post,
            pr("/comments/read"),
            SessionCsrf,
        ),
        (routes::LIST_COMMENT_ITEMS, Get, pr("/comments/items"), READ),
        (
            routes::GET_COMMENT_CONTEXT,
            Get,
            pr("/comments/items/{item}/context"),
            READ,
        ),
        (
            routes::ASSESS_COMMENT,
            Post,
            pr("/comments/items/{item}/assess"),
            SessionCsrf,
        ),
        (
            routes::DECIDE_COMMENT,
            Post,
            pr("/comments/items/{item}/decisions"),
            SessionCsrf,
        ),
        (
            routes::DECIDE_COMMENT_BATCH,
            Post,
            pr("/comments/decision-batches"),
            SessionCsrf,
        ),
        (
            routes::UNDO_COMMENT_DECISION,
            Post,
            pr("/comments/items/{item}/undo"),
            SessionCsrf,
        ),
        (
            routes::REOPEN_COMMENT_ITEM,
            Post,
            pr("/comments/items/{item}/reopen"),
            SessionCsrf,
        ),
        (
            routes::SKIP_COMMENT_ITEM,
            Post,
            pr("/comments/items/{item}/skip"),
            SessionCsrf,
        ),
        (
            routes::MARK_COMMENT_GATE,
            Post,
            pr("/comments/items/{item}/gates"),
            SessionCsrf,
        ),
        (
            routes::PREVIEW_COMMENT_EFFECTS,
            Post,
            pr("/comments/effects-preview"),
            SessionCsrf,
        ),
        (
            routes::GET_DECIDE_REST,
            Get,
            pr("/comments/decide-rest"),
            READ,
        ),
        (routes::GET_COMMENT_LOG, Get, pr("/comments/log"), READ),
        (
            routes::LIST_CARRY_OVERS,
            Get,
            "/api/v1/comments/carry-overs".into(),
            READ,
        ),
        (routes::WATCH_PR, Put, pr("/watch"), SessionCsrf),
        (routes::UNWATCH_PR, Delete, pr("/watch"), SessionCsrf),
        (routes::LIST_WATCHES, Get, "/api/v1/watches".into(), READ),
        (routes::USER_EVENTS, Get, "/api/v1/events".into(), READ),
        (
            routes::GET_NOTIFICATION_SETTINGS,
            Get,
            "/api/v1/settings/notifications".into(),
            Session,
        ),
        (
            routes::PUT_NOTIFICATION_CHANNEL,
            Put,
            "/api/v1/settings/notifications/channels/{kind}".into(),
            SessionCsrf,
        ),
        (
            routes::DELETE_NOTIFICATION_CHANNEL,
            Delete,
            "/api/v1/settings/notifications/channels/{kind}".into(),
            SessionCsrf,
        ),
        (
            routes::TEST_NOTIFICATION_CHANNEL,
            Post,
            "/api/v1/settings/notifications/channels/{kind}/test".into(),
            SessionCsrf,
        ),
        (
            routes::GET_DECISION_FEED,
            Get,
            "/api/v1/decision-feed".into(),
            READ,
        ),
        (
            routes::GET_FEED_DECISION,
            Get,
            "/api/v1/decision-feed/decisions/{id}".into(),
            READ,
        ),
        (
            routes::DECISION_FEED_EVENTS,
            Get,
            "/api/v1/decision-feed/events".into(),
            READ,
        ),
        (
            routes::CLAIM_DECISION,
            Post,
            "/api/v1/decision-feed/decisions/{id}/claim".into(),
            EXECUTE,
        ),
        (
            routes::REPORT_EFFECT,
            Post,
            "/api/v1/decision-feed/effects/{id}/reports".into(),
            EXECUTE,
        ),
        (
            routes::PUT_PUSH_TARGET,
            Put,
            "/api/v1/decision-feed/push-target".into(),
            EXECUTE,
        ),
        (
            routes::DELETE_PUSH_TARGET,
            Delete,
            "/api/v1/decision-feed/push-target".into(),
            EXECUTE,
        ),
    ];
    for (route, method, path, auth) in expected {
        assert_eq!(
            (route.method, route.path, route.auth),
            (method, path.as_str(), auth),
            "{}",
            route.name
        );
        assert_eq!(route.listener, Listener::Public, "{}", route.name);
        assert!(ROUTES.contains(&route), "{} is not in ROUTES", route.name);
    }
}

/// ADR-019 amendment, T22, T25: the execute scope opens only the feed's claim, report and push-target routes,
/// never a session route, and no decision route accepts it.
#[test]
fn execute_scope_is_only_on_feed_write_routes() {
    let token_only: BTreeSet<&str> = ROUTES
        .iter()
        .filter(|route| matches!(route.auth, RouteAuth::TokenOnly(_)))
        .map(|route| route.name)
        .collect();
    let expected: BTreeSet<&str> = [
        routes::CLAIM_DECISION,
        routes::REPORT_EFFECT,
        routes::PUT_PUSH_TARGET,
        routes::DELETE_PUSH_TARGET,
    ]
    .iter()
    .map(|route| route.name)
    .collect();
    assert_eq!(token_only, expected);
    for route in ROUTES
        .iter()
        .filter(|route| matches!(route.auth, RouteAuth::TokenOnly(_)))
    {
        assert_eq!(route.auth, EXECUTE, "{}", route.path);
        assert!(
            route.path.starts_with("/api/v1/decision-feed/"),
            "{}",
            route.path
        );
    }
    for route in [
        routes::DECIDE_COMMENT,
        routes::DECIDE_COMMENT_BATCH,
        routes::ASSESS_COMMENT,
        routes::READ_COMMENTS,
        routes::WATCH_PR,
        routes::UNWATCH_PR,
        routes::UNDO_COMMENT_DECISION,
        routes::REOPEN_COMMENT_ITEM,
        routes::SKIP_COMMENT_ITEM,
        routes::MARK_COMMENT_GATE,
    ] {
        assert_eq!(route.auth, RouteAuth::SessionCsrf, "{}", route.name);
    }
    assert_eq!(
        TokenScope::ReviewLoopExecute.as_str(),
        "review_loop_execute"
    );
    assert_eq!(
        serde_json::to_value(TokenScope::ReviewLoopExecute).expect("serialises"),
        json!("review_loop_execute")
    );
    assert_ne!(READ.as_str(), EXECUTE.as_str());
}

fn decision() -> Decision {
    Decision {
        id: DecisionId(Uuid::nil()),
        item_no: 14,
        kind: DecisionKind::Reject,
        reason: Some("The lines are not part of this change.".into()),
        target: None,
        r#override: Some(Override::BlockerGate { gate: Gate::Funds }),
        resolve_ticked: true,
        reply_text: None,
        comment: Some("keep the test small".into()),
        recommendation: None,
        via: DecisionVia::Question,
        carried_from: None,
        supersedes: None,
        head: CommitSha("52d2da140191a9d47becf6a446310be803c007fc".into()),
        decided_by: "alice".into(),
        decided_at: "2026-11-22T10:00:00Z".parse::<Timestamp>().expect("time"),
        undone_at: None,
        effects: vec![],
    }
}

/// D8 is a reject with a `blocker_gate` override (RQ6); gate ids keep their severity names; the `override` key
/// has no raw-identifier prefix.
#[test]
fn decision_wire_form() {
    let json = serde_json::to_value(decision()).expect("serialises");
    assert_eq!(
        json["override"],
        json!({"type": "blocker_gate", "gate": "G-FUNDS"})
    );
    assert_eq!(json["kind"], json!("reject"));
    assert_eq!(json["via"], json!("question"));
    assert!(json.get("r#override").is_none());
    assert!(json.get("undone_at").is_none(), "omitted while it stands");
    let target = DecisionTarget::UpstreamIssue { url: None };
    assert_eq!(
        serde_json::to_value(target).expect("serialises"),
        json!({"type": "upstream_issue", "url": null})
    );
}

/// RC9 rows carry the item number beside the fields of RC8 (flattened).
#[test]
fn batch_rows_flatten_the_decision_fields() {
    let body = json!({"via": "table", "rows": [{
        "item_no": 3, "kind": "fix_here", "reason": null, "target": null, "override": null,
        "resolve_ticked": true, "reply_text": null, "comment": null, "item_revision": 2
    }]});
    let batch: DecisionBatchRequest = serde_json::from_value(body.clone()).expect("parses");
    assert_eq!(batch.rows[0].item_no, 3);
    let DecideRequest {
        kind,
        item_revision,
        ..
    } = batch.rows[0].decision.clone();
    assert_eq!((kind, item_revision), (DecisionKind::FixHere, 2));
    assert_eq!(serde_json::to_value(&batch).expect("serialises"), body);
}

#[test]
fn feed_event_name_is_the_tag() {
    let event = FeedEvent::DecisionAvailable {
        decision: DecisionId(Uuid::nil()),
        pr: PrRef {
            repo: RepoRef {
                owner: "acme".into(),
                name: "node-client".into(),
            },
            number: 482,
        },
        fetch: "/api/v1/decision-feed/decisions/00000000-0000-0000-0000-000000000000".into(),
    };
    let json = serde_json::to_value(event).expect("serialises");
    assert_eq!(json["event"], json!("decision_available"));
}

fn property_names<T: JsonSchema>() -> BTreeSet<String> {
    let schema = serde_json::to_value(schemars::schema_for!(T)).expect("schema");
    let mut names = BTreeSet::new();
    collect_properties(&schema, &mut names);
    names
}

fn collect_properties(node: &Value, names: &mut BTreeSet<String>) {
    match node {
        Value::Object(map) => {
            if let Some(Value::Object(properties)) = map.get("properties") {
                names.extend(properties.keys().cloned());
            }
            map.values()
                .for_each(|value| collect_properties(value, names));
        }
        Value::Array(values) => values
            .iter()
            .for_each(|value| collect_properties(value, names)),
        _ => {}
    }
}

/// T28: responses about channels and push targets never carry a secret, only its last four characters.
#[test]
fn channel_responses_carry_no_secret_field() {
    for names in [
        property_names::<NotificationSettings>(),
        property_names::<NotificationChannel>(),
        property_names::<PushTarget>(),
    ] {
        assert!(names.contains("secret_last4"), "{names:?}");
        assert!(!names.contains("secret"), "{names:?}");
        assert!(
            !names.iter().any(|name| name.contains("token")),
            "{names:?}"
        );
    }
}

/// T28, plan §5.5: an agent push message carries ids and a fetch link only; a webhook carries the short text.
#[test]
fn push_payloads_have_a_fixed_shape() {
    let agent: BTreeSet<String> = ["v", "event", "decision_id", "pr", "fetch"]
        .map(String::from)
        .into();
    assert_eq!(property_names::<AgentPushPayload>(), agent);
    let webhook: BTreeSet<String> = ["v", "event", "text", "url", "pr", "at"]
        .map(String::from)
        .into();
    assert_eq!(property_names::<WebhookPayload>(), webhook);
}

/// T28: a save names what happens to the stored secret (absent keeps it, `remove` deletes it, never an empty
/// string), and `Debug` never shows a new secret.
#[test]
fn secret_change_is_explicit() {
    let channel = |secret: Option<Value>| {
        let mut body =
            json!({"url": "https://ntfy.sh", "topic": "t", "events": [], "enabled": true});
        if let Some(secret) = secret {
            body["secret"] = secret;
        }
        serde_json::from_value::<PutChannelRequest>(body).expect("request")
    };
    assert_eq!(channel(None).secret, SecretChange::Keep);
    assert_eq!(
        channel(Some(json!({"action": "keep"}))).secret,
        SecretChange::Keep
    );
    assert_eq!(
        channel(Some(json!({"action": "remove"}))).secret,
        SecretChange::Remove
    );
    let replaced = channel(Some(
        json!({"action": "replace", "value": "tk_live_secret"}),
    ));
    assert_eq!(
        replaced.secret,
        SecretChange::Replace {
            value: "tk_live_secret".into()
        }
    );
    assert!(!format!("{replaced:?}").contains("tk_live_secret"));
    assert!(
        serde_json::from_value::<PutChannelRequest>(json!({"url": "u", "topic": null,
        "secret": "", "events": [], "enabled": true}))
        .is_err()
    );
    let target: PutPushTargetRequest = serde_json::from_value(
        json!({"kind": "webhook", "url": "http://homeassistant.lan/api/webhook/x", "topic": null}),
    )
    .expect("push target");
    assert_eq!(target.secret, SecretChange::Keep);
}

/// T28: a job's spec has no place for a notification channel, push target or their secrets; the runner never
/// sees them.
#[test]
fn job_spec_has_no_channel_secret() {
    let names = property_names::<contracts::job::spec::JobSpec>();
    for forbidden in [
        "channel",
        "push",
        "ntfy",
        "webhook",
        "topic",
        "notification",
    ] {
        assert!(
            !names.iter().any(|name| name.contains(forbidden)),
            "{forbidden}: {names:?}"
        );
    }
}
