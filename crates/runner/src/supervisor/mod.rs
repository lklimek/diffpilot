//! Process isolation (ADR-013, threat model T1a/b/d, R1): the root supervisor binary
//! (`diffpilot-supervisor`, `serve` plus helper modes) and the runner's client of it. Without
//! `SR_SUPERVISOR_FD` the runner runs agents as its own uid (development; logged).

pub mod client;
pub mod fsops;
pub mod procs;
pub mod protocol;
pub mod serve;

use std::ffi::OsString;
use std::io::Write as _;
use std::path::Path;

/// Runs a helper mode (`copy-in`, `copy-out`, `list`, `clear`, `inspect`) and returns the exit code. Helpers
/// never run as root.
pub fn helper(args: &[OsString]) -> i32 {
    if args.first().is_some_and(|mode| mode == serve::EXEC_AGENT) {
        return exec_agent(args.get(1..).unwrap_or_default());
    }
    if rustix::process::geteuid().is_root() {
        eprintln!("helpers never run as root");
        return 2;
    }
    let arg = |index: usize| args.get(index).map(Path::new);
    let result = match (args.first().and_then(|mode| mode.to_str()), arg(1), arg(2)) {
        (Some("copy-in"), Some(root), Some(rel)) => {
            fsops::copy_in(root, rel, &mut std::io::stdin().lock())
        }
        (Some("copy-out"), Some(root), Some(rel)) => {
            let max = args
                .get(3)
                .and_then(|max| max.to_str()?.parse::<u64>().ok())
                .unwrap_or(0);
            match fsops::copy_out(root, rel, max) {
                Ok(bytes) => std::io::stdout().lock().write_all(&bytes),
                Err(error) => {
                    eprintln!("{error}");
                    return error.exit_code();
                }
            }
        }
        (Some("list"), Some(root), None) => fsops::list(root).and_then(|entries| {
            serde_json::to_writer(std::io::stdout().lock(), &entries).map_err(std::io::Error::other)
        }),
        (Some("clear"), Some(root), None) => fsops::clear(root),
        (Some("lock"), Some(path), None) => fsops::lock_dir(path),
        (Some("inspect"), Some(path), None) => {
            fsops::inspect(path).and_then(|line| writeln!(std::io::stdout().lock(), "{line}"))
        }
        _ => {
            eprintln!(
                "usage: diffpilot-supervisor serve -- <runner argv> | copy-in|copy-out|list|clear|inspect|lock ..."
            );
            return 2;
        }
    };
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

/// Becomes the agent process `argv` (environment and cwd as the supervisor set them): `oom_score_adj` 1000 so
/// the OOM killer takes agents first, its own session and process group, then `execve`.
fn exec_agent(args: &[OsString]) -> i32 {
    use std::os::unix::process::CommandExt as _;
    let (Some(cwd), Some(identity), Some(argv)) = (
        args.first(),
        args.get(1).and_then(|identity| identity.to_str()),
        args.get(3..),
    ) else {
        eprintln!("exec-agent: usage: exec-agent CWD UID:GID|- -- ARGV");
        return 2;
    };
    let Some((program, rest)) = argv.split_first() else {
        eprintln!("exec-agent: no program");
        return 2;
    };
    let switch = match identity {
        "-" => None,
        pair => match pair
            .split_once(':')
            .and_then(|(uid, gid)| Some((uid.parse::<u32>().ok()?, gid.parse::<u32>().ok()?)))
        {
            Some((uid, gid)) if uid != 0 && gid != 0 => Some((uid, gid)),
            _ => {
                eprintln!("exec-agent: bad UID:GID");
                return 2;
            }
        },
    };
    if switch.is_none() && rustix::process::geteuid().is_root() {
        eprintln!("exec-agent: refusing to run an agent as root");
        return 2;
    }
    if let Err(error) = std::fs::write("/proc/self/oom_score_adj", "1000") {
        eprintln!("exec-agent: oom_score_adj: {error}");
        return 126;
    }
    if let Err(error) = rustix::process::setsid() {
        eprintln!("exec-agent: setsid: {error}");
        return 126;
    }
    let mut command = std::process::Command::new(program);
    // std switches groups, gid and uid, then changes into `cwd` (as the agent: the directory is the agent's).
    command.args(rest).current_dir(cwd);
    if let Some((uid, gid)) = switch {
        command.uid(uid).gid(gid);
    }
    let error = command.exec();
    eprintln!("exec-agent: {error}");
    127
}
