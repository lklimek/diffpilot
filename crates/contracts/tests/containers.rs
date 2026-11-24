//! Common container contract round trips, schemas and usage accounting.

use contracts::gateway::{CredentialState, CredentialStatus, RelinkReason};
use contracts::ids::UserId;
use contracts::job::result::JobFailureKind;
use contracts::runtime::{AskPreflight, ContainerId, SessionSeed, ThreadKind, Workspace};
use serde_json::json;

#[test]
fn fixed_workspace_and_thread_capabilities() {
    assert_eq!(Workspace::PR.cwd(), "/work");
    assert_eq!(Workspace::PR.flow_output(), Some("/work/out/flow.md"));
    assert_eq!(Workspace::Judge.cwd(), "/tmp");
    assert!(!Workspace::Judge.allows_commands());
    assert!(ThreadKind::Analytic.memcan_write());
    assert!(!ThreadKind::Ask.memcan_write());
    assert!(!ThreadKind::Verify.memcan_read());
    assert!(!ThreadKind::Judge.memcan_read());
    let id = ContainerId::Pr {
        pull_request_id: 42,
        payer: UserId("u".into()),
    };
    assert_eq!(
        serde_json::from_value::<ContainerId>(serde_json::to_value(&id).unwrap()).unwrap(),
        id
    );
}

#[test]
fn preflight_is_bound_to_seed_and_generation() {
    let preflight = AskPreflight {
        seed: SessionSeed::Fork,
        generation: 7,
        state: contracts::runtime::PreflightState::Pending,
    };
    assert_eq!(
        serde_json::from_value::<AskPreflight>(serde_json::to_value(&preflight).unwrap()).unwrap(),
        preflight
    );
    assert_eq!(
        serde_json::to_value(preflight).unwrap(),
        json!({"seed":"fork","generation":7,"state":"pending"})
    );
}

#[test]
fn terminal_and_quarantined_credentials_are_safe_and_distinct() {
    for reason in [RelinkReason::Terminal, RelinkReason::Quarantined] {
        let status = CredentialStatus {
            telemetry: Default::default(),
            challenge: None,
            mode: contracts::gateway::CredentialMode::ChatgptPlan,
            state: CredentialState::RelinkNeeded,
            relink_reason: Some(reason),
            label: None,
            plan: None,
            paused_until: None,
            last_call: None,
        };
        assert_eq!(
            serde_json::from_value::<CredentialStatus>(serde_json::to_value(&status).unwrap())
                .unwrap(),
            status
        );
    }
    assert!(!JobFailureKind::InvalidJsonSchema.retryable());
    assert!(!JobFailureKind::CredentialRelink.retryable());
    assert!(JobFailureKind::WorkerLost.retryable());
}

#[test]
fn report_rows_parse_independently_and_unknown_fields_are_ignored() {
    use contracts::agent::comment_report::{CommentReport, ReportRow};
    let report: CommentReport = serde_json::from_value(json!({
        "schema_version":"4.2.1",
        "metadata":{"report_type":"comment_check","input_mode":"host_supplied","pr_number":12,"commit":"abc","future":true},
        "comment_assessments":[{"id":"CMT-001","source_kind":"inline","reviewer":"bot","comment_url":"https://github.com/o/r/pull/12#x","thread_id":"T1","description":"Issue","reasoning":"Read code","assessment":"already_fixed","proposed_action":"resolve","proposed_decision":{"kind":"resolve"},"future":42}, {"broken":true}], "future":null
    })).unwrap();
    assert!(report.supported());
    let row: ReportRow = serde_json::from_value(report.comment_assessments[0].clone()).unwrap();
    assert_eq!(
        serde_json::to_value(row.assessment).unwrap(),
        "already_fixed"
    );
    assert!(serde_json::from_value::<ReportRow>(report.comment_assessments[1].clone()).is_err());
    let mut incompatible = report;
    incompatible.schema_version = "4.1.0".into();
    assert!(!incompatible.supported());
    incompatible.schema_version = "4.2.0".into();
    incompatible.metadata.input_mode = "fetched".into();
    assert!(!incompatible.supported());
}

#[test]
fn usage_charges_turn_deltas_and_compaction_without_fork_inheritance() {
    use contracts::job::result::Usage;
    let inherited = Usage {
        input_tokens: 300_000,
        cached_input_tokens: 200_000,
        output_tokens: 500,
        requests: Some(4),
    };
    let compacted = Usage {
        input_tokens: 350_000,
        cached_input_tokens: 200_000,
        output_tokens: 700,
        requests: Some(5),
    };
    let answered = Usage {
        input_tokens: 450_000,
        cached_input_tokens: 225_000,
        output_tokens: 900,
        requests: Some(6),
    };
    let compact = compacted.delta_from(inherited).unwrap();
    let answer = answered.delta_from(compacted).unwrap();
    assert_eq!(
        compact.checked_add(answer).unwrap(),
        Usage {
            input_tokens: 150_000,
            cached_input_tokens: 25_000,
            output_tokens: 400,
            requests: Some(2)
        }
    );
    assert!(inherited.delta_from(compacted).is_err());
    let missing_requests = Usage {
        requests: None,
        ..compacted
    };
    assert_eq!(
        missing_requests.delta_from(inherited).unwrap().requests,
        None
    );
}

#[test]
fn common_runtime_schemas_validate_round_trips() {
    let value = serde_json::json!({"seed":"fork","generation":9,"state":"pending"});
    let schema =
        serde_json::to_value(schemars::schema_for!(contracts::runtime::AskPreflight)).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    assert!(validator.is_valid(&value));
    assert!(
        !validator
            .is_valid(&serde_json::json!({"seed":"fork","generation":9,"state":"assumed_done"}))
    );
    let status =
        serde_json::to_value(schemars::schema_for!(contracts::gateway::CredentialStatus)).unwrap();
    assert!(!status.to_string().contains("api_key"));
    assert!(!status.to_string().contains("credential_mode"));
}

#[test]
fn host_files_and_launcher_admin_messages_round_trip_without_exposing_secrets() {
    use contracts::{
        agent::pr_data::PrData,
        gateway::{MintedKey, RevokeKeys, UnlinkCredential},
        launcher::LauncherRequest,
    };
    let files = json!({"manifest":{"repo":"o/r","pr":1,"head":"head","base_ref":"main","base_sha":"base","title":"Title","body":"Body","pr_author":"author","maintainers":["maintainer"],"bots":["bot"]},"threads":[],"review_comments":[],"reviews":[],"comments":[],"conventions":{"AGENTS.md":"Conventions"},"ids":{"next_id":2,"items":[{"id":"CMT-001","key":"review:1:1"}]},"scope":{"threads":["T1"],"sources":["https://github.com/o/r/pull/1#review-1"]}});
    assert_eq!(
        serde_json::to_value(serde_json::from_value::<PrData>(files.clone()).unwrap()).unwrap(),
        files
    );
    let ensure = json!({"id":"req-1","action":{"operation":"ensure","container":{"shape":"pr","pull_request_id":1,"payer":"u1"},"memcan":{"url":"http://fake.invalid/mcp","token":"fake-memcan-secret","allow_write":true}}});
    let request: LauncherRequest = serde_json::from_value(ensure.clone()).unwrap();
    assert_eq!(serde_json::to_value(&request).unwrap(), ensure);
    assert!(!format!("{request:?}").contains("fake-memcan-secret"));
    let minted = json!({"id":"k1","key":"fake-virtual-secret","expires_at":"2026-11-23T12:00:00Z"});
    let key: MintedKey = serde_json::from_value(minted).unwrap();
    assert_eq!(
        serde_json::from_value::<MintedKey>(serde_json::to_value(&key).unwrap()).unwrap(),
        key
    );
    assert!(!format!("{key:?}").contains("fake-virtual-secret"));
    let local = json!({"container":{"shape":"judge","payer":"u1"},"reason":"retirement"});
    assert!(serde_json::from_value::<RevokeKeys>(local.clone()).is_ok());
    assert!(serde_json::from_value::<UnlinkCredential>(local).is_err());
    assert!(serde_json::from_value::<RevokeKeys>(json!({"user":"u1"})).is_err());
}

#[test]
fn host_report_metadata_survives_decoding() {
    use contracts::agent::comment_report::CommentReport;
    let envelope = json!({"schema_version":"4.2.0", "metadata":{"report_type":"comment_check", "input_mode":"host_supplied", "pr_number":42,"commit":"frozen"}, "comment_assessments":[]});
    let envelope = std::env::var("CLAUDIUS_HOST_FIXTURE")
        .map(|path| serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap())
        .unwrap_or(envelope);
    let report: CommentReport = serde_json::from_value(envelope).unwrap();
    assert!(report.supported());
    for row in &report.comment_assessments {
        serde_json::from_value::<contracts::agent::comment_report::ReportRow>(row.clone()).unwrap();
    }
}

#[test]
fn host_standing_decision_has_targets_without_invented_provenance() {
    use contracts::agent::pr_data::PrDataItem;
    let item = json!({"id":"CMT-001","key":"review:42:1","source_kind":null,
        "decision":{"kind":"handled_in_pr","target_pr":17,"target_url":null,"title":null}});
    let decoded: PrDataItem = serde_json::from_value(item).unwrap();
    assert_eq!(
        serde_json::to_value(decoded).unwrap()["decision"]["target_pr"],
        17
    );
}

#[test]
fn duplicate_targets_and_relay_author_survive_decoding() {
    use contracts::agent::comment_report::ReportRow;
    for (id, canonical) in [("CMT-003", "CMT-001"), ("CMT-004", "CMT-002")] {
        let row = json!({"id":id,"source_kind":"review_body","reviewer":"reviewer","comment_url":"https://github.com/o/r/pull/42#review-1","source_quote":format!("Point {id}"),"description":"Duplicate","reasoning":"Same concern","proposed_action":"Fix here","proposed_decision":{"kind":"fix_here"},"merged_into":canonical,"recorded_decision":{"kind":"fix_here","by":"delegate","on_behalf_of":"owner","comment_url":"https://github.com/o/r/pull/42#comment-2","quote":"Owner requests a fix"}});
        let decoded: ReportRow = serde_json::from_value(row).unwrap();
        let stored = serde_json::to_value(decoded).unwrap();
        assert_eq!(stored["merged_into"], canonical);
        assert_eq!(stored["recorded_decision"]["on_behalf_of"], "owner");
    }
}

#[test]
fn host_manifest_requires_intent_base_and_preserves_delegates() {
    use contracts::agent::pr_data::Manifest;
    let manifest = json!({"repo":"o/r","pr":42,"head":"frozen","base_ref":"main","base_sha":"base","title":"Intent","body":"Requested change","pr_author":"author","maintainers":["owner"],"bots":[],"delegates":["relay"]});
    let decoded: Manifest = serde_json::from_value(manifest.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), manifest);
    let schema = serde_json::to_value(schemars::schema_for!(Manifest)).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    for field in ["base_ref", "base_sha", "title", "body"] {
        let mut missing = manifest.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(!validator.is_valid(&missing), "{field}");
        assert!(
            serde_json::from_value::<Manifest>(missing).is_err(),
            "{field}"
        );
    }
}

#[test]
fn plan_credentials_reject_other_modes_and_link_secrets() {
    let status = json!({"mode":"api_key","state":"linked","relink_reason":null,"label":null,"plan":null,"paused_until":null,"last_call":null});
    assert!(serde_json::from_value::<CredentialStatus>(status).is_err());
    let generated = contracts::generate::generated_files().unwrap();
    let link = &generated
        .iter()
        .find(|f| f.path == "schemas/runtime/link_credential.schema.json")
        .unwrap()
        .contents;
    let schema: serde_json::Value = serde_json::from_str(link).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    assert!(validator.is_valid(&json!({"mode":"chatgpt_plan"})));
    for value in [
        json!({"mode":"api_key"}),
        json!({"mode":"chatgpt_plan","api_key":"fake"}),
        json!({"mode":"chatgpt_plan","base_url":"https://fake.invalid"}),
        json!({"mode":"chatgpt_plan","token":"fake"}),
    ] {
        assert!(!validator.is_valid(&value), "{value}");
    }
}

#[test]
fn assessment_contracts_include_both_native_formats_and_safe_quota() {
    let files = contracts::generate::generated_files().unwrap();
    for path in [
        "schemas/runtime/verify_comments_input.schema.json",
        "schemas/runtime/assessment_run_ref.schema.json",
        "schemas/api/assessment_settings.schema.json",
        "schemas/api/assessment_comparison.schema.json",
        "schemas/runtime/assessment_admission_failure.schema.json",
    ] {
        assert!(files.iter().any(|f| f.path == path), "{path}");
    }
}

#[test]
fn assessment_config_defaults_and_deadline_are_frozen_at_admission() {
    use contracts::assessment::{AssessmentAdmissionFailure, AssessmentConfig, AssessmentMethod};
    let config: AssessmentConfig = serde_json::from_value(json!({})).unwrap();
    assert_eq!(config.primary_method, AssessmentMethod::ClaudiusHost);
    assert!(config.ab_prs.is_empty());
    assert!(config.ab_until.is_none());
    let at = |s: &str| s.parse::<contracts::time::Timestamp>().unwrap();
    let config = AssessmentConfig {
        primary_method: AssessmentMethod::PromptV4,
        ab_prs: vec!["o/r#1".into()],
        ab_until: Some(at("2026-11-24T00:00:00Z")),
    };
    assert!(config.paired("o/r#1", at("2026-11-25T00:00:00Z")));
    assert!(config.paired("o/r#2", at("2026-11-23T23:59:59Z")));
    assert!(!config.paired("o/r#2", at("2026-11-24T00:00:00Z")));
    assert!(serde_json::from_value::<AssessmentConfig>(json!({"primary_method":"other"})).is_err());
    let failure = AssessmentAdmissionFailure::AssessmentDailyLimit {
        resets_at: at("2026-11-24T00:00:00Z"),
    };
    assert_eq!(
        serde_json::to_value(failure).unwrap(),
        json!({"code":"assessment_daily_limit","resets_at":"2026-11-24T00:00:00Z"})
    );
}

#[test]
fn assessment_typescript_numbers_match_json_numbers() {
    use ts_rs::TS;
    let config = ts_rs::Config::new();
    for declaration in [
        contracts::job::result::Usage::decl(&config),
        contracts::assessment::AssessmentInput::decl(&config),
        contracts::assessment::AssessmentMethodRun::decl(&config),
    ] {
        assert!(!declaration.contains("bigint"), "{declaration}");
    }
}

#[test]
fn device_link_response_contains_only_the_public_code_url_and_expiry() {
    let value = serde_json::json!({"user_code":"FAKE-CODE","verification_url":"https://auth.openai.com/codex/device",
        "expires_at":"2026-11-23T20:00:00Z"});
    let link: contracts::gateway::DeviceLink = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(link).unwrap(), value);
}
