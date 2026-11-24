use super::*;
use contracts::{ids::UserId, job::spec::WorkingSession};
use serde_json::json;

fn job(kind: JobKind) -> JobSpec {
    let mut spec = crate::test_support::spec(kind, std::path::Path::new("/unused"));
    spec.payer = UserId("payer-17".into());
    spec
}

#[test]
fn immutable_configs_have_fixed_cwd_and_tool_policy() {
    for kind in [
        ThreadKind::Analytic,
        ThreadKind::Ask,
        ThreadKind::Verify,
        ThreadKind::Judge,
    ] {
        let endpoint = contracts::job::access::McpHttpEndpoint {
            url: "http://memcan.fake/mcp".into(),
            token: contracts::job::access::Secret::new("fake-memcan"),
            allow_write: true,
        };
        let params =
            config::thread_params(kind, &job(JobKind::Plan).model, 272_000, Some(&endpoint));
        assert_eq!(
            params["cwd"],
            if kind == ThreadKind::Judge {
                "/tmp"
            } else {
                "/work"
            }
        );
        assert_eq!(params["approvalPolicy"], "never");
        assert_eq!(params["config"]["model_auto_compact_token_limit"], 244_800);
        assert_eq!(params["config"]["web_search"], "disabled");
        assert_eq!(
            params["config"]["mcp_servers"]["memcan"]["enabled"],
            kind.memcan_read()
        );
        if kind == ThreadKind::Ask {
            assert!(
                !params["config"]["mcp_servers"]["memcan"]["enabled_tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|x| x == "add_memory")
            );
        }
    }
}

#[test]
fn fork_context_is_never_billable_and_compaction_is_added() {
    let parent = Usage {
        input_tokens: 300_000,
        cached_input_tokens: 200_000,
        output_tokens: 9_000,
        requests: None,
    };
    let compact = Usage {
        input_tokens: 350_000,
        cached_input_tokens: 200_000,
        output_tokens: 9_500,
        requests: None,
    };
    let answer = Usage {
        input_tokens: 380_000,
        cached_input_tokens: 220_000,
        output_tokens: 10_000,
        requests: None,
    };
    let total = compact
        .delta_from(parent)
        .unwrap()
        .checked_add(answer.delta_from(compact).unwrap())
        .unwrap();
    assert_eq!(total.input_tokens, 80_000);
    assert_eq!(total.cached_input_tokens, 20_000);
    assert_eq!(total.output_tokens, 1_000);
    assert!(parent.delta_from(answer).is_err());
}

#[test]
fn schema_rejection_is_typed_even_with_other_error_info() {
    let error = rpc::failure(
        &json!({"message":"backend: {\"error\":{\"code\":\"invalid_json_schema\"}}", "codexErrorInfo":"other"}),
    );
    assert!(matches!(
        error,
        ProviderError::Runtime {
            kind: JobFailureKind::InvalidJsonSchema
        }
    ));
    assert!(!JobFailureKind::InvalidJsonSchema.retryable());
}

#[test]
fn preflight_requires_fresh_bound_context_and_acknowledgement() {
    let mut spec = job(JobKind::Ask);
    let working = spec.working_session.as_mut().unwrap();
    working.analytic_generation = 5;
    working.parent_context = Some(LastContext {
        tokens: 80_001,
        generation: 5,
    });
    assert!(config::needs_compaction(working, SessionSeed::Fork).unwrap());
    working.parent_context.as_mut().unwrap().tokens = 80_000;
    assert!(!config::needs_compaction(working, SessionSeed::Fork).unwrap());
    working.parent_context.as_mut().unwrap().generation = 4;
    assert!(config::needs_compaction(working, SessionSeed::Fork).is_err());
    assert!(!config::needs_compaction(working, SessionSeed::ContextPack).unwrap());
}

#[test]
fn payer_and_thread_kind_do_not_come_from_legacy_credential_or_paths() {
    let spec = job(JobKind::Ask);
    assert_eq!(
        container(&spec).unwrap(),
        ContainerId::Pr {
            pull_request_id: 1,
            payer: UserId("payer-17".into())
        }
    );
    let mut working: WorkingSession = spec.working_session.unwrap();
    working.kind = Some(ThreadKind::Verify);
    assert_eq!(config::working_kind(&working).unwrap(), ThreadKind::Verify);
    working.kind = Some(ThreadKind::Analytic);
    assert!(config::working_kind(&working).is_err());
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Normal,
    StartupRecovery,
    DropTurn,
    Schema,
    Tool,
    CompletionTool,
    LostWorking,
    Hang,
    BadMeter,
    Symlink,
    ModelsFailure,
    FetchNoLock,
    StopFailure,
    BadResponseId,
    BadJson,
    MissingThread,
    ServerRequest,
    MissingUsage,
    InvalidTemplate,
    Oversize,
}

struct Harness {
    provider: AppserverProvider,
    seen: Arc<Mutex<Vec<Value>>>,
    shutdown: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
    _dir: tempfile::TempDir,
    gateway: wiremock::MockServer,
}

impl Harness {
    async fn new(mode: Mode) -> Self {
        use wiremock::{
            Mock, ResponseTemplate,
            matchers::{method, path},
        };
        let gateway = wiremock::MockServer::start().await;
        let totals = Arc::new(std::sync::Mutex::new(Usage {
            requests: Some(0),
            ..Usage::default()
        }));
        Mock::given(method("POST"))
            .and(path("/admin/keys/mint"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"id":"key-17","key":"model-key-canary","expires_at":"2030-01-01T00:00:00Z"}),
            ))
            .mount(&gateway)
            .await;
        Mock::given(method("POST"))
            .and(path("/admin/keys/revoke"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&gateway)
            .await;
        let metered = Arc::clone(&totals);
        Mock::given(method("GET"))
            .and(path("/admin/keys/key-17/usage"))
            .respond_with(move |_: &wiremock::Request| {
                let mut usage = *metered.lock().unwrap();
                if mode == Mode::BadMeter {
                    usage.input_tokens += 1;
                }
                ResponseTemplate::new(200)
                    .set_body_json(json!({"user":"payer-17","usage":usage,"resets_at":null}))
            })
            .mount(&gateway)
            .await;
        let dir = tempfile::TempDir::new().unwrap();
        let socket = dir.path().join("launcher.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen);
        let (shutdown, mut stop) = watch::channel(false);
        let task = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    _ = stop.changed() => break,
                    Some(result) = connections.join_next(), if !connections.is_empty() => { result.unwrap(); },
                    accepted = listener.accept() => {
                        let (mut stream, _) = accepted.unwrap();
                        let seen = Arc::clone(&recorded);
                        let totals = Arc::clone(&totals);
                        connections.spawn(async move {
                            let mut bytes = Vec::new();
                            loop { let b = stream.read_u8().await.unwrap(); if b == b'\n' { break; } bytes.push(b); }
                            let request: Value = serde_json::from_slice(&bytes).unwrap();
                            let operation = request["action"]["operation"].as_str().unwrap().to_owned();
                            seen.lock().await.push(request.clone());
                            let workers = if operation == "list" && mode == Mode::StartupRecovery {
                                json!([{"container":{"shape":"pr","pull_request_id":1,"payer":"payer-17"},"running":true}])
                            } else {json!([])};
                            let response = if (mode == Mode::InvalidTemplate && operation == "ensure") || (mode == Mode::StopFailure && operation == "stop") {
                                json!({"id":request["id"],"error":"invalid_template","workers":[]})
                            } else if mode == Mode::FetchNoLock && operation == "fetch" {
                                json!({"id":request["id"],"fetch":"resolved_without_lock","workers":[]})
                            } else { json!({"id":request["id"],"workers":workers}) };
                            stream.write_all(format!("{response}\n").as_bytes()).await.unwrap();
                            if operation != "attach" { return; }
                            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                            let mut inherited = HashMap::<String, Usage>::new();
                            let mut last = HashMap::<String, u64>::new();
                            let mut serial = 0;
                            let mut ephemeral = std::collections::HashSet::new();
                            let mut materialized = std::collections::HashSet::from(["parent".to_owned(), "working".to_owned()]);
                            use futures::{SinkExt, StreamExt};
                            while let Some(Ok(message)) = ws.next().await {
                                let tokio_tungstenite::tungstenite::Message::Text(text) = message else { break; };
                                let request: Value = serde_json::from_str(&text).unwrap();
                                seen.lock().await.push(request.clone());
                                if request.get("id").is_none() { continue; }
                                let method = request["method"].as_str().unwrap_or("");
                                let p = &request["params"];
                                let mut result = json!({});
                                let mut notifications = Vec::new();
                                if method == "model/list" {
                                    let injected = match mode {
                                        Mode::BadJson => Some("invalid JSON".to_owned()),
                                        Mode::BadResponseId => Some(json!({"id":999999,"result":{}}).to_string()),
                                        Mode::ServerRequest => Some(json!({"id":"server-call","method":"item/tool/requestUserInput","params":{"secret":"model-key-canary"}}).to_string()),
                                        _ => None,
                                    };
                                    if let Some(injected) = injected {
                                        let _ = ws.send(tokio_tungstenite::tungstenite::Message::Text(injected.into())).await;
                                        if mode == Mode::ServerRequest { let _ = ws.next().await; }
                                        break;
                                    }
                                }
                                match method {
                                    "command/exec" => result = json!({"exitCode":0}),
                                    "skills/list" => result = json!({"data":[{"cwd":"/work","skills":[]}]}),
                                    "model/list" => {
                                        if mode == Mode::ModelsFailure {
                                            let _ = ws.send(tokio_tungstenite::tungstenite::Message::Text(json!({"id":request["id"],"error":{"code":-32603,"message":"models unavailable"}}).to_string().into())).await;
                                            continue;
                                        }
                                        result = json!({"data":[{"model":"gpt-6.1-sol"},{"model":"gpt-6-astra"}],"nextCursor":null});
                                    },
                                    "thread/start" | "thread/resume" | "thread/fork" => {
                                        let parent = p["threadId"].as_str().unwrap_or("");
                                        if (mode == Mode::LostWorking && parent == "lost")
                                            || (matches!(method, "thread/resume" | "thread/fork")
                                                && (ephemeral.contains(parent) || !materialized.contains(parent))) {
                                            let _ = ws.send(tokio_tungstenite::tungstenite::Message::Text(json!({"id":request["id"],"error":{"code":-32603,"message":format!("no rollout found for thread id {parent}")}}).to_string().into())).await;
                                            continue;
                                        }
                                        let thread = if method == "thread/resume" { parent.to_owned() } else { serial += 1; format!("thread-{serial}") };
                                        let baseline = inherited.get(parent).copied().unwrap_or(Usage { input_tokens:300_000, cached_input_tokens:200_000, output_tokens:9_000, requests: None });
                                        if p["ephemeral"] == true { ephemeral.insert(thread.clone()); }
                                        if method == "thread/fork" { materialized.insert(thread.clone()); }
                                        if method == "thread/start" { inherited.insert(thread.clone(),Usage::default()); last.insert(thread.clone(),0); }
                                        else { inherited.insert(thread.clone(),baseline); last.entry(thread.clone()).or_insert(90_000); }
                                        let total = inherited[&thread];
                                        let history = total.input_tokens != 0;
                                        result = json!({"thread":{"id":thread,"turns":if history { json!([{"id":"old","status":"completed","items":[]}]) } else { json!([]) }}});
                                        if history { notifications.push(token_notification(&thread, "old", total, last[&thread])); }
                                        if mode == Mode::MissingThread { result = json!({"thread":{}}); }
                                    },
                                    "turn/start" | "thread/compact/start" => {
                                        let thread = p["threadId"].as_str().unwrap();
                                        if method == "turn/start" && !ephemeral.contains(thread) { materialized.insert(thread.to_owned()); }
                                        if mode == Mode::DropTurn { break; }
                                        if mode == Mode::Oversize {
                                            let _ = ws.send(tokio_tungstenite::tungstenite::Message::Text("x".repeat(rpc::MAX_MESSAGE + 1).into())).await;
                                            break;
                                        }
                                        serial += 1;
                                        let turn = format!("turn-{serial}");
                                        result = if method == "turn/start" { json!({"turn":{"id":turn,"status":"inProgress","items":[]}}) } else { json!({}) };
                                        if mode != Mode::Hang {
                                            let compact = method == "thread/compact/start";
                                            let delta = Usage { input_tokens:if compact {20} else {100}, cached_input_tokens:if compact {0} else {40}, output_tokens:if compact {1} else {10}, requests:None };
                                            let total = inherited[thread].checked_add(delta).unwrap();
                                            inherited.insert(thread.into(),total);
                                            last.insert(thread.into(),if compact {30_000} else {31_000});
                                            let mut charge = *totals.lock().unwrap();
                                            charge.input_tokens += delta.input_tokens; charge.cached_input_tokens += delta.cached_input_tokens; charge.output_tokens += delta.output_tokens; charge.requests = Some(charge.requests.unwrap() + 1);
                                            *totals.lock().unwrap() = charge;
                                            if mode == Mode::Tool { notifications.push(json!({"method":"item/started","params":{"threadId":thread,"turnId":turn,"item":{"type":"dynamicToolCall","tool":"exec"}}})); }
                                            if mode != Mode::MissingUsage { notifications.push(token_notification(thread,&turn,total,last[thread])); }
                                            let item = json!({"type":"agentMessage","phase":"final_answer","text":"{\"answer\":\"model-key-canary github-token-canary\"}"});
                                            notifications.push(json!({"method":"item/completed","params":{"threadId":thread,"turnId":turn,"item":item}}));
                                            let error = if mode == Mode::Schema && !compact { json!({"message":"backend: {\"error\":{\"code\":\"invalid_json_schema\"}}", "codexErrorInfo":"other"}) } else { Value::Null };
                                            notifications.push(json!({"method":"turn/completed","params":{"threadId":thread,"turn":{"id":turn,"status":if mode == Mode::Schema && !compact {"failed"} else {"completed"},"error":error,"items":if mode == Mode::CompletionTool {json!([item,{"type":"commandExecution","command":"fake"}])} else {json!([item])}}}}));
                                        }
                                    },
                                    "fs/getMetadata" => result = json!({"isFile":true,"isDirectory":true,"isSymlink": mode == Mode::Symlink}),
                                    "fs/readFile" => {
                                        use base64::Engine;
                                        result = json!({"dataBase64":base64::engine::general_purpose::STANDARD.encode("flow model-key-canary github-token-canary")});
                                    },
                                    _ => {},
                                }
                                if ws.send(tokio_tungstenite::tungstenite::Message::Text(json!({"id":request["id"],"result":result}).to_string().into())).await.is_err() { break; }
                                for note in notifications {
                                    if ws.send(tokio_tungstenite::tungstenite::Message::Text(note.to_string().into())).await.is_err() { break; }
                                }
                            }
                        });
                    }
                }
            }
            connections.abort_all();
            while let Some(result) = connections.join_next().await {
                assert!(result.is_ok() || result.unwrap_err().is_cancelled());
            }
        });
        let provider = AppserverProvider::new(
            crate::launcher::Client::new(&socket),
            GatewayClient::new(
                url::Url::parse(&gateway.uri()).unwrap(),
                contracts::job::access::Secret::new("keys-scope-fake"),
            )
            .unwrap(),
            "71096d7".into(),
        );
        Self {
            provider,
            seen,
            shutdown,
            task,
            _dir: dir,
            gateway,
        }
    }

    async fn finish(self) -> Vec<Value> {
        self.shutdown.send(true).unwrap();
        self.task.await.unwrap();
        self.seen.lock().await.clone()
    }
}

fn token_notification(thread: &str, turn: &str, total: Usage, context: u64) -> Value {
    json!({"method":"thread/tokenUsage/updated","params":{"threadId":thread,"turnId":turn,"tokenUsage":{"total":{"inputTokens":total.input_tokens,"cachedInputTokens":total.cached_input_tokens,"outputTokens":total.output_tokens},"last":{"inputTokens":context,"outputTokens":0}}}})
}

fn secret_job(kind: JobKind) -> JobSpec {
    let mut spec = job(kind);
    spec.access.github = Some(contracts::job::access::InstallationToken {
        token: contracts::job::access::Secret::new("github-token-canary"),
        expires_at: contracts::time::Timestamp::from_second(1_900_000_000).unwrap(),
        repository: contracts::common::RepoRef {
            owner: "acme".into(),
            name: "node-client".into(),
        },
    });
    spec
}

#[tokio::test]
async fn flat_flow_restores_repair_and_scrubs_tokens_before_detach() {
    let harness = Harness::new(Mode::Normal).await;
    let mut spec = secret_job(JobKind::AnalyseFlow);
    spec.flow_file = Some(contracts::job::spec::FlowFile {
        context: json!({"frozen":true}),
        previous: Some("rejected flow".into()),
    });
    let (tx, _rx) = mpsc::channel(64);
    let outcome = harness.provider.execute_job(&spec, tx).await.unwrap();
    assert_eq!(outcome.output, "flow [redacted] [redacted]");
    assert_eq!(outcome.usage.input_tokens, 100);
    assert_eq!(outcome.usage.requests, Some(1));
    let requests = harness.gateway.received_requests().await.unwrap();
    assert!(
        requests
            .iter()
            .any(|r| r.url.path() == "/admin/keys/revoke")
    );
    let seen = harness.finish().await;
    let writes: Vec<_> = seen
        .iter()
        .filter(|r| r["method"] == "fs/writeFile")
        .collect();
    assert!(
        writes
            .iter()
            .any(|r| r["params"]["path"] == "/work/flow-context.json")
    );
    assert!(
        writes
            .iter()
            .any(|r| r["params"]["path"] == "/work/out/flow.md")
    );
    assert!(
        writes
            .iter()
            .rev()
            .take(2)
            .all(|r| r["params"]["dataBase64"] == "")
    );
    assert!(!seen.iter().any(|r| r["action"]["operation"] == "stop"));
    assert!(
        seen.iter()
            .filter(|r| r["method"] == "turn/start")
            .all(|r| r["params"].get("outputSchema").is_none())
    );
}

#[tokio::test]
async fn ask_fork_resumes_parent_and_compacts_once_without_billing_inheritance() {
    let harness = Harness::new(Mode::Normal).await;
    let mut spec = secret_job(JobKind::Ask);
    spec.session = Some(contracts::job::spec::SessionRef {
        pull_request_id: 1,
        thread_id: "parent".into(),
    });
    spec.working_session.as_mut().unwrap().analytic_generation = 7;
    let (tx, _rx) = mpsc::channel(64);
    let outcome = harness.provider.execute_job(&spec, tx).await.unwrap();
    assert_eq!(outcome.usage.input_tokens, 120);
    assert_eq!(outcome.usage.cached_input_tokens, 40);
    assert_eq!(outcome.usage.output_tokens, 11);
    assert_eq!(outcome.usage.requests, Some(2));
    let session = outcome.runtime_session.unwrap();
    assert_eq!(session.preflight.unwrap().state, PreflightState::Done);
    assert_eq!(session.last_context.unwrap().generation, 7);
    let seen = harness.finish().await;
    let methods: Vec<_> = seen.iter().filter_map(|r| r["method"].as_str()).collect();
    assert!(
        methods.iter().position(|m| *m == "thread/resume").unwrap()
            < methods.iter().position(|m| *m == "thread/fork").unwrap()
    );
    assert_eq!(
        methods
            .iter()
            .filter(|m| **m == "thread/compact/start")
            .count(),
        1
    );
    for r in seen.iter().filter(|r| {
        matches!(
            r["method"].as_str(),
            Some("thread/start" | "thread/resume" | "thread/fork")
        )
    }) {
        assert_eq!(r["params"]["cwd"], "/work");
        assert!(r["params"].get("config").is_some());
    }
}

#[tokio::test]
async fn cs2_has_no_commands_or_github_and_rejects_model_tool_items() {
    for mode in [Mode::Normal, Mode::Tool, Mode::CompletionTool] {
        let harness = Harness::new(mode).await;
        let mut spec = job(JobKind::JudgeWork);
        spec.target = JobTarget::Dashboard {
            refresh_id: contracts::ids::RefreshId::new_random(),
        };
        let (tx, _rx) = mpsc::channel(64);
        let result = harness
            .provider
            .execute_job(&spec, tx)
            .await
            .map_err(|e| e.into_parts().0);
        if mode == Mode::Normal {
            assert!(result.is_ok(), "{result:?}");
        } else {
            assert!(matches!(
                result,
                Err(ProviderError::Runtime {
                    kind: JobFailureKind::UpstreamRejected
                })
            ));
        }
        let seen = harness.finish().await;
        assert!(!seen.iter().any(
            |r| r["method"] == "command/exec" || r["params"]["path"] == "/run/dp/github-token"
        ));
        assert!(seen.iter().any(|r| r["action"]["operation"] == "stop"));
    }
}

#[tokio::test]
async fn relay_loss_and_oversize_stop_worker_without_accepting_output() {
    for mode in [Mode::DropTurn, Mode::Oversize] {
        let harness = Harness::new(mode).await;
        let (tx, mut rx) = mpsc::channel(64);
        let result = harness
            .provider
            .execute_job(&secret_job(JobKind::Plan), tx)
            .await
            .map_err(|e| e.into_parts().0);
        assert!(
            matches!(
                result,
                Err(ProviderError::Runtime {
                    kind: JobFailureKind::WorkerLost
                })
            ),
            "{result:?}"
        );
        while let Ok(event) = rx.try_recv() {
            assert!(!matches!(event, ProviderEvent::OutputReady(_)));
        }
        let seen = harness.finish().await;
        assert!(seen.iter().any(|r| r["action"]["operation"] == "stop"));
    }
}

#[tokio::test]
async fn schema_failure_and_meter_mismatch_fail_without_useful_output() {
    for (mode, kind) in [
        (Mode::Schema, JobFailureKind::InvalidJsonSchema),
        (Mode::BadMeter, JobFailureKind::ProviderFailed),
        (Mode::Symlink, JobFailureKind::ProviderFailed),
    ] {
        let harness = Harness::new(mode).await;
        let mut spec = secret_job(JobKind::Plan);
        if mode == Mode::Symlink {
            spec.flow_file = Some(contracts::job::spec::FlowFile {
                context: json!({}),
                previous: None,
            });
        }
        let (tx, _rx) = mpsc::channel(64);
        let result = harness
            .provider
            .execute_job(&spec, tx)
            .await
            .map_err(|e| e.into_parts().0);
        assert!(
            matches!(result,Err(ProviderError::Runtime {kind:actual}) if actual == kind),
            "{result:?}"
        );
        harness.finish().await;
    }
}

#[tokio::test]
async fn cancel_interrupts_subscribed_turn_then_stops_and_retires_key() {
    let harness = Harness::new(Mode::Hang).await;
    let spec = secret_job(JobKind::Plan);
    let (tx, _rx) = mpsc::channel(64);
    let run = harness.provider.execute_job(&spec, tx);
    let cancel = async {
        loop {
            if harness
                .seen
                .lock()
                .await
                .iter()
                .any(|r| r["method"] == "turn/start")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        harness.provider.cancel(spec.job_id).await.unwrap();
    };
    let (result, ()) =
        tokio::time::timeout(Duration::from_secs(20), async { tokio::join!(run, cancel) })
            .await
            .unwrap();
    assert!(matches!(result, Err(ProviderError::Cancelled)));
    let seen = harness.finish().await;
    assert!(seen.iter().any(|r| r["method"] == "turn/interrupt"));
    assert!(seen.iter().any(|r| r["action"]["operation"] == "stop"));
}

#[tokio::test]
async fn models_failure_is_typed_and_does_not_start_a_turn() {
    let harness = Harness::new(Mode::ModelsFailure).await;
    let (tx, _rx) = mpsc::channel(64);
    let result = harness
        .provider
        .execute_job(&secret_job(JobKind::Plan), tx)
        .await
        .map_err(|e| e.into_parts().0);
    assert!(
        matches!(
            result,
            Err(ProviderError::Runtime {
                kind: JobFailureKind::ModelUnavailable
            })
        ),
        "{result:?}"
    );
    let seen = harness.finish().await;
    assert!(!seen.iter().any(|r| r["method"] == "turn/start"));
}

#[tokio::test]
async fn private_context_pack_is_used_without_a_parent_and_no_compaction() {
    let harness = Harness::new(Mode::Normal).await;
    let mut spec = secret_job(JobKind::Ask);
    spec.context_pack = Some("private context sentinel".into());
    let (tx, _rx) = mpsc::channel(64);
    let result = harness.provider.execute_job(&spec, tx).await.unwrap();
    assert_eq!(
        result.runtime_session.unwrap().preflight.unwrap().seed,
        SessionSeed::ContextPack
    );
    let seen = harness.finish().await;
    let turn = seen.iter().find(|r| r["method"] == "turn/start").unwrap();
    assert!(
        turn["params"]["input"][0]["text"]
            .as_str()
            .unwrap()
            .contains("private context sentinel")
    );
    assert!(!seen.iter().any(|r| r["method"] == "thread/compact/start"));
}

#[tokio::test]
async fn timeout_stops_worker_and_retires_key() {
    let harness = Harness::new(Mode::Hang).await;
    let mut spec = secret_job(JobKind::Plan);
    spec.limits.max_runtime_secs = 1;
    let (tx, _rx) = mpsc::channel(64);
    assert!(matches!(
        harness.provider.execute_job(&spec, tx).await,
        Err(ProviderError::Timeout)
    ));
    let seen = harness.finish().await;
    assert!(seen.iter().any(|r| r["action"]["operation"] == "stop"));
}

#[test]
fn usage_limit_and_relink_are_distinct_from_retryable_provider_errors() {
    assert!(matches!(
        rpc::failure(&json!({"codexErrorInfo":"usageLimitExceeded","message":"Usage limited"})),
        ProviderError::UsageLimit { .. }
    ));
    assert!(matches!(
        rpc::failure(
            &json!({"message":"http 403: {\"error\":{\"code\":\"credential_unlinked\"}}"})
        ),
        ProviderError::Runtime {
            kind: JobFailureKind::CredentialRelink
        }
    ));
}

#[tokio::test]
async fn lost_working_thread_forks_the_payers_parent_before_context_pack_fallback() {
    let harness = Harness::new(Mode::LostWorking).await;
    let mut spec = secret_job(JobKind::Ask);
    spec.session = Some(contracts::job::spec::SessionRef {
        pull_request_id: 1,
        thread_id: "parent".into(),
    });
    spec.working_session.as_mut().unwrap().thread_id = Some("lost".into());
    spec.working_session.as_mut().unwrap().kind = Some(ThreadKind::Ask);
    spec.context_pack = Some("private recovery context".into());
    let (tx, _rx) = mpsc::channel(64);
    let result = harness.provider.execute_job(&spec, tx).await.unwrap();
    assert_eq!(
        result.runtime_session.unwrap().preflight.unwrap().seed,
        SessionSeed::Fork
    );
    let seen = harness.finish().await;
    assert!(seen.iter().any(|r| r["method"] == "thread/fork"));
}

#[test]
fn pending_preflight_cannot_authorize_an_answer_from_a_small_context() {
    let mut spec = job(JobKind::Ask);
    let working = spec.working_session.as_mut().unwrap();
    working.analytic_generation = 5;
    working.parent_context = Some(LastContext {
        tokens: 1000,
        generation: 5,
    });
    working.preflight = Some(AskPreflight {
        seed: SessionSeed::Fork,
        generation: 5,
        state: PreflightState::Pending,
    });
    assert!(config::needs_compaction(working, SessionSeed::Fork).unwrap());
}

#[tokio::test]
async fn failed_method_preserves_response_id_usage_separately_from_its_error() {
    let harness = Harness::new(Mode::Schema).await;
    let (tx, _rx) = mpsc::channel(64);
    let (error, usage) = harness
        .provider
        .execute_job(&secret_job(JobKind::Plan), tx)
        .await
        .unwrap_err()
        .into_parts();
    assert!(matches!(
        error,
        ProviderError::Runtime {
            kind: JobFailureKind::InvalidJsonSchema
        }
    ));
    assert_eq!(usage.unwrap().requests, Some(1));
    harness.finish().await;
}

#[tokio::test]
async fn startup_stops_existing_workers_before_the_first_new_turn() {
    let harness = Harness::new(Mode::StartupRecovery).await;
    let (tx, _rx) = mpsc::channel(64);
    harness
        .provider
        .execute_job(&secret_job(JobKind::Plan), tx)
        .await
        .unwrap();
    let seen = harness.finish().await;
    let stopped = seen
        .iter()
        .position(|r| r["action"]["operation"] == "stop")
        .unwrap();
    let ensured = seen
        .iter()
        .position(|r| r["action"]["operation"] == "ensure")
        .unwrap();
    assert!(stopped < ensured);
}

#[tokio::test]
async fn token_files_are_unlinked_before_replacement_on_a_warm_worker() {
    let harness = Harness::new(Mode::Normal).await;
    let (tx, _rx) = mpsc::channel(64);
    harness
        .provider
        .execute_job(&secret_job(JobKind::Plan), tx)
        .await
        .unwrap();
    let seen = harness.finish().await;
    for path in ["/run/dp/model-key", "/run/dp/github-token"] {
        let mut removed = false;
        for request in &seen {
            if request["params"]["path"] == path {
                if request["method"] == "fs/remove" {
                    removed = true;
                }
                if request["method"] == "fs/writeFile" {
                    assert!(removed, "token path must not follow an old symlink");
                    removed = false;
                }
            }
        }
    }
}

#[test]
fn schema_error_code_in_plain_backend_message_is_non_retryable() {
    assert!(matches!(
        rpc::failure(
            &json!({"message":"HTTP 400: invalid_json_schema (backend rejected the strict schema)","codexErrorInfo":"other"})
        ),
        ProviderError::Runtime {
            kind: JobFailureKind::InvalidJsonSchema
        }
    ));
}

#[test]
fn existing_working_threads_require_a_known_immutable_kind() {
    let mut working = job(JobKind::Ask).working_session.unwrap();
    working.thread_id = Some("working".into());
    assert!(config::working_kind(&working).is_err());
    working.kind = Some(ThreadKind::Ask);
    assert_eq!(config::working_kind(&working).unwrap(), ThreadKind::Ask);
    let mut spec = job(JobKind::Plan);
    spec.working_session = Some(working);
    assert!(container(&spec).is_err());
}

#[tokio::test]
async fn resumed_ask_keeps_acknowledged_preflight_and_excludes_inherited_usage() {
    let harness = Harness::new(Mode::Normal).await;
    let mut spec = secret_job(JobKind::Ask);
    let working = spec.working_session.as_mut().unwrap();
    working.kind = Some(ThreadKind::Ask);
    working.thread_id = Some("working".into());
    working.preflight = Some(AskPreflight {
        seed: SessionSeed::Fork,
        generation: 0,
        state: PreflightState::Done,
    });
    let (tx, _rx) = mpsc::channel(64);
    let result = harness.provider.execute_job(&spec, tx).await.unwrap();
    assert_eq!(result.usage.input_tokens, 100);
    assert_eq!(
        result.runtime_session.unwrap().preflight.unwrap().state,
        PreflightState::Done
    );
    let seen = harness.finish().await;
    assert!(
        !seen
            .iter()
            .any(|r| r["method"] == "thread/compact/start" || r["method"] == "thread/fork")
    );
}

#[test]
fn usage_limit_info_keeps_a_known_backend_reset_time() {
    let error = rpc::failure(
        &json!({"codexErrorInfo":"usageLimitExceeded","message":"HTTP 429: {\"error\":{\"code\":\"usage_limit_reached\",\"resets_at\":1900000000}}"}),
    );
    assert!(
        matches!(error,ProviderError::UsageLimit {resets_at:Some(at)} if at == contracts::time::Timestamp::from_second(1_900_000_000).unwrap())
    );
}

async fn assert_fresh_thread(kind: JobKind) {
    let harness = Harness::new(Mode::Normal).await;
    let mut spec = secret_job(kind);
    if kind == JobKind::Ask {
        spec.context_pack = Some("private context".into());
    }
    let (tx, _rx) = mpsc::channel(64);
    let result = harness.provider.execute_job(&spec, tx).await;
    let seen = harness.finish().await;
    assert!(result.is_ok(), "{kind:?}: {result:?}");
    assert!(!seen.iter().any(|r| r["method"] == "thread/resume"));
}

#[tokio::test]
async fn new_plan_starts_without_resuming_unmaterialized_thread() {
    assert_fresh_thread(JobKind::Plan).await;
}
#[tokio::test]
async fn context_pack_ask_starts_without_resuming_unmaterialized_thread() {
    assert_fresh_thread(JobKind::Ask).await;
}
#[tokio::test]
async fn verify_pass_never_resumes_ephemeral_thread() {
    assert_fresh_thread(JobKind::VerifyComments).await;
}

async fn assert_ephemeral_job_ignores_supplied_analytic_session(kind: JobKind) {
    let harness = Harness::new(Mode::Normal).await;
    let mut spec = job(kind);
    if kind == JobKind::JudgeWork {
        spec.target = JobTarget::Dashboard {
            refresh_id: contracts::ids::RefreshId::new_random(),
        };
    }
    spec.session = Some(contracts::job::spec::SessionRef {
        pull_request_id: 1,
        thread_id: "parent".into(),
    });
    let (tx, _rx) = mpsc::channel(64);
    let result = harness.provider.execute_job(&spec, tx).await;
    let seen = harness.finish().await;
    assert!(result.is_ok(), "{kind:?}: {result:?}");
    assert!(
        !seen.iter().any(|r| r["method"] == "thread/resume"),
        "{kind:?}"
    );
    let start = seen.iter().find(|r| r["method"] == "thread/start").unwrap();
    assert_eq!(start["params"]["ephemeral"], true, "{kind:?}");
}

#[tokio::test]
async fn verify_pass_ignores_supplied_analytic_session() {
    assert_ephemeral_job_ignores_supplied_analytic_session(JobKind::VerifyComments).await;
}

#[tokio::test]
async fn judge_job_ignores_supplied_analytic_session() {
    assert_ephemeral_job_ignores_supplied_analytic_session(JobKind::JudgeWork).await;
}

#[tokio::test]
async fn verify_ask_resumes_analytic_parent_with_analytic_policy() {
    let harness = Harness::new(Mode::Normal).await;
    let mut spec = secret_job(JobKind::Ask);
    spec.session = Some(contracts::job::spec::SessionRef {
        pull_request_id: 1,
        thread_id: "parent".into(),
    });
    spec.working_session.as_mut().unwrap().kind = Some(ThreadKind::Verify);
    spec.access.memcan = Some(contracts::job::access::McpHttpEndpoint {
        url: "http://memcan.fake/mcp".into(),
        token: contracts::job::access::Secret::new("fake"),
        allow_write: true,
    });
    let (tx, _rx) = mpsc::channel(64);
    harness.provider.execute_job(&spec, tx).await.unwrap();
    let seen = harness.finish().await;
    let parent = seen
        .iter()
        .find(|r| r["method"] == "thread/resume" && r["params"]["threadId"] == "parent")
        .unwrap();
    assert_eq!(
        parent["params"]["config"]["mcp_servers"]["memcan"]["enabled"],
        true
    );
    let fork = seen.iter().find(|r| r["method"] == "thread/fork").unwrap();
    assert_eq!(
        fork["params"]["config"]["mcp_servers"]["memcan"]["enabled"],
        false
    );
}

#[test]
fn deterministic_metadata_and_model_failures_are_not_retryable() {
    let spec = job(JobKind::Ask);
    assert!(matches!(
        config::needs_compaction(spec.working_session.as_ref().unwrap(), SessionSeed::Fork),
        Err(ProviderError::Runtime {
            kind: JobFailureKind::Configuration
        })
    ));
    assert!(matches!(
        rpc::failure(&json!({"codexErrorInfo":"modelNotFound","message":"unknown model"})),
        ProviderError::Runtime {
            kind: JobFailureKind::ModelUnavailable
        }
    ));
}

#[test]
fn memcan_allowlist_honours_thread_kind_and_endpoint_write_permission() {
    let read = json!([
        "search",
        "search_memories",
        "search_code",
        "search_standards"
    ]);
    let write = json!([
        "search",
        "search_memories",
        "search_code",
        "search_standards",
        "add_memory"
    ]);
    for kind in [
        ThreadKind::Analytic,
        ThreadKind::Ask,
        ThreadKind::Verify,
        ThreadKind::Judge,
    ] {
        for allow_write in [false, true] {
            let endpoint = contracts::job::access::McpHttpEndpoint {
                url: "http://memcan.fake/mcp".into(),
                token: contracts::job::access::Secret::new("fake"),
                allow_write,
            };
            let params =
                config::thread_params(kind, &job(JobKind::Plan).model, 500_000, Some(&endpoint));
            let expected = if kind.memcan_write() && allow_write {
                write.clone()
            } else {
                read.clone()
            };
            assert_eq!(
                params["config"]["mcp_servers"]["memcan"]["enabled_tools"],
                expected
            );
        }
    }
}

#[tokio::test]
async fn cancellation_between_lost_session_attempts_prevents_fallback_turn() {
    let harness = Harness::new(Mode::LostWorking).await;
    let mut spec = secret_job(JobKind::Ask);
    let working = spec.working_session.as_mut().unwrap();
    working.thread_id = Some("lost".into());
    working.kind = Some(ThreadKind::Ask);
    let (tx, _rx) = mpsc::channel(64);
    let run = harness.provider.execute_job(&spec, tx);
    let cancel = async {
        loop {
            if harness
                .seen
                .lock()
                .await
                .iter()
                .any(|r| r["action"]["operation"] == "stop")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        let guard = harness.provider.gate.lock().await;
        harness.provider.cancel(spec.job_id).await.unwrap();
        drop(guard);
    };
    let (result, ()) =
        tokio::time::timeout(Duration::from_secs(20), async { tokio::join!(run, cancel) })
            .await
            .unwrap();
    let seen = harness.finish().await;
    assert!(
        matches!(result, Err(ProviderError::Cancelled)),
        "{result:?}"
    );
    assert!(!seen.iter().any(|r| r["method"] == "turn/start"));
}

#[tokio::test]
async fn failed_answer_retains_fork_and_completed_preflight_checkpoint() {
    let harness = Harness::new(Mode::Schema).await;
    let mut spec = secret_job(JobKind::Ask);
    spec.session = Some(contracts::job::spec::SessionRef {
        pull_request_id: 1,
        thread_id: "parent".into(),
    });
    spec.working_session.as_mut().unwrap().analytic_generation = 7;
    let (tx, _rx) = mpsc::channel(64);
    let (_, usage, checkpoint) = harness
        .provider
        .execute_job(&spec, tx)
        .await
        .unwrap_err()
        .into_details();
    let seen = harness.finish().await;
    let checkpoint = checkpoint.expect("failed answer must retain acknowledged compaction");
    let fork = seen.iter().find(|r| r["method"] == "thread/fork").unwrap();
    assert_ne!(
        checkpoint.thread_id,
        fork["params"]["threadId"].as_str().unwrap()
    );
    assert_eq!(
        checkpoint.preflight.as_ref().unwrap().state,
        PreflightState::Done
    );
    assert_eq!(checkpoint.preflight.as_ref().unwrap().generation, 7);
    assert_eq!(usage.unwrap().requests, Some(2));
}

#[tokio::test]
async fn invalid_worker_template_is_configuration_failure() {
    let harness = Harness::new(Mode::InvalidTemplate).await;
    let (tx, _rx) = mpsc::channel(64);
    let error = harness
        .provider
        .execute_job(&secret_job(JobKind::Plan), tx)
        .await
        .unwrap_err()
        .into_parts()
        .0;
    harness.finish().await;
    assert!(
        matches!(
            error,
            ProviderError::Runtime {
                kind: JobFailureKind::Configuration
            }
        ),
        "{error:?}"
    );
}

#[tokio::test]
async fn dependency_source_facts_are_command_activity() {
    let harness = Harness::new(Mode::FetchNoLock).await;
    let (tx, mut rx) = mpsc::channel(64);
    let _ = harness
        .provider
        .execute_job(&secret_job(JobKind::Plan), tx)
        .await;
    harness.finish().await;
    let mut fact = None;
    while let Ok(event) = rx.try_recv() {
        if let ProviderEvent::Activity(event) = event
            && event.summary.starts_with("Dependency sources")
        {
            fact = Some(event);
        }
    }
    assert_eq!(
        fact.unwrap().activity,
        contracts::job::event::ActivityKind::Command
    );
}

fn provider_call(spec: &JobSpec) -> ProviderCall {
    ProviderCall {
        job_spec: Box::new(spec.clone()),
    }
}

#[tokio::test]
async fn legacy_trait_calls_preserve_self_contained_ask_routing() {
    for resume in [false, true] {
        let harness = Harness::new(Mode::LostWorking).await;
        let mut spec = secret_job(JobKind::Ask);
        spec.session = Some(contracts::job::spec::SessionRef {
            pull_request_id: 1,
            thread_id: "parent".into(),
        });
        let working = spec.working_session.as_mut().unwrap();
        working.thread_id = Some("lost".into());
        working.kind = Some(ThreadKind::Ask);
        working.preflight = Some(AskPreflight {
            seed: SessionSeed::Fork,
            generation: 0,
            state: PreflightState::Done,
        });
        let call = provider_call(&spec);
        let (tx, _rx) = mpsc::channel(64);
        let result = if resume {
            harness.provider.resume("parent", &call, tx).await
        } else {
            harness.provider.run(&call, tx).await
        };
        let seen = harness.finish().await;
        assert!(result.is_ok(), "{result:?}");
        assert!(seen.iter().any(|r| r["method"] == "thread/fork"));
        assert!(
            !seen
                .iter()
                .any(|r| r["method"] == "turn/start" && r["params"]["threadId"] == "parent")
        );
    }
}

#[tokio::test]
async fn checkout_reuses_objects_and_discards_hostile_git_configuration() {
    let harness = Harness::new(Mode::Normal).await;
    let (tx, _rx) = mpsc::channel(64);
    let _ = harness
        .provider
        .execute_job(&secret_job(JobKind::Plan), tx)
        .await;
    let seen = harness.finish().await;
    let command = seen
        .iter()
        .find(|r| r["method"] == "command/exec" && r["params"]["command"][0] == "bash")
        .unwrap();
    for sha256 in [true, false] {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path().join("repo");
        let script = command["params"]["command"][2]
            .as_str()
            .unwrap()
            .replace("/work", dir.path().to_str().unwrap());
        let commits = crate::test_support::commits();
        let mut source = crate::test_support::shared_repo().to_owned();
        let (head, base) = if sha256 {
            source = dir.path().join("source");
            std::fs::create_dir_all(&source).unwrap();
            let git = |args: &[&str]| {
                let output = std::process::Command::new("git")
                    .current_dir(&source)
                    .args(args)
                    .env("HOME", dir.path())
                    .env("XDG_CONFIG_HOME", dir.path())
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                String::from_utf8(output.stdout).unwrap().trim().to_owned()
            };
            git(&["init", "--quiet", "--object-format=sha256"]);
            std::fs::write(source.join("auth.py"), "secure_check = True").unwrap();
            git(&["add", "auth.py"]);
            git(&[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "fixture",
            ]);
            let head = git(&["rev-parse", "HEAD"]);
            (head.clone(), head)
        } else {
            (commits.head, commits.base)
        };
        let run = || {
            let output = std::process::Command::new("bash")
                .args(["-c", &script, "checkout-test"])
                .arg(&source)
                .arg(&head)
                .arg(&base)
                .env("HOME", dir.path())
                .env("XDG_CONFIG_HOME", dir.path())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run();
        let tracked = if sha256 { "auth.py" } else { "Cargo.toml" };
        let original = std::fs::read(repo.join(tracked)).unwrap();
        std::fs::write(repo.join(tracked), "secure_check = Fals").unwrap();
        let output = std::process::Command::new("git")
            .args(["update-index", "--skip-worktree", tracked])
            .current_dir(&repo)
            .env("HOME", dir.path())
            .env("XDG_CONFIG_HOME", dir.path())
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        std::fs::write(repo.join(".git/objects/reuse-canary"), "keep object store").unwrap();
        let multi_pack_index = repo.join(".git/objects/pack/multi-pack-index");
        std::fs::write(&multi_pack_index, "untrusted index").unwrap();
        std::fs::write(repo.join(".git/config"), "[invalid config").unwrap();
        std::fs::create_dir_all(repo.join(".git/hooks")).unwrap();
        std::fs::write(
            repo.join(".git/hooks/post-checkout"),
            "#!/bin/sh\nexit 42\n",
        )
        .unwrap();
        std::fs::write(repo.join("untracked"), "discard").unwrap();
        run();
        assert_eq!(std::fs::read(repo.join(tracked)).unwrap(), original);
        assert!(!multi_pack_index.exists());
        assert!(
            repo.join(".git/objects/reuse-canary").exists(),
            "object store must survive per-job preparation"
        );
        assert!(!repo.join("untracked").exists());
        assert!(!repo.join(".git/hooks/post-checkout").exists());
    }
}

#[derive(Clone, Default)]
struct CapturedLogs(Arc<std::sync::Mutex<Vec<u8>>>);
impl std::io::Write for CapturedLogs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLogs {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[tokio::test]
async fn reconciliation_failure_logs_both_sums_without_tokens() {
    use tracing::instrument::WithSubscriber;
    let logs = CapturedLogs::default();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_writer(logs.clone())
        .finish();
    let harness = Harness::new(Mode::BadMeter).await;
    let mut spec = secret_job(JobKind::Ask);
    spec.session = Some(contracts::job::spec::SessionRef {
        pull_request_id: 1,
        thread_id: "parent".into(),
    });
    let (tx, _rx) = mpsc::channel(64);
    let result = harness
        .provider
        .execute_job(&spec, tx)
        .with_subscriber(subscriber)
        .await;
    harness.finish().await;
    assert!(result.is_err());
    let log = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert!(log.contains("usage reconciliation mismatch"), "{log}");
    assert!(
        log.contains("input_tokens: 120") && log.contains("input_tokens: 121"),
        "{log}"
    );
    assert!(!log.contains("model-key-canary") && !log.contains("github-token-canary"));
}

#[tokio::test]
async fn protocol_failure_logs_identify_method_and_step_without_payloads() {
    use tracing::instrument::WithSubscriber;
    for (mode, step) in [
        (Mode::BadJson, "invalid JSON frame"),
        (Mode::BadResponseId, "response id mismatch"),
        (Mode::MissingThread, "missing thread.id"),
        (Mode::ServerRequest, "item/tool/requestUserInput"),
        (Mode::MissingUsage, "completed without token usage update"),
        (Mode::StopFailure, "worker may still be running"),
    ] {
        let logs = CapturedLogs::default();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_writer(logs.clone())
            .finish();
        let harness = Harness::new(mode).await;
        let mut spec = secret_job(JobKind::Plan);
        if mode == Mode::StopFailure {
            spec = job(JobKind::JudgeWork);
            spec.target = JobTarget::Dashboard {
                refresh_id: contracts::ids::RefreshId::new_random(),
            };
        }
        let (tx, _rx) = mpsc::channel(64);
        let result = harness
            .provider
            .execute_job(&spec, tx)
            .with_subscriber(subscriber)
            .await;
        harness.finish().await;
        assert!(result.is_err());
        let log = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
        assert!(
            log.contains(step) && log.contains("method=") && log.contains("step="),
            "{log}"
        );
        if mode == Mode::StopFailure {
            assert!(log.contains("ERROR"), "{log}");
        }
        assert!(!log.contains("model-key-canary") && !log.contains("github-token-canary"));
    }
}

#[tokio::test]
async fn retirement_removes_only_selected_pr_workers_and_revokes_local_keys() {
    let harness = Harness::new(Mode::StartupRecovery).await;
    harness.provider.retire(&[2]).await.unwrap();
    assert!(
        !harness
            .seen
            .lock()
            .await
            .iter()
            .any(|r| r["action"]["operation"] == "remove")
    );
    harness.provider.retire(&[1]).await.unwrap();
    let requests = harness.gateway.received_requests().await.unwrap();
    assert!(
        requests
            .iter()
            .any(|r| r.url.path() == "/admin/keys/revoke")
    );
    let seen = harness.finish().await;
    let removed: Vec<_> = seen
        .iter()
        .filter(|r| r["action"]["operation"] == "remove")
        .collect();
    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0]["action"]["container"]["pull_request_id"], 1);
}

#[tokio::test]
async fn failed_key_retirement_keeps_the_worker_for_a_retry() {
    use wiremock::{Mock, ResponseTemplate, matchers::path};
    let harness = Harness::new(Mode::StartupRecovery).await;
    Mock::given(path("/admin/keys/revoke"))
        .respond_with(ResponseTemplate::new(503))
        .with_priority(1)
        .mount(&harness.gateway)
        .await;
    assert!(harness.provider.retire(&[1]).await.is_err());
    let seen = harness.finish().await;
    assert!(!seen.iter().any(|r| r["action"]["operation"] == "remove"));
}
