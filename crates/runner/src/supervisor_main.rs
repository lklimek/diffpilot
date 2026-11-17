//! `smart-review-supervisor`: the runner container's root process (ADR-013). `serve -- <runner argv>` runs
//! the supervisor; other modes are helpers the runner starts as the agent uid.

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use runner::supervisor::serve::{EXIT_SETUP_FAILED, Ids, ServeConfig, serve};

const RUNNER_UID: &str = "SR_RUNNER_UID";
const AGENT_UID: &str = "SR_AGENT_UID";
const SWEEP_DEADLINE_MS: &str = "SR_SUPERVISOR_SWEEP_DEADLINE_MS";
const UNPRIVILEGED: &str = "SR_SUPERVISOR_UNPRIVILEGED";
/// Test-only fault injection (container test): every sweep fails. Honoured only with [`TEST_FAULTS`].
const FAULT_SWEEP_FAILS: &str = "R1_FAULT_SWEEP_FAILS";
/// Enables test-only fault injection; never set in the image.
const TEST_FAULTS: &str = "SR_SUPERVISOR_TEST_FAULTS";
/// Shortest sweep deadline accepted: a shorter one would fail every sweep of a busy agent.
const MIN_SWEEP_DEADLINE_MS: u64 = 100;

fn main() {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let code = match args.first().and_then(|mode| mode.to_str()) {
        Some("serve") => {
            let runner = match args.get(1).and_then(|sep| sep.to_str()) {
                Some("--") => &args[2..],
                _ => &args[1..],
            };
            match config() {
                Ok(config) => serve(config, runner),
                Err(message) => {
                    eprintln!("supervisor: {message}");
                    EXIT_SETUP_FAILED
                }
            }
        }
        _ => runner::supervisor::helper(&args),
    };
    std::process::exit(code);
}

fn config() -> Result<ServeConfig, String> {
    let privileged = rustix::process::geteuid().is_root();
    let unprivileged_allowed = std::env::var_os(UNPRIVILEGED).is_some_and(|value| value == "1");
    if !privileged && !unprivileged_allowed {
        return Err(format!(
            "must start as root (set {UNPRIVILEGED}=1 to run unprivileged for tests and development)"
        ));
    }
    let own = rustix::process::getuid().as_raw();
    let id = |key: &str, default: u32| -> Result<Ids, String> {
        let uid = match std::env::var(key) {
            Ok(value) => value.parse().map_err(|_| format!("{key} is not a uid"))?,
            Err(_) if privileged => default,
            Err(_) => own,
        };
        if privileged && uid == 0 {
            return Err(format!("{key} must not be root"));
        }
        Ok(Ids { uid, gid: uid })
    };
    let runner = id(RUNNER_UID, 10001)?;
    let agent = id(AGENT_UID, 10002)?;
    if privileged && runner == agent {
        return Err("the runner and agent uids must differ".to_owned());
    }
    let sweep_ms = match std::env::var(SWEEP_DEADLINE_MS) {
        Ok(value) => value
            .parse()
            .map_err(|_| format!("{SWEEP_DEADLINE_MS} is not a number"))?,
        Err(_) => 5000,
    };
    if sweep_ms < MIN_SWEEP_DEADLINE_MS {
        return Err(format!(
            "{SWEEP_DEADLINE_MS} must be at least {MIN_SWEEP_DEADLINE_MS}"
        ));
    }
    let set = |key: &str| std::env::var_os(key).is_some_and(|value| value == "1");
    let fail_sweeps = set(FAULT_SWEEP_FAILS);
    if fail_sweeps && !set(TEST_FAULTS) {
        return Err(format!(
            "{FAULT_SWEEP_FAILS} needs {TEST_FAULTS}=1 (tests only)"
        ));
    }
    if fail_sweeps {
        eprintln!("supervisor: TEST FAULT INJECTION: every sweep fails");
    }
    let path = |key: &str, default: &str| {
        std::env::var_os(key).map_or_else(|| PathBuf::from(default), PathBuf::from)
    };
    let state_dir = path("SR_RUNNER_STATE_DIR", "/var/lib/smart-review-runner");
    let work_dir = std::env::var_os("SR_RUNNER_WORK_DIR")
        .map_or_else(|| state_dir.join("work"), PathBuf::from);
    Ok(ServeConfig {
        runner,
        agent,
        privileged,
        state_dir,
        work_dir,
        sweep_deadline: Duration::from_millis(sweep_ms),
        fail_sweeps,
        binary: std::env::current_exe().map_err(|error| format!("own path: {error}"))?,
    })
}
