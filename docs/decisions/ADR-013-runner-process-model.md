# ADR-013: Runner process model (decision R1)

- Status: accepted by evidence (WP3-OPS phase 1, 2026-11-17); R1 holds, the two-container fallback is not needed.
- Source: threat model R1, T1a–T1d, T4, T8b, T13; plan §5.4.
- Evidence: `deploy/runner/r1-test/run.sh` (34 named checks, all PASS); ledger
  `/data/artifacts/smart-review/2026-11-17/wp3-ops/r1-test.log`. Docker 29.9.2, kernel 7.0, default seccomp,
  AppArmor `docker-default (enforce)`, real Codex CLI 0.167.0 and `github-mcp-server` 2.1.0, fake keys only.

## Decision
One runner container. PID 1 is Docker's init (`init: true`); its child is a root **supervisor** whose only
capabilities are `CHOWN`, `SETUID`, `SETGID` (CapEff = CapPrm = CapBnd = `0xc1`), with `no-new-privileges`. The
supervisor forks the runner as uid `runner` (10001) and starts every agent process as uid `agent` (10002), both
with no capability and no supplementary group. The runner, not root, never switches uid itself; it asks the
supervisor over a private socket. `no-new-privileges` is fully compatible: uid switches are `setres[ug]id` calls by
a process that already holds the capabilities; NNP only stops gains through `execve` (a setuid-root binary run by
`agent` keeps euid 10002).

## Supervisor protocol (the whole privileged surface)
Channel: `socketpair(AF_UNIX, SOCK_SEQPACKET | SOCK_CLOEXEC)` created before the runner is forked; the runner's end
is inherited, its number in `SR_SUPERVISOR_FD`. No filesystem path; agents inherit only fds 0–2. One JSON object
per datagram, `id` echoed in the reply; replies may arrive out of order (`wait`).

| Request | Effect |
|---|---|
| `spawn {argv, env, cwd}` + `SCM_RIGHTS` [stdin, stdout, stderr] → `{pid}` | fork; child: `setgroups(0)`, `setresgid(agent)`, `setresuid(agent)`, `setsid()`, `umask 077`, `chdir(cwd)`, dup2 the three fds, close the rest, default signals and empty mask, `execve(argv[0], argv, env)` with exactly `env` (T2). `argv[0]` and `cwd` absolute. The supervisor is the parent. |
| `wait {pid}` → `{status}` | answered when the supervisor reaps that spawned pid |
| `handover {path}` | `path` normalised and either `<state>/codex-homes/<name>` or `<work>/<name>/{home,tmp}`; opened with `openat2(O_PATH\|O_DIRECTORY\|O_NOFOLLOW, RESOLVE_NO_SYMLINKS)`; must be owned by `runner`, mode 0700 and empty (listed by a runner-uid child, same dev/ino, because root without `DAC_READ_SEARCH` cannot list it); then `fchownat(fd, "", agent, agent, AT_EMPTY_PATH)`. The only use of `CHOWN`. |

The supervisor is a subreaper (`PR_SET_CHILD_SUBREAPER`) and reaps everything; when the runner exits it runs the
sweep and exits with the runner's status. On SIGTERM it forwards SIGTERM to the runner (sent as uid `runner`; not covered by the test yet).

Everything else is an **unprivileged helper mode of the supervisor binary** that the runner starts through `spawn`
(so it runs as `agent`; refused as root):

| Helper | Does |
|---|---|
| `sweep` | loop: `kill(-1, SIGKILL)`, then scan `/proc/*/status`; done when no live (non-zombie) uid-`agent` process except itself remains; bounded rounds; exit 0 / 1 with `{rounds, survivors}` |
| `signal PGID SIG` | `kill(-PGID, SIG)` (graceful SIGTERM to Codex's group before the sweep) |
| `copy-in ROOT REL` | stdin → `ROOT/REL`: `mkdirat` 0700 parents, `openat2(O_CREAT\|O_EXCL, 0600, RESOLVE_BENEATH\|NO_SYMLINKS\|NO_MAGICLINKS\|NO_XDEV)` |
| `copy-out ROOT REL MAX` | `openat2(rootfd, REL, O_RDONLY\|O_NONBLOCK, same resolve flags)`; `fstat`: regular file, `st_nlink == 1`, size ≤ MAX; read at most MAX+1 bytes; stdout |
| `list ROOT` | entries below ROOT without following symlinks (to find `sessions/YYYY/MM/DD/rollout-*-<thread>.jsonl`) |
| `clear ROOT` | removes everything below ROOT with fd-relative calls, no symlink followed; ROOT stays |

Kill-all is race-free: `kill(-1)` from uid `agent` signals every process whose real or saved uid is `agent`
(except the caller) under the kernel's task-list lock, and a `fork` racing with the pending SIGKILL fails, so no
child escapes unsignalled; no process can leave uid `agent` (no capabilities, NNP, no setuid/setgid file in the
image, user namespaces denied); the `/proc` loop is the verification. Root lacks `CAP_KILL` and cannot signal
`agent` processes, so the sweep must run as `agent` — no fourth capability is needed. Measured: 1–2 rounds,
including a fork loop and SIGSTOPped and SIGTERM-ignoring processes in other sessions.

## Order per attempt (runner, WP3-RUN)
1. Start-up, before the first claim: `sweep`; for every leftover `codex-homes/<key>`: `copy-out auth.json`
   (T13 recovery), `clear`, `rmdir`; same for leftover work dirs.
2. Prepare: `mkdir <codex-homes>/<key>` 0700 → `handover`; `mkdir <work>/<key>` 0755, checkout into `repo/`
   as `runner` (umask 022: agent reads, cannot write); `mkdir home tmp` 0700 → `handover` each; `copy-in`
   `auth.json`, `config.toml`, schema, this analysis's session files (stdin = runner-opened fds).
3. `spawn` Codex (cwd `<work>/<key>`, constructed env); the runner keeps the stdout/stderr read ends.
4. End, cancel, runtime cap, self-fence, and before the compaction turn (T1d): `signal <codex pid> SIGTERM` →
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
| `…/codex-homes/` | volume `runner-codex-homes` (another fs than the session store, T8b; persistent, T1c/T13) | runner | 0711 | traverse only |
| `…/codex-homes/<job>-<attempt>/` = `CODEX_HOME` | same | agent (handover) | 0700 | read/write |
| `/work` (`SR_RUNNER_WORK_DIR`) | tmpfs, size cap, `uid=10001,mode=0711` | runner | 0711 | traverse only |
| `/work/<job>-<attempt>/` = Codex cwd; `repo/` | same | runner | 0755; files 0644 | read only |
| `/work/<job>-<attempt>/{home,tmp}` = `HOME`, `TMPDIR` | same | agent (handover) | 0700 | read/write |
| `/tmp` | tmpfs, `uid=10001,mode=0700` | runner | 0700 | no |
| `/dev/mqueue` | IPC namespace view (1777) | — | — | writable; emptied at job end |

Skills exposed to the agent must not live under a 0700 runner dir: copy them in per job or keep them in a
runner-owned 0755/0644 tree. Supervisor start-up `lchown`s the three mount roots to `runner` when a fresh mount
is root-owned; the runner sets the modes.

## Container options (compose, WP3-OPS phase 2)
`init: true`; no `user:` (the image's entrypoint is the supervisor as root); `cap_drop: [ALL]`;
`cap_add: [CHOWN, SETUID, SETGID]`; `security_opt: [no-new-privileges:true]` and nothing `unconfined`;
`read_only: true`; `ipc: none` (no `/dev/shm`); `pids_limit`; `mem_limit`; the two tmpfs and two volumes above; no
Docker socket; network per T4 (phase 2). Image: `deploy/runner/Dockerfile` (uids 10001/10002, no shared group,
setuid/setgid bits stripped).

## Rust implementation notes (WP3-RUN)
- Supervisor: a second binary in `crates/runner` (`smart-review-supervisor`), single-threaded, no tokio, `rustix`
  (`net` feature for `socketpair`/`sendmsg`/`recvmsg` with `ScmRights` and `RecvFlags::CMSG_CLOEXEC`).
  `spawn` may use `std::process::Command` with `.uid(10002).gid(10002)` (std calls `setgroups(0)` then `setuid`
  when a uid is set), `.env_clear().envs(..)`, `Stdio::from(OwnedFd)` and `pre_exec(setsid)`; reap with
  `waitpid(-1, WNOHANG)`, not `Child::wait`. Helpers are subcommands of the same binary.
- Runner: `kill_on_drop` and `kill_process_group` on Codex no longer work (different uid); every drop/cancel path
  must go through `signal` + `sweep`, and the supervisor's final sweep covers a runner crash. `RemoveOnDrop` cannot
  delete agent-owned contents: use `clear` + `rmdir`. Job dir mode 0755 (today 0700).
- Port the checks of `deploy/runner/r1-test/` to the Rust supervisor: same scenario, same check names.

## Alternatives rejected
- Runner holding `SETUID`/`SETGID` (permitted or ambient): violates CapEff 0 for `runner` (T1b).
- `sudo`/`su`/setuid helpers: blocked by NNP (`no_privilege_escalation`); NNP is required (T4).
- User namespaces, bubblewrap: `unshare(CLONE_NEWUSER)` is denied for `runner` and `agent` under the default
  seccomp profile and the host's AppArmor userns restriction (`runner_cannot_switch_uid_itself`).
- Process-group kill only (stage 2): misses `setsid` escapees, including the grandchild of a Codex MCP server
  (`process_group_kill_misses_escapees`, `real_codex_escapee_survives_group_kill`).
- `CAP_KILL` for the supervisor: unnecessary (sweep as `agent`).
- Two containers: unnecessary, every property holds in one.
- Prior art: init systems (tini, s6, runit `chpst`) switch uid only at start; `systemd-run` needs systemd/D-Bus;
  `bwrap` needs user namespaces. Hence a three-request supervisor.

## Residuals
- The bounding set of `runner` and `agent` still contains the three capabilities (dropping it needs
  `CAP_SETPCAP`); harmless under NNP with no setuid or file-capability binary.
- `/proc/*/cmdline` and `/proc/*/status` of every process are world-readable: never put a secret in argv.
- POSIX message queues and SysV IPC survive the sweep; step 5 clears them (`ipc_objects_cleared_at_job_end`).
- The agent may delete its files or make them unreadable (`chmod 000`); copy-back then fails closed (T13 marks
  the credential unusable): a denial of service for an attacker who already holds the credential (AR-4).
- A crashed run's per-job dirs stay agent-owned until start-up recovery (order step 1), which must finish before
  the first job: a later agent could traverse into a guessed `<job>-<attempt>` name.
