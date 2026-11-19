//! Processes of the agent uid, read from `/proc` (ADR-013 `sweep` and `signal`, run by the supervisor with
//! `CAP_KILL`): SIGKILL to every thread group with a live agent thread until no agent thread is left anywhere
//! (`/proc/*/task/*`) or the deadline passes, and a signal to the agent members of a spawned process group.
//! Liveness is decided per thread group, not by the leader: a leader that called `pthread_exit()` is a zombie
//! while its other threads run. Signals go through a pidfd opened before the target is checked again, so a
//! reused pid is never hit. Never `kill(-1)` as root.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};

/// Pause between two sweep rounds.
const ROUND_PAUSE: Duration = Duration::from_millis(20);

/// One thread group as `/proc` shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Process {
    pub(crate) pid: i32,
    /// Real, effective, saved and filesystem uid of each live (not zombie or dead) thread.
    pub(crate) uids: Vec<[u32; 4]>,
    pub(crate) ppid: i32,
    pub(crate) pgid: i32,
    /// No live thread: nothing left to kill.
    pub(crate) gone: bool,
}

/// Which processes are agent processes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    /// The agent uid.
    pub uid: u32,
    /// Unprivileged (tests, development): runner and agent share the uid, so only descendants of `root`
    /// outside the subtree of `exclude` count.
    pub scope: Option<Scope>,
}

/// The process subtree an unprivileged supervisor may touch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scope {
    /// The supervisor.
    pub root: i32,
    /// The runner (its subtree is never an agent's).
    pub exclude: i32,
}

impl Target {
    /// Any of the four uids is the agent's (sweep), or all four are (signal).
    fn uids_match(&self, process: &Process, strict: bool) -> bool {
        if strict {
            !process.uids.is_empty()
                && process
                    .uids
                    .iter()
                    .all(|task| task.iter().all(|uid| *uid == self.uid))
        } else {
            process.uids.iter().any(|task| task.contains(&self.uid))
        }
    }

    /// Whether `process` is an agent process; `parents` maps every pid to its parent.
    fn matches(&self, process: &Process, parents: &HashMap<i32, i32>, strict: bool) -> bool {
        if !self.uids_match(process, strict) {
            return false;
        }
        let Some(scope) = self.scope else {
            return true;
        };
        let mut pid = process.pid;
        for _ in 0..parents.len() + 1 {
            if pid == scope.exclude || pid == scope.root && pid == process.pid {
                return false;
            }
            if pid == scope.root {
                return true;
            }
            match parents.get(&pid) {
                Some(parent) if *parent > 0 => pid = *parent,
                _ => return false,
            }
        }
        false
    }

    /// The live agent processes among `all`.
    fn select(&self, all: &[Process], strict: bool) -> Vec<Process> {
        let parents: HashMap<i32, i32> = all.iter().map(|p| (p.pid, p.ppid)).collect();
        all.iter()
            .filter(|process| !process.gone && self.matches(process, &parents, strict))
            .cloned()
            .collect()
    }
}

/// A sweep ended with processes still alive.
#[derive(Debug, thiserror::Error)]
#[error("{} agent processes survived the sweep", survivors.len())]
pub struct SweepFailed {
    /// Their pids.
    pub survivors: Vec<i32>,
}

/// A signal request was refused.
#[derive(Debug, thiserror::Error)]
pub enum SignalRefused {
    /// The group is empty or has a member of another uid.
    #[error("process group {0} is not an agent process group")]
    NotAgentGroup(i32),
    /// Only SIGTERM, SIGINT and SIGKILL may be sent.
    #[error("signal {0} is not allowed")]
    Signal(i32),
    /// The kernel refused.
    #[error("the signal could not be sent")]
    Kill(#[source] rustix::io::Errno),
}

/// Every process in `/proc` (vanished ones skipped).
pub(crate) fn processes() -> Vec<Process> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse::<i32>().ok())
        .filter_map(read_process)
        .collect()
}

fn read_process(pid: i32) -> Option<Process> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let mut process = parse(pid, &status, &stat)?;
    // Every thread counts, the leader's state alone does not.
    if let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) {
        let live: Vec<[u32; 4]> = tasks
            .filter_map(Result::ok)
            .filter_map(|task| std::fs::read_to_string(task.path().join("status")).ok())
            .filter_map(|status| task_state(&status))
            .filter(|(state, _)| !matches!(state, 'Z' | 'X'))
            .map(|(_, uids)| uids)
            .collect();
        process.gone = live.is_empty();
        process.uids = live;
    }
    Some(process)
}

/// The state letter and the four uids of a task's `status` text.
fn task_state(status: &str) -> Option<(char, [u32; 4])> {
    let field = |name: &str| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))
            .map(str::trim)
    };
    let mut uids = field("Uid")?
        .split_whitespace()
        .map(|uid| uid.parse::<u32>());
    let uids = [
        uids.next()?.ok()?,
        uids.next()?.ok()?,
        uids.next()?.ok()?,
        uids.next()?.ok()?,
    ];
    Some((field("State")?.chars().next()?, uids))
}

/// A thread group from its leader's `status` and `stat` texts (the leader's thread only).
pub(crate) fn parse(pid: i32, status: &str, stat: &str) -> Option<Process> {
    let (state, uids) = task_state(status)?;
    // `stat`: "pid (comm) state ppid pgrp ..."; comm may hold spaces and parentheses.
    let mut rest = stat.rsplit_once(')')?.1.split_whitespace();
    let ppid = rest.nth(1)?.parse().ok()?;
    let pgid = rest.next()?.parse().ok()?;
    let gone = matches!(state, 'Z' | 'X');
    Some(Process {
        pid,
        uids: if gone { Vec::new() } else { vec![uids] },
        ppid,
        pgid,
        gone,
    })
}

/// Whether any agent thread is alive.
pub fn agent_alive(target: Target) -> bool {
    !target.select(&processes(), false).is_empty()
}

/// What a sweep did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Swept {
    /// Rounds needed.
    pub rounds: u32,
    /// SIGKILLs sent.
    pub killed: u32,
}

/// SIGKILL to every live agent process (any uid matching) until none is left; `between` runs after every
/// round (the supervisor reaps there, keeping every status). Fails at `deadline` with the survivors.
pub fn sweep(
    target: Target,
    deadline: Duration,
    between: impl FnMut(),
) -> Result<Swept, SweepFailed> {
    sweep_with(
        || {
            target
                .select(&processes(), false)
                .into_iter()
                .map(|process| process.pid)
                .collect()
        },
        |pid| kill_if(pid, target, false, Signal::KILL),
        between,
        deadline,
    )
}

/// The sweep loop over `live` (the pids still alive) and `kill` (whether a signal was sent).
pub(crate) fn sweep_with(
    live: impl Fn() -> Vec<i32>,
    kill: impl Fn(i32) -> bool,
    mut between: impl FnMut(),
    deadline: Duration,
) -> Result<Swept, SweepFailed> {
    let started = Instant::now();
    let mut swept = Swept::default();
    loop {
        let alive = live();
        if alive.is_empty() {
            return Ok(swept);
        }
        if started.elapsed() >= deadline {
            return Err(SweepFailed { survivors: alive });
        }
        swept.rounds += 1;
        for pid in alive {
            if kill(pid) {
                swept.killed += 1;
            }
        }
        std::thread::sleep(ROUND_PAUSE);
        between();
    }
}

/// `sig` to each live member of the process group `pgid` whose four uids all are the agent's; returns how many.
pub fn signal_group(pgid: i32, sig: i32, target: Target) -> Result<u32, SignalRefused> {
    let signal = match sig {
        2 => Signal::INT,
        9 => Signal::KILL,
        15 => Signal::TERM,
        other => return Err(SignalRefused::Signal(other)),
    };
    let members: Vec<i32> = target
        .select(&processes(), true)
        .iter()
        .filter(|process| process.pgid == pgid)
        .map(|process| process.pid)
        .collect();
    if members.is_empty() {
        return Err(SignalRefused::NotAgentGroup(pgid));
    }
    Ok(members
        .into_iter()
        .map(|pid| u32::from(kill_if(pid, target, true, signal)))
        .sum())
}

/// `signal` to `pid` through a pidfd opened before the target is checked again (ADR-013 pidfd rule).
fn kill_if(pid: i32, target: Target, strict: bool, signal: Signal) -> bool {
    let Some(raw) = Pid::from_raw(pid) else {
        return false;
    };
    let Ok(pidfd) = pidfd_open(raw, PidfdFlags::empty()) else {
        return false;
    };
    let still = match target.scope {
        None => read_process(pid).is_some_and(|process| target.uids_match(&process, strict)),
        Some(_) => target
            .select(&processes(), strict)
            .iter()
            .any(|process| process.pid == pid),
    };
    still && pidfd_send_signal(&pidfd, signal).is_ok()
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::os::unix::process::CommandExt as _;

    use super::*;

    const STATUS: &str = "Name:\tsleep\nState:\tS (sleeping)\nUid:\t10002\t10002\t10002\t10001\n";

    #[test]
    fn parses_uids_state_and_group() {
        let process = parse(42, STATUS, "42 (sl) eep)) S 1 77 77 0").unwrap();
        assert_eq!(process.uids, [[10002, 10002, 10002, 10001]]);
        assert_eq!(process.ppid, 1);
        assert_eq!(process.pgid, 77);
        assert!(!process.gone);
        let zombie = STATUS.replace("S (sleeping)", "Z (zombie)");
        assert!(parse(42, &zombie, "42 (x) Z 1 77 77").unwrap().gone);
    }

    fn process(pid: i32, uid: u32, ppid: i32) -> Process {
        Process {
            pid,
            uids: vec![[uid; 4]],
            ppid,
            pgid: pid,
            gone: false,
        }
    }

    #[test]
    fn agent_processes_are_selected_by_uid_and_scope() {
        let all = vec![
            process(1, 0, 0),
            process(10, 10001, 1),
            process(11, 10002, 1),
            process(12, 10002, 11),
            process(13, 10001, 10),
        ];
        let by_uid = Target {
            uid: 10002,
            scope: None,
        };
        let pids: Vec<i32> = by_uid.select(&all, false).iter().map(|p| p.pid).collect();
        assert_eq!(pids, [11, 12]);
        let scoped = Target {
            uid: 10001,
            scope: Some(Scope {
                root: 1,
                exclude: 10,
            }),
        };
        assert!(
            scoped.select(&all, false).is_empty(),
            "the runner subtree is never swept"
        );
        let scoped = Target {
            uid: 10002,
            scope: Some(Scope {
                root: 11,
                exclude: 10,
            }),
        };
        let pids: Vec<i32> = scoped.select(&all, false).iter().map(|p| p.pid).collect();
        assert_eq!(pids, [12], "the supervisor itself is never swept");
    }

    #[test]
    fn sweep_kills_until_none_is_left() {
        let alive = Cell::new(2);
        let between = Cell::new(0);
        let swept = sweep_with(
            || (0..alive.get()).collect(),
            |_| {
                alive.set(alive.get().saturating_sub(1));
                true
            },
            || between.set(between.get() + 1),
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(
            swept,
            Swept {
                rounds: 1,
                killed: 2
            }
        );
        assert_eq!(between.get(), 1, "the supervisor reaps between rounds");
    }

    #[test]
    fn sweep_deadline_reports_the_survivors() {
        let failed =
            sweep_with(|| vec![7], |_| false, || {}, Duration::from_millis(50)).unwrap_err();
        assert_eq!(failed.survivors, [7]);
    }

    #[test]
    fn signal_reaches_only_agent_members() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let pgid = i32::try_from(child.id()).unwrap();
        let uid = rustix::process::getuid().as_raw();
        let me = rustix::process::getpid().as_raw_nonzero().get();
        let agent = Target {
            uid,
            scope: Some(Scope {
                root: me,
                exclude: 0,
            }),
        };
        let other_uid = Target {
            uid: uid.wrapping_add(1),
            scope: None,
        };
        assert!(matches!(
            signal_group(pgid, 15, other_uid),
            Err(SignalRefused::NotAgentGroup(_))
        ));
        assert!(matches!(
            signal_group(pgid, 19, agent),
            Err(SignalRefused::Signal(19))
        ));
        assert!(matches!(
            signal_group(1, 15, agent),
            Err(SignalRefused::NotAgentGroup(1))
        ));
        assert_eq!(signal_group(pgid, 15, agent).expect("an agent group"), 1);
        assert!(!child.wait().unwrap().success());
    }

    /// A process whose leader called `pthread_exit()` while another thread runs (built with `cc`).
    fn zombie_leader_process(dir: &std::path::Path) -> std::process::Child {
        let source = dir.join("zl.c");
        std::fs::write(
            &source,
            "#include <pthread.h>\n#include <unistd.h>\n\
             static void *spin(void *a) { (void)a; for (;;) pause(); return 0; }\n\
             int main(void) { pthread_t t; pthread_create(&t, 0, spin, 0); pthread_exit(0); }\n",
        )
        .unwrap();
        let binary = dir.join("zl");
        let built = std::process::Command::new("cc")
            .arg("-pthread")
            .arg("-o")
            .arg(&binary)
            .arg(&source)
            .status()
            .expect("cc is needed for this test");
        assert!(built.success());
        std::process::Command::new(&binary).spawn().unwrap()
    }

    #[test]
    fn zombie_leader_thread_group_swept() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut child = zombie_leader_process(dir.path());
        let pid = i32::try_from(child.id()).unwrap();
        let started = Instant::now();
        while !std::fs::read_to_string(format!("/proc/{pid}/status"))
            .is_ok_and(|status| status.contains("State:\tZ"))
        {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "the leader never exited"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let me = rustix::process::getpid().as_raw_nonzero().get();
        let target = Target {
            uid: rustix::process::getuid().as_raw(),
            scope: Some(Scope {
                root: me,
                exclude: 0,
            }),
        };
        let found = read_process(pid).unwrap();
        assert!(!found.gone, "a live thread keeps the group alive");
        assert!(target.select(&[found], false).len() == 1);
        let swept = sweep(target, Duration::from_secs(10), || {}).expect("swept");
        assert!(swept.killed >= 1);
        assert!(!child.wait().unwrap().success());
    }
}
