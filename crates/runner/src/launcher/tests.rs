use super::*;
use contracts::ids::UserId;

fn settings() -> Settings {
    Settings {
        worker_image: format!("worker@sha256:{}", "a".repeat(64)),
        fetch_image: format!("fetch@sha256:{}", "b".repeat(64)),
        prefix: "v13w3-test".into(),
        worker_network: "v13w3-test-worker".into(),
        judge_network: "v13w3-test-judge".into(),
        proxy: "http://egress-proxy:3128".into(),
        memory: 2 * 1024 * 1024 * 1024,
        cpus: 2,
        pids: 1024,
        max_running: 3,
        idle_seconds: 1200,
    }
}
fn pr() -> ContainerId {
    ContainerId::Pr {
        pull_request_id: 7,
        payer: UserId("3".into()),
    }
}

#[test]
fn worker_template_has_no_host_access_or_real_credentials() {
    let s = settings();
    let config = s.template(&pr(), false, None).unwrap();
    let host = config.host_config.unwrap();
    assert_eq!(config.user.as_deref(), Some("10002:10002"));
    assert_eq!(config.working_dir.as_deref(), Some("/work"));
    assert_eq!(host.readonly_rootfs, Some(true));
    assert_eq!(host.privileged, Some(false));
    assert_eq!(host.init, Some(true));
    assert_eq!(host.cap_drop.unwrap(), ["ALL"]);
    assert_eq!(host.security_opt.unwrap(), ["no-new-privileges:true"]);
    assert_eq!(host.memory, Some(s.memory));
    assert_eq!(host.memory, host.memory_swap);
    assert_eq!(host.nano_cpus, Some(2_000_000_000));
    assert_eq!(host.pids_limit, Some(1024));
    assert_eq!(host.network_mode.as_deref(), Some("v13w3-test-worker"));
    let tmpfs = host.tmpfs.unwrap();
    assert_eq!(tmpfs["/tmp"], "uid=10002,gid=10002,mode=0700,size=2g");
    assert_eq!(tmpfs["/run/dp"], "uid=10002,gid=10002,mode=0700,size=1m");
    assert!(host.binds.is_none());
    assert!(host.port_bindings.is_none());
    let mounts = host.mounts.unwrap();
    assert!(
        mounts
            .iter()
            .all(|m| m.typ == Some(bollard::models::MountType::VOLUME))
    );
    assert_eq!(
        mounts.iter().filter(|m| m.read_only == Some(true)).count(),
        2
    );
    for (source, target, read_only) in [
        ("v13w3-test-pr-7-3-work", "/work", false),
        ("v13w3-test-c1", "/mirror", true),
        ("v13w3-test-c2", "/cargo", true),
    ] {
        assert!(mounts.iter().any(|m| m.source.as_deref() == Some(source)
            && m.target.as_deref() == Some(target)
            && m.read_only == Some(read_only)));
    }
    let env = config.env.unwrap().join("\n");
    assert!(env.contains("HOME=/work/home"));
    assert!(env.contains("CODEX_HOME=/work/codex"));
    assert!(!env.contains("GH_TOKEN="));
    assert!(!env.contains("OPENAI_API_KEY="));
}

#[test]
fn judge_has_only_tmpfs_and_gateway_network() {
    let config = settings()
        .template(
            &ContainerId::Judge {
                payer: UserId("3".into()),
            },
            false,
            None,
        )
        .unwrap();
    let host = config.host_config.unwrap();
    assert_eq!(host.network_mode.as_deref(), Some("v13w3-test-judge"));
    assert!(host.mounts.unwrap_or_default().is_empty());
    let tmpfs = host.tmpfs.unwrap();
    for path in ["/tmp", "/run/dp", "/home/worker", "/codex"] {
        assert!(tmpfs[path].contains("uid=10002,gid=10002,mode=0700"));
    }
    assert!(!config.env.unwrap().iter().any(|v| v.contains("PROXY=")));
    let endpoint = contracts::job::access::McpHttpEndpoint {
        url: "http://memcan:8190/mcp".into(),
        token: contracts::job::access::Secret::new("fake"),
        allow_write: true,
    };
    assert!(
        settings()
            .template(
                &ContainerId::Judge {
                    payer: UserId("3".into())
                },
                false,
                Some(&endpoint)
            )
            .is_err()
    );
}

#[test]
fn fetch_pins_toolchain_and_never_writes_the_checkout() {
    let config = settings().template(&pr(), true, None).unwrap();
    assert_eq!(config.working_dir.as_deref(), Some("/"));
    let mounts = config.host_config.unwrap().mounts.unwrap();
    assert!(
        mounts
            .iter()
            .any(|m| m.target.as_deref() == Some("/work") && m.read_only == Some(true))
    );
    assert!(
        mounts
            .iter()
            .any(|m| m.target.as_deref() == Some("/c2") && m.read_only == Some(false))
    );
    let env = config.env.unwrap();
    assert!(env.contains(&"RUSTUP_AUTO_INSTALL=0".into()));
    assert!(env.contains(&"RUSTUP_TOOLCHAIN=1.100.0-x86_64-unknown-linux-gnu".into()));
    assert!(
        settings()
            .template(
                &ContainerId::Judge {
                    payer: UserId("3".into())
                },
                true,
                None
            )
            .is_err()
    );
}

#[test]
fn names_and_labels_are_derived_and_fail_closed() {
    let s = settings();
    assert_eq!(s.name(&pr(), false), "v13w3-test-pr-7-3");
    assert_eq!(s.name(&pr(), true), "v13w3-test-fetch-7-3");
    let labels = s.template(&pr(), false, None).unwrap().labels.unwrap();
    assert_eq!(s.identity(&labels), Some(pr()));
    let mut wrong = labels;
    wrong.insert("dp.managed".into(), "another-project".into());
    assert_eq!(s.identity(&wrong), None);
    assert!(
        s.template(
            &ContainerId::Pr {
                pull_request_id: -1,
                payer: UserId("3".into())
            },
            false,
            None
        )
        .is_err()
    );
    let mut s = settings();
    s.worker_image = "worker:latest".into();
    assert!(s.validate().is_err());
    s = settings();
    s.prefix = "../host".into();
    assert!(s.validate().is_err());
}

#[tokio::test]
async fn bounded_framing_preserves_raw_upgrade_bytes() {
    use tokio::io::AsyncWriteExt;
    let (mut tx, mut rx) = tokio::net::UnixStream::pair().unwrap();
    tx.write_all(b"{\"id\":\"a\",\"workers\":[]}\nHTTP/1.1 101\r\n")
        .await
        .unwrap();
    let response: contracts::launcher::LauncherResponse =
        serve::read_message(&mut rx).await.unwrap();
    assert_eq!(response.id, "a");
    let mut raw = [0; 12];
    use tokio::io::AsyncReadExt;
    rx.read_exact(&mut raw).await.unwrap();
    assert_eq!(&raw, b"HTTP/1.1 101");
    let (mut tx, mut rx) = tokio::net::UnixStream::pair().unwrap();
    tx.write_all(&vec![b'x'; 16385]).await.unwrap();
    assert!(matches!(
        serve::read_limited::<contracts::launcher::LauncherRequest>(&mut rx, 16384).await,
        Err(Error::Protocol)
    ));
}

#[tokio::test]
async fn startup_reconciliation_stops_all_and_removes_retired() {
    use contracts::launcher::{LauncherAction, LauncherRequest, LauncherResponse, WorkerStatus};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("launcher.sock");
    let listener = tokio::net::UnixListener::bind(&path).unwrap();
    let p = pr();
    let judge = ContainerId::Judge {
        payer: UserId("3".into()),
    };
    let expected = [
        LauncherAction::List,
        LauncherAction::Remove {
            container: p.clone(),
        },
        LauncherAction::Stop {
            container: judge.clone(),
        },
    ];
    let server = tokio::spawn(async move {
        for (index, action) in expected.into_iter().enumerate() {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request: LauncherRequest = serve::read_message(&mut socket).await.unwrap();
            assert_eq!(request.action, action);
            let workers = if index == 0 {
                vec![
                    WorkerStatus {
                        container: p.clone(),
                        running: true,
                    },
                    WorkerStatus {
                        container: judge.clone(),
                        running: false,
                    },
                ]
            } else {
                vec![]
            };
            serve::write_message(
                &mut socket,
                &LauncherResponse {
                    error: None,
                    fetch: None,
                    id: request.id,
                    workers,
                },
            )
            .await
            .unwrap();
        }
    });
    assert_eq!(Client::new(&path).reconcile(&[7]).await.unwrap().len(), 2);
    server.await.unwrap();
}

async fn fake_launcher() -> (wiremock::MockServer, Launcher) {
    use wiremock::{
        Mock, ResponseTemplate,
        matchers::{method, path_regex},
    };
    let server = wiremock::MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(r"/volumes$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"Volumes":[]})))
        .with_priority(10)
        .mount(&server)
        .await;
    let docker =
        bollard::Docker::connect_with_http(&server.uri(), 10, bollard::API_DEFAULT_VERSION)
            .unwrap();
    (
        server,
        Launcher {
            docker,
            settings: settings(),
            used: HashMap::new(),
            attached: HashSet::new(),
            fetching: HashSet::new(),
        },
    )
}

#[tokio::test]
async fn failed_fetch_is_removed_and_does_not_hide_failure() {
    use wiremock::{
        Mock, ResponseTemplate,
        matchers::{method, path_regex},
    };
    let (server, launcher) = fake_launcher().await;
    mock_runtime(&server).await;
    Mock::given(method("GET"))
        .and(path_regex(r"/containers/v13w3-test-fetch-7-3/json$"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"/containers/create$"))
        .respond_with(
            ResponseTemplate::new(201)
                .set_body_json(serde_json::json!({"Id":"fetch", "Warnings":[]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"/containers/v13w3-test-fetch-7-3/start$"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"/containers/v13w3-test-fetch-7-3/wait$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"StatusCode":1})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path_regex(r"/containers/v13w3-test-fetch-7-3$"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    assert!(matches!(launcher.fetch(&pr()).await, Err(Error::Fetch)));
}

#[tokio::test]
async fn foreign_container_collision_is_never_started_or_removed() {
    use wiremock::{
        Mock, ResponseTemplate,
        matchers::{method, path_regex},
    };
    let (server, mut launcher) = fake_launcher().await;
    Mock::given(method("GET"))
        .and(path_regex(r"/containers/v13w3-test-pr-7-3/json$"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"Config":{"Labels":{"dp.managed":"foreign"}}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    assert!(matches!(
        launcher.remove(&pr()).await,
        Err(Error::InvalidTemplate)
    ));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn ensure_does_not_restart_a_worker_whose_old_relay_is_still_attached() {
    use wiremock::{
        Mock, ResponseTemplate,
        matchers::{method, path_regex},
    };
    let (server, mut launcher) = fake_launcher().await;
    let name = launcher.settings.name(&pr(), false);
    launcher.attached.insert(name);
    let labels = launcher
        .settings
        .template(&pr(), false, None)
        .unwrap()
        .labels
        .unwrap();
    Mock::given(method("GET"))
        .and(path_regex(r"/containers/v13w3-test-pr-7-3/json$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"Config":{"Labels":labels},"State":{"Running":false}}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(r"/containers/json$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(&server)
        .await;
    assert!(matches!(
        launcher.ensure(&pr(), None).await,
        Err(Error::Busy)
    ));
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.method.as_str() == "GET")
    );
}

#[tokio::test]
async fn relay_forwards_stdout_only_and_keeps_worker_warm() {
    use std::sync::Arc;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        sync::Mutex,
    };
    let engine = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", engine.local_addr().unwrap());
    let labels = settings()
        .template(&pr(), false, None)
        .unwrap()
        .labels
        .unwrap();
    let fake = tokio::spawn(async move {
        for step in 0..3 {
            let (mut socket, _) = engine.accept().await.unwrap();
            let mut headers = vec![];
            while !headers.ends_with(b"\r\n\r\n") {
                headers.push(socket.read_u8().await.unwrap());
            }
            let headers = String::from_utf8(headers).unwrap();
            let length: usize = headers
                .lines()
                .find_map(|s| {
                    s.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|s| s.trim().parse().unwrap())
                })
                .unwrap_or(0);
            let mut body = vec![0; length];
            socket.read_exact(&mut body).await.unwrap();
            let reply = match step {
                0 | 3 => {
                    assert!(headers.contains("/containers/v13w3-test-pr-7-3/json"));
                    serde_json::json!({"Config":{"Labels":labels}, "State":{"Running":true}})
                        .to_string()
                }
                1 => {
                    assert!(headers.contains("/containers/v13w3-test-pr-7-3/exec"));
                    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
                    assert_eq!(body["Tty"], false);
                    assert_eq!(
                        body["Cmd"],
                        serde_json::json!([
                            "codex",
                            "app-server",
                            "proxy",
                            "--sock",
                            "/run/dp/app.sock"
                        ])
                    );
                    "{\"Id\":\"exec\"}".into()
                }
                2 => {
                    assert!(headers.contains("/exec/exec/start"));
                    socket
                        .write_all(
                            b"HTTP/1.1 101 UPGRADED\r\nConnection: Upgrade\r\nUpgrade: tcp\r\n\r\n",
                        )
                        .await
                        .unwrap();
                    let mut ping = [0; 4];
                    socket.read_exact(&mut ping).await.unwrap();
                    assert_eq!(&ping, b"ping");
                    for (stream, message) in
                        [(2_u8, b"diagnostic".as_slice()), (1, b"pong".as_slice())]
                    {
                        let mut frame = vec![stream, 0, 0, 0];
                        frame.extend((message.len() as u32).to_be_bytes());
                        frame.extend(message);
                        socket.write_all(&frame).await.unwrap();
                    }
                    continue;
                }
                _ => {
                    assert!(headers.starts_with("POST "));
                    assert!(headers.contains("/containers/v13w3-test-pr-7-3/stop"));
                    String::new()
                }
            };
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len()).as_bytes()).await.unwrap();
        }
    });
    let docker =
        bollard::Docker::connect_with_http(&address, 10, bollard::API_DEFAULT_VERSION).unwrap();
    let shared = Arc::new(Mutex::new(Launcher {
        docker,
        settings: settings(),
        used: HashMap::new(),
        attached: HashSet::new(),
        fetching: HashSet::new(),
    }));
    let (socket, mut client) = tokio::net::UnixStream::pair().unwrap();
    let handler = tokio::spawn(serve::connection(socket, Arc::clone(&shared)));
    serve::write_message(
        &mut client,
        &contracts::launcher::LauncherRequest {
            id: "relay".into(),
            action: contracts::launcher::LauncherAction::Attach { container: pr() },
        },
    )
    .await
    .unwrap();
    let response: contracts::launcher::LauncherResponse =
        serve::read_message(&mut client).await.unwrap();
    assert_eq!(response.id, "relay");
    client.write_all(b"ping").await.unwrap();
    let mut output = vec![];
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.read_to_end(&mut output),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(output, b"pong");
    handler.await.unwrap().unwrap();
    fake.await.unwrap();
    let launcher = shared.lock().await;
    assert!(launcher.attached.is_empty());
    assert!(launcher.used.contains_key("v13w3-test-pr-7-3"));
}

#[tokio::test]
async fn reconciliation_list_can_exceed_the_request_size_limit() {
    let (mut tx, mut rx) = tokio::net::UnixStream::pair().unwrap();
    let response = contracts::launcher::LauncherResponse {
        error: None,
        fetch: None,
        id: "list".into(),
        workers: (1..=300)
            .map(|pull_request_id| contracts::launcher::WorkerStatus {
                container: ContainerId::Pr {
                    pull_request_id,
                    payer: UserId("payer".into()),
                },
                running: false,
            })
            .collect(),
    };
    assert!(serde_json::to_vec(&response).unwrap().len() > 16384);
    let writer = tokio::spawn(async move {
        serve::write_message(&mut tx, &response).await.unwrap();
    });
    let response: contracts::launcher::LauncherResponse =
        serve::read_message(&mut rx).await.unwrap();
    assert_eq!(response.workers.len(), 300);
    writer.await.unwrap();
}

fn summary(s: &Settings, id: &ContainerId, running: bool) -> serde_json::Value {
    serde_json::json!({"Id":s.name(id, false), "Names":[format!("/{}", s.name(id, false))],
        "Labels":s.template(id, false, None).unwrap().labels,
        "State":if running {"running"} else {"exited"}})
}

async fn mock_inspect(
    server: &wiremock::MockServer,
    s: &Settings,
    id: &ContainerId,
    running: bool,
) {
    use wiremock::{
        Mock, ResponseTemplate,
        matchers::{method, path_regex},
    };
    let config = s.template(id, false, None).unwrap();
    Mock::given(method("GET"))
        .and(path_regex(format!(
            "/containers/{}/json$",
            s.name(id, false)
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Config":{"Image":config.image,"Labels":config.labels},"State":{"Running":running}})))
        .mount(server)
        .await;
}

async fn mock_runtime(server: &wiremock::MockServer) {
    use wiremock::{
        Mock, ResponseTemplate,
        matchers::{method, path_regex},
    };
    for (verb, path, body) in [
        (
            "POST",
            r"/containers/.+/exec$",
            serde_json::json!({"Id":"reset"}),
        ),
        ("POST", r"/exec/reset/start$", serde_json::json!({})),
        (
            "GET",
            r"/exec/reset/json$",
            serde_json::json!({"Running":false,"ExitCode":0}),
        ),
    ] {
        Mock::given(method(verb))
            .and(path_regex(path))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(server)
            .await;
    }
    Mock::given(method("POST")).and(path_regex(r"/volumes/create$"))
        .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({"Name":"work","Driver":"local","Mountpoint":"/test","Labels":{},"Options":{},"Scope":"local"})))
        .mount(server).await;
    Mock::given(method("GET"))
        .and(path_regex(r"/volumes/.+$"))
        .respond_with(ResponseTemplate::new(404))
        .mount(server)
        .await;
}

#[tokio::test]
async fn ensure_recreates_stale_image_or_template_and_keeps_work_volume() {
    use wiremock::{
        Mock, ResponseTemplate,
        matchers::{method, path_regex},
    };
    for (running, stale_image) in [(false, true), (true, true), (false, false)] {
        let (server, mut launcher) = fake_launcher().await;
        let s = launcher.settings.clone();
        let mut config = s.template(&pr(), false, None).unwrap();
        if stale_image {
            config.image = Some(format!("old@sha256:{}", "c".repeat(64)));
        } else {
            config
                .labels
                .as_mut()
                .unwrap()
                .insert("dp.template".into(), "old".into());
        }
        Mock::given(method("GET")).and(path_regex(r"/containers/v13w3-test-pr-7-3/json$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Config":{"Image":config.image,"Labels":config.labels},"State":{"Running":running}})))
            .mount(&server).await;
        Mock::given(method("GET"))
            .and(path_regex(r"/containers/json$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;
        for (verb, path, status, body) in [
            (
                "POST",
                r"/containers/v13w3-test-pr-7-3/stop$",
                204,
                serde_json::json!(null),
            ),
            (
                "DELETE",
                r"/containers/v13w3-test-pr-7-3$",
                204,
                serde_json::json!(null),
            ),
            (
                "POST",
                r"/containers/create$",
                201,
                serde_json::json!({"Id":"worker","Warnings":[]}),
            ),
            (
                "POST",
                r"/containers/v13w3-test-pr-7-3/start$",
                204,
                serde_json::json!(null),
            ),
        ] {
            Mock::given(method(verb))
                .and(path_regex(path))
                .respond_with(ResponseTemplate::new(status).set_body_json(body))
                .mount(&server)
                .await;
        }
        mock_runtime(&server).await;
        launcher.ensure(&pr(), None).await.unwrap();
        let requests = server.received_requests().await.unwrap();
        let deleted = requests.iter().position(|r| r.method == "DELETE").unwrap();
        let created = requests
            .iter()
            .position(|r| r.url.path().ends_with("/containers/create"))
            .unwrap();
        let started = requests
            .iter()
            .position(|r| {
                r.url.path().ends_with("/pr-7-3/start") || r.url.path().ends_with("-pr-7-3/start")
            })
            .unwrap();
        assert!(deleted < created && created < started);
        assert!(
            !requests
                .iter()
                .any(|r| r.method == "DELETE" && r.url.path().contains("/volumes/"))
        );
        let volume = requests
            .iter()
            .find(|r| r.url.path().ends_with("/volumes/create"))
            .unwrap();
        let volume: serde_json::Value = serde_json::from_slice(&volume.body).unwrap();
        assert_eq!(volume["Name"], "v13w3-test-pr-7-3-work");
        assert_eq!(volume["Labels"]["dp.pr"], "7");
        assert_eq!(volume["Labels"]["dp.user"], "3");
        assert_eq!(volume["Labels"]["dp.managed"], "v13w3-test");
    }
}

#[tokio::test]
async fn ensure_evicts_lru_unattached_worker_before_starting() {
    use wiremock::{
        Mock, ResponseTemplate,
        matchers::{method, path_regex},
    };
    let (server, mut launcher) = fake_launcher().await;
    launcher.settings.max_running = 2;
    let s = launcher.settings.clone();
    let old = ContainerId::Judge {
        payer: UserId("old".into()),
    };
    let recent = ContainerId::Judge {
        payer: UserId("recent".into()),
    };
    launcher.used.insert(s.name(&recent, false), Instant::now());
    let attached = ContainerId::Judge {
        payer: UserId("attached".into()),
    };
    let new = ContainerId::Judge {
        payer: UserId("new".into()),
    };
    launcher.attached.insert(s.name(&attached, false));
    mock_inspect(&server, &s, &old, true).await;
    mock_inspect(&server, &s, &recent, true).await;
    Mock::given(method("GET"))
        .and(path_regex(r"/containers/v13w3-test-user-new/json$"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(r"/containers/json$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            summary(&s, &attached, true),
            summary(&s, &recent, true),
            summary(&s, &old, true)
        ])))
        .mount(&server)
        .await;
    for (verb, path, status) in [
        ("POST", r"/containers/v13w3-test-user-old/stop$", 204),
        ("DELETE", r"/containers/v13w3-test-user-old$", 204),
        ("POST", r"/containers/v13w3-test-user-recent/stop$", 204),
        ("DELETE", r"/containers/v13w3-test-user-recent$", 204),
        ("POST", r"/containers/v13w3-test-user-new/start$", 204),
    ] {
        Mock::given(method(verb))
            .and(path_regex(path))
            .respond_with(ResponseTemplate::new(status))
            .expect(1)
            .mount(&server)
            .await;
    }
    Mock::given(method("POST"))
        .and(path_regex(r"/containers/create$"))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(serde_json::json!({"Id":"new","Warnings":[]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    mock_runtime(&server).await;
    launcher.ensure(&new, None).await.unwrap();
    let requests = server.received_requests().await.unwrap();
    let stop = requests
        .iter()
        .position(|r| r.url.path().ends_with("/stop"))
        .unwrap();
    let create = requests
        .iter()
        .position(|r| r.url.path().ends_with("/containers/create"))
        .unwrap();
    let second_stop = requests
        .iter()
        .position(|r| r.url.path().ends_with("user-recent/stop"))
        .unwrap();
    assert!(stop < second_stop && second_stop < create);
    assert!(
        !requests
            .iter()
            .any(|r| r.method != "GET" && r.url.path().contains("user-attached"))
    );
}

#[tokio::test]
async fn sweep_stops_only_expired_unattached_workers() {
    use wiremock::{
        Mock, ResponseTemplate,
        matchers::{method, path_regex},
    };
    let (server, mut launcher) = fake_launcher().await;
    let s = launcher.settings.clone();
    let ids: Vec<_> = ["old", "recent", "attached", "untracked"]
        .map(|v| ContainerId::Judge {
            payer: UserId(v.into()),
        })
        .into();
    launcher.used.insert(s.name(&ids[1], false), Instant::now());
    launcher.used.insert(
        s.name(&ids[0], false),
        Instant::now() - std::time::Duration::from_secs(1201),
    );
    launcher.attached.insert(s.name(&ids[2], false));
    Mock::given(method("GET"))
        .and(path_regex(r"/containers/json$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::json!(ids.iter().map(|id|summary(&s,id,true)).collect::<Vec<_>>()),
        ))
        .mount(&server)
        .await;
    mock_inspect(&server, &s, &ids[0], true).await;
    mock_inspect(&server, &s, &ids[3], true).await;
    for (verb, path) in [
        ("POST", r"/containers/v13w3-test-user-untracked/stop$"),
        ("DELETE", r"/containers/v13w3-test-user-untracked$"),
    ] {
        Mock::given(method(verb))
            .and(path_regex(path))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
    }
    for (verb, path) in [
        ("POST", r"/containers/v13w3-test-user-old/stop$"),
        ("DELETE", r"/containers/v13w3-test-user-old$"),
    ] {
        Mock::given(method(verb))
            .and(path_regex(path))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
    }
    launcher.sweep().await.unwrap();
    assert!(
        !server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|r| r.method != "GET"
                && (r.url.path().contains("recent") || r.url.path().contains("attached")))
    );
}

#[tokio::test]
async fn reconcile_removes_abandoned_fetch_and_lists_all_work_volumes() {
    use wiremock::{
        Mock, ResponseTemplate,
        matchers::{method, path_regex},
    };
    let (server, mut launcher) = fake_launcher().await;
    let s = launcher.settings.clone();
    mock_inspect(&server, &s, &pr(), false).await;
    Mock::given(method("GET")).and(path_regex(r"/containers/json$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            summary(&s,&pr(),false), {"Id":"abandoned","Names":["/v13w3-test-fetch-7-3"],"Labels":{"dp.managed":s.prefix,"dp.shape":"cs3"}}
        ]))).mount(&server).await;
    let mut orphan = s.template(&pr(), false, None).unwrap().labels.unwrap();
    orphan.insert("dp.pr".into(), "8".into());
    Mock::given(method("GET")).and(path_regex(r"/volumes$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"Volumes":[
            {"Name":"v13w3-test-pr-7-3-work","Driver":"local","Mountpoint":"/test","Options":{},"Scope":"local","Labels":s.template(&pr(),false,None).unwrap().labels},
            {"Name":"v13w3-test-pr-8-3-work","Driver":"local","Mountpoint":"/test","Options":{},"Scope":"local","Labels":orphan},
            {"Name":"foreign-work","Driver":"local","Mountpoint":"/test","Options":{},"Scope":"local","Labels":{"dp.managed":"foreign"}},
            {"Name":"wrong-name-work","Driver":"local","Mountpoint":"/test","Options":{},"Scope":"local","Labels":orphan}
        ]}))).mount(&server).await;
    Mock::given(method("GET"))
        .and(path_regex(r"/containers/v13w3-test-pr-8-3/json$"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path_regex(r"/containers/abandoned$"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path_regex(r"/volumes/v13w3-test-pr-8-3-work$"))
        .respond_with(ResponseTemplate::new(204))
        .expect(0)
        .mount(&server)
        .await;
    launcher.reconcile().await.unwrap();
    let workers = launcher.list().await.unwrap();
    assert_eq!(workers.len(), 2);
    assert!(workers.iter().all(|w| !w.running));
    assert!(workers.iter().any(|w| w.container == pr()));
    assert!(workers.iter().any(|w| w.container
        == ContainerId::Pr {
            pull_request_id: 8,
            payer: UserId("3".into()),
        }));
    assert!(
        !server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|r| r.method == "DELETE" && r.url.path().ends_with("pr-7-3-work"))
    );
}

async fn assert_missing_container_volume_is_recoverable(
    server: &wiremock::MockServer,
    launcher: &mut Launcher,
) {
    use wiremock::{
        Mock, ResponseTemplate,
        matchers::{method, path_regex},
    };
    let labels = launcher
        .settings
        .template(&pr(), false, None)
        .unwrap()
        .labels;
    Mock::given(method("GET"))
        .and(path_regex(r"/containers/json$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(r"/volumes$"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"Volumes":[{
                "Name":"v13w3-test-pr-7-3-work","Driver":"local","Mountpoint":"/test",
                "Options":{},"Scope":"local","Labels":labels
            }]})),
        )
        .mount(server)
        .await;
    Mock::given(method("DELETE"))
        .and(path_regex(r"/volumes/v13w3-test-pr-7-3-work$"))
        .respond_with(ResponseTemplate::new(204))
        .mount(server)
        .await;
    launcher.reconcile().await.unwrap();
    assert!(
        !server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|r| r.method == "DELETE" && r.url.path().contains("/volumes/"))
    );
    assert_eq!(
        launcher.list().await.unwrap(),
        vec![WorkerStatus {
            container: pr(),
            running: false,
        }]
    );
    mock_runtime(server).await;
    Mock::given(method("POST"))
        .and(path_regex(r"/volumes/create$"))
        .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
            "Name":"v13w3-test-pr-7-3-work","Driver":"local","Mountpoint":"/test",
            "Options":{},"Scope":"local","Labels":labels
        })))
        .with_priority(1)
        .expect(1)
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"/containers/create$"))
        .respond_with(
            ResponseTemplate::new(201)
                .set_body_json(serde_json::json!({"Id":"restored","Warnings":[]})),
        )
        .expect(1)
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"/containers/v13w3-test-pr-7-3/start$"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(server)
        .await;
    assert!(launcher.ensure(&pr(), None).await.unwrap());
    let requests = server.received_requests().await.unwrap();
    let create = requests
        .iter()
        .find(|r| r.url.path().ends_with("/containers/create"))
        .unwrap();
    let config: serde_json::Value = serde_json::from_slice(&create.body).unwrap();
    assert!(
        config["HostConfig"]["Mounts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["Source"] == "v13w3-test-pr-7-3-work" && m["Target"] == "/work")
    );
    assert!(
        !requests
            .iter()
            .any(|r| r.method == "DELETE" && r.url.path().contains("/volumes/"))
    );
}

#[tokio::test]
async fn reconcile_keeps_pruned_idle_worker_volume_until_explicit_remove() {
    use wiremock::{
        Mock, ResponseTemplate,
        matchers::{method, path_regex},
    };
    let (server, mut launcher) = fake_launcher().await;
    // Docker prune removed the stopped container; its labeled volume still holds the archive.
    Mock::given(method("GET"))
        .and(path_regex(r"/containers/v13w3-test-pr-7-3/json$"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    assert_missing_container_volume_is_recoverable(&server, &mut launcher).await;
    // Remove must also retire a volume when the container has gone missing again.
    launcher.remove(&pr()).await.unwrap();
    assert_eq!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.method == "DELETE"
                && r.url.path().ends_with("/volumes/v13w3-test-pr-7-3-work"))
            .count(),
        1
    );
}

#[tokio::test]
async fn reconcile_keeps_volume_when_shutdown_interrupts_recreation() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use wiremock::{
        Mock, ResponseTemplate,
        matchers::{method, path_regex},
    };
    let (server, mut launcher) = fake_launcher().await;
    let removed = Arc::new(AtomicBool::new(false));
    let deleted = Arc::new(tokio::sync::Notify::new());
    let mut config = launcher.settings.template(&pr(), false, None).unwrap();
    config.image = Some(format!("old@sha256:{}", "c".repeat(64)));
    let exists = Arc::clone(&removed);
    Mock::given(method("GET"))
        .and(path_regex(r"/containers/v13w3-test-pr-7-3/json$"))
        .respond_with(move |_: &wiremock::Request| {
            if exists.load(Ordering::SeqCst) {
                ResponseTemplate::new(404)
            } else {
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "Config":{"Image":config.image,"Labels":config.labels},"State":{"Running":false}
                }))
            }
        })
        .mount(&server)
        .await;
    let missing = Arc::clone(&removed);
    let signal = Arc::clone(&deleted);
    Mock::given(method("DELETE"))
        .and(path_regex(r"/containers/v13w3-test-pr-7-3$"))
        .respond_with(move |_: &wiremock::Request| {
            missing.store(true, Ordering::SeqCst);
            signal.notify_one();
            ResponseTemplate::new(204).set_delay(std::time::Duration::from_secs(1))
        })
        .expect(1)
        .mount(&server)
        .await;
    {
        let id = pr();
        let ensure = launcher.ensure(&id, None);
        tokio::pin!(ensure);
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::select! {
                result = &mut ensure => panic!("ensure completed before cancellation: {result:?}"),
                () = deleted.notified() => {}
            }
        })
        .await
        .unwrap();
    }
    assert!(removed.load(Ordering::SeqCst));
    assert_missing_container_volume_is_recoverable(&server, &mut launcher).await;
}

#[tokio::test]
async fn fetch_releases_lifecycle_lock_and_reserves_its_identity() {
    use contracts::launcher::{LauncherAction, LauncherRequest, LauncherResponse};
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use wiremock::{
        Mock, ResponseTemplate,
        matchers::{method, path_regex},
    };
    let (server, launcher) = fake_launcher().await;
    mock_runtime(&server).await;
    Mock::given(method("GET"))
        .and(path_regex(r"/containers/v13w3-test-fetch-7-3/json$"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"/containers/create$"))
        .respond_with(
            ResponseTemplate::new(201)
                .set_body_json(serde_json::json!({"Id":"fetch","Warnings":[]})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"/containers/v13w3-test-fetch-7-3/start$"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"/containers/v13w3-test-fetch-7-3/wait$"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"StatusCode":0}))
                .set_delay(std::time::Duration::from_millis(500)),
        )
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path_regex(r"/containers/v13w3-test-fetch-7-3$"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(r"/containers/json$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(&server)
        .await;
    let shared = Arc::new(Mutex::new(launcher));
    let (socket, mut client) = tokio::net::UnixStream::pair().unwrap();
    let task = tokio::spawn(serve::connection(socket, Arc::clone(&shared)));
    serve::write_message(
        &mut client,
        &LauncherRequest {
            id: "fetch".into(),
            action: LauncherAction::Fetch { container: pr() },
        },
    )
    .await
    .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .any(|r| r.url.path().ends_with("/wait"))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let (socket, mut list) = tokio::net::UnixStream::pair().unwrap();
    let listing = tokio::spawn(serve::connection(socket, Arc::clone(&shared)));
    serve::write_message(
        &mut list,
        &LauncherRequest {
            id: "list".into(),
            action: LauncherAction::List,
        },
    )
    .await
    .unwrap();
    let response: LauncherResponse = tokio::time::timeout(
        std::time::Duration::from_millis(150),
        serve::read_message(&mut list),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.id, "list");
    {
        let mut launcher = shared.lock().await;
        assert!(matches!(
            launcher.ensure(&pr(), None).await,
            Err(Error::Busy)
        ));
        assert!(matches!(launcher.remove(&pr()).await, Err(Error::Busy)));
    }
    let _: LauncherResponse = serve::read_message(&mut client).await.unwrap();
    task.await.unwrap().unwrap();
    listing.await.unwrap().unwrap();
    assert!(shared.lock().await.fetching.is_empty());
}

#[tokio::test]
async fn periodic_sweep_retries_docker_errors_and_skips_busy_lock() {
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use wiremock::{
        Mock, ResponseTemplate,
        matchers::{method, path_regex},
    };
    let (server, launcher) = fake_launcher().await;
    Mock::given(method("GET"))
        .and(path_regex(r"/containers/json$"))
        .respond_with(
            ResponseTemplate::new(500).set_body_json(serde_json::json!({"message":"transient"})),
        )
        .mount(&server)
        .await;
    let shared = Arc::new(Mutex::new(launcher));
    let guard = shared.lock().await;
    let sweep = tokio::spawn(serve::housekeeping(
        Arc::clone(&shared),
        std::time::Duration::from_millis(5),
    ));
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert!(server.received_requests().await.unwrap().is_empty());
    drop(guard);
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if server.received_requests().await.unwrap().len() >= 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await;
    sweep.abort();
    assert!(sweep.await.unwrap_err().is_cancelled());
    result.unwrap();
}

#[test]
fn template_hash_tracks_limits_but_ignores_per_job_memcan() {
    use contracts::job::access::{McpHttpEndpoint, Secret};
    let s = settings();
    let endpoint = |token| McpHttpEndpoint {
        url: "http://memcan/mcp".into(),
        token: Secret::new(token),
        allow_write: true,
    };
    let config = s.template(&pr(), false, Some(&endpoint("fake-A"))).unwrap();
    for _ in 0..10 {
        assert_eq!(
            config.labels,
            s.template(&pr(), false, Some(&endpoint("fake-A")))
                .unwrap()
                .labels
        );
    }
    let rotated = s.template(&pr(), false, Some(&endpoint("fake-B"))).unwrap();
    assert_eq!(config.labels, rotated.labels);
    assert_eq!(
        config.labels,
        s.template(&pr(), false, None).unwrap().labels
    );
    let mut moved = endpoint("fake-B");
    moved.url = "http://other-memcan/mcp".into();
    assert_eq!(
        config.labels,
        s.template(&pr(), false, Some(&moved)).unwrap().labels
    );
    let mut changed = s.clone();
    changed.memory /= 2;
    assert_ne!(
        config.labels,
        changed
            .template(&pr(), false, Some(&endpoint("fake-A")))
            .unwrap()
            .labels
    );
    let json = serde_json::to_string(&config).unwrap();
    assert!(!json.contains("fake-A"));
    assert!(!json.contains("DP_MEMCAN_TOKEN"));
}

#[tokio::test]
async fn warm_ensure_changes_per_job_memcan_without_recreating() {
    use contracts::job::access::{McpHttpEndpoint, Secret};
    use wiremock::{
        Mock, ResponseTemplate,
        matchers::{method, path_regex},
    };
    let (server, mut launcher) = fake_launcher().await;
    let endpoint = |url: &str, token| McpHttpEndpoint {
        url: url.into(),
        token: Secret::new(token),
        allow_write: true,
    };
    let old = endpoint("http://memcan/mcp", "fake-A");
    let rotated = endpoint("http://memcan/mcp", "fake-B");
    let moved = endpoint("http://other-memcan/mcp", "fake-B");
    let config = launcher
        .settings
        .template(&pr(), false, Some(&old))
        .unwrap();
    Mock::given(method("GET"))
        .and(path_regex(r"/containers/v13w3-test-pr-7-3/json$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Config":{"Image":config.image,"Labels":config.labels},"State":{"Running":true}})))
        .mount(&server)
        .await;
    mock_runtime(&server).await;
    let jobs = [None, Some(&old), Some(&rotated), None, Some(&moved)];
    for next in jobs {
        assert!(!launcher.ensure(&pr(), next).await.unwrap());
    }
    let requests = server.received_requests().await.unwrap();
    let resets: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path().ends_with("/exec"))
        .collect();
    assert_eq!(resets.len(), jobs.len());
    for (reset, next) in resets.iter().zip(jobs) {
        let body: serde_json::Value = serde_json::from_slice(&reset.body).unwrap();
        assert_eq!(
            body["Cmd"],
            serde_json::json!(["python3", "/usr/local/bin/worker-config.py", "cs1"])
        );
        assert_eq!(
            body["Env"],
            serde_json::json!(next.map(|e| vec![
                format!("DP_MEMCAN_URL={}", e.url),
                format!("DP_MEMCAN_TOKEN={}", e.token.expose()),
            ]))
        );
    }
    assert!(!requests.iter().any(|r| r.method == "DELETE"
        || r.url.path().ends_with("/containers/create")
        || r.url.path().ends_with("/start") && r.url.path().contains("/containers/")));
}

#[tokio::test]
async fn ensure_refuses_when_every_running_slot_is_attached() {
    use wiremock::{
        Mock, ResponseTemplate,
        matchers::{method, path_regex},
    };
    let (server, mut launcher) = fake_launcher().await;
    launcher.settings.max_running = 1;
    let old = ContainerId::Judge {
        payer: UserId("old".into()),
    };
    let new = ContainerId::Judge {
        payer: UserId("new".into()),
    };
    launcher
        .attached
        .insert(launcher.settings.name(&old, false));
    Mock::given(method("GET"))
        .and(path_regex(r"/containers/v13w3-test-user-new/json$"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(r"/containers/json$"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!([summary(
                &launcher.settings,
                &old,
                true
            )])),
        )
        .mount(&server)
        .await;
    assert!(matches!(
        launcher.ensure(&new, None).await,
        Err(Error::Busy)
    ));
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.method == "GET")
    );
}

#[tokio::test]
async fn client_preserves_correlated_typed_launcher_failure() {
    use contracts::launcher::{LauncherAction, LauncherFailure, LauncherRequest, LauncherResponse};
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("launcher.sock");
    let listener = tokio::net::UnixListener::bind(&path).unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request: LauncherRequest = serve::read_message(&mut socket).await.unwrap();
        serve::write_message(
            &mut socket,
            &LauncherResponse {
                id: request.id,
                workers: vec![],
                fetch: None,
                error: Some(LauncherFailure::Busy),
            },
        )
        .await
        .unwrap();
    });
    let result = Client::new(&path)
        .call(LauncherRequest {
            id: "ensure".into(),
            action: LauncherAction::Ensure {
                container: pr(),
                memcan: None,
            },
        })
        .await;
    assert!(matches!(result, Err(Error::Remote(LauncherFailure::Busy))));
    server.await.unwrap();
}

#[tokio::test]
async fn fetch_reports_sources_resolved_without_a_lock_file() {
    use wiremock::{
        Mock, ResponseTemplate,
        matchers::{method, path_regex},
    };
    let (server, launcher) = fake_launcher().await;
    mock_runtime(&server).await;
    for (verb, route, response) in [
        (
            "GET",
            r"/containers/v13w3-test-fetch-7-3/json$",
            ResponseTemplate::new(404),
        ),
        (
            "POST",
            r"/containers/create$",
            ResponseTemplate::new(201)
                .set_body_json(serde_json::json!({"Id":"fetch","Warnings":[]})),
        ),
        (
            "POST",
            r"/containers/v13w3-test-fetch-7-3/start$",
            ResponseTemplate::new(204),
        ),
        (
            "POST",
            r"/containers/v13w3-test-fetch-7-3/wait$",
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"StatusCode":0})),
        ),
        (
            "DELETE",
            r"/containers/v13w3-test-fetch-7-3$",
            ResponseTemplate::new(204),
        ),
    ] {
        Mock::given(method(verb))
            .and(path_regex(route))
            .respond_with(response)
            .mount(&server)
            .await;
    }
    let marker = b"dependency sources resolved without a lock file\n";
    let mut frame = vec![1, 0, 0, 0];
    frame.extend_from_slice(&(marker.len() as u32).to_be_bytes());
    frame.extend_from_slice(marker);
    Mock::given(method("GET"))
        .and(path_regex(r"/containers/v13w3-test-fetch-7-3/logs$"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(frame, "application/vnd.docker.raw-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(
        launcher.fetch(&pr()).await.unwrap(),
        contracts::launcher::DependencySources::ResolvedWithoutLock
    );
}

#[tokio::test]
async fn service_replies_with_safe_error_before_any_relay_upgrade() {
    use contracts::launcher::{LauncherAction, LauncherFailure, LauncherRequest, LauncherResponse};
    let (_server, launcher) = fake_launcher().await;
    let (socket, mut client) = tokio::net::UnixStream::pair().unwrap();
    let handler = tokio::spawn(serve::connection(
        socket,
        std::sync::Arc::new(tokio::sync::Mutex::new(launcher)),
    ));
    serve::write_message(
        &mut client,
        &LauncherRequest {
            id: "bad-worker".into(),
            action: LauncherAction::Ensure {
                container: ContainerId::Pr {
                    pull_request_id: -1,
                    payer: UserId("payer".into()),
                },
                memcan: None,
            },
        },
    )
    .await
    .unwrap();
    let response: LauncherResponse = serve::read_message(&mut client).await.unwrap();
    assert_eq!(response.id, "bad-worker");
    assert_eq!(response.error, Some(LauncherFailure::InvalidTemplate));
    assert!(matches!(
        handler.await.unwrap(),
        Err(Error::InvalidTemplate)
    ));
}
