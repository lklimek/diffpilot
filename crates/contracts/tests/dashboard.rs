//! Dashboard and access-token contract (plan `docs/v1.1-dashboard-plan.md` §E, ADR-019): routes and their auth,
//! wire formats, the one-time token secret, settings defaults and the fixtures of `fixtures/dashboard/`.
#![expect(clippy::expect_used, reason = "test helpers outside #[test] functions")]

use contracts::api::access_tokens::{
    AccessToken, AccessTokenId, AccessTokenSecret, CreateAccessToken, CreatedAccessToken,
    TokenScope,
};
use contracts::api::auth::MeResponse;
use contracts::api::dashboard::{
    DashboardEvent, DashboardSettings, DashboardSettingsView, DashboardSummary, GithubKind,
    ItemCounts, ItemKey, ItemKind, ItemPage, ItemQuery, ItemScore, ItemTarget, PartSource,
    PartStatus, RankingState, RefreshFailureCode, RefreshState, RefreshStatus, ScoreComponent,
    ScoreState, Tier, WorkItem,
};
use contracts::api::settings::{LlmSelection, Settings};
use contracts::common::RepoRef;
use contracts::ids::RefreshId;
use contracts::routes::{self, HttpMethod, Listener, ROUTES, Route, RouteAuth};
use schemars::JsonSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use contracts::time::Timestamp;
use jiff::SignedDuration;

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
    let at = "2026-11-18T10:00:00Z".parse::<Timestamp>().expect("time");
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
            created_at: Timestamp::now(),
            expires_at: Timestamp::now(),
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
            "known_bots": ["thepastaclaw", "Claudius-Maginificent", "DCG-Claude"],
            "include_todos": true, "memcan_extra_projects": [], "schedule_enabled": true,
            "schedule_time": "06:00", "time_zone": null
        })
    );
}

/// A settings document without `known_bots` (an older client) reads the default known bots.
#[test]
fn dashboard_settings_without_known_bots_read_the_defaults() {
    let mut document = serde_json::to_value(DashboardSettings::default()).expect("serialises");
    document
        .as_object_mut()
        .expect("object")
        .remove("known_bots");
    let read: DashboardSettings = serde_json::from_value(document).expect("reads");
    assert_eq!(
        read.known_bots,
        ["thepastaclaw", "Claudius-Maginificent", "DCG-Claude"]
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
        (Some(Tier::P1), Some(GithubKind::Pr), Some(50))
    );
    // `kind` selects pull requests or issues only; TODOs are not a filter value.
    for kind in ["issue", "pr"] {
        assert!(
            serde_json::from_value::<ItemQuery>(json!({ "kind": kind })).is_ok(),
            "{kind}"
        );
    }
    assert!(serde_json::from_value::<ItemQuery>(json!({"kind": "todo"})).is_err());
    // TODOs are listed unless `todos=false`; there is no TODOs-only query.
    assert_eq!(ItemQuery::default().todos, None);
    let without: ItemQuery =
        serde_json::from_value(json!({"kind": "pr", "todos": false})).expect("todos");
    assert_eq!(
        (without.kind, without.todos),
        (Some(GithubKind::Pr), Some(false))
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

/// `ask_effort` is optional on the wire, so older servers and clients keep their JSON.
#[test]
fn llm_selection_ask_effort_is_optional() {
    let selection: LlmSelection = serde_json::from_value(json!({
        "models": [], "efforts": ["low"], "current": {"model": "m", "reasoning_effort": "high"},
        "changed_by": null, "changed_at": null
    }))
    .expect("deserialises without ask_effort");
    assert_eq!(selection.ask_effort, None);
    let with = LlmSelection {
        ask_effort: Some(contracts::job::spec::ReasoningEffort::Medium),
        ..selection
    };
    assert_eq!(
        serde_json::to_value(&with).expect("serialises")["ask_effort"],
        json!("medium")
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
        let expected_target = match (item.kind, &item.repo, item.number) {
            (ItemKind::Pr, Some(repo), Some(number)) => Some(ItemTarget::PullRequest {
                repo: repo.clone(),
                number,
            }),
            (ItemKind::Issue, Some(repo), Some(number)) => Some(ItemTarget::Issue {
                repo: repo.clone(),
                number,
            }),
            _ => None,
        };
        match (expected_target, item.key.target()) {
            (Some(expected), target) => assert_eq!(target, expected, "{at}: key"),
            (None, target) => assert!(matches!(target, ItemTarget::Todo(_)), "{at}: key"),
        }
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
                let uuid = item.key.as_str().trim_start_matches("memcan~");
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
        if let Some(llm) = &item.llm {
            for also in &llm.also_keys {
                let listed = page
                    .items
                    .iter()
                    .any(|other| &other.key == also && other.key != item.key);
                assert!(
                    listed,
                    "{at}: also_keys names {also}, not another item of the page"
                );
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
    let exclusions = snapshot.exclusions.as_ref().expect("exclusions");
    let listed = exclusions.items.as_deref().unwrap_or_default();
    assert_eq!(
        (
            exclusions.total,
            exclusions.counts.iter().map(|c| c.count).sum::<u32>(),
            u32::try_from(listed.len()).expect("count"),
        ),
        (snapshot.excluded, snapshot.excluded, snapshot.excluded)
    );
    assert!(
        listed
            .windows(2)
            .all(|pair| pair[0].reason <= pair[1].reason),
        "listed by reason"
    );
    // Every scanned item is in one place: Needs you, Waiting on others (pull requests) or Excluded.
    let excluded_of = |kind: ItemKind| {
        u32::try_from(listed.iter().filter(|item| item.kind == kind).count()).expect("count")
    };
    let waiting = u32::try_from(snapshot.awaiting.len()).expect("count");
    let counts = snapshot.item_counts;
    assert_eq!(
        (
            snapshot.checked.pr,
            snapshot.checked.issue,
            snapshot.checked.todo
        ),
        (
            counts.pr + waiting + excluded_of(ItemKind::Pr),
            counts.issue + excluded_of(ItemKind::Issue),
            counts.todo + excluded_of(ItemKind::Todo),
        )
    );
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
    let window_start = snapshot.scanned_at - SignedDuration::from_hours(12 * 7 * 24);
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
    let mut statuses = vec![
        ("api/summary.json".to_owned(), latest.clone()),
        ("api/refresh-failed.json".to_owned(), failed.clone()),
    ];
    for rel in ["api/refresh-scanning.json", "api/refresh-ranking.json"] {
        let status: RefreshStatus = typed(rel);
        assert!(status.failure.is_none() && status.percent <= 100, "{rel}");
        let _: RefreshId = status.id;
        statuses.push((rel.to_owned(), status));
    }
    // http-api.md "Refresh states": a ranking exactly on `ranking` (live state) and `finished` (ending state).
    for (rel, status) in &statuses {
        let ranking = status.ranking.as_ref().map(|ranking| ranking.state);
        let live = [
            RankingState::WaitingForAnalysis,
            RankingState::Running,
            RankingState::Paused,
        ];
        match status.state {
            RefreshState::Ranking => assert!(ranking.is_some_and(|r| live.contains(&r)), "{rel}"),
            RefreshState::Finished => assert!(ranking.is_some_and(|r| !live.contains(&r)), "{rel}"),
            _ => assert!(ranking.is_none() && status.scanned_at.is_none(), "{rel}"),
        }
        assert_eq!(
            status.failure.is_some(),
            status.state == RefreshState::Failed,
            "{rel}"
        );
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

    // Every pull request and issue item has its scan facts (`updated_at` = GitHub `updatedAt`); every TODO its
    // MemCan row (`updated_at` not before its creation).
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
                        let created: Timestamp = todo["created_at"]
                            .as_str()
                            .expect("created_at")
                            .parse()
                            .expect("RFC 3339");
                        item.key.as_str() == format!("memcan~{}", todo["id"].as_str().expect("id"))
                            && todo["title"] == json!(item.title)
                            && item.updated_at >= created
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
                    && fact["updatedAt"] == json!(item.updated_at)
                    && fact["title"] == json!(item.title)
            }),
            _ => false,
        };
        assert!(found, "{}: no scan facts", item.id);
    }
}

/// The normative key forms (http-api.md "Item keys"): built and parsed only by `ItemKey`, rejected otherwise.
#[test]
fn item_keys_have_one_normative_form() {
    let repo = RepoRef {
        owner: "acme".to_owned(),
        name: "wallet-ui".to_owned(),
    };
    let uuid = uuid::Uuid::parse_str("3fa91c2e-0000-4000-8000-000000000000").expect("uuid");
    for (target, text) in [
        (
            ItemTarget::PullRequest {
                repo: repo.clone(),
                number: 320,
            },
            "acme~wallet-ui~pr~320",
        ),
        (
            ItemTarget::Issue {
                repo: repo.clone(),
                number: 305,
            },
            "acme~wallet-ui~issue~305",
        ),
        (
            ItemTarget::Todo(uuid),
            "memcan~3fa91c2e-0000-4000-8000-000000000000",
        ),
    ] {
        let key = ItemKey::new(&target).expect("valid target");
        assert_eq!(key.as_str(), text);
        assert_eq!(ItemKey::parse(text), Ok(key.clone()));
        assert_eq!(key.target(), target);
        assert_eq!(serde_json::to_value(&key).expect("serialises"), json!(text));
        assert_eq!(
            serde_json::from_value::<ItemKey>(json!(text)).expect("deserialises"),
            key
        );
        assert!(
            text.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.~".contains(&b)),
            "URL-safe"
        );
    }
    let bad_owner = RepoRef {
        owner: "ac~me".to_owned(),
        name: "x".to_owned(),
    };
    assert!(
        ItemKey::new(&ItemTarget::Issue {
            repo: bad_owner,
            number: 1
        })
        .is_err()
    );
    let mut item = json_file("api/items.json")["items"][0].clone();
    item["key"] = json!("PLAT-1187");
    assert!(
        serde_json::from_value::<WorkItem>(item).is_err(),
        "an item with a display alias as key"
    );
}

/// GitHub names are case-insensitive: one item has one key, owner and repository lowercased.
#[test]
fn item_keys_fold_github_name_case() {
    let mixed = RepoRef {
        owner: "Octo_EMU".to_owned(),
        name: "Wallet.UI".to_owned(),
    };
    let built = ItemKey::new(&ItemTarget::PullRequest {
        repo: mixed,
        number: 7,
    })
    .expect("an Enterprise Managed User owner");
    assert_eq!(built.as_str(), "octo_emu~wallet.ui~pr~7");
    for text in ["Octo_EMU~Wallet.UI~pr~7", "octo_emu~wallet.ui~pr~7"] {
        assert_eq!(ItemKey::parse(text), Ok(built.clone()), "{text}");
        assert_eq!(
            serde_json::from_value::<ItemKey>(json!(text)).expect("deserialises"),
            built
        );
    }
    let ItemTarget::PullRequest { repo, .. } = built.target() else {
        panic!("a pull request key");
    };
    assert_eq!(
        (repo.owner.as_str(), repo.name.as_str()),
        ("octo_emu", "wallet.ui")
    );
}

/// The JSON Schema pattern accepts exactly what `ItemKey::parse` accepts.
#[test]
fn item_key_schema_matches_the_parser() {
    let schema =
        jsonschema::validator_for(&schemars::schema_for!(ItemKey).to_value()).expect("schema");
    let long_repo = "r".repeat(100);
    let too_long_repo = "r".repeat(101);
    let long_owner = "o".repeat(39);
    let too_long_owner = "o".repeat(40);
    let accepted = [
        "acme~wallet-ui~pr~320".to_owned(),
        "Acme~Wallet-UI~issue~1".to_owned(),
        "octo_emu~x~pr~9".to_owned(),
        "acme~.github~issue~3".to_owned(),
        "acme~..x~pr~3".to_owned(),
        "acme~a.b_c-d~pr~3".to_owned(),
        "acme~wallet-ui~pr~4294967295".to_owned(),
        "acme~wallet-ui~pr~999999999".to_owned(),
        "acme~wallet-ui~pr~3999999999".to_owned(),
        format!("{long_owner}~{long_repo}~pr~1"),
        "memcan~3fa91c2e-0000-4000-8000-000000000000".to_owned(),
    ];
    let rejected = [
        String::new(),
        "acme~wallet-ui~pr~0".to_owned(),
        "acme~wallet-ui~pr~0320".to_owned(),
        "acme~wallet-ui~pr~4294967296".to_owned(),
        "acme~wallet-ui~pr~9999999999".to_owned(),
        "acme~wallet-ui~pr~42949672950".to_owned(),
        "acme~wallet-ui~PR~320".to_owned(),
        "acme~wallet-ui~pull~320".to_owned(),
        "acme~wallet ui~pr~320".to_owned(),
        "acme~wallet-ui~pr~320~x".to_owned(),
        "ac~me~wallet-ui~pr~320".to_owned(),
        "ac.me~wallet-ui~pr~320".to_owned(),
        "acme~.~pr~1".to_owned(),
        "acme~..~pr~1".to_owned(),
        "acme_wallet-ui_pr_320".to_owned(),
        format!("{too_long_owner}~x~pr~1"),
        format!("acme~{too_long_repo}~pr~1"),
        "memcan~3FA91C2E-0000-4000-8000-000000000000".to_owned(),
        "memcan~3fa91c2e000040008000000000000000".to_owned(),
        "memcan~not-a-uuid".to_owned(),
        "todo~3fa91c2e-0000-4000-8000-000000000000".to_owned(),
    ];
    for text in &accepted {
        assert!(ItemKey::parse(text).is_ok(), "parser rejects {text}");
        assert!(schema.is_valid(&json!(text)), "schema rejects {text}");
    }
    for text in &rejected {
        assert!(ItemKey::parse(text).is_err(), "parser accepts {text}");
        assert!(
            serde_json::from_value::<ItemKey>(json!(text)).is_err(),
            "{text} deserialised"
        );
        assert!(!schema.is_valid(&json!(text)), "schema accepts {text}");
    }
}

/// LLM-chosen merges are LLM content and carry item keys.
#[test]
fn merged_items_are_llm_text_with_item_keys() {
    let mut item = json_file("api/items.json")["items"][1].clone();
    assert!(item.get("also_ids").is_none() && item.get("also_keys").is_none());
    item["llm"]["also_keys"] = json!(["acme~platform~issue~1187"]);
    let parsed: WorkItem = serde_json::from_value(item.clone()).expect("deserialises");
    let also = &parsed.llm.as_ref().expect("llm").also_keys;
    assert_eq!(
        also,
        &[ItemKey::parse("acme~platform~issue~1187").expect("key")]
    );
    item["llm"]["also_keys"] = json!(["PLAT-1187"]);
    assert!(serde_json::from_value::<WorkItem>(item).is_err());
}

/// Every ranking outcome is a wire value; `cancelled` ends a ranking the user cancelled (refresh `finished`).
#[test]
fn ranking_outcomes_include_cancelled() {
    assert_eq!(
        serde_json::to_value(RankingState::Cancelled).expect("serialises"),
        json!("cancelled")
    );
}
