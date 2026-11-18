//! Agent-output schemas: Codex strict subset, closed enums, trust rules (brief §1, §3; plan §5.3).
#![expect(clippy::expect_used, reason = "test helpers outside #[test] functions")]

use contracts::agent::analyse_flow::FlowAnalysisOutput;
use contracts::agent::ask::AskOutput;
use contracts::agent::judge_work::JudgeWorkOutput;
use contracts::agent::plan::PlanOutput;
use contracts::agent::pr_context::PrContextOutput;
use contracts::agent::strict::{self, StrictSchemaError};
use contracts::agent::{JobKind, output_schema};
use contracts::common::{
    AnnotationType, Basis, Blocking, Classification, FitTopic, FragmentKind, Origin,
    RelatedWorkKind, RiskBand, ThreadLlmStatus,
};
use schemars::JsonSchema;
use serde_json::{Value, json};

/// Visits every schema node (`properties`, `items`, `anyOf`), with the property name that led to it.
fn walk<'a>(
    node: &'a Value,
    name: Option<&'a str>,
    visit: &mut dyn FnMut(Option<&'a str>, &'a Value),
) {
    visit(name, node);
    if let Some(properties) = node.get("properties").and_then(Value::as_object) {
        for (key, child) in properties {
            walk(child, Some(key), visit);
        }
    }
    if let Some(items) = node.get("items") {
        walk(items, name, visit);
    }
    if let Some(branches) = node.get("anyOf").and_then(Value::as_array) {
        for branch in branches {
            walk(branch, name, visit);
        }
    }
}

fn has_type(node: &Value, wanted: &str) -> bool {
    match node.get("type") {
        Some(Value::String(kind)) => kind == wanted,
        Some(Value::Array(kinds)) => kinds.iter().any(|kind| kind == wanted),
        _ => false,
    }
}

fn all_schemas() -> Vec<(JobKind, Value)> {
    JobKind::ALL
        .into_iter()
        .map(|kind| (kind, output_schema(kind).expect("agent schema is strict")))
        .collect()
}

/// Schemas of the review module, which the brief §1 trust rules bind; the dashboard's `judge_work` scores by
/// design and is outside them (OD2, ADR-016).
fn review_schemas() -> Vec<(JobKind, Value)> {
    all_schemas()
        .into_iter()
        .filter(|(kind, _)| *kind != JobKind::JudgeWork)
        .collect()
}

fn enum_values<T: JsonSchema>() -> Vec<String> {
    let schema = strict::schema_for::<T>().expect("enum schema is strict");
    schema["enum"]
        .as_array()
        .expect("collapsed string enum")
        .iter()
        .map(|value| value.as_str().expect("string value").to_owned())
        .collect()
}

#[test]
fn agent_schemas_are_codex_strict() {
    for (kind, schema) in all_schemas() {
        assert_eq!(strict::check(&schema), Ok(()), "{kind:?}");
        assert!(schema.get("$schema").is_none(), "{kind:?}");
        assert!(
            schema.get("$defs").is_none(),
            "{kind:?}: subschemas are inlined"
        );
        assert!(!schema.to_string().contains("oneOf"), "{kind:?}");
    }
}

#[test]
fn closed_enums_match_brief() {
    assert_eq!(
        enum_values::<Classification>(),
        [
            "behavioural",
            "api_contract",
            "new_code",
            "refactor_pure",
            "move",
            "rename",
            "docs_comments",
            "generated_mechanical",
            "config_build",
            "dependency",
            "other"
        ]
    );
    assert_eq!(
        enum_values::<FragmentKind>(),
        [
            "modified",
            "added",
            "moved",
            "renamed",
            "deleted_site",
            "context"
        ]
    );
    assert_eq!(
        enum_values::<AnnotationType>(),
        ["behaviour", "consequence", "question", "note"]
    );
    assert_eq!(enum_values::<Basis>(), ["fact", "inference"]);
    assert_eq!(enum_values::<RiskBand>(), ["low", "medium", "high"]);
    assert_eq!(
        enum_values::<Origin>(),
        ["introduced", "pre_existing", "unclear"]
    );
    assert_eq!(
        enum_values::<Blocking>(),
        ["no_action_proposed", "blocks_this_pr", "follow_up"]
    );
    assert_eq!(
        enum_values::<RelatedWorkKind>(),
        [
            "duplicate",
            "competing_solution",
            "stacked_pr",
            "linked_issue",
            "companion_repository"
        ]
    );
    assert_eq!(
        enum_values::<FitTopic>(),
        [
            "layer_or_repository",
            "size_for_problem",
            "duplicate_mechanism",
            "other"
        ]
    );
    assert_eq!(
        enum_values::<ThreadLlmStatus>(),
        ["still_valid", "stale", "already_addressed"]
    );
}

#[test]
fn collapsed_enums_keep_variant_descriptions() {
    let schema = strict::schema_for::<RiskBand>().expect("strict");
    let description = schema["description"].as_str().expect("description");
    assert!(description.contains("`high`: A production user or operator would clearly notice."));
}

#[test]
fn no_floats_in_agent_schemas() {
    for (kind, schema) in all_schemas() {
        walk(&schema, None, &mut |name, node| {
            assert!(!has_type(node, "number"), "{kind:?}: float at {name:?}");
        });
    }
}

#[test]
fn integers_only_for_lines_and_rank() {
    const ALLOWED: [&str; 8] = [
        "line",
        "start_line",
        "end_line",
        "head_start",
        "head_end",
        "base_start",
        "base_end",
        "importance_rank",
    ];
    for (kind, schema) in review_schemas() {
        walk(&schema, None, &mut |name, node| {
            if has_type(node, "integer") {
                let name = name.unwrap_or_default();
                assert!(
                    ALLOWED.contains(&name),
                    "{kind:?}: integer property `{name}`"
                );
                assert_eq!(node["minimum"], json!(0), "{kind:?}: `{name}` is unsigned");
            }
        });
    }
}

#[test]
fn no_confidence_like_fields() {
    const BANNED: [&str; 9] = [
        "confidence",
        "score",
        "probability",
        "likelihood",
        "percent",
        "overall_risk",
        "importance",
        "review_effort",
        "recommendation",
    ];
    for (kind, schema) in review_schemas() {
        walk(&schema, None, &mut |name, _| {
            let name = name.unwrap_or_default();
            let hit = BANNED
                .iter()
                .any(|banned| name.contains(banned) && name != "importance_rank");
            assert!(!hit, "{kind:?}: field `{name}`");
        });
    }
}

#[test]
fn basis_objects_carry_evidence_or_a_location() {
    let mut seen = 0;
    for (kind, schema) in review_schemas() {
        walk(&schema, None, &mut |name, node| {
            let Some(properties) = node.get("properties").and_then(Value::as_object) else {
                return;
            };
            if !properties.contains_key("basis") {
                return;
            }
            seen += 1;
            let evidence = properties.contains_key("evidence");
            let location = ["file", "start_line", "end_line"]
                .iter()
                .all(|key| properties.contains_key(*key));
            assert!(
                evidence || location,
                "{kind:?}: {name:?} has basis without evidence"
            );
        });
    }
    assert_eq!(seen, 2, "annotations and impact references carry basis");
}

#[test]
fn fit_questions_have_no_basis() {
    let schema = output_schema(JobKind::PrContext).expect("strict");
    walk(&schema, None, &mut |name, _| {
        assert_ne!(name, Some("basis"))
    });
}

fn samples() -> Vec<(JobKind, Value)> {
    let evidence = json!([{"file": "src/rpc/client.rs", "side": "head", "line": 99}]);
    vec![
        (
            JobKind::Plan,
            json!({
                "flows": [{
                    "id": "f1", "title": "Retry timed-out RPC calls", "classification": "behavioural",
                    "importance_rank": 1, "intent": "Retry RpcClient::send on timeout",
                    "hunks": [{"hunk_id": "h1", "head_start": null, "head_end": null},
                              {"hunk_id": "h2", "head_start": 21, "head_end": 25}],
                    "depends_on": []
                }],
                "unassigned": [{"hunk_id": "h3", "head_start": null, "head_end": null, "reason": "import order only"}]
            }),
        ),
        (
            JobKind::PrContext,
            json!({
                "problem": "Timeouts fail the whole call.", "why_needed": "Slow nodes.",
                "what_it_gives": "Fewer failed payments.",
                "related_work": [{"reference": "acme/node-client#479", "kind": "competing_solution", "overlap": "Also retries."}],
                "fit_questions": [{"question": "Should retries live in the transport?", "topic": "layer_or_repository", "evidence": evidence}]
            }),
        ),
        (
            JobKind::AnalyseFlow,
            json!({
                "flow_id": "f1",
                "introduction": {"problem_before": "A timeout ended the call.", "change": "Retry up to 3 times.",
                                 "example_before_after": "Before: Timeout. After: Ok on attempt 2."},
                "steps": [
                    {"id": "s1", "title": "Retry allowed?", "file": "src/rpc/client.rs", "side": "head", "start_line": 99, "end_line": 99,
                     "symbol": "RpcClient::send", "kind": "added", "moved_from": null,
                     "called_by": "step s2", "decides": "Retry or give up.", "next": "s2", "why": "Stops the loop.",
                     "annotations": [{"type": "question", "basis": "inference", "start_line": 99, "end_line": 99,
                                      "was": "A timeout was returned.", "is": "A timeout is retried.",
                                      "text": "Can broadcast_tx be sent twice?", "evidence": []},
                                     {"type": "behaviour", "basis": "fact", "start_line": 99, "end_line": 99,
                                      "was": "A timeout was returned.", "is": "A timeout is retried.",
                                      "text": null, "evidence": evidence}],
                     "assessment": {"headline": "A payment may be broadcast twice.", "risk": "high", "reason": "Value-moving.",
                                    "basis_note": "Read: 2 files", "not_checked": "node deduplication",
                                    "origin": "introduced", "blocking": "blocks_this_pr", "smallest_correction": null}},
                    {"id": "s2", "title": "Wait", "file": "src/rpc/client.rs", "side": "head", "start_line": 100, "end_line": 103,
                     "symbol": null, "kind": "moved", "moved_from": {"file": "src/rpc/old.rs", "base_start": 1, "base_end": 4},
                     "called_by": "s1", "decides": "Sleeps.", "next": "s1", "why": "Backoff.",
                     "annotations": [], "assessment": {"headline": "The bounded wait may delay a response.", "risk": "low", "reason": "The wait is confined to a single request.", "basis_note": "Read: the wait and callers.", "not_checked": "Runtime timing.", "origin": "introduced", "blocking": "no_action_proposed", "smallest_correction": null}},
                    {"id": "s3", "title": "Single-attempt API removed", "file": "src/rpc/old.rs", "side": "base",
                     "start_line": 1, "end_line": 40, "symbol": "send_once", "kind": "deleted_site", "moved_from": null,
                     "called_by": "mempool before this PR", "decides": "Nothing; removed.", "next": "none",
                     "why": "Callers move to the retrying send.",
                     "annotations": [{"type": "consequence", "basis": "fact", "start_line": 1, "end_line": 3,
                                      "was": "send_once sent exactly one request.", "is": "No single-attempt call exists.",
                                      "text": "Callers that cannot tolerate a repeat lose their opt-out.",
                                      "evidence": [{"file": "src/rpc/old.rs", "side": "base", "line": 2}]}],
                     "assessment": {"headline": "The bounded wait may delay a response.", "risk": "low", "reason": "The wait is confined to a single request.", "basis_note": "Read: the wait and callers.", "not_checked": "Runtime timing.", "origin": "introduced", "blocking": "no_action_proposed", "smallest_correction": null}}
                ],
                "edges": [{"from_step_id": "s1", "to_step_id": "s2", "label": "yes"},
                          {"from_step_id": "s2", "to_step_id": "s1", "label": null}],
                "impact_refs": [{"step_id": "s1", "file": "src/mempool/mod.rs", "start_line": 126, "end_line": 132,
                                 "symbol": null, "reason": "Calls send.", "basis": "fact"}],
                "pattern_repetitions": [{"description": "Timeout mapping", "explained_in_step_id": "s1",
                                         "sites": [{"file": "src/rpc/client.rs", "side": "head", "start_line": 120, "end_line": 121}]}],
                "thread_statuses": [{"thread_alias": "t1", "status": "still_valid", "reason": "Still retries."}]
            }),
        ),
        (
            JobKind::Ask,
            json!({"answer": "Does the node deduplicate? Check node-sdk.", "evidence": evidence}),
        ),
        (
            JobKind::JudgeWork,
            json!({"items": [{
                "key": "acme~node-client~pr~482",
                "defect": {"likelihood": 4, "impact": 6, "basis": "Timeouts fail payments on slow nodes."},
                "parts": [{"component": "completion", "points": 5, "basis": "One review round left."}],
                "eligibility": null, "also_keys": [], "alarm": null,
                "why": "A real defect with a small fix.", "next": "Review the retry loop."
            }]}),
        ),
    ]
}

fn validator(kind: JobKind) -> jsonschema::Validator {
    jsonschema::validator_for(&output_schema(kind).expect("strict")).expect("valid JSON Schema")
}

#[test]
fn samples_validate_and_round_trip() {
    for (kind, sample) in samples() {
        let validator = validator(kind);
        let errors: Vec<String> = validator
            .iter_errors(&sample)
            .map(|error| error.to_string())
            .collect();
        assert!(errors.is_empty(), "{kind:?}: {errors:?}");
        let round_trip = match kind {
            JobKind::Plan => serde_json::to_value(
                serde_json::from_value::<PlanOutput>(sample.clone()).expect("plan"),
            ),
            JobKind::PrContext => serde_json::to_value(
                serde_json::from_value::<PrContextOutput>(sample.clone()).expect("pr_context"),
            ),
            JobKind::AnalyseFlow => serde_json::to_value(
                serde_json::from_value::<FlowAnalysisOutput>(sample.clone()).expect("analyse_flow"),
            ),
            JobKind::Ask => serde_json::to_value(
                serde_json::from_value::<AskOutput>(sample.clone()).expect("ask"),
            ),
            JobKind::JudgeWork => serde_json::to_value(
                serde_json::from_value::<JudgeWorkOutput>(sample.clone()).expect("judge_work"),
            ),
        }
        .expect("serialises");
        assert_eq!(round_trip, sample, "{kind:?}");
    }
}

#[test]
fn mutated_outputs_are_rejected() {
    let (_, flow) = samples().swap_remove(2);
    let validator = validator(JobKind::AnalyseFlow);
    type Mutation = (&'static str, fn(&mut Value));
    let mutations: [Mutation; 11] = [
        ("null assessment", |v| {
            v["steps"][0]["assessment"] = Value::Null
        }),
        ("missing assessment", |v| {
            v["steps"][0].as_object_mut().unwrap().remove("assessment");
        }),
        ("unknown side", |v| v["steps"][0]["side"] = json!("left")),
        ("annotation without was", |v| {
            v["steps"][0]["annotations"][0]
                .as_object_mut()
                .expect("annotation")
                .remove("was");
        }),
        ("evidence without side", |v| {
            v["steps"][0]["annotations"][1]["evidence"][0]
                .as_object_mut()
                .expect("evidence")
                .remove("side");
        }),
        ("extra property", |v| {
            v["steps"][0]["confidence"] = json!("high")
        }),
        ("missing property", |v| {
            v["steps"][0].as_object_mut().expect("step").remove("why");
        }),
        ("float line", |v| v["steps"][0]["start_line"] = json!(99.5)),
        ("string line", |v| v["steps"][0]["end_line"] = json!("99")),
        ("unknown enum value", |v| {
            v["steps"][0]["assessment"]["risk"] = json!("critical")
        }),
        ("negative line", |v| v["steps"][1]["start_line"] = json!(-1)),
    ];
    for (label, mutate) in mutations {
        let mut instance = flow.clone();
        mutate(&mut instance);
        assert!(!validator.is_valid(&instance), "{label} accepted");
    }
}

#[test]
fn check_rejects_schemas_outside_the_subset() {
    let cases = [
        (
            json!({"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a"]}),
            StrictSchemaError::OpenObject {
                path: String::new(),
            },
        ),
        (
            json!({"type": "object", "properties": {"a": {"type": "string"}}, "required": [],
                   "additionalProperties": false}),
            StrictSchemaError::OptionalProperty {
                path: String::new(),
                property: "a".to_owned(),
            },
        ),
        (
            json!({"oneOf": [{"type": "string"}, {"type": "integer"}]}),
            StrictSchemaError::ForbiddenKeyword {
                keyword: "oneOf".to_owned(),
                path: String::new(),
            },
        ),
        (
            json!({"$ref": "https://example.com/s.json"}),
            StrictSchemaError::NonLocalRef {
                reference: "https://example.com/s.json".to_owned(),
                path: String::new(),
            },
        ),
        (
            json!({"type": "integer", "format": "uint32"}),
            StrictSchemaError::UnsupportedFormat {
                format: "uint32".to_owned(),
                path: String::new(),
            },
        ),
    ];
    for (schema, expected) in cases {
        assert_eq!(strict::check(&schema), Err(expected));
    }
}

/// A data-carrying enum: schemars emits `oneOf` of objects, which the subset forbids.
#[derive(JsonSchema)]
#[expect(dead_code, reason = "schema-only test type")]
enum Tagged {
    /// A variant with data.
    A { x: String },
    /// Another variant with data.
    B { y: String },
}

#[test]
fn strict_transform_rejects_data_enums() {
    let error = strict::schema_for::<Tagged>().expect_err("oneOf of objects is outside the subset");
    assert!(
        matches!(error, StrictSchemaError::ForbiddenKeyword { ref keyword, .. } if keyword == "oneOf")
    );
}

#[test]
fn strict_transform_closes_objects_and_requires_nullable_fields() {
    #[derive(JsonSchema)]
    #[expect(dead_code, reason = "schema-only test type")]
    struct Probe {
        a: Option<String>,
        b: Vec<u32>,
    }
    let schema = strict::schema_for::<Probe>().expect("strict");
    assert_eq!(schema["required"], json!(["a", "b"]));
    assert_eq!(schema["additionalProperties"], json!(false));
    assert_eq!(schema["properties"]["a"]["type"], json!(["string", "null"]));
    assert!(schema["properties"]["b"]["items"].get("format").is_none());
}

#[test]
fn check_rejects_keywords_outside_the_openai_subset() {
    for keyword in [
        "dependentRequired",
        "dependentSchemas",
        "allOf",
        "not",
        "if",
        "then",
        "else",
        "patternProperties",
        "unevaluatedProperties",
        "propertyNames",
        "contains",
        "uniqueItems",
    ] {
        let mut schema = json!({"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a"],
                                "additionalProperties": false});
        schema[keyword] = json!({});
        assert_eq!(
            strict::check(&schema),
            Err(StrictSchemaError::ForbiddenKeyword {
                keyword: keyword.to_owned(),
                path: String::new(),
            }),
            "{keyword}"
        );
    }
    let nested = json!({"type": "object", "properties": {"a": {"type": "object", "properties": {},
        "required": [], "additionalProperties": false, "dependentRequired": {"x": ["y"]}}},
        "required": ["a"], "additionalProperties": false});
    assert_eq!(
        strict::check(&nested),
        Err(StrictSchemaError::ForbiddenKeyword {
            keyword: "dependentRequired".to_owned(),
            path: "/properties/a".to_owned(),
        })
    );
}

#[test]
fn check_rejects_dangling_local_refs() {
    let schema = json!({"type": "object", "properties": {"a": {"$ref": "#/$defs/Missing"}}, "required": ["a"],
                        "additionalProperties": false,
                        "$defs": {"Present": {"type": "string"}}});
    assert_eq!(
        strict::check(&schema),
        Err(StrictSchemaError::DanglingRef {
            reference: "#/$defs/Missing".to_owned(),
            path: "/properties/a".to_owned(),
        })
    );
    let resolved = json!({"type": "object", "properties": {"a": {"$ref": "#/$defs/Present"}, "b": {"$ref": "#"}},
                          "required": ["a", "b"], "additionalProperties": false,
                          "$defs": {"Present": {"type": "string"}}});
    assert_eq!(strict::check(&resolved), Ok(()));
}

#[test]
fn output_schemas_need_an_object_root() {
    let list = strict::schema_for::<Vec<String>>().expect("generic subset");
    assert_eq!(
        strict::check_output(&list),
        Err(StrictSchemaError::RootNotObject)
    );
    let any_of = json!({"anyOf": [{"type": "object", "properties": {}, "required": [], "additionalProperties": false},
                                  {"type": "string"}]});
    assert_eq!(
        strict::check_output(&any_of),
        Err(StrictSchemaError::RootNotObject)
    );
    for (kind, schema) in all_schemas() {
        assert_eq!(strict::check_output(&schema), Ok(()), "{kind:?}");
    }
}

#[test]
fn concise_low_assessment_schema_accepts_omitted_detail_fields() {
    let (_, mut flow) = samples()
        .into_iter()
        .find(|(kind, _)| *kind == JobKind::AnalyseFlow)
        .expect("flow");
    flow["steps"][0]["assessment"] =
        json!({"risk":"low","reason":"The retry remains bounded to this request."});
    let validator = validator(JobKind::AnalyseFlow);
    assert!(
        validator.is_valid(&flow),
        "{:?}",
        validator
            .iter_errors(&flow)
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
    );
    flow["steps"][0]["assessment"]["risk"] = json!("high");
    assert!(!validator.is_valid(&flow));
}

#[test]
fn plan_schema_and_decoder_exclude_test_classification() {
    let schema = output_schema(JobKind::Plan).unwrap();
    let choices = schema["properties"]["flows"]["items"]["properties"]["classification"]["enum"]
        .as_array()
        .unwrap();
    assert!(!choices.contains(&json!("test")));
    let mut plan: Value =
        serde_json::from_str(include_str!("../../../fixtures/pr-482/agent/plan.json")).unwrap();
    plan["flows"][0]["classification"] = json!("test");
    assert!(serde_json::from_value::<PlanOutput>(plan).is_err());
}
