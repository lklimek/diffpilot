# ADR-013: Runner process model (decision R1)

- Status: accepted by evidence (WP3-OPS phase 1, 2026-11-17, revised after reviews SEC-001/SEC-002 and
  SEC-003/QA-001); R1 holds,
  the two-container fallback is not needed.
- Source: threat model R1, T1a–T1d, T4, T8b, T13; ADR-003 (per-job `CODEX_HOME` persistent); plan §5.4.
- Evidence: `deploy/runner/r1-test/run.sh` (43 named checks, all PASS); ledger
  `/data/artifacts/smart-review/2026-11-17/wp3-ops/r1-test.log`. Docker 29.9.2, kernel 7.0, default seccomp,
  AppArmor `docker-default (enforce)`, real Codex CLI 0.167.0 and `github-mcp-server` 2.1.0, fake keys only.

## Decision
One runner container. PID 1 is Docker's init (`init: true`); its child is a root **supervisor** whose only
capabilities are `CHOWN`, `SETUID`, `SETGID`, `KILL` (CapEff = CapPrm = CapBnd = `0xe1`), with
`no-new-privileges`. The supervisor forks the runner as uid `runner` (10001) and starts every agent process as uid
`agent` (10002), both with no capability in any set but the bounding set and no supplementary group. The runner
never switches uid itself; it asks the supervisor over a private socket. Everything that stops or kills agent
processes runs in the supervisor as root, so no agent can signal it. `no-new-privileges` is fully compatible: uid
switches are `setres[ug]id` calls by a process that already holds the capabilities; NNP only stops gains through
`execve` (a setuid-root binary run by `agent` keeps euid 10002).

## Supervisor protocol (the whole privileged surface)
Channel: `socketpair(AF_UNIX, SOCK_SEQPACKET | SOCK_CLOEXEC)` created before the runner is forked; the runner's end
is inherited, its number in `SR_SUPERVISOR_FD`. No filesystem path; agents inherit only fds 0–2. One JSON object
per datagram, `id` echoed in the reply; replies may arrive out of order (`wait`).

| Request | Effect |
|---|---|
| `spawn {argv, env, cwd, exclusive}` + `SCM_RIGHTS` [stdin, stdout, stderr] → `{pid}` | refused while any agent thread is alive (same test as `sweep`) when `exclusive` is set or argv starts this binary's helper mode; then fork; child: `oom_score_adj` 1000 (OOM killer takes agents first), `setsid()`, `setgroups(0)`, `setresgid(agent)`, `setresuid(agent)`, `umask 077`, `chdir(cwd)`, dup2 the three fds, close the rest, default signals and empty mask, `execve(argv[0], argv, env)` with exactly `env` (T2). `argv[0]` and `cwd` absolute. The supervisor is the parent. A failed fork (pids exhausted) is an error reply. |
| `wait {pid}` → `{status}` | answered when the supervisor reaps that spawned pid |
| `signal {pid, signal}` → `{signalled}` | `pid` must be a spawned pid (= its process group, `setsid`); the signal goes to each thread group of that process group whose live threads all have the four uids = `agent`, through a pidfd (below). Used for the graceful SIGTERM to Codex. |
| `sweep` → `{rounds, killed, ms}` | loop: for every thread group in `/proc`, read `/proc/<tgid>/task/*/status`; a group is live when **any thread** is neither zombie nor dead, whatever the leader's state (a leader that called `pthread_exit()` is a zombie while its other threads run); every live group with any of real/effective/saved/fs uid = `agent` on a live thread gets SIGKILL through a pidfd (kills the whole group); reap (a zombie leader is reaped after its last thread); until no live agent thread exists in `/proc/*/task/*`. In-process: no fork, no exec, so a full `pids_limit` cannot stop it. Hard deadline (default 5 s): past it, or on any error, the supervisor logs and exits 70 — init then ends the container and every process in it, the restart policy brings a clean one (fail closed). Never `kill(-1)` as root. |
| `handover {path}` | `path` normalised and either `<state>/codex-homes/<name>` or `<work>/<name>/{home,tmp}`; opened with `openat2(O_PATH\|O_DIRECTORY\|O_NOFOLLOW, RESOLVE_NO_SYMLINKS)`; must be owned by `runner`, mode 0700 and empty (listed by a runner-uid child, same dev/ino, because root without `DAC_READ_SEARCH` cannot list it); then `fchownat(fd, "", agent, agent, AT_EMPTY_PATH)`. The only use of `CHOWN`. |

pidfd rule (no reused pid is ever hit): `pidfd_open(tgid)` (works on a zombie leader with live threads), then read
`/proc/<tgid>/task/*/status` and check the uids of the live threads, then
`pidfd_send_signal(pidfd, sig)`. If the pinned process died and the pid was reused before the read, the check sees
the new owner and skips it; if it dies after the read, `pidfd_send_signal` fails with `ESRCH`.

The supervisor is a subreaper (`PR_SET_CHILD_SUBREAPER`) and reaps everything; when the runner exits it sweeps and
exits with the runner's status. On SIGTERM it forwards SIGTERM to the runner (not covered by the test yet).

Unprivileged **helper modes of the supervisor binary**, started by the runner through `spawn` (uid `agent`;
refused as root), because they read or write agent-owned files:

| Helper | Does |
|---|---|
| `copy-in ROOT REL` | stdin → `ROOT/REL`: `mkdirat` 0700 parents, `openat2(O_CREAT\|O_EXCL, 0600, RESOLVE_BENEATH\|NO_SYMLINKS\|NO_MAGICLINKS\|NO_XDEV)` |
| `copy-out ROOT REL MAX` | `openat2(rootfd, REL, O_RDONLY\|O_NONBLOCK, same resolve flags)`; `fstat`: regular file, `st_nlink == 1`, size ≤ MAX; read at most MAX+1 bytes; stdout |
| `list ROOT` | entries below ROOT without following symlinks (to find `sessions/YYYY/MM/DD/rollout-*-<thread>.jsonl`) |
| `clear ROOT` | removes everything below ROOT with fd-relative calls, no symlink followed; ROOT stays |

Helpers run only while no other agent thread is alive (after a `sweep`, or before Codex starts): a same-uid
process can stop them and open their pipes through `/proc/<pid>/fd`. The trusted runner orders its calls that way,
and the supervisor enforces it: it refuses a `spawn` of its own helper modes, and of any `exclusive` request (the
runner sets it for `ipcrm`), while an agent thread exists (`supervisor_refuses_helper_while_agent_alive`). The
check and the fork are not atomic, but between them only the supervisor can create an agent process.

One deadline covers reading an agent command's output **and** the `wait` reply (helpers 30 s; writes to a helper's
stdin non-blocking with the same deadline); past it the runner requests `sweep` and then collects the status, and
if the sweep fails the container ends. Proven for a SIGSTOPped command (`hung_agent_command_hits_deadline_then_swept`)
and for one that closes stdout and stderr, ignores SIGTERM and keeps running
(`closed_streams_sigterm_ignored_hits_deadline_then_swept`); the prototype bounds every other wait the same way.
The Rust runner must keep this property (one deadline per wait, then sweep) — it is not proven for it until the
checks are ported.

`pids_limit` is mandatory: it bounds a fork loop racing the sweep (measured: 509 processes holding every pid of a
512 limit, swept in 2 rounds, 45 ms).

## Order per attempt (runner, WP3-RUN)
1. Start-up, before the first claim: `sweep`; for every leftover `codex-homes/<key>`: `copy-out auth.json`
   (T13 recovery), `clear`, `rmdir`; same for leftover work dirs.
2. Prepare: `mkdir <codex-homes>/<key>` 0700 → `handover`; `mkdir <work>/<key>` 0755, checkout into `repo/`
   as `runner` (umask 022: agent reads, cannot write); `mkdir home tmp` 0700 → `handover` each; `copy-in`
   `auth.json`, `config.toml`, schema, this analysis's session files (stdin = runner-opened fds).
3. `spawn` Codex (cwd `<work>/<key>`, constructed env); the runner keeps the stdout/stderr read ends.
4. End, cancel, runtime cap, self-fence, and before the compaction turn (T1d): `signal {codex pid, SIGTERM}` →
   grace → `sweep` → `wait`.
5. Copy-back (after the sweep, so nothing races): `list`, then `copy-out` of `auth.json` and the rollout whose
   name matches this analysis's thread id; then the runner validates (JSON, `auth_mode`, account id, CAS, T13;
   path pattern, size, T8b). Then `clear` the three per-job dirs and `/dev/mqueue`, `spawn /usr/bin/ipcrm --all`,
   and `rmdir` the empty agent-owned dirs (allowed: the parent is the runner's).

Copy-in/out run as `agent` because the runner cannot read the agent's 0700 home (check
`runner_cannot_read_agent_home_directly`) and because a planted link then reaches only what `agent` could read
anyway (`copy_back_as_agent_cannot_reach_runner_files`) — no confused deputy.

## Filesystem layout
| Path | Filesystem | Owner | Mode | Agent |
|---|---|---|---|---|
| `/var/lib/smart-review-runner` (`SR_RUNNER_STATE_DIR`) | volume `runner-state` | runner | 0711 | traverse only |
| `…/credentials/`, `…/credentials/<id>/auth.json` | same | runner | 0700 / 0600 | no |
| `…/sessions/`, `…/sessions/<analysis>/` | same | runner | 0700 | no |
| `…/mirrors/` (and any claudius checkout) | same | runner | 0700 | no |
| `…/codex-homes/` | **bounded persistent filesystem** (below), mounted here | runner | 0711 | traverse only |
| `…/codex-homes/<job>-<attempt>/` = `CODEX_HOME` | same | agent (handover) | 0700 | read/write |
| `/work` (`SR_RUNNER_WORK_DIR`) | tmpfs, `size=`, `uid=10001,mode=0711` | runner | 0711 | traverse only |
| `/work/<job>-<attempt>/` = Codex cwd; `repo/` | same | runner | 0755; files 0644 | read only |
| `/work/<job>-<attempt>/{home,tmp}` = `HOME`, `TMPDIR` | same | agent (handover) | 0700 | read/write |
| `/tmp` | tmpfs, `uid=10001,mode=0700` | runner | 0700 | no |
| `/dev/mqueue` | IPC namespace view (1777) | — | — | writable; emptied at job end |

**`codex-homes` must be persistent and bounded.** A tmpfs is not viable: Codex may rotate the single-use refresh
token during a job, and until copy-back the only valid token is in the per-job `auth.json`; a container crash
would then lose the credential (ADR-003, T1c, T13 start-up recovery). A plain named volume is not viable either:
the agent could fill the host disk (no capacity or inode limit). Decision: a dedicated filesystem of fixed size
and inode count, separate from the session store (T8b): an ext4 image file created by the owner (e.g. 2 GiB,
`mkfs.ext4 -N 65536`), loop-mounted on the host (`/etc/fstab`: `loop,nosuid,nodev,noexec`) and bind-mounted at
`…/codex-homes`. Docker's `local` volume driver cannot loop-mount an image (tried: `loop: invalid argument`), so
this is host preparation: **WP3-OPS phase 2** (runbook, compose bind mount, start-up check that the mount is the
bounded filesystem). The test uses a `size=16m,nr_inodes=2048` tmpfs as a stand-in for the bound
(`codex_home_exhaustion_fails_job_cleanly`: the agent hits `ENOSPC` for bytes and inodes, the runner still writes
its state, copies `auth.json` back and cleans up).

Skills exposed to the agent must not live under a 0700 runner dir: copy them in per job or keep them in a
runner-owned 0755/0644 tree. Supervisor start-up `lchown`s the three mount roots to `runner` when a fresh mount
is root-owned; the runner sets the modes.

## Container options (compose, WP3-OPS phase 2)
`init: true`; `restart: unless-stopped` (a fail-closed exit brings a clean container); no `user:` (the image's
entrypoint is the supervisor as root); `cap_drop: [ALL]`; `cap_add: [CHOWN, SETUID, SETGID, KILL]`;
`security_opt: [no-new-privileges:true]` and nothing `unconfined`; `read_only: true`; `ipc: none` (no `/dev/shm`);
`pids_limit` (mandatory); `mem_limit`; the two tmpfs, the state volume and the bounded `codex-homes` mount above;
no Docker socket; network per T4 (phase 2). Image: `deploy/runner/Dockerfile` (uids 10001/10002, no shared group,
setuid/setgid bits stripped).

## Rust implementation notes (WP3-RUN)
- Supervisor: a second binary in `crates/runner` (`smart-review-supervisor`), single-threaded, no tokio, `rustix`
  (`net` feature for `socketpair`/`sendmsg`/`recvmsg` with `ScmRights` and `RecvFlags::CMSG_CLOEXEC`;
  `process` for `pidfd_open`/`pidfd_send_signal`). `spawn` may use `std::process::Command` with
  `.uid(10002).gid(10002)` (std calls `setgroups(0)` then `setuid` when a uid is set), `.env_clear().envs(..)`,
  `Stdio::from(OwnedFd)` and `pre_exec(setsid)`; reap with `waitpid(-1, WNOHANG)`, not `Child::wait`. `sweep` and
  `signal` run in the supervisor process and decide liveness per thread (`/proc/<tgid>/task/*`), never by the
  leader's state; helpers are subcommands of the same binary, and `spawn` refuses them (and `exclusive` requests)
  while an agent thread lives.
- Runner: `kill_on_drop` and `kill_process_group` on Codex no longer work (different uid); every drop/cancel path
  must go through `signal` + `sweep`, and the supervisor's final sweep covers a runner crash. Reading an agent
  command's output and receiving its `wait` reply share one deadline, then `sweep`; the socket `recv` is bounded
  too. `RemoveOnDrop` cannot delete agent-owned
  contents: use `clear` + `rmdir`. Job dir mode 0755 (today 0700).
- Port the checks of `deploy/runner/r1-test/` to the Rust supervisor: same scenarios, same check names.

## Alternatives rejected
- Runner holding `SETUID`/`SETGID`/`KILL` (permitted or ambient): violates CapEff 0 for `runner` (T1b).
- `sudo`/`su`/setuid helpers: blocked by NNP (`no_privilege_escalation`); NNP is required (T4).
- User namespaces, bubblewrap: `unshare(CLONE_NEWUSER)` is denied for `runner` and `agent` under the default
  seccomp profile and the host's AppArmor userns restriction (`runner_cannot_switch_uid_itself`).
- Process-group kill only (stage 2): misses `setsid` escapees, including the grandchild of a Codex MCP server
  (`process_group_kill_misses_escapees`, `real_codex_escapee_survives_group_kill`).
- **Sweep as uid `agent`** (`kill(-1, SIGKILL)` from a helper, the first version of this ADR): a hostile process of
  the same uid can `kill(-1, SIGSTOP)` the helper between its uid switch and its first kill, and a full
  `pids_limit` stops the helper from being forked at all; the mechanism that kills the agent must not be signalable
  by the agent. Hence `CAP_KILL` and the in-process root sweep (`hostile_agent_swept_within_deadline`,
  `pid_exhaustion_sweep_still_works`).
- Liveness from the thread-group leader's state (first root-sweep version): a zombie leader with running threads
  was skipped and the sweep reported success (`zombie_leader_thread_group_swept`, RED before the fix).
- `kill(-1)` as root: would hit the runner and the supervisor's other children; the root sweep selects by uid.
- Two containers: unnecessary, every property holds in one.
- Prior art: init systems (tini, s6, runit `chpst`) switch uid only at start; `systemd-run` needs systemd/D-Bus;
  `bwrap` needs user namespaces. Hence a five-request supervisor.

## Residuals
- The bounding set of `runner` and `agent` still contains the four capabilities (dropping it needs
  `CAP_SETPCAP`); harmless under NNP with no setuid or file-capability binary (`cap_kill_not_inherited`).
- `/proc/*/cmdline` and `/proc/*/status` of every process are world-readable: never put a secret in argv.
- POSIX message queues and SysV IPC survive the sweep; step 5 clears them (`ipc_objects_cleared_at_job_end`).
- The agent may delete its files or make them unreadable (`chmod 000`); copy-back then fails closed (T13 marks
  the credential unusable): a denial of service for an attacker who already holds the credential (AR-4).
- A crashed run's per-job dirs stay agent-owned until start-up recovery (order step 1), which must finish before
  the first job: a later agent could traverse into a guessed `<job>-<attempt>` name.
- **Accepted denial of service:** a hostile job can make a sweep fail (for example a process the kernel cannot
  kill within the deadline) and so force the fail-closed exit 70 and a container restart, once per attempt; the
  job's attempt limit (`max_attempts` = 3, `runner-protocol.md`) bounds the repetitions per job, other jobs of the
  queue are delayed meanwhile.
- Until phase 2 lands the bounded `codex-homes` filesystem, a plain volume there lets the agent fill the host
  disk; v1 must not deploy without it.
