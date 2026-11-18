# ADR-013: Runner process model (decision R1)

- Status: accepted by evidence (WP3-OPS phase 1, 2026-11-17, revised after its security and correctness
  reviews); R1 holds, the two-container fallback is not needed.
- Source: threat model R1, T1a–T1d, T4, T8b, T13; ADR-003 (per-job `CODEX_HOME` persistent); plan §5.4.
- Evidence: `deploy/runner/r1-test/run.sh` (43 named checks, all PASS); ledger
  `/data/artifacts/smart-review/2026-11-17/wp3-ops/r1-test.log`. Docker 29.9.2, kernel 7.0, default seccomp,
  AppArmor `docker-default (enforce)`, real Codex CLI 0.167.0 and `github-mcp-server` 2.1.0, fake keys only.
- Revised 2026-11-18 (owner decision): every scratch area is a disposable Docker volume on disk instead of a
  tmpfs or the bounded codex-homes filesystem ("Scratch volumes" below).

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
| `wait {pid}` → `{status}` | answered when the supervisor reaps that spawned pid; `status` is the exit code, or the negated signal number (`-9` for SIGKILL), so a signal never reads as an exit code. The supervisor's own exit code for the runner uses 128 + signal. |
| `signal {pid, signal}` → `{signalled}` | `pid` must be a spawned pid (= its process group, `setsid`); the signal goes to each thread group of that process group whose live threads all have the four uids = `agent`, through a pidfd (below). Used for the graceful SIGTERM to Codex. |
| `sweep` → `{rounds, killed, ms}` | loop: for every thread group in `/proc`, read `/proc/<tgid>/task/*/status`; a group is live when **any thread** is neither zombie nor dead, whatever the leader's state (a leader that called `pthread_exit()` is a zombie while its other threads run); every live group with any of real/effective/saved/fs uid = `agent` on a live thread gets SIGKILL through a pidfd (kills the whole group); reap (a zombie leader is reaped after its last thread); until no live agent thread exists in `/proc/*/task/*`. In-process: no fork, no exec, so a full `pids_limit` cannot stop it. Hard deadline (default 5 s): past it, or on any error, the supervisor logs and exits 70 — init then ends the container and every process in it, the restart policy brings a clean one (fail closed). Never `kill(-1)` as root. |
| `quarantine {path}` | same path forms and open as `handover`; refused while an agent thread lives or a spawned child is unreaped; owner `agent` or `runner`; a `lock` helper as that owner sets mode 0700 (root without `CAP_FOWNER` cannot), `fstat` on the supervisor's fd checks 0700 on the same inode, then `fchownat(fd, "", runner, runner, AT_EMPTY_PATH)` and a final `fstat` (runner, 0700), all before the reply; the client checks again. Mode before owner: an interruption leaves a 0700 directory of its old owner, never a runner-owned open one. For a per-job directory that `clear` could not empty: the agent uid can no longer enter it (fail closed). Idempotent; start-up recovery repeats it. |
| `handover {path}` | `path` normalised and either `<state>/codex-homes/<name>` or `<work>/<name>/{home,tmp}`; opened with `openat2(O_PATH\|O_DIRECTORY\|O_NOFOLLOW, RESOLVE_NO_SYMLINKS)`; must be owned by `runner`, mode 0700 and empty (listed by a runner-uid child, same dev/ino, because root without `DAC_READ_SEARCH` cannot list it); then `fchownat(fd, "", agent, agent, AT_EMPTY_PATH)`. The only use of `CHOWN`. |

pidfd rule (no reused pid is ever hit): `pidfd_open(tgid)` (works on a zombie leader with live threads), then read
`/proc/<tgid>/task/*/status` and check the uids of the live threads, then
`pidfd_send_signal(pidfd, sig)`. If the pinned process died and the pid was reused before the read, the check sees
the new owner and skips it; if it dies after the read, `pidfd_send_signal` fails with `ESRCH`.

The supervisor is a subreaper (`PR_SET_CHILD_SUBREAPER`) and reaps everything; when the runner exits it sweeps and
exits with the runner's status. On SIGTERM (and SIGINT) it forwards the signal to the runner
(`sigterm_forwarded_to_runner`, Rust test).

Unprivileged **helper modes of the supervisor binary**, started by the runner through `spawn` (uid `agent`;
refused as root), because they read or write agent-owned files:

| Helper | Does |
|---|---|
| `copy-in ROOT REL` | stdin → `ROOT/REL`: `mkdirat` 0700 parents, `openat2(O_CREAT\|O_EXCL, 0600, RESOLVE_BENEATH\|NO_SYMLINKS\|NO_MAGICLINKS\|NO_XDEV)` of `REL.sr-new`, then `renameat` over `REL` (replaces a planted link, never follows it; the config is rewritten before the compaction turn) |
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
| `…/codex-homes/` | volume `tmp-runner-codex-homes` (below), mounted here | runner | 0711 | traverse only |
| `…/codex-homes/<job>-<attempt>/` = `CODEX_HOME` | same | agent (handover) | 0700 | read/write |
| `/work` (`SR_RUNNER_WORK_DIR`) | volume `tmp-runner-work` | runner | 0711 | traverse only |
| `/work/<job>-<attempt>/` = Codex cwd; `repo/` | same | runner | 0755; files 0644 | read only |
| `/work/<job>-<attempt>/{home,tmp}` = `HOME`, `TMPDIR` | same | agent (handover) | 0700 | read/write |
| `/tmp` | volume `tmp-runner` | runner | 0700 | no |
| `/dev/mqueue` | IPC namespace view (1777) | — | — | writable; emptied at job end |

**Scratch volumes (owner decision 2026-11-18: nothing in RAM, no size limit for now).** `codex-homes`, `/work`
and `/tmp` are Docker named volumes (`tmp-*`, label `smart-review.volume=temporary`, never backed up). They are
persistent, which `codex-homes` needs: Codex may rotate the single-use refresh token during a job, and until
copy-back the only valid token is in the per-job `auth.json` (ADR-003, T1c, T13 start-up recovery). Volumes have
no uid, mode or mount options and survive restarts, so start-up establishes what the tmpfs options gave: the
supervisor `lchown`s each root that is not the runner's yet (`serve.rs`, setup), the runner sets `0711`
(`codex-homes`, `/work`) and `0700` (`/tmp`), recovers and empties `codex-homes` and the job directories (order
step 1), and empties `/tmp`, failing the start if a leftover there cannot be removed (`main.rs`,
`empty_private_tmp`; `scratch::clear`). Per job, steps 2 and 5 are unchanged, so no job reads another job's area.
The image gives fresh volumes the same owners and modes. Accepted losses (threat model AR-8):
- **No size bound.** The scratch volumes live on the filesystem that holds Docker's data root, together with the
  state volume and the database. A job can fill it (bytes or inodes); where Docker's root is on the host's root
  filesystem, the whole host is affected, not only this stack (on the owner's development host Docker's root is
  `/data/docker` on a separate ext4). One consequence costs a sign-in: if copy-back cannot write the staging
  `auth.json` (`ENOSPC`), the credential is marked unknown (`isolated.rs` `pull` and `copy_back`), and `leave`
  then removes the agent's `CODEX_HOME`, the only copy of a refresh token Codex rotated, so the owner must sign
  in to Codex again. Before this change, `codex-homes` was a separate bounded filesystem (T8b) and `/work` a
  capped tmpfs. Guard: the runner claims no job while the work directory's filesystem has less than
  `SR_RUNNER_DISK_RESERVE_MIB` free (default 2048; `statvfs`, `scratch::Reserve`). It logs each change of that
  state once. The server has no "cannot take jobs" signal, so jobs simply stay queued. This is a start
  condition, not a quota: a job started above the reserve can still fill the disk. TODO: a soft per-job quota
  once needed.
- **`nosuid`, `nodev`, `noexec` are gone** (Docker mounts volumes without them). No replacement is needed for this
  threat: `nodev` — creating a device node needs `CAP_MKNOD`, which no process here has (`cap_drop: ALL`; the
  supervisor holds only `CHOWN`, `SETUID`, `SETGID`, `KILL`), and Docker's device cgroup refuses unlisted devices
  anyway; `nosuid` — the agent can set the setuid bit only on its own files (no `CHOWN`), and
  `no-new-privileges` stops any gain at `execve`; `noexec` (codex-homes only) — the agent could already execute
  files in its `HOME`/`TMPDIR` under `/work`, which never had `noexec`.
- **Remanence:** deleted scratch stays on the host disk until overwritten (readable by host root only).
- A job directory that start-up recovery quarantined stays until removed by hand (it is runner-owned, 0700).
The r1 test keeps a `size=16m,nr_inodes=2048` tmpfs for `codex-homes` (`codex_home_exhaustion_fails_job_cleanly`).
The agent hits `ENOSPC` there for bytes and inodes, while the runner still writes its state, copies `auth.json`
back and cleans up. That models only a full agent home. It no longer models production, where one shared disk
fills and the runner's own writes fail too (above).

Skills exposed to the agent must not live under a 0700 runner dir: copy them in per job or keep them in a
runner-owned 0755/0644 tree.

## Container options (compose, WP3-OPS phase 2)
`init: true`; `restart: unless-stopped` (a fail-closed exit brings a clean container); no `user:` (the image's
entrypoint is the supervisor as root); `cap_drop: [ALL]`; `cap_add: [CHOWN, SETUID, SETGID, KILL]`;
`security_opt: [no-new-privileges:true]` and nothing `unconfined`; `read_only: true`; `ipc: none` (no `/dev/shm`);
`pids_limit` (mandatory); `mem_limit`; `cpus` (default 2); the state volume and the three scratch volumes above;
no Docker socket. Image: `deploy/runner/Dockerfile` (uids 10001/10002, no shared group,
setuid/setgid bits stripped, no package manager or interpreter beyond Codex's needs).
Restart policy: exits 70 (sweep failed), 71 (supervisor setup failed) and 78 (preflight refused) end the
container; `unless-stopped` restarts it clean, and a repeating exit is visible in the restart count (runbook).
Network (T4): the runner is only on the `runner` network, `internal: true` **and** bridge gateway mode `isolated`
(IPv4 and IPv6; Docker Engine 28.0+). `internal` alone removes the route out but keeps a host address on the
bridge, through which host services listening on that address or `0.0.0.0` stay reachable without the proxy;
`isolated` assigns no host address. The runner reaches only the server's runner listener and the egress proxy
(`deploy/test-stack.sh`, `runner_reaches_no_host_service`). An Engine older than 28.0 ignores the unknown option
without an error, so the preflight also proves isolation from inside the container before the runner starts:
no IPv4 or IPv6 default route, and no gateway candidate (the first address of every attached subnet and every
route's next hop, own addresses excluded) answers a TCP connect on a few common ports within 1 s; a connection or
a refusal means an IP stack answers there and ends the start with exit 78, naming Engine 28.0 and the option
(`runner_preflight_refuses_unisolated_network`). A container added to the runner network by hand at the subnet's
first address also trips the check; compose pins every address. `SR_RUNNER_DEV_HOST_GATEWAY=1` skips it for the
development smoke only (rejected by `check-production-config.sh` and the image check).

## Rust implementation (WP3-RUN)
`smart-review-supervisor` in `crates/runner` (`src/supervisor/`, `src/supervisor_main.rs`); the crate forbids
`unsafe`, which fixes these choices:
- **Event loop:** a current-thread tokio runtime (safe signal handling for SIGTERM, SIGINT, SIGCHLD); requests
  are handled synchronously in it. The control socket is wrapped as `tokio::net::UnixDatagram` for readiness only
  (`AsyncFd::new` is deprecated as unsound, `AsyncFd::register` is unsafe); every receive is `recvmsg` with
  `SCM_RIGHTS`.
- **Control fd:** the runner's end is its stdin, `SR_SUPERVISOR_FD=0` (no `pre_exec` to place it); the
  supervisor names its own binary in `SR_SUPERVISOR_BIN` for the helper modes.
- **Spawn:** `std::process::Command` (`env_clear`, `Stdio::from(OwnedFd)`) of the binary's
  `exec-agent CWD UID:GID -- ARGV` mode, still as root; it writes `oom_score_adj` 1000, calls `setsid()`, then
  `exec`s `ARGV` through `Command` with `uid`/`gid`/`current_dir` (std: `setgroups(0)`, gid, uid, then `chdir`
  as the agent). The uid switch is in the wrapper, not in the forked child, because std's `spawn` blocks until
  the child `exec`s: an agent that SIGSTOPs every process of its uid stopped a child that had already switched,
  and the supervisor hung (container test, hostile agent). The wrapper is root until its `exec`, so the
  supervisor treats every unreaped spawned child as running: `exclusive` spawns are refused while one exists,
  `sweep` SIGKILLs them by pid first (an unreaped child's pid cannot be reused), and `signal` to a spawned pid
  that is not an agent group yet goes to that child. umask 077 is set around the spawn. Reaping:
  `waitpid(-1, WNOHANG)` as subreaper, statuses kept while a sweep reaps.
- **Helper exit codes:** 0 ok; `copy-out` 2 = open refused (link, mount, escape, missing), 3 = not a regular file
  with one link, 4 = larger than MAX; 1 = any other error; 2 also for usage errors. A helper first restores its
  own access to entries the agent locked (mode 000) through an `O_PATH` no-follow descriptor, so `clear` and
  `copy-out` are not defeated by `chmod 000`.
- **Configuration:** `SR_RUNNER_UID`/`SR_AGENT_UID` (10001/10002), `SR_SUPERVISOR_SWEEP_DEADLINE_MS` (default
  5000, at least 100). Test-only: `SR_SUPERVISOR_UNPRIVILEGED=1` lets a non-root supervisor run (no uid switch,
  sweep scoped to its own descendants outside the runner; ignored as root); `R1_FAULT_SWEEP_FAILS=1` (every sweep
  fails) and `R1_FAULT_CLEAR_FAILS=<name>` (`clear` of a directory with that name is refused, so it gets
  quarantined) are refused unless `SR_SUPERVISOR_TEST_FAULTS=1`. The image sets none of them.
- **Runner secret file:** under the supervisor the runner refuses to start (exit 78) when the agent could read
  `SR_RUNNER_SECRET_FILE`: the file is owned by the agent uid, has any `other` bit, or has any group bit with the
  agent's gid as its group (the agent has gid = uid and no supplementary group). Only the file itself is checked;
  the directories leading to it are the operator's. The supervisor tells the runner the agent uid in
  `SR_AGENT_UID`. Without the supervisor a world-readable file is only a warning.
- **Runner without a supervisor:** refuses to start unless `SR_RUNNER_DEV_NO_ISOLATION=1` (dev stack, smoke),
  then warns on every job; the image never sets it.
- **Runner layout:** the provider works on a runner-private staging `CODEX_HOME` (`<state>/codex-staging/<key>`,
  0700); the agent's `<state>/codex-homes/<key>` is filled by `copy-in` and read back by `copy-out`, so the T13
  CAS and T8b validation run on runner-owned copies.

Invariant: an attempt is finished only when signal → sweep → reap → copy-back → cleanup have completed or failed
closed. Nothing publishes a terminal result, releases the credential or claims the next job before that:
- the runtime cap and a cancel start the stop sequence through the provider and await it; a supervised call is
  never dropped (the compaction turn is bounded by its own deadline inside the run, not by dropping it);
- attempts run as their own task holding a per-runner gate; the worker waits for the gate before posting a
  terminal result and before the next claim;
- `auth.json` that cannot be copied back (missing, link, oversized, not a single regular file) means the
  credential state is unknown: the master is marked unusable (a new login is needed), the old staged copy proves
  nothing;
- the job loop stops reading the attempt's events (closes the channel) before it waits for the gate, so
  back-pressure never blocks the attempt's signal/sweep/clean-up; events already buffered are still posted;
- a per-job directory that `clear` cannot empty is quarantined; if that fails too, the runner takes no further
  job (its credential is reported unusable);
- start-up recovery, before the first claim, handles every leftover the same way (remove, else quarantine and
  verify runner-owned 0700, an already quarantined one included); any failure stops start-up. A leftover agent
  `auth.json` that cannot be copied back marks the master the job started from unusable (`<key>.unknown`), unless
  the owner signed in again since (another digest).

Live text (events, stderr) is scrubbed of the known secrets and, because a token rotated by the agent is known
only after the copy-back, of token shapes: JWTs (`eyJ…` with three base64url segments, the first two at least 10
characters) and, for every known secret with a prefix up to its first `_` or `-` within 8 characters (`ghs_`,
`codex-`, `sk-`), that prefix followed by at least 20 token characters including an upper-case letter or a digit
(so lowercase identifiers stay). Shapes are found anywhere in a word, also glued to other characters
(`x<token>`); each segment is tried once, so the cost stays linear. Everything stored or returned (output,
result text, session rollout) is scrubbed with the exact rotated values after the copy-back. Residual: a rotated
token without such a prefix, or one the agent encodes, can appear in live progress text only (AR-4).

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
- The agent may delete or replace its `auth.json`; copy-back then fails closed (T13 marks the credential unusable):
  a denial of service for an attacker who already holds the credential (AR-4). `chmod 000` alone is undone by
  the helper (owner access restored).
- A crashed run's per-job dirs stay agent-owned until start-up recovery (order step 1), which must finish before
  the first job: a later agent could traverse into a guessed `<job>-<attempt>` name.
- **Accepted denial of service:** a hostile job can make a sweep fail (for example a process the kernel cannot
  kill within the deadline) and so force the fail-closed exit 70 and a container restart, once per attempt; the
  job's attempt limit (`max_attempts` = 3, `runner-protocol.md`) bounds the repetitions per job, other jobs of the
  queue are delayed meanwhile.
- The development smoke (`deploy/dev/real-codex-smoke.sh`, `SR_SMOKE_ISOLATION=supervisor`) keeps a host address
  on its internal network, because the dev server runs on the host; never a deployment.
