//! HTTP API contract: error model, routes, wire formats, derivations, no floats (plan §5.1, §5.3).

use std::collections::HashSet;

use contracts::agent::analyse_flow::AgentAnnotation;
use contracts::agent::analyse_flow::{Assessment, FlowIntroduction};
use contracts::agent::ask::AskOutput;
use contracts::agent::pr_context::PrContextOutput;
use contracts::api::access_tokens::{CreateAccessToken, CreatedAccessToken};
use contracts::api::analyses::{
    AnalysisStage, AnalysisState, AnalysisSummary, CreateAnalysisRequest, FlowRef, StageFlow,
    StageKind,
};
use contracts::api::auth::MeResponse;
use contracts::api::code::{CommentScope, DeletedBlock, DiffResponse, LinesResponse, RangeQuery};
use contracts::api::comments::{
    CreateCommentRequest, CreateCommentResponse, PendingReview, UpdateCommentRequest,
};
use contracts::api::dashboard::{
    DashboardEvent, DashboardSettingsView, DashboardSummary, ItemPage, ItemQuery,
};
use contracts::api::error::{ApiError, ApiErrorCode};
use contracts::api::events::AnalysisEvent;
use contracts::api::feedback::{
    Feedback, FeedbackRequest, FeedbackTarget, FeedbackValue, UserFeedbackValue,
};
use contracts::api::files::{
    FileGroup, FileHunk, FileViewedState, FilesResponse, HunkPart, PartOwner, SetViewedRequest,
    ViewedStatus,
};
use contracts::api::pr::{PrSummary, ResolvePrResponse};
use contracts::api::questions::{AnchoredAskRequest, AskAnchor, AskQuestion, AskRequest, AskState};
use contracts::api::review::{BlockRef, QuietRowRef, ReviewState};
use contracts::api::settings::{AppInstallation, RepoSettings, Settings};
use contracts::api::tour::{
    Annotation, CiState, CiStatus, Counters, Flow, FlowFacts, FlowGraph, FlowGroup, FlowLlm,
    GraphEdge, GraphNode, PrContextBlock, PrContextFacts, ProjectProfileStatus, QuietRows,
    SourceCheck, Step, StepFacts, StepLlm, ThreadsStatus, Tour,
};
use contracts::common::{
    AnnotationType, Basis, Blocking, Classification, Evidence, FileRanges, Flag, FlagLevel,
    FragmentKind, LineRange, LineSide, LlmLanguage, NodeKind, NotAnalysedCategory, Origin, RepoRef,
    RiskBand,
};
use contracts::ids::{
    AnalysisId, AnnotationId, CommentItemId, CommitSha, FlowId, QuestionId, StepId,
};
use contracts::routes::{HttpMethod, Listener, ROUTES, RouteAuth};
use contracts::time::Timestamp;
use schemars::JsonSchema;
use serde_json::{Value, json};

fn summary() -> AnalysisSummary {
    AnalysisSummary {
        review_ready: None,
        background_analysis: None,
        models_used: None,
        usage: None,
        progress: None,
        has_my_state: None,
        id: AnalysisId::new_random(),
        pr: PrSummary {
            repo: RepoRef {
                owner: "acme".to_owned(),
                name: "node-client".to_owned(),
            },
            number: 482,
            title: "Retry timed-out RPC calls".to_owned(),
            author: "alice".to_owned(),
            html_url: "https://github.com/acme/node-client/pull/482".to_owned(),
            base_sha: CommitSha("1".repeat(40)),
            head_sha: CommitSha("9".repeat(40)),
        },
        language: LlmLanguage::En,
        state: AnalysisState::Finished { total: 1 },
        created_at: Timestamp::now(),
        finished_at: Some(Timestamp::now()),
    }
}

fn tour() -> Tour {
    let step_id = StepId("f1.1".to_owned());
    Tour {
        analysis: summary(),
        pr_context: PrContextBlock {
            facts: PrContextFacts {
                related_work_links: None,
                memcan_status: None,
                ci: CiStatus {
                    state: CiState::Passing,
                    check_runs: None,
                    statuses: None,
                    total: 6,
                    failing: vec![],
                },
                threads_status: ThreadsStatus::Checked,
                counters: Counters {
                    lines_added_to_tests: None,
                    lines_removed_from_tests: 12,
                    dependencies_changed: vec!["tokio".to_owned()],
                    dependency_files_not_checked: vec!["package-lock.json".to_owned()],
                },
                project_profile: ProjectProfileStatus {
                    sources: vec![],
                    memcan: SourceCheck::NotChecked,
                },
                agent_instruction_files: vec!["AGENTS.md".to_owned()],
                related_work_checked: true,
                flow_count: 1,
                step_count: 1,
            },
            llm: PrContextOutput {
                problem: "p".to_owned(),
                why_needed: "w".to_owned(),
                what_it_gives: "g".to_owned(),
                related_work: vec![],
                fit_questions: vec![],
            },
        },
        flows: vec![Flow {
            analysis_status: None,
            planned: None,
            id: FlowId("f1".to_owned()),
            position: 1,
            group: FlowGroup::Behaviour,
            facts: FlowFacts {
                files: vec!["src/rpc/client.rs".to_owned()],
                step_count: 1,
            },
            llm: FlowLlm {
                title: "Retry".to_owned(),
                classification: Classification::Behavioural,
                risk: Some(RiskBand::High),
                introduction: FlowIntroduction {
                    problem_before: "b".to_owned(),
                    change: "c".to_owned(),
                    example_before_after: "e".to_owned(),
                },
                depends_on: vec![],
                pattern_repetitions: vec![],
            },
            graph: FlowGraph {
                nodes: vec![GraphNode {
                    step_id: step_id.clone(),
                    number: 1,
                    kind: NodeKind::Added,
                    label: "Retry allowed?".to_owned(),
                }],
                edges: vec![GraphEdge {
                    from: step_id.clone(),
                    to: step_id.clone(),
                    label: Some("loop".to_owned()),
                }],
            },
            steps: vec![Step {
                id: step_id,
                number: 1,
                facts: StepFacts {
                    leading_context: None,
                    file: "src/rpc/client.rs".to_owned(),
                    side: LineSide::Head,
                    range: LineRange {
                        start_line: 99,
                        end_line: 99,
                    },
                    github_url: "https://github.com/acme/node-client/blob/9/src/rpc/client.rs#L99"
                        .to_owned(),
                    changed_lines: vec![LineRange {
                        start_line: 99,
                        end_line: 99,
                    }],
                    moved: None,
                    call_hierarchy: None,
                    thread_ids: vec![],
                },
                llm: StepLlm {
                    title: "Retry allowed?".to_owned(),
                    kind: FragmentKind::Added,
                    symbol: None,
                    called_by: "step 3".to_owned(),
                    decides: "retry".to_owned(),
                    next: "step 6".to_owned(),
                    why: "stop rule".to_owned(),
                    annotations: vec![Annotation {
                        id: AnnotationId("f1.1.a1".to_owned()),
                        kind: AnnotationType::Flag,
                        basis: Basis::Inference,
                        range: LineRange {
                            start_line: 99,
                            end_line: 99,
                        },
                        was: String::new(),
                        is: String::new(),
                        text: Some("Potential double broadcast:".to_owned()),
                        evidence: vec![],
                        flag: Some(Flag {
                            level: FlagLevel::Red,
                            scenario: vec![
                                "The node accepts broadcast_tx but its reply arrives late."
                                    .to_owned(),
                                "The client retries; the payment is sent twice.".to_owned(),
                            ],
                            not_checked: "Not reproduced against a node.".to_owned(),
                        }),
                    }],
                    assessment: Some(Assessment {
                        headline: Some("h".to_owned()),
                        risk: RiskBand::High,
                        reason: "r".to_owned(),
                        basis_note: Some("Read: 4 files".to_owned()),
                        not_checked: Some("node".to_owned()),
                        origin: Some(Origin::Introduced),
                        blocking: Some(Blocking::BlocksThisPr),
                        smallest_correction: None,
                    }),
                    moved_from: None,
                    impact: vec![],
                },
            }],
        }],
        quiet: QuietRows {
            unexplained: vec![],
            moves: vec![],
            not_analysed: vec![],
        },
        threads: vec![],
    }
}

#[test]
fn tour_round_trips() {
    let tour = tour();
    let json = serde_json::to_value(&tour).expect("serialises");
    assert_eq!(
        serde_json::from_value::<Tour>(json.clone()).expect("deserialises"),
        tour
    );
    let annotation = &json["flows"][0]["steps"][0]["llm"]["annotations"][0];
    assert_eq!(annotation["type"], json!("flag"));
    assert_eq!(annotation["flag"]["level"], json!("red"));
    // Other annotation types carry no flag field.
    let mut note = tour.flows[0].steps[0].llm.annotations[0].clone();
    note.kind = AnnotationType::Note;
    note.flag = None;
    assert!(
        serde_json::to_value(&note)
            .expect("note")
            .get("flag")
            .is_none()
    );
}

#[test]
fn error_codes_map_to_statuses() {
    let expected = [
        ("configuration", 500),
        ("unauthenticated", 401),
        ("login_not_allowed", 403),
        ("csrf_rejected", 403),
        ("not_found", 404),
        ("invalid_request", 400),
        ("not_a_pr_url", 400),
        ("private_repository", 422),
        ("app_not_installed", 422),
        ("analysis_not_finished", 409),
        ("conflict", 409),
        ("pr_head_changed", 409),
        ("position_rejected", 422),
        ("comment_published", 409),
        ("range_too_large", 400),
        ("not_rendered", 422),
        ("github_unavailable", 502),
        ("internal", 500),
        ("not_implemented", 501),
        ("unavailable", 503),
        ("refresh_running", 409),
        ("github_rate_limited", 429),
        ("invalid_dashboard_settings", 422),
        ("cursor_expired", 409),
        ("insufficient_scope", 403),
        ("token_not_accepted", 403),
        ("token_limit_reached", 409),
        ("rate_limited", 429),
        ("github_refused", 422),
        ("item_changed", 409),
        ("claimed", 409),
        ("lease_expired", 409),
        ("capability_missing", 422),
        ("watch_limit_reached", 409),
        ("scheme_not_allowed", 422),
        ("secret_required", 422),
    ];
    assert_eq!(ApiErrorCode::ALL.len(), expected.len());
    for (code, (name, status)) in ApiErrorCode::ALL.into_iter().zip(expected) {
        assert_eq!(serde_json::to_value(code).expect("serialises"), json!(name));
        assert_eq!(code.http_status(), status, "{name}");
    }
    let error = ApiError {
        code: ApiErrorCode::PrHeadChanged,
        message: "The PR changed; analyse it again.".to_owned(),
    };
    assert_eq!(
        serde_json::to_value(error).expect("serialises"),
        json!({"code": "pr_head_changed", "message": "The PR changed; analyse it again."})
    );
}

#[test]
fn routes_are_unique_and_split_by_listener() {
    let mut endpoints = HashSet::new();
    let mut names = HashSet::new();
    for route in ROUTES {
        assert!(
            endpoints.insert((route.method, route.path)),
            "duplicate {:?} {}",
            route.method,
            route.path
        );
        assert!(names.insert(route.name), "duplicate name {}", route.name);
        let runner_auth = matches!(route.auth, RouteAuth::RunnerSecret | RouteAuth::JobToken);
        match route.listener {
            Listener::Webhook => {
                assert_eq!(route.path, "/webhooks/github");
                assert_eq!(route.method, HttpMethod::Post);
                assert_eq!(route.auth, RouteAuth::GithubSignature);
            }
            Listener::Runner => {
                assert!(route.path.starts_with("/runner/v1/"), "{}", route.path);
                assert!(runner_auth, "{}", route.path);
            }
            Listener::Public => {
                assert!(!route.path.starts_with("/runner"), "{}", route.path);
                assert!(!runner_auth, "{}", route.path);
                assert!(
                    route.path.starts_with("/api/v1/")
                        || route.path.starts_with("/auth/")
                        || route.path == "/healthz",
                    "{}",
                    route.path
                );
            }
        }
    }
}

/// Browser routes: every mutation needs the session's CSRF token. Token-only routes (ADR-023 feed writes) take no
/// session and are covered by `execute_scope_is_only_on_feed_write_routes`.
#[test]
fn mutating_browser_routes_require_csrf() {
    for route in ROUTES.iter().filter(|route| {
        route.listener == Listener::Public && !matches!(route.auth, RouteAuth::TokenOnly(_))
    }) {
        let mutating = route.method != HttpMethod::Get;
        assert_eq!(
            mutating,
            route.auth == RouteAuth::SessionCsrf,
            "{} {}",
            route.method.as_str(),
            route.path
        );
    }
}

#[test]
fn flow_groups_follow_classification() {
    assert_eq!(
        FlowGroup::from(Classification::RefactorPure),
        FlowGroup::LlmClassifiedRefactor
    );
    assert_eq!(
        FlowGroup::from(Classification::Rename),
        FlowGroup::LlmClassifiedRefactor
    );
    assert_eq!(
        FlowGroup::from(Classification::Behavioural),
        FlowGroup::Behaviour
    );
    assert_eq!(FlowGroup::from(Classification::Move), FlowGroup::Behaviour);
    assert!(FlowGroup::Behaviour < FlowGroup::LlmClassifiedRefactor);
    assert!(FlowGroup::LlmClassifiedRefactor < FlowGroup::LlmClassifiedTests);
}

#[test]
fn node_kinds_follow_step_kinds() {
    let expected = [
        (FragmentKind::Added, NodeKind::Added),
        (FragmentKind::Context, NodeKind::Unchanged),
        (FragmentKind::Modified, NodeKind::Changed),
        (FragmentKind::Moved, NodeKind::Changed),
        (FragmentKind::Renamed, NodeKind::Changed),
        (FragmentKind::DeletedSite, NodeKind::Changed),
    ];
    for (kind, node) in expected {
        assert_eq!(NodeKind::from(kind), node, "{kind:?}");
    }
}

fn stage_started() -> Timestamp {
    "2026-11-17T09:30:00Z"
        .parse::<Timestamp>()
        .unwrap_or_default()
}

#[test]
fn tagged_wire_formats() {
    let cases = [
        (
            serde_json::to_value(AnalysisState::Running {
                done: 2,
                total: 5,
                idle: false,
                stage: None,
            }),
            json!({"state": "running", "done": 2, "total": 5, "idle": false}),
        ),
        (
            serde_json::to_value(AnalysisState::Planning { stage: None }),
            json!({"state": "planning"}),
        ),
        (
            serde_json::to_value(AnalysisState::Planning {
                stage: Some(AnalysisStage {
                    part: None,
                    kind: StageKind::Plan,
                    flow: None,
                    started_at: None,
                }),
            }),
            json!({"state": "planning", "stage": {"kind": "plan", "started_at": null}}),
        ),
        (
            serde_json::to_value(AnalysisState::Running {
                done: 1,
                total: 5,
                idle: true,
                stage: Some(AnalysisStage {
                    part: None,
                    kind: StageKind::Flow,
                    flow: Some(StageFlow {
                        index: 2,
                        title: "t".to_owned(),
                    }),
                    started_at: Some(stage_started()),
                }),
            }),
            json!({"state": "running", "done": 1, "total": 5, "idle": true, "stage": {
                "kind": "flow", "flow": {"index": 2, "title": "t"}, "started_at": "2026-11-17T09:30:00Z"}}),
        ),
        (
            serde_json::to_value(AnalysisEvent::Heartbeat {
                at: stage_started(),
            }),
            json!({"event": "heartbeat", "at": "2026-11-17T09:30:00Z"}),
        ),
        (
            serde_json::to_value(AnalysisState::Failed {
                reason: None,
                flow: Some(FlowRef {
                    id: FlowId("f2".to_owned()),
                    title: "t".to_owned(),
                }),
                message: "m".to_owned(),
            }),
            json!({"state": "failed", "flow": {"id": "f2", "title": "t"}, "message": "m"}),
        ),
        (
            serde_json::to_value(BlockRef::Quiet {
                row: QuietRowRef::NotAnalysed {
                    category: NotAnalysedCategory::Tests,
                },
            }),
            json!({"block": "quiet", "row": {"row": "not_analysed", "category": "tests"}}),
        ),
        (
            serde_json::to_value(FeedbackTarget::Line {
                path: "a.rs".to_owned(),
                side: LineSide::Base,
                line: 3,
            }),
            json!({"target": "line", "path": "a.rs", "side": "base", "line": 3}),
        ),
        (
            serde_json::to_value(CommentScope::InDiff {
                line_thread_ranges: vec![LineRange {
                    start_line: 95,
                    end_line: 104,
                }],
            }),
            json!({"scope": "in_diff", "line_thread_ranges": [{"start_line": 95, "end_line": 104}]}),
        ),
        (
            serde_json::to_value(CommentScope::NotInDiff),
            json!({"scope": "not_in_diff"}),
        ),
    ];
    for (actual, expected) in cases {
        assert_eq!(actual.expect("serialises"), expected);
    }
}

#[test]
fn ask_anchors_and_files_wire_formats() {
    let lines = LineRange {
        start_line: 100,
        end_line: 102,
    };
    let cases = [
        (
            serde_json::to_value(AskAnchor::Step {
                step_id: StepId("f1.6".to_owned()),
                lines: Some(lines),
            }),
            json!({"kind": "step", "step_id": "f1.6", "lines": {"start_line": 100, "end_line": 102}}),
        ),
        (
            serde_json::to_value(AnchoredAskRequest {
                text: "q".to_owned(),
                anchor: AskAnchor::Lines {
                    path: "src/a.rs".to_owned(),
                    side: LineSide::Head,
                    range: None,
                },
                parent_id: None,
            }),
            json!({"text": "q", "anchor": {"kind": "lines", "path": "src/a.rs", "side": "head", "range": null}}),
        ),
        (
            serde_json::to_value(AskAnchor::Flow {
                flow_id: FlowId("f2".to_owned()),
            }),
            json!({"kind": "flow", "flow_id": "f2"}),
        ),
        (serde_json::to_value(AskAnchor::Pr), json!({"kind": "pr"})),
        (
            serde_json::to_value(AskAnchor::CommentItem {
                item: CommentItemId(uuid::Uuid::nil()),
            }),
            json!({"kind": "comment_item", "item": "00000000-0000-0000-0000-000000000000"}),
        ),
        (
            serde_json::to_value(AnchoredAskRequest {
                text: "and then?".to_owned(),
                anchor: AskAnchor::Pr,
                parent_id: Some(QuestionId(uuid::Uuid::nil())),
            }),
            json!({"text": "and then?", "anchor": {"kind": "pr"}, "parent_id": "00000000-0000-0000-0000-000000000000"}),
        ),
        (
            serde_json::to_value(PartOwner::Unexplained { reason: None }),
            json!({"kind": "unexplained"}),
        ),
        (
            serde_json::to_value(PartOwner::Step {
                flow_id: FlowId("f1".to_owned()),
                step_id: StepId("f1.2".to_owned()),
            }),
            json!({"kind": "step", "flow_id": "f1", "step_id": "f1.2"}),
        ),
        (
            serde_json::to_value(PartOwner::WhitespaceOnly),
            json!({"kind": "whitespace_only"}),
        ),
        (
            serde_json::to_value(PartOwner::Incidental {
                flow_id: FlowId("f1".to_owned()),
                text: "Makes `send` public.".to_owned(),
            }),
            json!({"kind": "incidental", "flow_id": "f1", "text": "Makes `send` public."}),
        ),
        (
            serde_json::to_value(FileGroup::Code),
            json!({"kind": "code"}),
        ),
        (
            serde_json::to_value(SetViewedRequest {
                path: "src/a.rs".to_owned(),
                viewed: true,
            }),
            json!({"path": "src/a.rs", "viewed": true}),
        ),
        (
            serde_json::to_value(FilesResponse {
                files: Vec::new(),
                viewed_status: ViewedStatus::NotChecked,
                current_head: None,
            }),
            json!({"files": [], "viewed_status": "not_checked", "current_head": null}),
        ),
        (
            serde_json::to_value(FileViewedState::Dismissed),
            json!("dismissed"),
        ),
    ];
    for (actual, expected) in cases {
        assert_eq!(actual.expect("serialises"), expected);
    }
    let hunk = FileHunk {
        id: contracts::ids::HunkId("h3".to_owned()),
        base: Some(lines),
        head: None,
        after_line: Some(99),
        parts: vec![HunkPart {
            side: LineSide::Base,
            range: lines,
            owner: PartOwner::Flow {
                flow_id: FlowId("f5".to_owned()),
            },
            also_in: vec![StepId("f1.2".to_owned())],
        }],
    };
    let json = serde_json::to_value(&hunk).expect("serialises");
    assert_eq!(
        json["parts"][0]["owner"],
        json!({"kind": "flow", "flow_id": "f5"})
    );
    assert_eq!(
        serde_json::from_value::<FileHunk>(json).expect("round trip"),
        hunk
    );
}

#[test]
fn sse_event_name_is_the_tag() {
    let question = AskQuestion {
        asked_by: None,
        mine: None,
        id: QuestionId::new_random(),
        anchor: AskAnchor::Step {
            step_id: StepId("f1.1".to_owned()),
            lines: None,
        },
        step_id: Some(StepId("f1.1".to_owned())),
        parent_id: None,
        text: "q".to_owned(),
        state: AskState::Answered,
        answer: Some(AskOutput {
            answer: "a".to_owned(),
            evidence: vec![],
        }),
        failure: None,
        created_at: Timestamp::now(),
        answered_at: None,
        resumes_at: None,
    };
    for event in [
        AnalysisEvent::Analysis { summary: summary() },
        AnalysisEvent::Question { question },
        AnalysisEvent::Heartbeat {
            at: stage_started(),
        },
    ] {
        let json = serde_json::to_value(&event).expect("serialises");
        assert_eq!(json["event"], json!(event.name()));
    }
}

/// `resumes_at` is on the wire only while a question waits for the usage limit; a question without it
/// (every stored fixture) keeps its JSON.
#[test]
fn ask_question_resumes_at_only_while_waiting_for_the_usage_limit() {
    let at = Timestamp::now();
    let mut question = AskQuestion {
        asked_by: None,
        mine: None,
        id: QuestionId::new_random(),
        anchor: AskAnchor::Step {
            step_id: StepId("f1.1".to_owned()),
            lines: None,
        },
        step_id: Some(StepId("f1.1".to_owned())),
        parent_id: None,
        text: "q".to_owned(),
        state: AskState::Pending,
        answer: None,
        failure: None,
        created_at: at,
        answered_at: None,
        resumes_at: None,
    };
    let json = serde_json::to_value(&question).expect("serialises");
    assert!(json.get("resumes_at").is_none(), "{json}");
    assert!(
        json.get("parent_id").is_none(),
        "a first question has no parent: {json}"
    );
    assert_eq!(
        serde_json::from_value::<AskQuestion>(json).expect("deserialises"),
        question
    );
    question.resumes_at = Some(at);
    let json = serde_json::to_value(&question).expect("serialises");
    assert_eq!(json["resumes_at"], serde_json::to_value(at).expect("time"));
    assert_eq!(
        AskRequest::MAX_TEXT_CHARS,
        2000,
        "docs/contracts/http-api.md"
    );
}

#[test]
fn user_feedback_values_are_stored_values_without_missed() {
    let user = [
        UserFeedbackValue::Correct,
        UserFeedbackValue::Wrong,
        UserFeedbackValue::Irrelevant,
        UserFeedbackValue::Overrated,
        UserFeedbackValue::Underrated,
    ];
    for value in user {
        assert_eq!(
            serde_json::to_value(value).expect("serialises"),
            serde_json::to_value(FeedbackValue::from(value)).expect("serialises")
        );
    }
    assert_eq!(
        serde_json::to_value(FeedbackValue::MissedByLlm).expect("serialises"),
        json!("missed_by_llm")
    );
}

#[test]
fn range_cap_is_2000_lines() {
    assert_eq!(RangeQuery::MAX_LINES, 2000);
}

fn contains_float(node: &Value) -> bool {
    match node {
        Value::Object(map) => {
            map.get("type").is_some_and(|kind| {
                kind == "number"
                    || kind
                        .as_array()
                        .is_some_and(|k| k.contains(&json!("number")))
            }) || map.values().any(contains_float)
        }
        Value::Array(items) => items.iter().any(contains_float),
        _ => false,
    }
}

fn schema<T: JsonSchema>() -> Value {
    schemars::schema_for!(T).to_value()
}

#[test]
fn no_floats_in_api_types() {
    assert!(contains_float(&schema::<f32>()), "the float detector works");
    let schemas = [
        ("MeResponse", schema::<MeResponse>()),
        ("ResolvePrResponse", schema::<ResolvePrResponse>()),
        ("CreateAnalysisRequest", schema::<CreateAnalysisRequest>()),
        ("Tour", schema::<Tour>()),
        ("LinesResponse", schema::<LinesResponse>()),
        ("DiffResponse", schema::<DiffResponse>()),
        ("ReviewState", schema::<ReviewState>()),
        ("AskQuestion", schema::<AskQuestion>()),
        ("FeedbackRequest", schema::<FeedbackRequest>()),
        ("Feedback", schema::<Feedback>()),
        ("PendingReview", schema::<PendingReview>()),
        ("CreateCommentRequest", schema::<CreateCommentRequest>()),
        ("CreateCommentResponse", schema::<CreateCommentResponse>()),
        ("UpdateCommentRequest", schema::<UpdateCommentRequest>()),
        ("Settings", schema::<Settings>()),
        ("RepoSettings", schema::<RepoSettings>()),
        ("AnalysisEvent", schema::<AnalysisEvent>()),
        ("ApiError", schema::<ApiError>()),
        ("DashboardSummary", schema::<DashboardSummary>()),
        ("ItemPage", schema::<ItemPage>()),
        ("ItemQuery", schema::<ItemQuery>()),
        ("DashboardEvent", schema::<DashboardEvent>()),
        ("DashboardSettingsView", schema::<DashboardSettingsView>()),
        ("CreatedAccessToken", schema::<CreatedAccessToken>()),
        ("CreateAccessToken", schema::<CreateAccessToken>()),
    ];
    for (name, schema) in schemas {
        assert!(!contains_float(&schema), "{name}");
    }
}

fn lines(side: LineSide, total_lines: u8, deleted: Vec<DeletedBlock>) -> LinesResponse {
    LinesResponse {
        path: "src/rpc/old.rs".to_owned(),
        side,
        from: 1,
        to: u32::from(total_lines),
        total_lines: u32::from(total_lines),
        lines: vec!["x".to_owned(); usize::from(total_lines)],
        changed: vec![],
        context_only: None,
        deleted,
        moved: vec![],
        comments: CommentScope::InDiff {
            line_thread_ranges: vec![],
        },
    }
}

fn removed(after_line: u32, base_start: u32) -> DeletedBlock {
    DeletedBlock {
        after_line,
        base_start,
        lines: vec!["fn send_once() {}".to_owned()],
        total_lines: 1,
        replacement: None,
        context_only: None,
    }
}

/// Every deletion has a valid location: base-side lines where no head line exists, deletion
/// markers inside head lines otherwise.
#[test]
fn deletions_have_valid_locations() {
    let base_range = LineRange {
        start_line: 1,
        end_line: 40,
    };
    // Whole file deleted: base-side step, unexplained base lines, base-side lines endpoint.
    let step = StepFacts {
        leading_context: None,
        file: "src/rpc/old.rs".to_owned(),
        side: LineSide::Base,
        range: base_range,
        github_url: "https://github.com/acme/node-client/blob/1/src/rpc/old.rs#L1-L40".to_owned(),
        changed_lines: vec![base_range],
        moved: None,
        call_hierarchy: None,
        thread_ids: vec![],
    };
    let json = serde_json::to_value(&step).expect("serialises");
    assert_eq!(json["side"], json!("base"));
    assert_eq!(
        serde_json::from_value::<StepFacts>(json).expect("deserialises"),
        step
    );
    let unexplained = FileRanges {
        file: "src/rpc/old.rs".to_owned(),
        side: LineSide::Base,
        ranges: vec![base_range],
    };
    assert_eq!(
        serde_json::to_value(&unexplained).expect("serialises")["side"],
        json!("base")
    );
    let query: RangeQuery = serde_json::from_value(
        json!({"path": "src/rpc/old.rs", "side": "base", "from": 1, "to": 40}),
    )
    .expect("base-side range query");
    assert_eq!(query.side, LineSide::Base);
    let deleted_file = lines(LineSide::Base, 40, vec![]);
    assert_eq!(deleted_file.side, LineSide::Base);
    // File emptied: the head file exists with no lines; its removed lines sit at the marker before line 1.
    let emptied = lines(LineSide::Head, 0, vec![removed(0, 1)]);
    assert_eq!(
        serde_json::to_value(&emptied).expect("serialises")["deleted"][0]["after_line"],
        json!(0)
    );
    // Deletions at the start and at the end of a file: markers after line 0 and after the last line.
    let ends = lines(LineSide::Head, 10, vec![removed(0, 1), removed(10, 15)]);
    let back: LinesResponse =
        serde_json::from_value(serde_json::to_value(&ends).expect("serialises"))
            .expect("round trip");
    assert_eq!(back, ends);
    // Feedback and evidence carry the side.
    let evidence = Evidence {
        file: "src/rpc/old.rs".to_owned(),
        side: LineSide::Base,
        line: 12,
    };
    assert_eq!(
        serde_json::to_value(&evidence).expect("serialises"),
        json!({"file": "src/rpc/old.rs", "side": "base", "line": 12})
    );
}

/// A highlighted question keeps its own was → is for the tooltip, separate from the question.
#[test]
fn annotation_projection_keeps_was_is_and_remark() {
    let agent: AgentAnnotation = serde_json::from_value(json!({
        "type": "question", "basis": "inference", "start_line": 99, "end_line": 99,
        "was": "A timeout was returned to the caller.", "is": "A timeout is retried while attempt < 3.",
        "text": "Can broadcast_tx be sent twice?", "evidence": []
    }))
    .expect("agent annotation");
    let annotation = Annotation::from_agent(AnnotationId("f1.1.a1".to_owned()), agent);
    assert_eq!(annotation.kind, AnnotationType::Flag);
    assert_eq!(annotation.was, "A timeout was returned to the caller.");
    assert_eq!(annotation.is, "A timeout is retried while attempt < 3.");
    assert_eq!(
        annotation.text.as_deref(),
        Some("Can broadcast_tx be sent twice?")
    );
    assert_eq!(
        annotation.range,
        LineRange {
            start_line: 99,
            end_line: 99
        }
    );
    let json = serde_json::to_value(&annotation).expect("serialises");
    assert_eq!(json["was"], json!("A timeout was returned to the caller."));
    assert_eq!(json["is"], json!("A timeout is retried while attempt < 3."));
}

/// A question stored before flags reads as a flag of level unknown: its text is the title, no scenario.
#[test]
fn stored_question_projects_to_a_flag_of_unknown_level() {
    let agent: AgentAnnotation = serde_json::from_value(json!({
        "type": "question", "basis": "inference", "start_line": 99, "end_line": 99,
        "was": "", "is": "", "text": "Can broadcast_tx be sent twice?", "evidence": []
    }))
    .expect("stored question");
    assert_eq!((agent.kind, &agent.flag), (AnnotationType::Flag, &None));
    assert!(serde_json::from_value::<AnnotationType>(json!("assessment")).is_err());
    assert_eq!(
        serde_json::to_value(&agent).expect("agent")["type"],
        json!("flag")
    );
    let annotation = Annotation::from_agent(AnnotationId("f1.1.a1".to_owned()), agent);
    let wire = serde_json::to_value(&annotation).expect("serialises");
    assert_eq!(wire["type"], json!("flag"));
    assert_eq!(wire["text"], json!("Can broadcast_tx be sent twice?"));
    assert_eq!(
        wire["flag"],
        json!({"level": "unknown", "scenario": [], "not_checked": ""})
    );
    // A new flag keeps its own level and scenario.
    let agent: AgentAnnotation = serde_json::from_value(json!({
        "type": "flag", "basis": "inference", "start_line": 99, "end_line": 99, "was": "", "is": "",
        "text": "Potential double send:", "evidence": [],
        "flag": {"level": "yellow", "scenario": ["A sends.", "B sends."], "not_checked": "Not run."}
    }))
    .expect("flag");
    let flag = Annotation::from_agent(AnnotationId("f1.1.a2".to_owned()), agent).flag;
    assert_eq!(flag.map(|f| f.level), Some(FlagLevel::Yellow));
}

/// Settings lists the tenant's known repositories, so per-repository settings can be opened.
#[test]
fn settings_lists_known_repositories() {
    let settings = Settings {
        llm_selection: None,
        llm_job_slots: None,
        llm_usage: None,
        memcan_status: None,
        llm_status: None,
        language: LlmLanguage::En,
        app_installations: Some(vec![AppInstallation {
            account: "acme".to_owned(),
            all_repositories: true,
        }]),
        repos: vec![RepoRef {
            owner: "acme".to_owned(),
            name: "node-client".to_owned(),
        }],
        controlled_owners: None,
    };
    let json = serde_json::to_value(&settings).expect("serialises");
    assert_eq!(
        json["repos"],
        json!([{"owner": "acme", "name": "node-client"}])
    );
}

#[test]
fn flow_facts_have_no_changed_line_counter() {
    let json = serde_json::to_value(tour()).expect("serialises");
    assert!(json["flows"][0]["facts"].get("changed_lines").is_none());
}

/// CCR-ST1: installations that could not be checked are null, distinct from an empty list (none installed).
#[test]
fn settings_installations_not_checked_is_null_not_empty() {
    let mut settings = Settings {
        llm_selection: None,
        llm_job_slots: None,
        llm_usage: None,
        memcan_status: None,
        llm_status: None,
        language: LlmLanguage::En,
        app_installations: None,
        repos: Vec::new(),
        controlled_owners: None,
    };
    let json = serde_json::to_value(&settings).expect("serialises");
    assert_eq!(json["app_installations"], serde_json::Value::Null);
    settings.app_installations = Some(Vec::new());
    let json = serde_json::to_value(&settings).expect("serialises");
    assert_eq!(json["app_installations"], json!([]));
}

/// A diff line carries its ending when it is not LF.
#[test]
fn diff_line_ending_is_null_for_lf_and_named_otherwise() {
    use contracts::api::code::{DiffLine, DiffLineKind, LineEnding};
    let line = |eol| DiffLine {
        kind: DiffLineKind::Removed,
        old_line: Some(1),
        new_line: None,
        text: "x".to_owned(),
        eol,
    };
    let names: Vec<serde_json::Value> = [None, Some(LineEnding::Crlf), Some(LineEnding::NoNewline)]
        .into_iter()
        .map(|eol| serde_json::to_value(line(eol)).expect("serialises")["eol"].clone())
        .collect();
    assert_eq!(names, [json!(null), json!("crlf"), json!("no_newline")]);
}

#[test]
fn usage_and_size_warning_are_additive_json_numbers() {
    use contracts::api::usage::UsageTotals;
    let mut analysis = summary();
    analysis.usage = Some(UsageTotals {
        input_tokens: 1_200_000,
        cached_input_tokens: 900_000,
        output_tokens: 45_000,
        reasoning_output_tokens: None,
        jobs: 7,
    });
    let mut wire = serde_json::to_value(&analysis).unwrap();
    assert_eq!(
        wire["usage"],
        json!({"input_tokens":1200000,"cached_input_tokens":900000,"output_tokens":45000,"reasoning_output_tokens":null,"jobs":7})
    );
    wire.as_object_mut().unwrap().remove("usage");
    assert_eq!(
        serde_json::from_value::<AnalysisSummary>(wire)
            .unwrap()
            .usage,
        None
    );
    let mut wire =
        json!({"pr": analysis.pr, "commenting_available":true,"analysis":null,"head_moved":false});
    assert_eq!(
        serde_json::from_value::<ResolvePrResponse>(wire.clone())
            .unwrap()
            .size_warning,
        None
    );
    // The size and the planning estimate are optional fields of their own: absent in older responses.
    let resolved = serde_json::from_value::<ResolvePrResponse>(wire.clone()).unwrap();
    assert_eq!((resolved.size, resolved.planning_estimate), (None, None));
    assert_eq!(serde_json::to_value(&resolved).unwrap().get("size"), None);
    wire["size"] = json!({"changed_lines":238,"changed_files":13});
    wire["planning_estimate"] = json!({"remaining_seconds":720,"basis":"lines_only","rough":true});
    let resolved = serde_json::from_value::<ResolvePrResponse>(wire.clone()).unwrap();
    assert_eq!(resolved.size.unwrap().changed_lines, 238);
    assert_eq!(resolved.planning_estimate.unwrap().remaining_seconds, 720);
    assert_eq!(
        serde_json::to_value(&resolved).unwrap()["size"],
        wire["size"]
    );
    // `idle` is absent in older running states.
    let running: AnalysisState =
        serde_json::from_value(json!({"state":"running","done":2,"total":5})).unwrap();
    assert!(matches!(
        running,
        AnalysisState::Running { idle: false, .. }
    ));
    wire["size_warning"] = json!({"changed_lines":27081,"changed_files":371,"estimate":null});
    assert_eq!(
        serde_json::from_value::<ResolvePrResponse>(wire)
            .unwrap()
            .size_warning
            .unwrap()
            .changed_lines,
        27081
    );
}

#[test]
fn github_webhook_status_reports_only_installation_wide_totals() {
    use contracts::api::settings::GithubWebhook;

    let wire = json!({
        "configured": true,
        "webhook_url": "https://hooks.example.test/diffpilot/github",
        "last_delivery_at": "2026-11-25T10:00:00Z",
        "dropped_deliveries": 3,
        "refused_connections": 2
    });
    let status: GithubWebhook = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(status).unwrap(), wire);
}
