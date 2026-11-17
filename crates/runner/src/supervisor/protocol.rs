//! The supervisor protocol (ADR-013): one JSON object per `SOCK_SEQPACKET` datagram over the socket the runner
//! inherits as fd 0 (`SR_SUPERVISOR_FD=0`); `spawn` carries the agent's stdin, stdout and stderr as
//! `SCM_RIGHTS`. Replies echo the request id and may arrive out of order (`wait`).

use std::io::{IoSlice, IoSliceMut};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, OwnedFd};

use rustix::net::{
    RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, SendAncillaryBuffer,
    SendAncillaryMessage, SendFlags, recvmsg, sendmsg,
};
use serde::{Deserialize, Serialize};

/// Largest datagram accepted.
pub const MAX_DATAGRAM: usize = 1 << 20;
/// Environment variable naming the runner's control socket fd.
pub const SUPERVISOR_FD: &str = "SR_SUPERVISOR_FD";
/// Environment variable naming the supervisor binary (for helper modes).
pub const SUPERVISOR_BIN: &str = "SR_SUPERVISOR_BIN";

/// A request from the runner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// Echoed in the reply.
    pub id: u64,
    /// The operation.
    #[serde(flatten)]
    pub op: Op,
}

/// The operations: the whole privileged surface.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// Start `argv` as the agent uid with exactly `env` in `cwd`, in its own process group; 3 fds attached.
    Spawn {
        /// Program (absolute) and arguments; never a secret.
        argv: Vec<String>,
        /// `KEY=VALUE` entries.
        env: Vec<String>,
        /// Absolute working directory.
        cwd: String,
        /// Refused while any agent thread is alive (helpers, `ipcrm`): a live agent process could stop it or
        /// open its pipes through `/proc/<pid>/fd`. Spawns of the supervisor binary are always exclusive.
        #[serde(default)]
        exclusive: bool,
    },
    /// Answered once the spawned `pid` was reaped.
    Wait {
        /// A pid returned by `spawn`.
        pid: i32,
    },
    /// Give an empty runner-owned 0700 per-job directory to the agent uid.
    Handover {
        /// `<state>/codex-homes/<key>` or `<work>/<key>/{home,tmp}`.
        path: String,
    },
    /// SIGKILL every agent-uid process; the supervisor exits non-zero if any survives the deadline.
    Sweep,
    /// A signal to the agent members of a spawned process's group (each through a pidfd).
    Signal {
        /// A pid `spawn` returned (its own session and process group).
        pid: i32,
        /// SIGINT (2), SIGKILL (9) or SIGTERM (15).
        signal: i32,
    },
}

/// The reply to one request.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    /// The request id.
    pub id: u64,
    /// Whether it succeeded.
    pub ok: bool,
    /// `spawn`: the new pid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
    /// `wait`: exit code, or 128 + signal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<i32>,
    /// `sweep`: rounds needed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rounds: Option<u32>,
    /// `sweep`: SIGKILLs sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub killed: Option<u32>,
    /// `sweep`: milliseconds taken.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms: Option<u64>,
    /// `signal`: processes signalled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signalled: Option<u32>,
    /// Why it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Sends `bytes` with `fds` attached.
pub fn send(socket: impl AsFd, bytes: &[u8], fds: &[OwnedFd]) -> std::io::Result<()> {
    let mut space = [MaybeUninit::<u8>::uninit(); rustix::cmsg_space!(ScmRights(3))];
    let mut ancillary = SendAncillaryBuffer::new(&mut space);
    let borrowed: Vec<_> = fds.iter().map(AsFd::as_fd).collect();
    if !borrowed.is_empty() && !ancillary.push(SendAncillaryMessage::ScmRights(&borrowed)) {
        return Err(std::io::Error::other("too many fds"));
    }
    let sent = sendmsg(
        socket,
        &[IoSlice::new(bytes)],
        &mut ancillary,
        SendFlags::empty(),
    )?;
    if sent != bytes.len() {
        return Err(std::io::Error::other("datagram truncated"));
    }
    Ok(())
}

/// Receives one datagram and the fds attached to it (close-on-exec); empty bytes mean the peer closed.
pub fn receive(socket: impl AsFd) -> std::io::Result<(Vec<u8>, Vec<OwnedFd>)> {
    let mut space = [MaybeUninit::<u8>::uninit(); rustix::cmsg_space!(ScmRights(3))];
    let mut ancillary = RecvAncillaryBuffer::new(&mut space);
    let mut data = vec![0_u8; MAX_DATAGRAM];
    let received = recvmsg(
        socket,
        &mut [IoSliceMut::new(&mut data)],
        &mut ancillary,
        RecvFlags::CMSG_CLOEXEC,
    )?;
    let mut fds = Vec::new();
    for message in ancillary.drain() {
        if let RecvAncillaryMessage::ScmRights(rights) = message {
            fds.extend(rights);
        }
    }
    data.truncate(received.bytes);
    Ok((data, fds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_have_the_adr_shape() {
        let spawn = Request {
            id: 3,
            op: Op::Spawn {
                argv: vec!["/usr/bin/codex".to_owned()],
                env: vec!["HOME=/work/k/home".to_owned()],
                cwd: "/work/k".to_owned(),
                exclusive: false,
            },
        };
        assert_eq!(
            serde_json::to_value(&spawn).unwrap(),
            serde_json::json!({"id": 3, "op": "spawn", "argv": ["/usr/bin/codex"], "env": ["HOME=/work/k/home"], "cwd": "/work/k", "exclusive": false})
        );
        let signal: Request =
            serde_json::from_str(r#"{"id":1,"op":"signal","pid":9,"signal":15}"#).unwrap();
        assert_eq!(signal.op, Op::Signal { pid: 9, signal: 15 });
    }
}
