//! Dashboard and access-token contract (plan `docs/v1.1-dashboard-plan.md` §E, ADR-019): routes and their auth,
//! wire formats, the one-time token secret, settings defaults and the fixtures of `fixtures/dashboard/`.
#![expect(clippy::expect_used, reason = "test helpers outside #[test] functions")]

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{TimeZone, Utc};
use contracts::api::access_tokens::{
    AccessToken, AccessTokenId, AccessTokenSecret, CreateAccessToken, CreatedAccessToken,
    TokenScope,
};
use contracts::api::auth::MeResponse;
use contracts::api::dashboard::{
    DashboardEvent, DashboardSettings, DashboardSettingsView, DashboardSummary, ItemCounts,
    ItemKind, ItemPage, ItemQuery, ItemScore, PartSource, PartStatus, RefreshFailureCode,
    RefreshId, RefreshState, RefreshStatus, ScoreComponent, ScoreState, Tier,
};
use contracts::api::settings::{LlmSelection, Settings};
use contracts::routes::{self, HttpMethod, Listener, ROUTES, Route, RouteAuth};
use schemars::JsonSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

const READ: RouteAuth = RouteAuth::SessionOrToken(TokenScope::Read);

#[test]
fn dashboard_routes_follow_the_plan() {
    use HttpMethod::{Delete, Get, Post, Put};
    use RouteAuth::{Session, SessionCsrf};
    let expected: [(Route, HttpMethod, &str, RouteAuth); 14] = [
        (routes::GET_DASHBOARD, Get, "/api/v1/dashboard", READ),
        (
            routes::LIST_DASHBOARD_ITEMS,
            Get,
            "/api/v1/dashboard/items",
            READ,
        ),
        (
            routes::START_DASHBOARD_REFRESH,
            Post,
            "/api/v1/dashboard/refreshes",
            SessionCsrf,
        ),
        (
            routes::GET_DASHBOARD_REFRESH,
            Get,
            "/api/v1/dashboard/refreshes/{id}",
            READ,
        ),
        (
            routes::CANCEL_DASHBOARD_REFRESH,
            Post,
            "/api/v1/dashboard/refreshes/{id}/cancel",
            SessionCsrf,
        ),
        (
            routes::RANK_DASHBOARD_REFRESH,
            Post,
            "/api/v1/dashboard/refreshes/{id}/rank",
            SessionCsrf,
        ),
        (
            routes::DASHBOARD_REFRESH_EVENTS,
            Get,
            "/api/v1/dashboard/refreshes/{id}/events",
            READ,
        ),
        (
            routes::GET_DASHBOARD_SETTINGS,
            Get,
            "/api/v1/settings/dashboard",
            READ,
        ),
        (
            routes::SET_DASHBOARD_SETTINGS,
            Put,
            "/api/v1/settings/dashboard",
            SessionCsrf,
        ),
        (
            routes::SET_DASHBOARD_TIME_ZONE,
            Put,
            "/api/v1/settings/dashboard/time-zone",
            SessionCsrf,
        ),
        (
            routes::SET_JUDGE_EFFORT,
            Put,
            "/api/v1/settings/llm/judge-effort",
            SessionCsrf,
        ),
        (
            routes::LIST_ACCESS_TOKENS,
            Get,
            "/api/v1/settings/access-tokens",
            Session,
        ),
        (
            routes::CREATE_ACCESS_TOKEN,
            Post,
            "/api/v1/settings/access-tokens",
            SessionCsrf,
        ),
        (
            routes::REVOKE_ACCESS_TOKEN,
            Delete,
            "/api/v1/settings/access-tokens/{id}",
            SessionCsrf,
        ),
    ];
    for (route, method, path, auth) in expected {
        assert_eq!(
            (route.method, route.path, route.auth),
            (method, path, auth),
            "{}",
            route.name
        );
        assert_eq!(route.listener, Listener::Public, "{}", route.name);
        assert!(ROUTES.contains(&route), "{} is not in ROUTES", route.name);
    }
}

/// A personal access token is accepted only on safe reads of the public listener and never lists or mints
/// tokens (ADR-019 §4–§6); routes.ts names the auth `read`.
#[test]
fn tokens_are_accepted_only_on_public_safe_reads() {
    assert_eq!(READ.as_str(), "read");
    assert_eq!(TokenScope::Read.as_str(), "read");
    let token_routes: Vec<&Route> = ROUTES
        .iter()
        .filter(|route| matches!(route.auth, RouteAuth::SessionOrToken(_)))
        .collect();
    assert!(!token_routes.is_empty());
    for route in token_routes {
        assert_eq!(route.method, HttpMethod::Get, "{}", route.path);
        assert_eq!(route.listener, Listener::Public, "{}", route.path);
        assert!(route.path.starts_with("/api/v1/"), "{}", route.path);
        assert!(!route.path.contains("access-tokens"), "{}", route.path);
    }
}

#[test]
fn dashboard_event_name_is_the_tag() {
    let at = Utc
        .with_ymd_and_hms(2026, 11, 18, 10, 0, 0)
        .single()
        .expect("time");
    let heartbeat = DashboardEvent::Heartbeat { at };
    assert_eq!(
        serde_json::to_value(&heartbeat).expect("serialises"),
        json!({"event": "heartbeat", "at": "2026-11-18T10:00:00Z"})
    );
    let status: RefreshStatus = typed("api/refresh-scanning.json");
    for event in [heartbeat, DashboardEvent::Refresh { status }] {
        let json = serde_json::to_value(&event).expect("serialises");
        assert_eq!(json["event"], json!(event.name()));
    }
}

fn schema_text<T: JsonSchema>() -> String {
    schemars::schema_for!(T).to_value().to_string()
}

/// The token secret travels only in the creation response and never shows in `Debug` (ADR-019 §1, T20).
#[test]
fn token_secret_is_only_in_the_creation_response() {
    let secret = "srp_Ab3dE5gH_canary-secret-value";
    let created = CreatedAccessToken {
        token: AccessTokenSecret(secret.to_owned()),
        access_token: AccessToken {
            id: AccessTokenId("Ab3dE5gH".to_owned()),
            name: "agent".to_owned(),
            prefix: "srp_Ab3dE5gH".to_owned(),
            scopes: vec![TokenScope::Read],
            created_at: Utc::now(),
            expires_at: Utc::now(),
            last_used_at: None,
        },
    };
    assert_eq!(
        serde_json::to_value(&created).expect("serialises")["token"],
        json!(secret)
    );
    assert!(
        !format!("{created:?}").contains("canary"),
        "Debug shows the secret"
    );
    assert!(schema_text::<CreatedAccessToken>().contains("AccessTokenSecret"));
    for (name, schema) in [
        ("AccessToken", schema_text::<AccessToken>()),
        ("Vec<AccessToken>", schema_text::<Vec<AccessToken>>()),
        ("DashboardSummary", schema_text::<DashboardSummary>()),
        ("ItemPage", schema_text::<ItemPage>()),
        (
            "DashboardSettingsView",
            schema_text::<DashboardSettingsView>(),
        ),
        ("Settings", schema_text::<Settings>()),
        ("MeResponse", schema_text::<MeResponse>()),
    ] {
        assert!(
            !schema.contains("AccessTokenSecret"),
            "{name} carries a token secret"
        );
        assert!(
            !schema.contains("token_hash"),
            "{name} carries a token hash"
        );
    }
    assert_eq!(CreateAccessToken::EXPIRY_DAYS, [30, 90, 365]);
    assert_eq!(CreateAccessToken::DEFAULT_EXPIRY_DAYS, 90);
    assert_eq!(CreateAccessToken::MAX_LIVE_PER_USER, 10);
}

/// The defaults of plan §F.6; an absent settings row means these.
#[test]
fn dashboard_settings_default_to_plan_f6() {
    assert_eq!(
        serde_json::to_value(DashboardSettings::default()).expect("serialises"),
        json!({
            "linked_logins": [], "repositories": [], "enumerate_owners": [], "lookback_days": 7,
            "ready_label": "ready-for-human", "skip_authors": ["dependabot", "dependabot[bot]"],
            "include_todos": true, "memcan_extra_projects": [], "schedule_enabled": true,
            "schedule_time": "06:00", "time_zone": null
        })
    );
}

/// Components keep the daily-priorities part names, so its archived runs map one to one.
#[test]
fn score_components_are_the_twelve_skill_parts() {
    let names: Vec<Value> = ScoreComponent::ALL
        .iter()
        .map(|component| serde_json::to_value(component).expect("serialises"))
        .collect();
    assert_eq!(
        names,
        [
            "advancement",
            "defect_severity",
            "unblocks",
            "human_waiting",
            "recency",
            "change_impact",
            "completion",
            "change_size",
            "urgency_bonus",
            "blocked_penalty",
            "default_branch_bonus",
            "milestone_bonus",
        ]
        .map(|name| json!(name))
    );
}

#[test]
fn item_query_is_optional_and_bounded() {
    assert_eq!((ItemQuery::DEFAULT_LIMIT, ItemQuery::MAX_LIMIT), (100, 500));
    assert_eq!(
        serde_json::to_value(ItemQuery::default()).expect("serialises"),
        json!({})
    );
    let query: ItemQuery = serde_json::from_value(json!({"tier": "p1", "kind": "pr", "limit": 50}))
        .expect("deserialises");
    assert_eq!(
        (query.tier, query.kind, query.limit),
        (Some(Tier::P1), Some(ItemKind::Pr), Some(50))
    );
}

/// `judge_effort` is optional on the wire, so stored settings and older servers keep their JSON.
#[test]
fn llm_selection_judge_effort_is_optional() {
    let selection: LlmSelection = serde_json::from_value(json!({
        "models": [], "efforts": ["low"], "current": {"model": "m", "reasoning_effort": "high"},
        "changed_by": null, "changed_at": null
    }))
    .expect("deserialises without judge_effort");
    assert_eq!(selection.judge_effort, None);
    assert!(
        serde_json::to_value(&selection)
            .expect("serialises")
            .get("judge_effort")
            .is_none()
    );
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/dashboard")
}

fn json_file(rel: &str) -> Value {
    let path = root().join(rel);
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// The fixture parsed as `T`; it must serialise back to the same JSON (no unknown or defaulted field).
fn typed<T: Serialize + DeserializeOwned>(rel: &str) -> T {
    let value = json_file(rel);
    let typed: T = serde_json::from_value(value.clone()).unwrap_or_else(|e| panic!("{rel}: {e}"));
    assert_eq!(
        serde_json::to_value(&typed).expect("serialises"),
        value,
        "{rel} round trip"
    );
    typed
}

fn tier_of(score: &ItemScore, alarm: bool) -> Tier {
    match score.total_tenths {
        _ if alarm => Tier::P0,
        650.. => Tier::P1,
        350..650 => Tier::P2,
        _ => Tier::P3,
    }
}

/// Items follow the score rules of plan §C: parts are the twelve components, the total is their sum, the tier
/// follows the total (P0 only with an LLM alarm), unjudged items carry named estimates and no LLM text, keys are
/// unique and URL-safe, links are server-built.
fn check_items(rel: &str) -> ItemPage {
    let page: ItemPage = typed(rel);
    let mut keys = HashSet::new();
    for item in &page.items {
        let at = format!("{rel} {}", item.id);
        assert!(keys.insert(item.key.clone()), "{at}: duplicate key");
        assert!(
            item.key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.~".contains(c)),
            "{at}: key is not URL-safe"
        );
        let components: Vec<_> = item.score.parts.iter().map(|part| part.component).collect();
        assert_eq!(components, ScoreComponent::ALL, "{at}: components");
        let sum: i32 = item
            .score
            .parts
            .iter()
            .filter_map(|part| part.points_tenths)
            .sum();
        assert_eq!(item.score.total_tenths, sum, "{at}: total");
        let alarm = item.llm.as_ref().is_some_and(|llm| llm.alarm.is_some());
        assert_eq!(item.score.tier, tier_of(&item.score, alarm), "{at}: tier");
        for part in &item.score.parts {
            assert!(
                !part.basis.is_empty(),
                "{at}: {:?} without basis",
                part.component
            );
            match part.source {
                PartSource::Computed | PartSource::NotApplicable => {
                    assert_eq!(
                        part.status,
                        PartStatus::Computed,
                        "{at}: {:?}",
                        part.component
                    );
                }
                PartSource::Llm => assert!(
                    matches!(
                        part.status,
                        PartStatus::Judged | PartStatus::Inherited | PartStatus::Estimated
                    ),
                    "{at}: {:?}",
                    part.component
                ),
            }
            if part.status == PartStatus::Estimated {
                assert!(
                    part.basis.starts_with("estimate until judged:"),
                    "{at}: estimate basis"
                );
            }
        }
        let has = |status| item.score.parts.iter().any(|part| part.status == status);
        let expected = if has(PartStatus::Estimated) {
            ScoreState::Unjudged
        } else if has(PartStatus::Inherited) {
            ScoreState::Provisional
        } else {
            ScoreState::Final
        };
        assert_eq!(item.score.state, expected, "{at}: state");
        assert_eq!(
            item.llm.is_none(),
            expected == ScoreState::Unjudged,
            "{at}: LLM text"
        );
        if let Some(llm) = &item.llm {
            assert_eq!(
                llm.inherited,
                expected == ScoreState::Provisional,
                "{at}: inherited"
            );
        }
        match item.kind {
            ItemKind::Todo => {
                assert!(item.repo.is_none() && item.number.is_none(), "{at}");
                let uuid = item.key.trim_start_matches("memcan~");
                assert_eq!(
                    item.url.as_deref(),
                    Some(format!("http://memcan.example/ui/tasks/{uuid}").as_str()),
                    "{at}"
                );
            }
            ItemKind::Pr | ItemKind::Issue => {
                let repo = item.repo.as_ref().expect("repository");
                let number = item.number.expect("number");
                let segment = if item.kind == ItemKind::Pr {
                    "pull"
                } else {
                    "issues"
                };
                let url = format!(
                    "https://github.com/{}/{}/{segment}/{number}",
                    repo.owner, repo.name
                );
                assert_eq!(item.url.as_deref(), Some(url.as_str()), "{at}");
            }
        }
        if let Some(review) = &item.review {
            assert_eq!(item.kind, ItemKind::Pr, "{at}: review link on a non-PR");
            assert_eq!(review.analysed_head.0.len(), 40, "{at}");
        }
    }
    page
}

fn counts(items: &ItemPage) -> ItemCounts {
    let mut counts = ItemCounts {
        total: 0,
        pr: 0,
        issue: 0,
        todo: 0,
        p0: 0,
        p1: 0,
        p2: 0,
        p3: 0,
        not_judged: 0,
    };
    for item in &items.items {
        counts.total += 1;
        *match item.kind {
            ItemKind::Pr => &mut counts.pr,
            ItemKind::Issue => &mut counts.issue,
            ItemKind::Todo => &mut counts.todo,
        } += 1;
        *match item.score.tier {
            Tier::P0 => &mut counts.p0,
            Tier::P1 => &mut counts.p1,
            Tier::P2 => &mut counts.p2,
            Tier::P3 => &mut counts.p3,
        } += 1;
        counts.not_judged += u32::from(item.score.state == ScoreState::Unjudged);
    }
    counts
}

#[test]
fn dashboard_fixtures_are_typed_and_consistent() {
    let summary: DashboardSummary = typed("api/summary.json");
    let items = check_items("api/items.json");
    let provisional = check_items("api/items-provisional.json");
    let snapshot = summary.snapshot.as_ref().expect("snapshot");
    assert_eq!(snapshot.item_counts, counts(&items));
    assert_eq!(items.revision, summary.revision);
    let latest = summary.latest_refresh.as_ref().expect("latest refresh");
    assert_eq!(latest.id, snapshot.refresh_id);
    assert_eq!(latest.state, RefreshState::Finished);
    assert!(
        items
            .items
            .iter()
            .all(|item| item.score.state == ScoreState::Final)
    );
    let states: BTreeSet<_> = provisional
        .items
        .iter()
        .map(|item| format!("{:?}", item.score.state))
        .collect();
    assert_eq!(
        states,
        ["Final", "Provisional", "Unjudged"]
            .map(str::to_owned)
            .into()
    );
    let window_start = snapshot.scanned_at - chrono::Duration::weeks(12);
    assert!(
        snapshot
            .history
            .iter()
            .all(|event| event.at >= window_start && event.at <= snapshot.scanned_at)
    );

    let failed: RefreshStatus = typed("api/refresh-failed.json");
    let failure = failed.failure.as_ref().expect("failure");
    assert_eq!(
        (failed.state, failure.code),
        (RefreshState::Failed, RefreshFailureCode::GithubRateLimited)
    );
    assert!(failure.retry_at.is_some());
    for rel in ["api/refresh-scanning.json", "api/refresh-ranking.json"] {
        let status: RefreshStatus = typed(rel);
        assert!(status.failure.is_none() && status.percent <= 100, "{rel}");
        let _: RefreshId = status.id;
    }

    let view: DashboardSettingsView = typed("api/settings.json");
    assert_eq!(view.defaults, DashboardSettings::default());
    let tokens: Vec<AccessToken> = typed("api/access-tokens.json");
    let created: CreatedAccessToken = typed("api/access-token-created.json");
    assert!(
        tokens
            .iter()
            .any(|token| token.id == created.access_token.id)
    );
    for token in tokens.iter().chain([&created.access_token]) {
        assert_eq!(token.prefix, format!("srp_{}", token.id), "{}", token.id);
        assert_eq!(token.id.0.len(), 8);
    }
    assert!(
        created
            .token
            .0
            .starts_with(&format!("{}_fake-", created.access_token.prefix))
    );

    // Every pull request and issue item has its scan facts; every TODO its MemCan row.
    let scan = json_file("scan/scan.json");
    let todos = json_file("scan/todos.json");
    for item in &items.items {
        let found = match (item.kind, &item.repo, item.number) {
            (ItemKind::Todo, ..) => todos.as_object().expect("projects").values().any(|answer| {
                answer["todos"]
                    .as_array()
                    .expect("todos")
                    .iter()
                    .any(|todo| {
                        item.key == format!("memcan~{}", todo["id"].as_str().expect("id"))
                            && todo["title"] == json!(item.title)
                    })
            }),
            (kind, Some(repo), Some(number)) => scan[if kind == ItemKind::Pr {
                "prs"
            } else {
                "issues"
            }]
            .as_array()
            .expect("facts")
            .iter()
            .any(|fact| {
                fact["repo"] == json!(format!("{}/{}", repo.owner, repo.name))
                    && fact["number"] == json!(number)
                    && fact["title"] == json!(item.title)
            }),
            _ => false,
        };
        assert!(found, "{}: no scan facts", item.id);
    }
}
