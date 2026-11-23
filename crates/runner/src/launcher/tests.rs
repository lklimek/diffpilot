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
    assert_eq!(host.memory, host.memory_swap);
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
    let server = wiremock::MockServer::start().await;
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
async fn relay_forwards_stdout_only_and_stops_after_disconnect() {
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
        for step in 0..5 {
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
    assert!(shared.lock().await.attached.is_empty());
}

#[tokio::test]
async fn reconciliation_list_can_exceed_the_request_size_limit() {
    let (mut tx, mut rx) = tokio::net::UnixStream::pair().unwrap();
    let response = contracts::launcher::LauncherResponse {
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
