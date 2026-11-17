//! The runner's side of the supervisor protocol. Every wait has a deadline: a helper that hangs or is stopped
//! by an agent process ends in a sweep (the supervisor exits if that fails), so nothing the agent does can
//! hang the runner.

use std::collections::HashMap;
use std::io;
use std::os::fd::{AsFd as _, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::unix::pipe;
use tokio::sync::oneshot;

use super::fsops::Entry;
use super::protocol::{self, Op, Reply, Request, SUPERVISOR_BIN, SUPERVISOR_FD};

/// Deadline of one helper run, from spawn to reaped.
pub const HELPER_DEADLINE: Duration = Duration::from_secs(30);
/// Deadline of a request the supervisor answers at once.
const REQUEST_DEADLINE: Duration = Duration::from_secs(60);
/// Most stderr kept from a helper.
const STDERR_MAX: usize = 2048;
/// Most bytes of a `list` answer.
const LIST_MAX: usize = 4 * 1024 * 1024;

/// A supervisor operation failed.
#[derive(Debug, thiserror::Error)]
pub enum SupervisorError {
    /// `SR_SUPERVISOR_FD` or `SR_SUPERVISOR_BIN` is unusable.
    #[error("{0}")]
    Setup(&'static str),
    /// The control socket or a pipe failed.
    #[error("supervisor channel: {0}")]
    Io(#[from] io::Error),
    /// The supervisor went away.
    #[error("the supervisor closed the control socket")]
    Closed,
    /// No answer in time.
    #[error("{0} missed its deadline")]
    Deadline(&'static str),
    /// The supervisor refused the request.
    #[error("the supervisor refused {op}: {message}")]
    Refused {
        /// The operation.
        op: &'static str,
        /// Its reason.
        message: String,
    },
    /// A helper failed.
    #[error("helper {mode} exited with {status}: {message}")]
    Helper {
        /// Its mode.
        mode: String,
        /// Its exit status.
        status: i32,
        /// Its stderr, clipped.
        message: String,
    },
    /// A helper's output exceeded its cap.
    #[error("helper output exceeds its cap")]
    TooLarge,
}

impl SupervisorError {
    /// Whether the agent filesystem is full (a clean job failure, not a runner fault).
    pub fn is_no_space(&self) -> bool {
        match self {
            Self::Io(error) => {
                error.raw_os_error() == Some(rustix::io::Errno::NOSPC.raw_os_error())
            }
            Self::Helper { message, .. } => message.contains("No space left"),
            _ => false,
        }
    }
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Reply>>>>;

/// A connection to the supervisor.
#[derive(Debug)]
pub struct Supervisor {
    socket: Arc<OwnedFd>,
    next: AtomicU64,
    pending: Pending,
    binary: PathBuf,
    helper_deadline: Duration,
}

/// An agent process started through the supervisor, with the runner's ends of its standard streams.
#[derive(Debug)]
pub struct AgentProcess {
    /// Its pid, also its process group.
    pub pid: i32,
    /// Its stdin.
    pub stdin: pipe::Sender,
    /// Its stdout.
    pub stdout: pipe::Receiver,
    /// Its stderr.
    pub stderr: pipe::Receiver,
}

impl Supervisor {
    /// The supervisor named by `SR_SUPERVISOR_FD` (always fd 0) and `SR_SUPERVISOR_BIN`; `None` when unset.
    pub fn from_env() -> Result<Option<Arc<Self>>, SupervisorError> {
        let Some(fd) = std::env::var_os(SUPERVISOR_FD) else {
            return Ok(None);
        };
        if fd != "0" {
            return Err(SupervisorError::Setup("SR_SUPERVISOR_FD must be 0"));
        }
        let binary = std::env::var_os(SUPERVISOR_BIN)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or(SupervisorError::Setup(
                "SR_SUPERVISOR_BIN must be an absolute path",
            ))?;
        let socket = std::io::stdin().as_fd().try_clone_to_owned()?;
        Ok(Some(Self::connect(socket, binary, HELPER_DEADLINE)?))
    }

    /// A connection over `socket` (a `SOCK_SEQPACKET` socket); helpers run `binary`.
    pub fn connect(
        socket: OwnedFd,
        binary: PathBuf,
        helper_deadline: Duration,
    ) -> Result<Arc<Self>, SupervisorError> {
        if rustix::net::sockopt::socket_type(&socket).ok()
            != Some(rustix::net::SocketType::SEQPACKET)
        {
            return Err(SupervisorError::Setup(
                "SR_SUPERVISOR_FD is not a SOCK_SEQPACKET socket",
            ));
        }
        let socket = Arc::new(socket);
        let pending: Pending = Arc::default();
        let reader = (Arc::clone(&socket), Arc::clone(&pending));
        std::thread::Builder::new()
            .name("supervisor-replies".to_owned())
            .spawn(move || read_replies(&reader.0, &reader.1))?;
        Ok(Arc::new(Self {
            socket,
            next: AtomicU64::new(1),
            pending,
            binary,
            helper_deadline,
        }))
    }

    /// Starts `argv` as the agent with exactly `env` in `cwd`, in its own session and process group.
    pub async fn spawn(
        &self,
        argv: Vec<String>,
        env: Vec<String>,
        cwd: &Path,
    ) -> Result<AgentProcess, SupervisorError> {
        self.spawn_as(argv, env, cwd, false).await
    }

    async fn spawn_as(
        &self,
        argv: Vec<String>,
        env: Vec<String>,
        cwd: &Path,
        exclusive: bool,
    ) -> Result<AgentProcess, SupervisorError> {
        let (stdin_r, stdin_w) = std::io::pipe()?;
        let (stdout_r, stdout_w) = std::io::pipe()?;
        let (stderr_r, stderr_w) = std::io::pipe()?;
        let fds = [stdin_r.into(), stdout_w.into(), stderr_w.into()];
        let reply = self
            .request(
                "spawn",
                Op::Spawn {
                    argv,
                    env,
                    cwd: cwd.to_string_lossy().into_owned(),
                    exclusive,
                },
                fds,
                REQUEST_DEADLINE,
            )
            .await?;
        let pid = reply.pid.ok_or(SupervisorError::Closed)?;
        Ok(AgentProcess {
            pid,
            stdin: pipe::Sender::from_owned_fd(OwnedFd::from(stdin_w))?,
            stdout: pipe::Receiver::from_owned_fd(OwnedFd::from(stdout_r))?,
            stderr: pipe::Receiver::from_owned_fd(OwnedFd::from(stderr_r))?,
        })
    }

    /// Waits for the spawned `pid` to exit: its exit code, or the negated signal number.
    pub async fn wait(&self, pid: i32) -> Result<i32, SupervisorError> {
        let reply = self
            .request("wait", Op::Wait { pid }, Vec::new(), Duration::MAX)
            .await?;
        reply.status.ok_or(SupervisorError::Closed)
    }

    /// Hands the empty runner-owned 0700 directory `path` to the agent.
    pub async fn handover(&self, path: &Path) -> Result<(), SupervisorError> {
        self.request(
            "handover",
            Op::Handover {
                path: path.to_string_lossy().into_owned(),
            },
            Vec::new(),
            REQUEST_DEADLINE,
        )
        .await
        .map(drop)
    }

    /// Takes the leftover per-job directory `path` away from the agent: the supervisor sets mode 0700 and makes
    /// the runner its owner, so the agent uid can no longer enter it; the result is checked here as well.
    pub async fn quarantine(&self, path: &Path) -> Result<(), SupervisorError> {
        self.request(
            "quarantine",
            Op::Quarantine {
                path: path.to_string_lossy().into_owned(),
            },
            Vec::new(),
            REQUEST_DEADLINE,
        )
        .await?;
        use std::os::unix::fs::MetadataExt as _;
        let meta = std::fs::symlink_metadata(path)?;
        if !meta.is_dir()
            || meta.uid() != rustix::process::getuid().as_raw()
            || meta.mode() & 0o7777 != 0o700
        {
            return Err(SupervisorError::Refused {
                op: "quarantine",
                message: format!("{} is not a runner-owned 0700 directory", path.display()),
            });
        }
        Ok(())
    }

    /// Kills every agent process; the supervisor exits (failing closed) if any survives.
    pub async fn sweep(&self) -> Result<(), SupervisorError> {
        self.request("sweep", Op::Sweep, Vec::new(), REQUEST_DEADLINE)
            .await
            .map(drop)
    }

    /// `sig` to the agent members of the process group of the spawned `pid`.
    pub async fn signal(&self, pid: i32, sig: i32) -> Result<(), SupervisorError> {
        self.request(
            "signal",
            Op::Signal { pid, signal: sig },
            Vec::new(),
            REQUEST_DEADLINE,
        )
        .await
        .map(drop)
    }

    /// `sig` to the agent members of the process group of the spawned `pid` without waiting for the answer (for synchronous paths).
    pub fn signal_now(&self, pid: i32, sig: i32) {
        let request = Request {
            id: self.next.fetch_add(1, Ordering::Relaxed),
            op: Op::Signal { pid, signal: sig },
        };
        if let Ok(bytes) = serde_json::to_vec(&request)
            && let Err(error) = protocol::send(&*self.socket, &bytes, &[])
        {
            tracing::warn!(%error, "cannot ask the supervisor to signal an agent process group");
        }
    }

    /// Writes `contents` to `root/rel` as the agent (replacing it, never through a link).
    pub async fn copy_in(
        &self,
        root: &Path,
        rel: &Path,
        contents: &[u8],
    ) -> Result<(), SupervisorError> {
        self.helper(&["copy-in", &path(root), &path(rel)], Some(contents), 0)
            .await
            .map(drop)
    }

    /// Reads `root/rel` as the agent: a regular file with one link of at most `max` bytes.
    pub async fn copy_out(
        &self,
        root: &Path,
        rel: &Path,
        max: usize,
    ) -> Result<Vec<u8>, SupervisorError> {
        self.helper(
            &["copy-out", &path(root), &path(rel), &max.to_string()],
            None,
            max,
        )
        .await
    }

    /// Every entry below `root`, as the agent.
    pub async fn list(&self, root: &Path) -> Result<Vec<Entry>, SupervisorError> {
        let bytes = self.helper(&["list", &path(root)], None, LIST_MAX).await?;
        serde_json::from_slice(&bytes).map_err(|error| SupervisorError::Io(io::Error::other(error)))
    }

    /// Removes everything below `root`, as the agent.
    pub async fn clear(&self, root: &Path) -> Result<(), SupervisorError> {
        self.helper(&["clear", &path(root)], None, 0)
            .await
            .map(drop)
    }

    /// Runs the program `argv` (absolute) as the agent under the helper deadline; for cleanup tools such as
    /// `ipcrm --all`.
    pub async fn run_agent_tool(&self, argv: &[&str]) -> Result<(), SupervisorError> {
        self.run(argv.iter().map(|arg| (*arg).to_owned()).collect(), None, 0)
            .await
            .map(drop)
    }

    async fn helper(
        &self,
        args: &[&str],
        input: Option<&[u8]>,
        max_out: usize,
    ) -> Result<Vec<u8>, SupervisorError> {
        let mut argv = vec![path(&self.binary)];
        argv.extend(args.iter().map(|arg| (*arg).to_owned()));
        self.run(argv, input, max_out).await
    }

    /// Runs `argv` as the agent to its exit within the helper deadline; on a miss, sweeps.
    async fn run(
        &self,
        argv: Vec<String>,
        input: Option<&[u8]>,
        max_out: usize,
    ) -> Result<Vec<u8>, SupervisorError> {
        let mode = argv.get(1).cloned().unwrap_or_default();
        // The helper's pid once spawned (0: not yet).
        let spawned = std::sync::atomic::AtomicI32::new(0);
        let run = async {
            let AgentProcess {
                pid,
                mut stdin,
                mut stdout,
                mut stderr,
            } = self
                .spawn_as(argv, vec!["LANG=C".to_owned()], Path::new("/"), true)
                .await?;
            spawned.store(pid, Ordering::Relaxed);
            let write = async move {
                if let Some(input) = input {
                    stdin.write_all(input).await?;
                }
                Ok::<_, io::Error>(())
            };
            let (written, out, err, status) = tokio::join!(
                write,
                read_capped(&mut stdout, max_out),
                read_capped(&mut stderr, STDERR_MAX),
                self.wait(pid),
            );
            let status = status?;
            let message = String::from_utf8_lossy(&err.ok().flatten().unwrap_or_default())
                .trim()
                .to_owned();
            if status != 0 {
                return Err(SupervisorError::Helper {
                    mode: mode.clone(),
                    status,
                    message,
                });
            }
            written?;
            out?.ok_or(SupervisorError::TooLarge)
        };
        match tokio::time::timeout(self.helper_deadline, run).await {
            Ok(result) => result,
            Err(_) => {
                tracing::error!(helper = %mode, "a helper missed its deadline; sweeping the agent processes");
                self.sweep().await?;
                // Collect the swept helper's status so the supervisor forgets it.
                let pid = spawned.load(Ordering::Relaxed);
                if pid != 0 {
                    let _ = tokio::time::timeout(REQUEST_DEADLINE, self.wait(pid)).await;
                }
                Err(SupervisorError::Deadline("a helper"))
            }
        }
    }

    /// Sends `op` as is (tests of the supervisor's own behaviour).
    #[cfg(test)]
    pub(crate) async fn request_for_tests(&self, op: Op) -> Result<Reply, SupervisorError> {
        self.request("test", op, Vec::new(), REQUEST_DEADLINE).await
    }

    async fn request(
        &self,
        op_name: &'static str,
        op: Op,
        fds: impl AsRef<[OwnedFd]>,
        deadline: Duration,
    ) -> Result<Reply, SupervisorError> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let bytes = serde_json::to_vec(&Request { id, op }).map_err(io::Error::other)?;
        let (sender, receiver) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, sender);
        let sent = protocol::send(&*self.socket, &bytes, fds.as_ref());
        // The supervisor holds its own copies now.
        drop(fds);
        if let Err(error) = sent {
            self.forget(id);
            return Err(error.into());
        }
        let reply = match tokio::time::timeout(deadline, receiver).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(_)) => return Err(SupervisorError::Closed),
            Err(_) => {
                self.forget(id);
                return Err(SupervisorError::Deadline(op_name));
            }
        };
        if reply.ok {
            Ok(reply)
        } else {
            Err(SupervisorError::Refused {
                op: op_name,
                message: reply.error.unwrap_or_default(),
            })
        }
    }

    fn forget(&self, id: u64) {
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
    }
}

/// Delivers replies until the supervisor closes the socket; then every waiter sees `Closed`.
fn read_replies(socket: &OwnedFd, pending: &Pending) {
    loop {
        match protocol::receive(socket) {
            Ok((data, _)) if data.is_empty() => break,
            Ok((data, _)) => match serde_json::from_slice::<Reply>(&data) {
                Ok(reply) => {
                    let waiter = pending
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .remove(&reply.id);
                    if let Some(waiter) = waiter {
                        let _ = waiter.send(reply);
                    }
                }
                Err(error) => tracing::warn!(%error, "malformed supervisor reply"),
            },
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                tracing::error!(%error, "the supervisor control socket failed");
                break;
            }
        }
    }
    pending
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
}

/// Reads `reader` to its end; `None` when it holds more than `max` bytes (the rest is drained, so the writer
/// never blocks on a full pipe).
async fn read_capped(
    reader: &mut (impl AsyncRead + Unpin),
    max: usize,
) -> io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    let limit = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
    (&mut *reader).take(limit).read_to_end(&mut bytes).await?;
    if bytes.len() > max {
        tokio::io::copy(reader, &mut tokio::io::sink()).await?;
        return Ok(None);
    }
    Ok(Some(bytes))
}

fn path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
