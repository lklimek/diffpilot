//! The real `smart-review-supervisor` binary, unprivileged (runner and agents share the test's uid; the sweep
//! is scoped to the supervisor's descendants outside the runner). The runner is this test binary re-run with
//! `SR_TEST_SCENARIO`, talking to the supervisor through `runner::supervisor::client`. Check names follow the
//! ADR-013 container test (`deploy/runner/r1-test/`) where a scenario runs without a container.
#![expect(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

use std::os::fd::AsFd as _;
use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use runner::supervisor::client::{Supervisor, SupervisorError};
use tempfile::TempDir;

const SUPERVISOR: &str = env!("CARGO_BIN_EXE_smart-review-supervisor");
const SCENARIO: &str = "SR_TEST_SCENARIO";

/// Runs `scenario` as the runner under the supervisor; the supervisor's exit code.
fn supervise(scenario: &str, state: &Path, extra: &[(&str, &str)]) -> i32 {
    let mut command = Command::new(SUPERVISOR);
    command
        .args(["serve", "--"])
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "scenario_runner",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("SR_SUPERVISOR_UNPRIVILEGED", "1")
        .env("SR_RUNNER_STATE_DIR", state)
        .env("SR_RUNNER_WORK_DIR", state.join("work"))
        .env(SCENARIO, scenario)
        .stdin(Stdio::null());
    for (key, value) in extra {
        command.env(key, value);
    }
    command.status().unwrap().code().unwrap_or(-1)
}

fn state() -> TempDir {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join("codex-homes")).unwrap();
    std::fs::create_dir(dir.path().join("work")).unwrap();
    dir
}

#[test]
fn supervisor_helpers_and_handover() {
    let dir = state();
    assert_eq!(supervise("helpers", dir.path(), &[]), 0);
}

#[test]
fn spawned_agent_has_exactly_its_env_its_own_session_and_oom_score() {
    let dir = state();
    assert_eq!(supervise("spawn", dir.path(), &[]), 0);
}

#[test]
fn signal_to_non_agent_pgid_refused() {
    let dir = state();
    assert_eq!(supervise("signal", dir.path(), &[]), 0);
}

#[test]
fn hung_helper_hits_deadline_then_swept() {
    let dir = state();
    assert_eq!(supervise("hung_helper", dir.path(), &[]), 0);
}

#[test]
fn closed_streams_sigterm_ignored_hits_deadline_then_swept() {
    let dir = state();
    assert_eq!(supervise("closed_output", dir.path(), &[]), 0);
}

#[test]
fn hung_agent_command_hits_deadline_then_swept() {
    let dir = state();
    assert_eq!(supervise("hung_command", dir.path(), &[]), 0);
}

#[test]
fn supervisor_refuses_helper_while_agent_alive() {
    let dir = state();
    assert_eq!(supervise("exclusive", dir.path(), &[]), 0);
}

#[test]
fn hostile_agent_swept_within_deadline() {
    let dir = state();
    assert_eq!(supervise("hostile", dir.path(), &[]), 0);
}

#[test]
fn sweep_failure_terminates_container() {
    let dir = state();
    let code = supervise(
        "sweep_fails",
        dir.path(),
        &[("SR_SUPERVISOR_SWEEP_DEADLINE_MS", "0")],
    );
    assert_eq!(code, 70);
}

#[test]
fn supervisor_exits_with_the_runner_status() {
    let dir = state();
    let status = Command::new(SUPERVISOR)
        .args(["serve", "--", "/bin/sh", "-c", "exit 3"])
        .env("SR_SUPERVISOR_UNPRIVILEGED", "1")
        .env("SR_RUNNER_STATE_DIR", dir.path())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(3));
}

#[test]
fn supervisor_refuses_to_run_unprivileged_by_accident() {
    if rustix::process::geteuid().is_root() {
        return;
    }
    let status = Command::new(SUPERVISOR)
        .args(["serve", "--", "/bin/true"])
        .env_remove("SR_SUPERVISOR_UNPRIVILEGED")
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(71));
}

#[test]
fn sigterm_forwarded_to_runner() {
    let dir = state();
    let mut supervisor = Command::new(SUPERVISOR)
        .args([
            "serve",
            "--",
            "/bin/sh",
            "-c",
            "trap 'exit 42' TERM; echo ready; while :; do sleep 0.05; done",
        ])
        .env("SR_SUPERVISOR_UNPRIVILEGED", "1")
        .env("SR_RUNNER_STATE_DIR", dir.path())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    std::io::BufRead::read_line(
        &mut std::io::BufReader::new(supervisor.stdout.take().unwrap()),
        &mut line,
    )
    .unwrap();
    assert_eq!(line.trim(), "ready");
    let pid = rustix::process::Pid::from_raw(i32::try_from(supervisor.id()).unwrap()).unwrap();
    rustix::process::kill_process(pid, rustix::process::Signal::TERM).unwrap();
    assert_eq!(supervisor.wait().unwrap().code(), Some(42));
}

/// The runner side of each scenario; a no-op unless started by `supervise`.
#[test]
fn scenario_runner() {
    let Ok(scenario) = std::env::var(SCENARIO) else {
        return;
    };
    let state = PathBuf::from(std::env::var("SR_RUNNER_STATE_DIR").unwrap());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        // One connection per process: a second reader on the socket would take replies of the first.
        let supervisor = if matches!(
            scenario.as_str(),
            "hung_helper" | "closed_output" | "hung_command"
        ) {
            let socket = std::io::stdin().as_fd().try_clone_to_owned().unwrap();
            Supervisor::connect(
                socket,
                PathBuf::from(SUPERVISOR),
                Duration::from_millis(800),
            )
            .unwrap()
        } else {
            Supervisor::from_env().unwrap().expect("SR_SUPERVISOR_FD")
        };
        match scenario.as_str() {
            "helpers" => helpers(&supervisor, &state).await,
            "spawn" => spawn(&supervisor).await,
            "signal" => signal(&supervisor).await,
            "hung_helper" => {
                deadline_then_swept(&supervisor, &["/bin/sh", "-c", "kill -STOP $$"]).await;
            }
            "closed_output" => {
                deadline_then_swept(
                    &supervisor,
                    &[
                        "/bin/sh",
                        "-c",
                        "exec >&- 2>&-; trap '' TERM; while :; do sleep 1; done",
                    ],
                )
                .await;
            }
            "hung_command" => {
                deadline_then_swept(
                    &supervisor,
                    &["/bin/sh", "-c", "trap '' TERM; while :; do sleep 1; done"],
                )
                .await;
            }
            "exclusive" => {
                let agent = supervisor
                    .spawn(sh("exec sleep 30"), vec![], Path::new("/"))
                    .await
                    .unwrap();
                let home = state.join("codex-homes");
                assert!(matches!(
                    supervisor.list(&home).await,
                    Err(SupervisorError::Refused { .. })
                ));
                assert!(matches!(
                    supervisor.run_agent_tool(&["/bin/true"]).await,
                    Err(SupervisorError::Refused { .. })
                ));
                supervisor.sweep().await.unwrap();
                assert_eq!(supervisor.wait(agent.pid).await.unwrap(), 128 + 9);
                assert!(supervisor.list(&home).await.unwrap().is_empty());
            }
            "hostile" => {
                let agent = supervisor
                    .spawn(
                        sh("trap '' TERM; while :; do sleep 0.01 & done"),
                        vec![],
                        Path::new("/"),
                    )
                    .await
                    .unwrap();
                tokio::time::sleep(Duration::from_millis(300)).await;
                let started = Instant::now();
                supervisor.sweep().await.expect("swept");
                assert!(started.elapsed() < Duration::from_secs(5));
                assert_ne!(supervisor.wait(agent.pid).await.unwrap(), 0);
            }
            "sweep_fails" => {
                let _agent = supervisor
                    .spawn(sh("sleep 30"), vec![], Path::new("/"))
                    .await
                    .unwrap();
                assert!(supervisor.sweep().await.is_err());
                // The supervisor exits 70 and kills this runner.
                tokio::time::sleep(Duration::from_secs(30)).await;
                panic!("the supervisor did not fail closed");
            }
            other => panic!("unknown scenario {other}"),
        }
    });
}

fn sh(script: &str) -> Vec<String> {
    vec!["/bin/sh".to_owned(), "-c".to_owned(), script.to_owned()]
}

async fn helpers(supervisor: &Supervisor, state: &Path) {
    let home = state.join("codex-homes").join("job-1");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&home)
        .unwrap();
    supervisor.handover(&home).await.expect("handover");
    supervisor
        .copy_in(&home, Path::new("sessions/2026/r.jsonl"), b"rollout\n")
        .await
        .unwrap();
    supervisor
        .copy_in(&home, Path::new("auth.json"), b"{}")
        .await
        .unwrap();
    assert_eq!(
        supervisor
            .copy_out(&home, Path::new("auth.json"), 16)
            .await
            .unwrap(),
        b"{}"
    );
    assert!(matches!(
        supervisor.copy_out(&home, Path::new("auth.json"), 1).await,
        Err(SupervisorError::Helper { .. })
    ));
    // copy_back_as_agent_cannot_reach_runner_files
    let secret = state.join("credentials-auth.json");
    std::fs::write(&secret, "secret").unwrap();
    symlink(&secret, home.join("link.json")).unwrap();
    assert!(
        supervisor
            .copy_out(&home, Path::new("link.json"), 1024)
            .await
            .is_err()
    );
    let listed = supervisor.list(&home).await.unwrap();
    assert!(
        listed
            .iter()
            .any(|e| e.path == "sessions/2026/r.jsonl" && e.file)
    );
    supervisor.clear(&home).await.unwrap();
    assert!(supervisor.list(&home).await.unwrap().is_empty());
    std::fs::remove_dir(&home).unwrap();
    assert!(secret.exists());
    // handover_refuses_unsafe_paths
    let full = state.join("codex-homes").join("job-2");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&full)
        .unwrap();
    std::fs::write(full.join("x"), "x").unwrap();
    assert!(supervisor.handover(&full).await.is_err(), "not empty");
    let open = state.join("codex-homes").join("job-3");
    std::fs::DirBuilder::new()
        .mode(0o755)
        .create(&open)
        .unwrap();
    assert!(supervisor.handover(&open).await.is_err(), "not 0700");
    let elsewhere = state.join("elsewhere");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&elsewhere)
        .unwrap();
    assert!(supervisor.handover(&elsewhere).await.is_err(), "outside");
    symlink(&elsewhere, state.join("codex-homes").join("job-4")).unwrap();
    assert!(
        supervisor
            .handover(&state.join("codex-homes").join("job-4"))
            .await
            .is_err(),
        "symlink"
    );
    let job = state.join("work").join("job-5");
    std::fs::DirBuilder::new().mode(0o755).create(&job).unwrap();
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(job.join("home"))
        .unwrap();
    supervisor.handover(&job.join("home")).await.expect("home");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(job.join("repo"))
        .unwrap();
    assert!(supervisor.handover(&job.join("repo")).await.is_err());
    assert_eq!(
        std::fs::metadata(job.join("home"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}

async fn spawn(supervisor: &Supervisor) {
    let mut agent = supervisor
        .spawn(
            sh("cat /proc/$$/oom_score_adj; cut -d' ' -f6 /proc/$$/stat; echo $$; /usr/bin/env"),
            vec!["FOO=bar".to_owned(), "PATH=/usr/bin:/bin".to_owned()],
            Path::new("/"),
        )
        .await
        .unwrap();
    let mut out = String::new();
    tokio::io::AsyncReadExt::read_to_string(&mut agent.stdout, &mut out)
        .await
        .unwrap();
    assert_eq!(supervisor.wait(agent.pid).await.unwrap(), 0);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "1000", "OOM killer takes agents first");
    assert_eq!(lines[1], lines[2], "the agent leads its own session");
    assert_eq!(lines[2], agent.pid.to_string());
    let env: Vec<&str> = lines[3..]
        .iter()
        .copied()
        .filter(|line| {
            !line.starts_with("PWD=") && !line.starts_with("SHLVL=") && !line.starts_with("_=")
        })
        .collect();
    let mut env = env;
    env.sort_unstable();
    assert_eq!(
        env,
        ["FOO=bar", "PATH=/usr/bin:/bin"],
        "exactly the given environment"
    );
}

async fn signal(supervisor: &Supervisor) {
    let agent = supervisor
        .spawn(sh("exec sleep 30"), vec![], Path::new("/"))
        .await
        .unwrap();
    let own = rustix::process::getpid().as_raw_nonzero().get();
    assert!(matches!(
        supervisor.signal(own, 15).await,
        Err(SupervisorError::Refused { .. })
    ));
    assert!(matches!(
        supervisor.signal(agent.pid, 19).await,
        Err(SupervisorError::Refused { .. })
    ));
    supervisor.signal(agent.pid, 15).await.expect("own agent");
    assert_eq!(supervisor.wait(agent.pid).await.unwrap(), 128 + 15);
}

/// The helper deadline is short here; the run must end in `Deadline` after a sweep, not hang.
async fn deadline_then_swept(supervisor: &Supervisor, argv: &[&str]) {
    let started = Instant::now();
    let result = supervisor.run_agent_tool(argv).await;
    assert!(
        matches!(result, Err(SupervisorError::Deadline(_))),
        "{result:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(10));
}
