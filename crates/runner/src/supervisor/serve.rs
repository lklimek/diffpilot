//! The supervisor (ADR-013): starts the runner as the runner uid with the control socket as its stdin, starts
//! agent processes as the agent uid on request, hands per-job directories over, signals agent process groups
//! and sweeps agent processes. A subreaper, so escaped agent processes come back to it. Fails closed: a sweep
//! that leaves an agent process alive ends the supervisor (and with it the container) with
//! [`EXIT_SWEEP_FAILED`].

use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::process::CommandExt as _;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use rustix::fs::{AtFlags, Mode, OFlags, ResolveFlags, chownat, fstat, openat2};
use rustix::net::{AddressFamily, SocketFlags, SocketType, socketpair};
use rustix::process::{Gid, Pid, Signal, Uid, WaitOptions, kill_process};
use tokio::io::Interest;
use tokio::net::UnixDatagram;
use tokio::signal::unix::{SignalKind, signal};

use super::procs::{self, Scope, Target};
use super::protocol::{self, Op, Reply, Request, SUPERVISOR_BIN, SUPERVISOR_FD};

/// Exit code of the supervisor when a sweep left an agent process alive.
pub const EXIT_SWEEP_FAILED: i32 = 70;
/// The helper mode that becomes an agent process.
pub const EXEC_AGENT: &str = "exec-agent";
/// Exit code when the supervisor itself cannot run.
pub const EXIT_SETUP_FAILED: i32 = 71;

/// A uid and gid pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ids {
    /// The uid.
    pub uid: u32,
    /// The gid.
    pub gid: u32,
}

/// How the supervisor runs.
#[derive(Clone, Debug)]
pub struct ServeConfig {
    /// The runner's ids.
    pub runner: Ids,
    /// The agent's ids.
    pub agent: Ids,
    /// Root: switches uids. Otherwise (tests, development) runner and agents keep the supervisor's uid.
    pub privileged: bool,
    /// `SR_RUNNER_STATE_DIR`; per-job `CODEX_HOME`s are handed over below `<state>/codex-homes`.
    pub state_dir: PathBuf,
    /// `SR_RUNNER_WORK_DIR`; `<work>/<job>/{home,tmp}` are handed over.
    pub work_dir: PathBuf,
    /// How long a sweep may take before the supervisor gives up.
    pub sweep_deadline: Duration,
    /// Test-only fault injection: every sweep fails (the container test's fail-closed check).
    pub fail_sweeps: bool,
    /// This binary, given to the runner for helper modes.
    pub binary: PathBuf,
}

/// The request handler, kept apart from the event loop for tests.
#[derive(Debug)]
pub(crate) struct Server {
    config: ServeConfig,
    target: Target,
    /// The runner's pid (`None` in tests: then only spawned pids are reaped, not every child).
    runner: Option<i32>,
    /// The runner's exit code once reaped (kept, also when reaped during a sweep).
    runner_status: Option<i32>,
    /// Spawned agent pids and, once reaped, their status.
    spawned: HashMap<i32, Option<i32>>,
    /// Reaped spawned pids whose waiters were not answered yet.
    exited: Vec<i32>,
    /// Pending `wait` request ids per pid.
    waiters: HashMap<i32, Vec<u64>>,
}

/// What the loop does after a request.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum After {
    /// Keep serving.
    Continue,
    /// Exit with this code (fail closed).
    Exit(i32),
}

impl Server {
    /// A handler; `runner` is the runner's pid (its subtree is never swept when unprivileged).
    pub(crate) fn new(config: ServeConfig, runner: Option<i32>) -> Self {
        let target = Target {
            uid: config.agent.uid,
            scope: (!config.privileged).then(|| Scope {
                root: rustix::process::getpid().as_raw_nonzero().get(),
                exclude: runner.unwrap_or(0),
            }),
        };
        Self {
            config,
            target,
            runner,
            runner_status: None,
            spawned: HashMap::new(),
            exited: Vec::new(),
            waiters: HashMap::new(),
        }
    }

    /// Handles one datagram; replies go to `socket`.
    pub(crate) fn handle(&mut self, socket: impl AsFd, data: &[u8], fds: Vec<OwnedFd>) -> After {
        let request: Request = match serde_json::from_slice(data) {
            Ok(request) => request,
            Err(error) => {
                eprintln!("supervisor: malformed request: {error}");
                return After::Continue;
            }
        };
        let id = request.id;
        let mut after = After::Continue;
        let reply = match request.op {
            Op::Spawn {
                argv,
                env,
                cwd,
                exclusive,
            } => self
                .spawn(&argv, &env, &cwd, exclusive, fds)
                .map(|pid| Reply {
                    pid: Some(pid),
                    ..Reply::default()
                }),
            Op::Wait { pid } => match self.spawned.get(&pid) {
                None => Err("not a spawned pid".to_owned()),
                Some(Some(status)) => Ok(Reply {
                    status: Some(*status),
                    ..Reply::default()
                }),
                Some(None) => {
                    self.waiters.entry(pid).or_default().push(id);
                    return After::Continue;
                }
            },
            Op::Handover { path } => self.handover(Path::new(&path)).map(|()| Reply::default()),
            Op::Quarantine { path } => self.quarantine(Path::new(&path)).map(|()| Reply::default()),
            Op::Sweep => match self.sweep() {
                Ok(reply) => Ok(reply),
                Err(error) => {
                    after = After::Exit(EXIT_SWEEP_FAILED);
                    Err(error)
                }
            },
            Op::Signal { pid, signal } => match self.spawned.get(&pid) {
                Some(None) => match procs::signal_group(pid, signal, self.target) {
                    // Still the root wrapper (not yet the agent's group): signal the unreaped child itself;
                    // its pid cannot have been reused.
                    Err(procs::SignalRefused::NotAgentGroup(_)) => Self::signal_child(pid, signal)
                        .map(|signalled| Reply {
                            signalled: Some(signalled),
                            ..Reply::default()
                        }),
                    result => result
                        .map(|signalled| Reply {
                            signalled: Some(signalled),
                            ..Reply::default()
                        })
                        .map_err(|error| error.to_string()),
                },
                _ => Err("not a running spawned pid".to_owned()),
            },
        };
        let reply = match reply {
            Ok(reply) => Reply {
                id,
                ok: true,
                ..reply
            },
            Err(error) => Reply {
                id,
                ok: false,
                error: Some(error),
                ..Reply::default()
            },
        };
        send_reply(&socket, &reply);
        self.answer_waiters(&socket);
        after
    }

    /// SIGKILL to every agent process, reaping between rounds; an error means fail closed.
    /// `signal` to an unreaped spawned child that is not the agent yet.
    fn signal_child(pid: i32, signal: i32) -> Result<u32, String> {
        let signal = match signal {
            2 => Signal::INT,
            9 => Signal::KILL,
            15 => Signal::TERM,
            other => return Err(format!("signal {other} is not allowed")),
        };
        let pid = Pid::from_raw(pid).ok_or_else(|| "bad pid".to_owned())?;
        Ok(u32::from(kill_process(pid, signal).is_ok()))
    }

    /// An agent thread lives, or a spawned child is not reaped yet (it may still be the root wrapper that is
    /// about to become the agent).
    fn agent_running(&mut self) -> bool {
        self.collect();
        self.spawned.values().any(Option::is_none) || procs::agent_alive(self.target)
    }

    fn sweep(&mut self) -> Result<Reply, String> {
        let started = std::time::Instant::now();
        // A spawned child still in its root wrapper has no agent uid yet: kill it by pid. It is an unreaped
        // child of this process, so the pid cannot have been reused.
        self.collect();
        for (&pid, _) in self.spawned.iter().filter(|(_, status)| status.is_none()) {
            if let Some(pid) = Pid::from_raw(pid) {
                let _ = kill_process(pid, Signal::KILL);
            }
        }
        let (target, deadline) = (self.target, self.config.sweep_deadline);
        let result = if self.config.fail_sweeps {
            Err(procs::SweepFailed {
                survivors: Vec::new(),
            })
        } else {
            procs::sweep(target, deadline, || self.collect())
        };
        self.collect();
        match result {
            Ok(swept) => Ok(Reply {
                rounds: Some(swept.rounds),
                killed: Some(swept.killed),
                ms: Some(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)),
                ..Reply::default()
            }),
            Err(error) => {
                eprintln!(
                    "supervisor: sweep failed, terminating the container: {error}: {:?}",
                    error.survivors
                );
                Err(error.to_string())
            }
        }
    }

    /// Reaps exited children (every child as the subreaper; in tests only spawned ones) and keeps their
    /// statuses; never answers here (also called between sweep rounds).
    fn collect(&mut self) {
        let mut reaped = Vec::new();
        if self.runner.is_some() {
            while let Ok(Some((pid, status))) = rustix::process::wait(WaitOptions::NOHANG) {
                reaped.push((pid.as_raw_nonzero().get(), status));
            }
        } else {
            let pending: Vec<i32> = self
                .spawned
                .iter()
                .filter(|(_, status)| status.is_none())
                .map(|(pid, _)| *pid)
                .collect();
            for pid in pending {
                if let Some(raw) = Pid::from_raw(pid)
                    && let Ok(Some((_, status))) =
                        rustix::process::waitpid(Some(raw), WaitOptions::NOHANG)
                {
                    reaped.push((pid, status));
                }
            }
        }
        for (pid, status) in reaped {
            let code = exit_code(status);
            if Some(pid) == self.runner {
                self.runner_status = Some(code);
            } else if let Some(slot) = self.spawned.get_mut(&pid) {
                *slot = Some(code);
                self.exited.push(pid);
            }
        }
    }

    fn answer_waiters(&mut self, socket: &impl AsFd) {
        for pid in std::mem::take(&mut self.exited) {
            let status = self.spawned.get(&pid).copied().flatten();
            for id in self.waiters.remove(&pid).unwrap_or_default() {
                send_reply(
                    socket,
                    &Reply {
                        id,
                        ok: true,
                        status,
                        ..Reply::default()
                    },
                );
            }
        }
    }

    /// Reaps and answers waiters; the runner's exit code once it was reaped.
    pub(crate) fn reap(&mut self, socket: impl AsFd) -> Option<i32> {
        self.collect();
        self.answer_waiters(&socket);
        self.runner_status
    }

    /// The final sweep: the exit code to use.
    pub(crate) fn final_sweep(&mut self, runner_status: i32) -> i32 {
        match self.sweep() {
            Ok(_) => process_exit_code(runner_status),
            Err(_) => EXIT_SWEEP_FAILED,
        }
    }

    /// Starts `argv` as the agent through the `exec-agent` mode of this binary, which sets `oom_score_adj`
    /// 1000 and `setsid()` before it executes `argv` (no `pre_exec`: the crate forbids unsafe code).
    fn spawn(
        &mut self,
        argv: &[String],
        env: &[String],
        cwd: &str,
        exclusive: bool,
        fds: Vec<OwnedFd>,
    ) -> Result<i32, String> {
        let [stdin, stdout, stderr]: [OwnedFd; 3] =
            fds.try_into().map_err(|_| "spawn needs 3 fds".to_owned())?;
        let Some(program) = argv.first() else {
            return Err("empty argv".to_owned());
        };
        if !Path::new(program).is_absolute() || !Path::new(cwd).is_absolute() {
            return Err("program and cwd must be absolute".to_owned());
        }
        let own_helper = Path::new(program) == self.config.binary
            || std::fs::canonicalize(program).ok()
                == std::fs::canonicalize(&self.config.binary).ok();
        if (exclusive || own_helper) && self.agent_running() {
            return Err(
                "refused: helpers run only after a sweep (an agent process is alive)".to_owned(),
            );
        }
        // The wrapper starts as root and switches to the agent itself, so `spawn` returns at its exec: an agent
        // that stops every process of its uid cannot stop a child that std's spawn is still waiting for.
        let identity = if self.config.privileged {
            format!("{}:{}", self.config.agent.uid, self.config.agent.gid)
        } else {
            "-".to_owned()
        };
        let mut command = Command::new(&self.config.binary);
        command
            .arg(EXEC_AGENT)
            .arg(cwd)
            .arg(identity)
            .arg("--")
            .args(argv)
            .env_clear()
            .current_dir("/")
            .stdin(Stdio::from(stdin))
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        for entry in env {
            let (key, value) = entry
                .split_once('=')
                .ok_or_else(|| "env entries are KEY=VALUE".to_owned())?;
            command.env(key, value);
        }
        // Agent files are private to the agent (single-threaded: the mask is restored right after).
        let previous = rustix::process::umask(Mode::from_raw_mode(0o077));
        let spawned = command.spawn();
        rustix::process::umask(previous);
        let child = spawned.map_err(|error| format!("spawn failed: {error}"))?;
        let pid = i32::try_from(child.id()).map_err(|_| "pid out of range".to_owned())?;
        // Reaped by `collect`, never by std.
        drop(child);
        self.spawned.insert(pid, None);
        Ok(pid)
    }

    /// Gives an empty runner-owned 0700 per-job directory to the agent.
    /// Takes a per-job directory the agent could not empty away from it (fail closed): chown to the runner
    /// through a no-symlink `O_PATH` descriptor, after which the runner sets mode 0700. Only while no agent
    /// process is alive.
    fn quarantine(&self, path: &Path) -> Result<(), String> {
        self.per_job_dir(path)?;
        if procs::agent_alive(self.target) {
            return Err(
                "refused: helpers run only after a sweep (an agent process is alive)".to_owned(),
            );
        }
        let fd = openat2(
            rustix::fs::CWD,
            path,
            OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
        )
        .map_err(|error| format!("open: {error}"))?;
        let owner = fstat(&fd).map_err(|error| format!("stat: {error}"))?.st_uid;
        if owner != self.config.agent.uid && owner != self.config.runner.uid {
            return Err("not an agent or runner directory".to_owned());
        }
        chownat(
            &fd,
            "",
            Some(Uid::from_raw(self.config.runner.uid)),
            Some(Gid::from_raw(self.config.runner.gid)),
            AtFlags::EMPTY_PATH,
        )
        .map_err(|error| format!("chown: {error}"))
    }

    /// `<state>/codex-homes/<name>` or `<work>/<name>/{home,tmp}`, normalised.
    fn per_job_dir(&self, path: &Path) -> Result<(), String> {
        let parent = path.parent().unwrap_or(Path::new(""));
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let codex_homes = self.config.state_dir.join("codex-homes");
        let allowed = parent == codex_homes
            || (parent.parent() == Some(self.config.work_dir.as_path())
                && matches!(name, "home" | "tmp"));
        let plain = path
            .components()
            .all(|c| matches!(c, Component::RootDir | Component::Normal(_)));
        if !plain || !allowed || !valid_name(name) {
            return Err("not a per-job directory".to_owned());
        }
        Ok(())
    }

    fn handover(&self, path: &Path) -> Result<(), String> {
        self.per_job_dir(path)?;
        let fd = openat2(
            rustix::fs::CWD,
            path,
            OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
        )
        .map_err(|error| format!("open: {error}"))?;
        let stat = fstat(&fd).map_err(|error| format!("stat: {error}"))?;
        if stat.st_uid != self.config.runner.uid || stat.st_mode & 0o7777 != 0o700 {
            return Err("not a runner-owned 0700 directory".to_owned());
        }
        // Root without CAP_DAC_READ_SEARCH cannot list it: a helper as the runner does.
        let mut inspect = Command::new(&self.config.binary);
        inspect
            .arg("inspect")
            .arg(path)
            .env_clear()
            .stdin(Stdio::null())
            .stderr(Stdio::null());
        if self.config.privileged {
            inspect
                .uid(self.config.runner.uid)
                .gid(self.config.runner.gid);
        }
        let output = inspect
            .output()
            .map_err(|error| format!("inspect: {error}"))?;
        let expected = format!("{} {} empty", stat.st_dev, stat.st_ino);
        if !output.status.success() || String::from_utf8_lossy(&output.stdout).trim() != expected {
            return Err("directory not empty or replaced".to_owned());
        }
        chownat(
            &fd,
            "",
            Some(Uid::from_raw(self.config.agent.uid)),
            Some(Gid::from_raw(self.config.agent.gid)),
            AtFlags::EMPTY_PATH,
        )
        .map_err(|error| format!("chown: {error}"))
    }
}

/// A per-job directory name.
pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

fn send_reply(socket: impl AsFd, reply: &Reply) {
    match serde_json::to_vec(reply) {
        Ok(bytes) => {
            // A runner that went away is reaped next.
            let _ = protocol::send(socket, &bytes, &[]);
        }
        Err(error) => eprintln!("supervisor: cannot encode a reply: {error}"),
    }
}

/// A `wait` status as ADR-013 encodes it: the exit code, or the negated signal number (`-9` for SIGKILL), so a
/// signal never looks like an exit code.
fn exit_code(status: rustix::process::WaitStatus) -> i32 {
    if let Some(code) = status.exit_status() {
        code
    } else if let Some(signal) = status.terminating_signal() {
        -signal
    } else {
        1
    }
}

/// The supervisor's own exit code for the runner's `wait` status (a signal as 128 + signal).
fn process_exit_code(status: i32) -> i32 {
    if status < 0 { 128 - status } else { status }
}

/// Runs the supervisor around `runner_argv` and returns its exit code.
pub fn serve(config: ServeConfig, runner_argv: &[OsString]) -> i32 {
    match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime.block_on(serve_inner(config, runner_argv)),
        Err(error) => {
            eprintln!("supervisor: cannot start: {error}");
            EXIT_SETUP_FAILED
        }
    }
}

async fn serve_inner(config: ServeConfig, runner_argv: &[OsString]) -> i32 {
    let setup = || -> io::Result<_> {
        rustix::process::set_child_subreaper(Some(rustix::process::getpid()))?;
        if config.privileged {
            for root in [
                config.state_dir.clone(),
                config.state_dir.join("codex-homes"),
                config.work_dir.clone(),
            ] {
                // Fresh volume roots; their modes are the runner's job.
                if let Ok(meta) = std::fs::symlink_metadata(&root)
                    && meta.is_dir()
                {
                    std::os::unix::fs::lchown(
                        &root,
                        Some(config.runner.uid),
                        Some(config.runner.gid),
                    )?;
                }
            }
        }
        let (mine, runner_end) = socketpair(
            AddressFamily::UNIX,
            SocketType::SEQPACKET,
            SocketFlags::CLOEXEC,
            None,
        )?;
        rustix::fs::fcntl_setfl(&mine, OFlags::NONBLOCK)?;
        let terminate = signal(SignalKind::terminate())?;
        let interrupt = signal(SignalKind::interrupt())?;
        let child = signal(SignalKind::child())?;
        Ok((mine, runner_end, terminate, interrupt, child))
    };
    let (mine, runner_end, mut terminate, mut interrupt, mut child) = match setup() {
        Ok(parts) => parts,
        Err(error) => {
            eprintln!("supervisor: setup failed: {error}");
            return EXIT_SETUP_FAILED;
        }
    };
    let Some((program, args)) = runner_argv.split_first() else {
        eprintln!("supervisor: no runner command");
        return EXIT_SETUP_FAILED;
    };
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::from(runner_end))
        .env(SUPERVISOR_FD, "0")
        .env(SUPERVISOR_BIN, &config.binary);
    if config.privileged {
        command.uid(config.runner.uid).gid(config.runner.gid);
    } else {
        eprintln!(
            "supervisor: unprivileged; runner and agents share a uid (tests and development only)"
        );
    }
    let runner = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            eprintln!("supervisor: cannot start the runner: {error}");
            return EXIT_SETUP_FAILED;
        }
    };
    let runner_pid = i32::try_from(runner.id()).unwrap_or(0);
    drop(runner);
    let mut server = Server::new(config, Some(runner_pid));
    // A SOCK_SEQPACKET socket: tokio only polls readiness, every receive is `recvmsg` with SCM_RIGHTS.
    let socket = match UnixDatagram::from_std(std::os::unix::net::UnixDatagram::from(mine)) {
        Ok(socket) => socket,
        Err(error) => {
            eprintln!("supervisor: {error}");
            return EXIT_SETUP_FAILED;
        }
    };
    let code = run_loop(
        &mut server,
        &socket,
        Some(runner_pid),
        &mut terminate,
        &mut interrupt,
        &mut child,
    )
    .await;
    match code {
        After::Exit(code) => {
            if let Some(pid) = Pid::from_raw(runner_pid) {
                let _ = kill_process(pid, Signal::KILL);
            }
            code
        }
        After::Continue => EXIT_SETUP_FAILED,
    }
}

/// Serves requests until the runner exits (then sweeps) or a sweep fails.
pub(crate) async fn run_loop(
    server: &mut Server,
    socket: &UnixDatagram,
    runner: Option<i32>,
    terminate: &mut tokio::signal::unix::Signal,
    interrupt: &mut tokio::signal::unix::Signal,
    child: &mut tokio::signal::unix::Signal,
) -> After {
    let mut tick = tokio::time::interval(Duration::from_millis(200));
    loop {
        tokio::select! {
            _ = terminate.recv() => forward(runner, Signal::TERM),
            _ = interrupt.recv() => forward(runner, Signal::INT),
            _ = child.recv() => {}
            _ = tick.tick() => {}
            ready = socket.readable() => {
                if ready.is_err() {
                    return After::Exit(EXIT_SETUP_FAILED);
                }
                loop {
                    match socket.try_io(Interest::READABLE, || protocol::receive(socket)) {
                        Ok((data, _)) if data.is_empty() => {
                            if runner.is_none() {
                                return After::Exit(0);
                            }
                            // The runner closed its end; its exit is reaped next.
                            tokio::time::sleep(Duration::from_millis(50)).await;
                            break;
                        }
                        Ok((data, fds)) => {
                            if let After::Exit(code) = server.handle(socket, &data, fds) {
                                return After::Exit(code);
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                        Err(error) => {
                            eprintln!("supervisor: control socket: {error}");
                            break;
                        }
                    }
                }
            }
        }
        if let Some(code) = server.reap(socket) {
            return After::Exit(server.final_sweep(code));
        }
    }
}

fn forward(runner: Option<i32>, signal: Signal) {
    if let Some(pid) = runner.and_then(Pid::from_raw) {
        let _ = kill_process(pid, signal);
    }
}

/// A supervisor serving `config` on a thread of this process, for tests of the runner side; returns the
/// runner's end of the control socket. Unprivileged: agents are this process's children.
#[cfg(test)]
pub(crate) fn in_process(config: ServeConfig) -> io::Result<OwnedFd> {
    let (mine, theirs) = socketpair(
        AddressFamily::UNIX,
        SocketType::SEQPACKET,
        SocketFlags::CLOEXEC,
        None,
    )?;
    rustix::fs::fcntl_setfl(&mine, OFlags::NONBLOCK)?;
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let socket = UnixDatagram::from_std(std::os::unix::net::UnixDatagram::from(mine))
                .expect("socket");
            let mut terminate = signal(SignalKind::terminate()).expect("signal");
            let mut interrupt = signal(SignalKind::interrupt()).expect("signal");
            let mut child = signal(SignalKind::child()).expect("signal");
            let mut server = Server::new(config, None);
            run_loop(
                &mut server,
                &socket,
                None,
                &mut terminate,
                &mut interrupt,
                &mut child,
            )
            .await
        })
    });
    Ok(theirs)
}
