//! HTTP API contract: error model, routes, wire formats, derivations, no floats (plan §5.1, §5.3).

use std::collections::HashSet;

use chrono::Utc;
use contracts::agent::analyse_flow::AgentAnnotation;
use contracts::agent::analyse_flow::{Assessment, FlowIntroduction};
use contracts::agent::ask::AskOutput;
use contracts::agent::pr_context::PrContextOutput;
use contracts::api::analyses::{AnalysisState, AnalysisSummary, CreateAnalysisRequest, FlowRef};
use contracts::api::auth::MeResponse;
use contracts::api::code::{CommentScope, DeletedBlock, DiffResponse, LinesResponse, RangeQuery};
use contracts::api::comments::{
    CreateCommentRequest, CreateCommentResponse, PendingReview, UpdateCommentRequest,
};
use contracts::api::error::{ApiError, ApiErrorCode};
use contracts::api::events::AnalysisEvent;
use contracts::api::feedback::{
    Feedback, FeedbackRequest, FeedbackTarget, FeedbackValue, UserFeedbackValue,
};
use contracts::api::pr::{PrSummary, ResolvePrResponse};
use contracts::api::questions::{AskQuestion, AskState};
use contracts::api::review::{BlockRef, QuietRowRef, ReviewState};
use contracts::api::settings::{AppInstallation, RepoSettings, Settings};
use contracts::api::tour::{
    Annotation, CiState, CiStatus, Counters, Flow, FlowFacts, FlowGraph, FlowGroup, FlowLlm,
    GraphEdge, GraphNode, PrContextBlock, PrContextFacts, ProjectProfileStatus, QuietRows, Step,
    StepFacts, StepLlm, Tour,
};
use contracts::common::{
    AnnotationType, Basis, Blocking, Classification, Evidence, FileRanges, FragmentKind, LineRange,
    LineSide, LlmLanguage, NodeKind, NotAnalysedCategory, Origin, RepoRef, RiskBand,
};
use contracts::ids::{AnalysisId, AnnotationId, CommitSha, FlowId, QuestionId, StepId};
use contracts::routes::{HttpMethod, Listener, ROUTES, RouteAuth};
use schemars::JsonSchema;
use serde_json::{Value, json};

fn summary() -> AnalysisSummary {
    AnalysisSummary {
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
        created_at: Utc::now(),
        finished_at: Some(Utc::now()),
    }
}

fn tour() -> Tour {
    let step_id = StepId("f1.1".to_owned());
    Tour {
        analysis: summary(),
        pr_context: PrContextBlock {
            facts: PrContextFacts {
                ci: CiStatus {
                    state: CiState::Passing,
                    total: 6,
                    failing: vec![],
                },
                counters: Counters {
                    lines_removed_from_tests: 12,
                    dependencies_changed: vec!["tokio".to_owned()],
                },
                project_profile: ProjectProfileStatus { sources: vec![] },
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
                        kind: AnnotationType::Question,
                        basis: Basis::Inference,
                        range: LineRange {
                            start_line: 99,
                            end_line: 99,
                        },
                        was: "A timeout was returned to the caller.".to_owned(),
                        is: "A timeout is retried while attempt < 3.".to_owned(),
                        text: Some("Can broadcast_tx be sent twice?".to_owned()),
                        evidence: vec![],
                    }],
                    assessment: Some(Assessment {
                        headline: "h".to_owned(),
                        risk: RiskBand::High,
                        reason: "r".to_owned(),
                        basis_note: "read 4 files".to_owned(),
                        not_checked: "node".to_owned(),
                        origin: Origin::Introduced,
                        blocking: Blocking::BlocksThisPr,
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
    assert_eq!(annotation["type"], json!("question"));
}

#[test]
fn error_codes_map_to_statuses() {
    let expected = [
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
        ("range_too_large", 400),
        ("not_rendered", 422),
        ("github_unavailable", 502),
        ("internal", 500),
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

#[test]
fn mutating_browser_routes_require_csrf() {
    for route in ROUTES
        .iter()
        .filter(|route| route.listener == Listener::Public)
    {
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
        FlowGroup::from(Classification::Test),
        FlowGroup::LlmClassifiedTests
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

#[test]
fn tagged_wire_formats() {
    let cases = [
        (
            serde_json::to_value(AnalysisState::Running { done: 2, total: 5 }),
            json!({"state": "running", "done": 2, "total": 5}),
        ),
        (
            serde_json::to_value(AnalysisState::Failed {
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
fn sse_event_name_is_the_tag() {
    let question = AskQuestion {
        id: QuestionId::new_random(),
        step_id: StepId("f1.1".to_owned()),
        text: "q".to_owned(),
        state: AskState::Answered,
        answer: Some(AskOutput {
            answer: "a".to_owned(),
            evidence: vec![],
        }),
        failure: None,
        created_at: Utc::now(),
    };
    for event in [
        AnalysisEvent::Analysis { summary: summary() },
        AnalysisEvent::Question { question },
    ] {
        let json = serde_json::to_value(&event).expect("serialises");
        assert_eq!(json["event"], json!(event.name()));
    }
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
    ];
    for (name, schema) in schemas {
        assert!(!contains_float(&schema), "{name}");
    }
}

fn lines(side: LineSide, total_lines: u32, deleted: Vec<DeletedBlock>) -> LinesResponse {
    LinesResponse {
        path: "src/rpc/old.rs".to_owned(),
        side,
        from: 1,
        to: total_lines,
        total_lines,
        lines: vec!["x".to_owned(); usize::try_from(total_lines).expect("small")],
        changed: vec![],
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
    }
}

/// ARCH-001: every deletion has a valid location: base-side lines where no head line exists, deletion
/// markers inside head lines otherwise.
#[test]
fn deletions_have_valid_locations() {
    let base_range = LineRange {
        start_line: 1,
        end_line: 40,
    };
    // Whole file deleted: base-side step, unexplained base lines, base-side lines endpoint.
    let step = StepFacts {
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

/// ARCH-002: a highlighted question keeps its own was → is for the tooltip, separate from the question.
#[test]
fn annotation_projection_keeps_was_is_and_remark() {
    let agent: AgentAnnotation = serde_json::from_value(json!({
        "type": "question", "basis": "inference", "start_line": 99, "end_line": 99,
        "was": "A timeout was returned to the caller.", "is": "A timeout is retried while attempt < 3.",
        "text": "Can broadcast_tx be sent twice?", "evidence": []
    }))
    .expect("agent annotation");
    let annotation = Annotation::from_agent(AnnotationId("f1.1.a1".to_owned()), agent);
    assert_eq!(annotation.kind, AnnotationType::Question);
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

/// ARCH-006: Settings lists the tenant's known repositories, so per-repository settings can be opened.
#[test]
fn settings_lists_known_repositories() {
    let settings = Settings {
        llm_status: None,
        language: LlmLanguage::En,
        app_installations: vec![AppInstallation {
            account: "acme".to_owned(),
            all_repositories: true,
        }],
        repos: vec![RepoRef {
            owner: "acme".to_owned(),
            name: "node-client".to_owned(),
        }],
    };
    let json = serde_json::to_value(&settings).expect("serialises");
    assert_eq!(
        json["repos"],
        json!([{"owner": "acme", "name": "node-client"}])
    );
}

#[test]
fn flow_facts_have_no_changed_line_counter() {
    let json = serde_json::to_value(&tour()).expect("serialises");
    assert!(json["flows"][0]["facts"].get("changed_lines").is_none());
}
