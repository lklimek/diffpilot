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
        "schema_version":"4.2.1", "report_type":"comment_check",
        "metadata":{"input_mode":"host_supplied","pr_number":12,"commit":"abc","future":true},
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
    let files = json!({"manifest":{"repo":"o/r","pr":1,"head":"head","pr_author":"author","maintainers":["maintainer"],"bots":["bot"]},"threads":[],"review_comments":[],"reviews":[],"comments":[],"conventions":{"AGENTS.md":"Conventions"},"ids":{"next_id":2,"items":[{"id":"CMT-001","key":"review:1:1"}]},"scope":{"threads":["T1"],"sources":["https://github.com/o/r/pull/1#review-1"]}});
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
