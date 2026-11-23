//! Job contract: wire format, secrets, state machine, no floats (plan §5.3; runner protocol).
#![expect(clippy::expect_used, reason = "test helpers outside #[test] functions")]

use contracts::agent::JobKind;
use contracts::common::RepoRef;
use contracts::ids::{AnalysisId, CommitSha, JobId, PassId, RefreshId};
use contracts::job::access::{AgentAccess, InstallationToken, McpHttpEndpoint, Secret};
use contracts::job::event::{ActivityKind, JobEvent};
use contracts::job::protocol::{
    Claim, ClaimRequest, ClaimResponse, CredentialStatus, EventBatch, EventsAck, LeaseTiming,
    OutputReady,
};
use contracts::job::result::{
    JobFailureKind, JobOutcome, JobResult, LlmCallOutcome, LlmCallStatus, SessionOutcome,
    SessionReport, Usage,
};
use contracts::job::spec::{
    CredentialKind, JobLimits, JobSpec, JobTarget, LlmCredentialRef, ModelChoice,
    PullRequestCommentsTarget, PullRequestTarget, ReasoningEffort, RepoCheckout, SessionRef,
};
use contracts::job::state::{JobState, JobTransition};
use contracts::time::Timestamp;
use schemars::JsonSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

const GITHUB_TOKEN: &str = "ghs_example.installation.token";
const MEMCAN_TOKEN: &str = "memcan-example-token";
const JOB_TOKEN: &str = "job-example-token";

fn spec() -> JobSpec {
    let analysis_id = AnalysisId::new_random();
    let at = "2026-11-16T18:00:00Z"
        .parse::<Timestamp>()
        .expect("valid time");
    JobSpec {
        job_id: JobId::new_random(),
        attempt: 1,
        kind: JobKind::AnalyseFlow,
        target: JobTarget::PullRequest(PullRequestTarget {
            analysis_id,
            pull_request_id: 1,
            repo: RepoCheckout {
                clone_url: "https://github.com/acme/node-client.git".to_owned(),
                base_sha: CommitSha("1".repeat(40)),
                head_sha: CommitSha("9".repeat(40)),
            },
        }),
        prompt: "Job: analyse_flow f1.".to_owned(),
        output_schema: json!({"type": "object"}),
        flow_file: None,
        assessment: None,
        pr_data: None,
        model: ModelChoice {
            model: "gpt-6.1-sol".to_owned(),
            reasoning_effort: ReasoningEffort::High,
        },
        limits: JobLimits::default(),
        payer: contracts::ids::UserId("codex-default".to_owned()),
        session: Some(SessionRef {
            pull_request_id: 1,
            thread_id: "019a0000-0000-7000-8000-000000000001".to_owned(),
        }),
        working_session: None,
        context_pack: Some("PR context and plan.".to_owned()),
        skills: vec!["severity".to_owned()],
        access: AgentAccess {
            github: Some(InstallationToken {
                token: Secret::new(GITHUB_TOKEN),
                expires_at: at,
                repository: RepoRef {
                    owner: "acme".to_owned(),
                    name: "node-client".to_owned(),
                },
            }),
            memcan: Some(McpHttpEndpoint {
                url: "https://memcan.example/mcp".to_owned(),
                token: Secret::new(MEMCAN_TOKEN),
                allow_write: true,
            }),
        },
    }
}

fn claim_response() -> ClaimResponse {
    ClaimResponse {
        retire: Vec::new(),
        claim: Some(Claim {
            job_token: Secret::new(JOB_TOKEN),
            lease_expires_at: Timestamp::now(),
            spec: spec(),
        }),
        drop_sessions: vec![1],
    }
}

fn result() -> JobResult {
    JobResult {
        assessment: None,
        report: None,
        job_id: JobId::new_random(),
        attempt: 2,
        outcome: JobOutcome::Succeeded {
            output: json!({"answer": "x", "evidence": []}),
        },
        session: Some(SessionReport {
            last_context: None,
            preflight: None,
            thread_id: "019a0000-0000-7000-8000-000000000001".to_owned(),
            outcome: SessionOutcome::StartedAfterLoss,
        }),
        usage: Some(Usage {
            input_tokens: 214_700,
            cached_input_tokens: 190_000,
            output_tokens: 3_000,
            requests: None,
        }),
        skills_commit: Some("d31993add8b8332ae03352c471be2931c72f7c24".to_owned()),
        llm_call: Some(LlmCallOutcome {
            at: Timestamp::now(),
            status: LlmCallStatus::Working,
        }),
    }
}

fn round_trip<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(value: &T) {
    let json = serde_json::to_value(value).expect("serialises");
    let back: T = serde_json::from_value(json.clone()).expect("deserialises");
    assert_eq!(&back, value);
    assert_eq!(serde_json::to_value(&back).expect("serialises"), json);
}

#[test]
fn job_messages_round_trip() {
    round_trip(&spec());
    round_trip(&claim_response());
    round_trip(&result());
    round_trip(&ClaimRequest {
        runner_id: "runner-1".to_owned(),
        credentials: vec![CredentialStatus {
            credential: LlmCredentialRef {
                id: "codex-default".to_owned(),
                kind: CredentialKind::CodexChatgpt,
            },
            usable: true,
        }],
        dropped_sessions: vec![],
    });
    round_trip(&EventBatch {
        attempt: 1,
        events: vec![JobEvent {
            seq: 1,
            at: Timestamp::now(),
            activity: ActivityKind::McpToolCall,
            summary: "github.pull_request_read".to_owned(),
        }],
    });
    round_trip(&EventsAck {
        cancel_requested: true,
        lease_expires_at: Timestamp::now(),
    });
}

#[test]
fn secrets_travel_on_the_wire_but_never_in_debug() {
    let response = claim_response();
    let debug = format!("{response:?}");
    let wire = serde_json::to_string(&response).expect("serialises");
    for secret in [GITHUB_TOKEN, MEMCAN_TOKEN, JOB_TOKEN] {
        assert!(!debug.contains(secret), "{secret} in Debug output");
        assert!(wire.contains(secret), "{secret} missing on the wire");
    }
    assert!(debug.contains("Secret([redacted])"));
}

#[test]
fn outcome_wire_format_is_internally_tagged() {
    let cases = [
        (JobOutcome::Cancelled, json!({"outcome": "cancelled"})),
        (
            JobOutcome::UsageLimited { resets_at: None },
            json!({"outcome": "usage_limited", "resets_at": null}),
        ),
        (
            JobOutcome::Failed {
                kind: JobFailureKind::CredentialUnusable,
                message: "Sign in again.".to_owned(),
            },
            json!({"outcome": "failed", "kind": "credential_unusable", "message": "Sign in again."}),
        ),
    ];
    for (outcome, expected) in cases {
        assert_eq!(
            serde_json::to_value(&outcome).expect("serialises"),
            expected
        );
    }
    assert_eq!(
        serde_json::to_value(LlmCallStatus::NotWorking {
            message: "limit".to_owned()
        })
        .expect("serialises"),
        json!({"state": "not_working", "message": "limit"})
    );
}

#[test]
fn job_target_wire_format_is_internally_tagged() {
    let refresh = RefreshId::new_random();
    let mut dashboard = spec();
    dashboard.target = JobTarget::Dashboard {
        refresh_id: refresh,
    };
    round_trip(&dashboard);
    let wire = serde_json::to_value(&dashboard).expect("serialises");
    assert_eq!(
        wire["target"],
        json!({"type": "dashboard", "refresh_id": refresh})
    );
    assert!(wire.get("analysis_id").is_none() && wire.get("repo").is_none());
    assert_eq!(dashboard.target.pull_request(), None);
    let mut comments = spec();
    let repo = comments.target.checkout().expect("checkout").clone();
    comments.target = JobTarget::PullRequestComments(PullRequestCommentsTarget {
        pull_request_id: 7,
        repo,
        pass_id: PassId::new_random(),
    });
    round_trip(&comments);
    let wire = serde_json::to_value(&comments).expect("serialises");
    assert_eq!(wire["target"]["type"], "pull_request_comments");
    assert_eq!(wire["target"]["pull_request_id"], 7);
    assert_eq!(comments.target.pull_request(), None);
    assert!(comments.target.checkout().is_some());
    assert_eq!(dashboard.target.checkout(), None);
    let pr = serde_json::to_value(spec()).expect("serialises");
    assert_eq!(pr["target"]["type"], "pull_request");
    assert_eq!(pr["target"]["pull_request_id"], 1);
    assert_eq!(pr["target"]["repo"]["head_sha"], "9".repeat(40));
}

#[test]
fn job_limits_default_to_45_minutes() {
    assert_eq!(JobLimits::default().max_runtime_secs, 2700);
}

#[test]
fn job_state_transitions() {
    use JobState::{Cancelled, Claimed, Failed, Paused, Queued, Running, Succeeded};
    use JobTransition::{Cancel, Claim, Fail, LeaseExpired, Pause, Resume, Start, Succeed};
    let transitions = [
        Claim,
        Start,
        Succeed,
        Fail,
        Cancel,
        Pause,
        Resume,
        LeaseExpired {
            exhausted: false,
            cancel_requested: false,
        },
        LeaseExpired {
            exhausted: true,
            cancel_requested: false,
        },
        LeaseExpired {
            exhausted: false,
            cancel_requested: true,
        },
        LeaseExpired {
            exhausted: true,
            cancel_requested: true,
        },
    ];
    let allowed = [
        (Queued, Claim, Claimed),
        (Queued, Cancel, Cancelled),
        (Claimed, Start, Running),
        (Claimed, Succeed, Succeeded),
        (Claimed, Fail, Failed),
        (Claimed, Cancel, Cancelled),
        (Claimed, Pause, Paused),
        (
            Claimed,
            LeaseExpired {
                exhausted: false,
                cancel_requested: false,
            },
            Queued,
        ),
        (
            Claimed,
            LeaseExpired {
                exhausted: true,
                cancel_requested: false,
            },
            Failed,
        ),
        // Cancel wins at lease expiry too: never re-claimed.
        (
            Claimed,
            LeaseExpired {
                exhausted: false,
                cancel_requested: true,
            },
            Cancelled,
        ),
        (
            Claimed,
            LeaseExpired {
                exhausted: true,
                cancel_requested: true,
            },
            Cancelled,
        ),
        (Running, Succeed, Succeeded),
        (Running, Fail, Failed),
        (Running, Cancel, Cancelled),
        (Running, Pause, Paused),
        (
            Running,
            LeaseExpired {
                exhausted: false,
                cancel_requested: false,
            },
            Queued,
        ),
        (
            Running,
            LeaseExpired {
                exhausted: true,
                cancel_requested: false,
            },
            Failed,
        ),
        // Cancel wins at lease expiry too: never re-claimed.
        (
            Running,
            LeaseExpired {
                exhausted: false,
                cancel_requested: true,
            },
            Cancelled,
        ),
        (
            Running,
            LeaseExpired {
                exhausted: true,
                cancel_requested: true,
            },
            Cancelled,
        ),
        (Paused, Resume, Queued),
        (Paused, Cancel, Cancelled),
    ];
    for from in JobState::ALL {
        for transition in transitions {
            let expected = allowed
                .iter()
                .find(|(state, t, _)| *state == from && *t == transition)
                .map(|(_, _, to)| *to);
            assert_eq!(
                from.transition(transition).ok(),
                expected,
                "{from:?} + {transition:?}"
            );
        }
        assert_eq!(
            from.is_terminal(),
            transitions.iter().all(|t| from.transition(*t).is_err()),
            "{from:?}"
        );
    }
}

#[test]
fn state_names_match_serde() {
    for state in JobState::ALL {
        assert_eq!(
            serde_json::to_value(state).expect("serialises"),
            json!(state.as_str())
        );
    }
    for kind in JobKind::ALL {
        assert_eq!(
            serde_json::to_value(kind).expect("serialises"),
            json!(kind.as_str())
        );
    }
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
fn no_floats_in_job_types() {
    assert!(contains_float(&schema::<f64>()), "the float detector works");
    for (name, schema) in [
        ("JobSpec", schema::<JobSpec>()),
        ("JobResult", schema::<JobResult>()),
        ("ClaimRequest", schema::<ClaimRequest>()),
        ("ClaimResponse", schema::<ClaimResponse>()),
        ("EventBatch", schema::<EventBatch>()),
        ("EventsAck", schema::<EventsAck>()),
    ] {
        assert!(!contains_float(&schema), "{name}");
    }
}

/// The validated final output travels before session archival, scrubbed and per attempt.
#[test]
fn output_ready_is_sent_per_attempt() {
    let ready = OutputReady {
        attempt: 2,
        output: json!({"answer": "x", "evidence": []}),
    };
    round_trip(&ready);
    assert_eq!(
        serde_json::to_value(&ready).expect("serialises"),
        json!({"attempt": 2, "output": {"answer": "x", "evidence": []}})
    );
    let route = contracts::routes::ROUTES
        .iter()
        .find(|route| route.path == "/runner/v1/jobs/{id}/output")
        .expect("output route");
    assert_eq!(route.auth, contracts::routes::RouteAuth::JobToken);
    assert_eq!(route.listener, contracts::routes::Listener::Runner);
}

/// Self-fencing: measured from the send time of the last acked request, the runner stops the agent, kills
/// it, copies the credential back and still has a margin before the lease granted for that request ends.
#[test]
fn self_fencing_ends_before_the_lease() {
    let t = LeaseTiming::V1;
    assert_eq!(t.lease_secs, 60);
    assert!(
        t.fence_after_send_secs + t.term_grace_secs + t.kill_and_cleanup_secs + t.margin_secs
            <= t.lease_secs,
        "{t:?}"
    );
    assert!(t.margin_secs >= 10, "{t:?}");
    assert!(
        2 * t.heartbeat_secs <= t.fence_after_send_secs,
        "one lost heartbeat must not fence: {t:?}"
    );
    assert!(t.output_retry_secs.iter().sum::<u32>() < t.fence_after_send_secs);
}

#[test]
fn memcan_endpoint_without_allow_write_is_read_only() {
    let endpoint: McpHttpEndpoint =
        serde_json::from_value(json!({"url": "https://memcan.example/mcp", "token": MEMCAN_TOKEN}))
            .expect("endpoint");
    assert!(!endpoint.allow_write);
}

#[test]
fn private_pr_ask_target_needs_neither_analysis_nor_pass() {
    let target: JobTarget = serde_json::from_value(json!({
        "type": "pull_request_ask", "pull_request_id": 42,
        "repo": {"clone_url": "file:///fake", "base_sha": "base", "head_sha": "head"}
    }))
    .expect("private Ask target");
    assert_eq!(target.analysis_id(), None);
    assert!(target.checkout().is_some());
    assert_eq!(
        serde_json::to_value(target).unwrap()["type"],
        "pull_request_ask"
    );
}

#[test]
fn private_pr_ask_target_rejects_analysis_and_pass_fields() {
    for field in ["analysis_id", "pass_id"] {
        let mut target = json!({"type":"pull_request_ask", "pull_request_id":42,
            "repo":{"clone_url":"file:///fake", "base_sha":"b", "head_sha":"h"}});
        target[field] = json!(AnalysisId::new_random());
        assert!(serde_json::from_value::<JobTarget>(target).is_err());
    }
}

#[test]
fn job_target_typescript_numbers_match_json_numbers() {
    use ts_rs::TS;
    let config = ts_rs::Config::new();
    for declaration in [
        contracts::job::spec::PullRequestAskTarget::decl(&config),
        PullRequestTarget::decl(&config),
        PullRequestCommentsTarget::decl(&config),
    ] {
        assert!(!declaration.contains("bigint"), "{declaration}");
    }
}
