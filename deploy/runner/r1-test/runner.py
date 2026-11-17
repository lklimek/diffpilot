#!/usr/bin/env python3
"""Stand-in runner for the R1 container test (ADR-013). Runs as uid `runner` under supervisor.py, drives stand-in
jobs (benign, hostile, pid exhaustion, CODEX_HOME exhaustion) and a real-Codex job through the supervisor protocol
and prints `CHECK <name> PASS|FAIL <detail>`. R1_SCENARIO=sweep-failure runs only the fail-closed scenario.
Canary values start with the prefix C; SR_RUNNER_SECRET is one (set by run.sh)."""

import collections
import json
import os
import re
import secrets
import select
import socket
import subprocess
import time

SUP, AGENT, PY = "/opt/r1-test/supervisor.py", "/opt/r1-test/agent.py", "/usr/bin/python3"
STATE = os.environ["SR_RUNNER_STATE_DIR"]
WORK = os.environ["SR_RUNNER_WORK_DIR"]
HOMES = os.path.join(STATE, "codex-homes")
AGENT_UID, RUNNER_UID = 10002, 10001
CAPS_FOUR = "00000000000000e1"  # CHOWN | KILL | SETGID | SETUID
CAP_KILL_BIT = 1 << 5
ZERO = "0000000000000000"
C = "".join(("SR", "CANARY"))  # split so that this file itself is no canary hit
JOB_TOKEN = f"{C}-job-token-" + secrets.token_hex(8)  # lives only in this process's memory
MASTER = os.path.join(STATE, "credentials/default/auth.json")
THREAD_A, THREAD_B = "019a0000-aaaa-7000-8000-00000000000a", "019a0000-bbbb-7000-8000-00000000000b"
ROLLOUT = "sessions/2026/11/17/rollout-2026-11-17T00-00-00-{}.jsonl"
COPY_CAP = 1 << 20
HELPER_DEADLINE = 30  # seconds; every wait on an agent process is bounded

results = []


def check(name, ok, detail):
    results.append(ok)
    print(f"CHECK {name} {'PASS' if ok else 'FAIL'} {detail}", flush=True)


def status(pid):
    with open(f"/proc/{pid}/status") as f:
        return {k: v.strip() for k, v in (line.split(":", 1) for line in f if ":" in line)}


def processes():
    """{pid: (real uid, state, cmdline)} of every live process."""
    found = {}
    for name in filter(str.isdigit, os.listdir("/proc")):
        try:
            st = status(name)
            with open(f"/proc/{name}/cmdline", "rb") as f:
                cmd = f.read().replace(b"\0", b" ").decode(errors="replace").strip()
        except OSError:
            continue
        found[int(name)] = (int(st["Uid"].split()[0]), st["State"][0], " ".join(cmd.split())[:100])
    return found


def agent_threads():
    """(tgid, tid, state) of every live thread with any uid = agent, from /proc/*/task/* (a zombie thread-group
    leader with running threads is a live process)."""
    found = []
    for tgid in filter(str.isdigit, os.listdir("/proc")):
        try:
            tids = os.listdir(f"/proc/{tgid}/task")
        except OSError:
            continue
        for tid in tids:
            try:
                st = status(f"{tgid}/task/{tid}")
            except OSError:
                continue
            if str(AGENT_UID) in st["Uid"].split() and st["State"][0] not in "ZX":
                found.append((int(tgid), int(tid), st["State"][0]))
    return found


def agent_live():
    return {p: v for p, v in processes().items() if v[0] == AGENT_UID and v[1] not in "ZX"}


class Supervisor:
    """Client of the supervisor protocol over the inherited SOCK_SEQPACKET socket."""

    def __init__(self):
        self.sock = socket.socket(fileno=int(os.environ["SR_SUPERVISOR_FD"]))
        self.next = 0
        self.early = {}

    def send(self, op, fds=(), **fields):
        self.next += 1
        msg = json.dumps({"id": self.next, "op": op, **fields}).encode()
        socket.send_fds(self.sock, [msg], list(fds)) if fds else self.sock.send(msg)
        return self.next

    def reply(self, rid, timeout=None):
        """The reply to request rid, or None when the timeout passes first."""
        deadline = None if timeout is None else time.monotonic() + timeout
        while rid not in self.early:
            left = None if deadline is None else deadline - time.monotonic()
            if left is not None and (left <= 0 or not select.select([self.sock], [], [], left)[0]):
                return None
            reply = json.loads(self.sock.recv(1 << 20))
            self.early[reply["id"]] = reply
        return self.early.pop(rid)

    def call(self, op, fds=(), **fields):
        """Requests the supervisor answers at once (spawn, signal, sweep, handover)."""
        return self.reply(self.send(op, fds, **fields))


class Proc:
    def __init__(self, pid, out, err):
        self.pid, self.out, self.err, self.buf = pid, out, err, b""

    def line(self, timeout):
        deadline = time.monotonic() + timeout
        while b"\n" not in self.buf:
            left = deadline - time.monotonic()
            if left <= 0 or not select.select([self.out], [], [], left)[0]:
                return None
            chunk = os.read(self.out, 65536)
            if not chunk:
                return None
            self.buf += chunk
        line, self.buf = self.buf.split(b"\n", 1)
        return line.decode()

    def drain(self, timeout):
        """(stdout, stderr text), or None when the deadline passes first."""
        deadline = time.monotonic() + timeout
        out, err = [self.buf], []
        open_fds = {self.out: out, self.err: err}
        while open_fds:
            left = deadline - time.monotonic()
            ready = select.select(list(open_fds), [], [], max(left, 0))[0] if left > 0 else []
            if not ready:
                return None
            for fd in ready:
                chunk = os.read(fd, 65536)
                if chunk:
                    open_fds[fd].append(chunk)
                else:
                    del open_fds[fd]
                    os.close(fd)
        return b"".join(out), b"".join(err).decode(errors="replace").strip()


Result = collections.namedtuple("Result", "status out err timed_out sweep state_at_timeout")


def agent_env(key):
    return [
        f"HOME={WORK}/{key}/home",
        f"TMPDIR={WORK}/{key}/tmp",
        f"CODEX_HOME={HOMES}/{key}",
        "PATH=/usr/local/bin:/usr/bin:/bin",
        "LANG=C.UTF-8",
    ]


def spawn(sup, argv, key, stdin=b"", keep_stdin=False, exclusive=False):
    """Starts argv as uid agent via the supervisor; stdin is written and closed unless keep_stdin."""
    in_r, in_w = os.pipe()
    out_r, out_w = os.pipe()
    err_r, err_w = os.pipe()
    reply = sup.call(
        "spawn", fds=(in_r, out_w, err_w), argv=argv, env=agent_env(key), cwd=f"{WORK}/{key}", exclusive=exclusive
    )
    for fd in (in_r, out_w, err_w):
        os.close(fd)
    if not reply["ok"]:
        raise RuntimeError(f"spawn refused: {reply}")
    if stdin:
        os.set_blocking(in_w, False)
        deadline = time.monotonic() + HELPER_DEADLINE
        while stdin and select.select([], [in_w], [], max(deadline - time.monotonic(), 0))[1]:
            stdin = stdin[os.write(in_w, stdin) :]
    if not keep_stdin:
        os.close(in_w)
    return Proc(reply["pid"], out_r, err_r)


def finish(sup, proc, deadline):
    """Reads the agent command's output AND its exit status within one deadline; past it the supervisor sweeps
    every agent process, so neither a stopped command nor one that closed its streams and lingers blocks."""
    end = time.monotonic() + deadline
    drained = proc.drain(deadline)
    rid = sup.send("wait", pid=proc.pid)
    reply = sup.reply(rid, max(end - time.monotonic(), 0)) if drained is not None else None
    swept = state = None
    if reply is None:
        state = processes().get(proc.pid, (None, "gone"))[1]
        swept = sup.call("sweep")
        drained = drained or proc.drain(5) or (b"", "")
        reply = sup.reply(rid, 5)
    return Result((reply or {}).get("status"), *drained, swept is not None, swept, state)


def wait_status(sup, pid, timeout):
    """Exit status once reaped, or None after timeout (bounded; the caller sweeps next either way)."""
    return (sup.reply(sup.send("wait", pid=pid), timeout) or {}).get("status")


def run(sup, argv, key, stdin=b"", deadline=HELPER_DEADLINE, exclusive=False):
    """Runs an agent command to its end within the deadline (see finish)."""
    return finish(sup, spawn(sup, argv, key, stdin, exclusive=exclusive), deadline)


def write_private(path, data, mode=0o600):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, mode)
    os.write(fd, data.encode())
    os.close(fd)


def layout():
    """State volume as ADR-013 specifies; canaries in every runner-private file."""
    os.chmod(STATE, 0o711)
    os.chmod(HOMES, 0o711)
    os.chmod(WORK, 0o711)
    for d in (
        "credentials",
        "credentials/default",
        "sessions",
        f"sessions/{THREAD_A}",
        f"sessions/{THREAD_B}",
        "mirrors",
        "mirrors/github.com_x_y.git",
    ):
        os.mkdir(os.path.join(STATE, d), 0o700)
    write_private(MASTER, f'{{"canary":"{C}-master-auth"}}')
    write_private(os.path.join(STATE, f"sessions/{THREAD_A}/rollout-a.jsonl"), f"{C}-session-A\n")
    write_private(os.path.join(STATE, f"sessions/{THREAD_B}/rollout-b.jsonl"), f"{C}-session-B\n")
    write_private(os.path.join(STATE, "mirrors/github.com_x_y.git/config"), f"{C}-mirror\n")
    modes = {
        p: oct(os.stat(os.path.join(STATE, p)).st_mode & 0o777)
        for p in (".", "credentials", "sessions", "mirrors", "codex-homes")
    }
    want = {".": "0o711", "credentials": "0o700", "sessions": "0o700", "mirrors": "0o700", "codex-homes": "0o711"}
    check(
        "state_layout_modes",
        modes == want and oct(os.stat(WORK).st_mode & 0o777) == "0o711",
        f"state={modes} work={oct(os.stat(WORK).st_mode & 0o777)}",
    )


def prepare_job(sup, key, files):
    """Runner-made empty 0700 dirs, handed over to agent; files copied in as agent (O_EXCL, no symlinks)."""
    home = os.path.join(HOMES, key)
    job = os.path.join(WORK, key)
    os.mkdir(home, 0o700)
    os.mkdir(job, 0o755)
    os.mkdir(os.path.join(job, "repo"), 0o755)
    with open(os.path.join(job, "repo/README.md"), "w") as f:
        f.write("checkout stand-in\n")
    os.chmod(os.path.join(job, "repo/README.md"), 0o644)
    replies = []
    for path in (home, os.path.join(job, "home"), os.path.join(job, "tmp")):
        if path != home:
            os.mkdir(path, 0o700)
        replies.append(sup.call("handover", path=path))
    copied = [run(sup, [PY, SUP, "copy-in", home, rel], key, data.encode())[0] for rel, data in files.items()]
    return home, job, replies, copied


def ipc_objects():
    """POSIX message queues and uid-agent SysV objects (IPC outlives the processes that made it)."""
    found = [f"mq:{n}" for n in os.listdir("/dev/mqueue")]
    for kind in ("shm", "msg", "sem"):
        with open(f"/proc/sysvipc/{kind}") as f:
            rows = [line.split() for line in f.readlines()[1:]]
        found += [f"{kind}:{r[1]}" for r in rows if str(AGENT_UID) in r[7:11]]
    return found


def cleanup_job(sup, key):
    """After kill_all: per-job dirs and IPC objects emptied as agent, then the empty dirs removed by runner."""
    home, job = os.path.join(HOMES, key), os.path.join(WORK, key)
    statuses = [run(sup, [PY, SUP, "clear", d], key)[0] for d in (home, f"{job}/home", f"{job}/tmp", "/dev/mqueue")]
    statuses.append(run(sup, ["/usr/bin/ipcrm", "--all"], key, exclusive=True)[0])
    for d in (home, f"{job}/home", f"{job}/tmp"):
        os.rmdir(d)  # empty agent-owned dir in a runner-owned parent
    os.remove(f"{job}/repo/README.md")
    os.rmdir(f"{job}/repo")
    os.rmdir(job)
    return statuses, [d for d in (home, job) if os.path.lexists(d)]


def kill_all(sup):
    """Supervisor sweep: SIGKILL to every uid-agent process, as root with CAP_KILL (fail closed)."""
    return sup.call("sweep")


def signal_group(sup, pgid, sig):
    return sup.call("signal", pid=pgid, signal=sig)


def copy_out(sup, key, rel, cap=COPY_CAP):
    started = time.monotonic()
    r = run(sup, [PY, SUP, "copy-out", os.path.join(HOMES, key), rel, str(cap)], key)
    return r.status, r.out, r.err, time.monotonic() - started


def identity_checks():
    sup_pid = os.getppid()
    s, me = status(sup_pid), status("self")
    sup_ok = (
        s["Uid"].split()[0] == "0"
        and s["CapEff"] == s["CapPrm"] == s["CapBnd"] == CAPS_FOUR
        and s["CapAmb"] == ZERO
        and s["NoNewPrivs"] == "1"
        and s["Seccomp"] == "2"
    )
    check(
        "supervisor_caps_only_four",
        sup_ok,
        f"pid={sup_pid} uid={s['Uid'].split()[0]} CapEff={s['CapEff']} CapPrm={s['CapPrm']} "
        f"CapBnd={s['CapBnd']} NoNewPrivs={s['NoNewPrivs']} Seccomp={s['Seccomp']}",
    )
    tries = {
        name: subprocess.run(argv, capture_output=True, text=True)
        for name, argv in (
            ("setpriv_agent", ["/usr/bin/setpriv", "--reuid=10002", "--regid=10002", "--clear-groups", "/bin/true"]),
            ("unshare_userns", ["/usr/bin/unshare", "--user", "--map-user=10002", "/bin/true"]),
        )
    }
    check(
        "runner_cannot_switch_uid_itself",
        all(r.returncode != 0 for r in tries.values()),
        "; ".join(f"{k}: rc={r.returncode} {r.stderr.strip()!r}" for k, r in tries.items()) + " (hence the supervisor)",
    )
    return sup_pid, me


def stand_in_job(sup, sup_pid, me):
    key = "job1-1"
    files = {
        "auth.json": f'{{"canary":"{C}-job-auth-copy"}}',
        "config.toml": "# stand-in\n",
        ROLLOUT.format(THREAD_A): f"{C}-session-A-copy\n",
    }
    home, job, replies, copied = prepare_job(sup, key, files)
    owned = {p: (os.stat(p).st_uid, oct(os.stat(p).st_mode & 0o777)) for p in (home, f"{job}/home", f"{job}/tmp")}
    check(
        "per_job_home_owned_by_agent",
        all(r["ok"] for r in replies) and all(v == (AGENT_UID, "0o700") for v in owned.values()) and copied == [0] * 3,
        f"owner/mode={owned} copy_in_status={copied}",
    )

    os.mkdir(os.path.join(HOMES, "full"), 0o700)
    open(os.path.join(HOMES, "full/x"), "w").close()
    os.symlink(os.path.join(STATE, "credentials"), os.path.join(HOMES, "evil"))
    refused = {
        p: sup.call("handover", path=p)["ok"]
        for p in (
            os.path.join(STATE, "credentials"),
            os.path.join(HOMES, "evil"),
            os.path.join(HOMES, "full"),
            os.path.join(HOMES, "..", "credentials"),
            f"{job}/repo",
        )
    }
    check("handover_refuses_unsafe_paths", not any(refused.values()), f"accepted={refused}")
    os.remove(os.path.join(HOMES, "full/x"))
    os.rmdir(os.path.join(HOMES, "full"))
    os.remove(os.path.join(HOMES, "evil"))

    deny = [
        f"/proc/{os.getpid()}/environ",
        f"/proc/{os.getpid()}/mem",
        f"/proc/{os.getpid()}/fd",
        f"/proc/{sup_pid}/environ",
        "/proc/1/environ",
        STATE,
        os.path.join(STATE, "credentials"),
        MASTER,
        os.path.join(STATE, "sessions"),
        os.path.join(STATE, f"sessions/{THREAD_A}/rollout-a.jsonl"),
        os.path.join(STATE, f"sessions/{THREAD_B}/rollout-b.jsonl"),
        os.path.join(STATE, "mirrors"),
        os.path.join(STATE, "mirrors/github.com_x_y.git/config"),
        HOMES,
        "/tmp",
    ]
    args = {
        "runner_pid": os.getpid(),
        "supervisor_pid": sup_pid,
        "deny": deny,
        "canary_prefix": C,
        "setuid_probe": "/usr/local/bin/setuid-id",
        "own": [f"{home}/auth.json", f"{home}/config.toml", f"{job}/home", f"{job}/tmp"],
    }
    probe = spawn(sup, [PY, AGENT, "probe"], key, (json.dumps(args) + "\n").encode(), keep_stdin=True)
    findings = json.loads(probe.line(300) or "{}")
    ident = findings.get("identity", {})
    agent_ok = (
        ident.get("Uid") == "10002\t10002\t10002\t10002"
        and ident.get("Gid") == "10002\t10002\t10002\t10002"
        and ident.get("Groups") == ""
        and all(ident.get(c) == ZERO for c in ("CapInh", "CapPrm", "CapEff", "CapAmb"))
        and ident.get("NoNewPrivs") == "1"
        and findings.get("oom_score_adj") == "1000"
    )
    runner_ok = (
        me["Uid"] == "10001\t10001\t10001\t10001"
        and me["Gid"] == "10001\t10001\t10001\t10001"
        and me["Groups"] == ""
        and all(me[c] == ZERO for c in ("CapInh", "CapPrm", "CapEff", "CapAmb"))
        and me["NoNewPrivs"] == "1"
    )
    check(
        "supervisor_uid_switch",
        agent_ok and runner_ok,
        f"runner Uid={me['Uid']!r} CapEff={me['CapEff']} CapPrm={me['CapPrm']}; agent Uid={ident.get('Uid')!r} "
        f"Groups={ident.get('Groups')!r} CapEff={ident.get('CapEff')} CapPrm={ident.get('CapPrm')} "
        f"NoNewPrivs={ident.get('NoNewPrivs')} oom_score_adj={findings.get('oom_score_adj')}",
    )
    held = {
        who: [c for c in ("CapInh", "CapPrm", "CapEff", "CapAmb") if int(caps.get(c, "0"), 16) & CAP_KILL_BIT]
        for who, caps in (("runner", me), ("agent", ident))
    }
    check(
        "cap_kill_not_inherited",
        bool(ident) and not any(held.values()),
        f"sets holding CAP_KILL: {held} (bounding set keeps it; NNP and no setuid/file-cap binary)",
    )
    check(
        "default_seccomp_and_apparmor_kept",
        ident.get("Seccomp") == "2" and findings.get("apparmor") == "docker-default (enforce)",
        f"agent Seccomp={ident.get('Seccomp')} apparmor={findings.get('apparmor')!r}",
    )
    check(
        "agent_env_constructed",
        findings.get("env_keys") == sorted(e.split("=")[0] for e in agent_env(key)),
        f"keys={findings.get('env_keys')}",
    )
    reads = findings.get("reads", {})
    check(
        "agent_cannot_read_runner_state",
        len(reads) == len(deny) and all(v.startswith("denied") for v in reads.values()),
        "; ".join(f"{k}: {v}" for k, v in reads.items()),
    )
    fds = findings.get("fds", {})
    check(
        "agent_holds_no_supervisor_socket",
        sorted(fds) == ["0", "1", "2"] and not any(v.startswith("socket:") for v in fds.values()),
        f"agent fds={fds}",
    )
    hits = findings.get("canaries", {}).get("hits", [])
    expected = {f"{home}/auth.json", f"{home}/{ROLLOUT.format(THREAD_A)}"}
    check(
        "agent_canary_sweep",
        set(hits) == expected,
        f"hits={hits} (expected only the own per-job copies, AR-4) files_scanned="
        f"{findings.get('canaries', {}).get('files_scanned')}",
    )
    sig = findings.get("signal", {})
    check(
        "agent_cannot_signal_runner_or_supervisor",
        len(sig) == 3 and all(v.startswith("denied") for v in sig.values()),
        f"{sig}",
    )
    esc = findings.get("escalation", {})
    check(
        "no_privilege_escalation",
        esc.get("setuid_binary_euid", {}).get("out") == "10002"
        and all(esc.get(k, {}).get("rc", 0) != 0 for k in ("setpriv_root", "setpriv_runner", "unshare_userns")),
        f"{esc}",
    )
    writable = findings.get("writable", [])
    own_dirs = {home, f"{job}/home", f"{job}/tmp"}
    check(
        "agent_writable_paths_only_job_dirs",
        set(writable) <= own_dirs | {"/dev/mqueue"} and own_dirs <= set(writable),
        f"writable={writable} (/dev/mqueue: view of the IPC namespace, emptied at job end)",
    )
    own = findings.get("own_files", {})
    check(
        "agent_owns_copied_in_files",
        all(v.endswith("uid=10002") and v.startswith(("0o600", "0o700")) for v in own.values()) and len(own) == 4,
        f"{own}",
    )

    left = probe.line(30)
    time.sleep(0.5)
    before = agent_live()
    signal_group(sup, probe.pid, 15)
    probe_status = wait_status(sup, probe.pid, 10)  # its children keep stdout open; the sweep follows
    time.sleep(0.5)
    after_group_kill = agent_live()
    check(
        "process_group_kill_misses_escapees",
        len(after_group_kill) >= 4,
        f"left_behind={left is not None} agent processes before={len(before)} probe_status={probe_status} "
        f"alive after SIGTERM to the group={len(after_group_kill)}: "
        f"{sorted(set(v[2] for v in after_group_kill.values()))}",
    )
    killed = kill_all(sup)
    time.sleep(0.3)
    remaining = {p: v for p, v in processes().items() if v[0] == AGENT_UID}
    check(
        "agent_processes_killed_at_job_end",
        killed["ok"] and not remaining and os.path.exists(f"/proc/{sup_pid}"),
        f"sweep={ {k: killed.get(k) for k in ('ok', 'rounds', 'killed', 'ms')} } uid-agent processes left="
        f"{len(remaining)} supervisor alive={os.path.exists(f'/proc/{sup_pid}')}",
    )
    return key, home


def copy_back_checks(sup, key, home):
    plant_args = {
        "codex_home": home,
        "master_auth": MASTER,
        "foreign_session": os.path.join(STATE, f"sessions/{THREAD_B}/rollout-b.jsonl"),
    }
    planted = json.loads(run(sup, [PY, AGENT, "plant"], key, (json.dumps(plant_args) + "\n").encode()).out or b"{}")
    kill_all(sup)
    try:
        open(os.path.join(home, "auth.json.real")).read()
        naive = "READ"
    except OSError as error:
        naive = f"denied: {error.strerror}"
    check(
        "runner_cannot_read_agent_home_directly",
        naive.startswith("denied"),
        f"runner open of the per-job auth.json: {naive} (hence copy-out runs as agent)",
    )
    cat_rc, cat_out, cat_err = run(sup, ["/bin/cat", os.path.join(home, "auth.json")], key)[:3]
    check(
        "copy_back_as_agent_cannot_reach_runner_files",
        cat_rc != 0 and C.encode() not in cat_out,
        f"agent `cat` through the auth.json symlink to the master: status={cat_rc} {cat_err!r}",
    )
    r = {name: copy_out(sup, key, rel) for name, rel in planted.items() if name != "hardlink_master"}
    r["real_auth"] = copy_out(sup, key, "auth.json.real")
    show = {k: (v[0], v[2], round(v[3], 2)) for k, v in r.items()}
    check(
        "auth_writeback_rejects_symlink",
        r["auth_symlink_to_master"][0] == 2 and not r["auth_symlink_to_master"][1],
        f"auth.json -> master: {show['auth_symlink_to_master']}",
    )
    check(
        "copy_back_rejects_symlink_and_foreign_thread",
        r["dir_symlink_to_foreign"][0] == 2
        and r["escape"][0] == 2
        and not r["dir_symlink_to_foreign"][1]
        and not r["escape"][1]
        and planted.get("hardlink_master", "").startswith("denied"),
        f"dir symlink to another analysis: {show['dir_symlink_to_foreign']}; ../ escape: {show['escape']}; "
        f"hardlink to master: {planted.get('hardlink_master')} (thread-id name match is the runner's, WP3-RUN)",
    )
    check(
        "copy_back_rejects_non_regular",
        r["fifo"][0] == 3 and r["fifo"][3] < 5 and r["second_link"][0] == 3,
        f"fifo: {show['fifo']}; second hard link: {show['second_link']}",
    )
    check(
        "copy_back_size_capped",
        r["oversize"][0] == 4 and not r["oversize"][1],
        f"2 MiB file, cap {COPY_CAP}: {show['oversize']}",
    )
    check(
        "copy_back_regular_file_ok",
        r["good"][0] == 0
        and b'"THREAD"' in r["good"][1]
        and r["real_auth"][0] == 0
        and f"{C}-job-auth-copy".encode() in r["real_auth"][1],
        f"rollout: {show['good']}; per-job auth.json: status={r['real_auth'][0]}",
    )
    ipc_before = ipc_objects()
    statuses, left = cleanup_job(sup, key)
    ipc_after = ipc_objects()
    check("job_dirs_removed", statuses == [0] * 5 and not left, f"clear status={statuses} left={left}")
    check(
        "ipc_objects_cleared_at_job_end",
        len(ipc_before) >= 2 and not ipc_after,
        f"after kill_all={ipc_before} after cleanup={ipc_after}",
    )


def real_codex_job(sup, sup_pid):
    key = "job2-1"
    config = (
        '[mcp_servers.escapee]\ncommand = "/bin/sh"\n'
        'args = ["-c", "(setsid /bin/sleep 1010 &) ; exec /bin/sleep 1011"]\nstartup_timeout_sec = 5\n'
        '[mcp_servers.github]\ncommand = "/usr/local/bin/github-mcp-server"\n'
        'args = ["stdio", "--read-only", "--tools=pull_request_read"]\n'
        'env = { GITHUB_PERSONAL_ACCESS_TOKEN = "ghs_SRFAKE_not_a_token" }\n'
    )
    fake_auth = '{"auth_mode":"apikey","OPENAI_API_KEY":"sk-SRFAKE-not-a-credential"}'
    home, job, replies, copied = prepare_job(sup, key, {"auth.json": fake_auth, "config.toml": config})
    codex = spawn(sup, ["/usr/local/bin/codex", "exec", "--json", "--skip-git-repo-check", "-"], key, b"say hi\n")
    thread = None
    deadline = time.monotonic() + 120
    seen = set()
    turn_started = False
    while time.monotonic() < deadline:
        line = codex.line(2)
        if line and thread is None and '"thread.started"' in line:
            thread = json.loads(line)["thread_id"]
        turn_started = turn_started or bool(line and '"turn.started"' in line)
        seen |= {v[2] for v in processes().values() if v[0] == AGENT_UID}
        cmds = " | ".join(v[2] for v in agent_live().values())
        if thread and turn_started and "github-mcp-server" in cmds and "sleep 1010" in cmds:
            time.sleep(2)  # the rollout exists from turn start; it is listed after the sweep
            break
    procs = processes()
    other = {p: v for p, v in procs.items() if v[0] != AGENT_UID and p not in (1, sup_pid, os.getpid())}
    agent_caps = {p: status(p)["CapEff"] for p, v in procs.items() if v[0] == AGENT_UID and v[1] not in "ZX"}
    names = " | ".join(sorted(seen))
    check(
        "real_codex_tree_runs_as_agent",
        thread is not None
        and "codex exec" in names
        and "github-mcp-server" in names
        and "sleep 1010" in names
        and not other
        and set(agent_caps.values()) == {ZERO},
        f"thread={thread} non-agent processes besides tini/supervisor/runner={other} agent CapEff="
        f"{set(agent_caps.values())} seen={names}",
    )
    out = run(sup, [PY, AGENT, "environ-scan", C], key).out
    scan = json.loads(out or b"{}")
    check(
        "real_codex_env_has_no_runner_secret",
        codex.pid in scan.get("readable", []) and not scan.get("hits"),
        f"codex pid {codex.pid} environ readable to agent={codex.pid in scan.get('readable', [])} "
        f"environs with the canary={scan.get('hits')}",
    )
    signal_group(sup, codex.pid, 15)
    codex_status = wait_status(sup, codex.pid, 10)
    time.sleep(0.5)
    survivors = sorted(set(v[2] for v in agent_live().values()))
    check(
        "real_codex_escapee_survives_group_kill",
        any("sleep 1010" in c for c in survivors),
        f"codex status={codex_status}; alive after SIGTERM to Codex's group: {survivors}",
    )
    killed = kill_all(sup)
    time.sleep(0.3)
    left = [p for p, v in processes().items() if v[0] == AGENT_UID]
    check("real_codex_processes_killed", killed["ok"] and not left, f"sweep={killed} left={left}")
    names = [e["path"] for e in json.loads(run(sup, [PY, SUP, "list", home], key).out or b"[]")]
    pattern = re.compile(rf"^sessions/\d{{4}}/\d{{2}}/\d{{2}}/rollout-[0-9T:-]+-{re.escape(thread or '-')}\.jsonl$")
    rollout = next((n for n in names if pattern.match(n)), None)
    rc_roll, data, _, _ = copy_out(sup, key, rollout or "missing", cap=64 << 20)
    rc_auth, auth, _, _ = copy_out(sup, key, "auth.json")
    first = data.split(b"\n", 1)[0]
    check(
        "real_codex_rollout_copied_back",
        rollout is not None
        and rc_roll == 0
        and thread.encode() in first
        and rc_auth == 0
        and auth == fake_auth.encode(),
        f"rollout={rollout} status={rc_roll} first line has thread id={thread is not None and thread.encode() in first} "
        f"auth.json status={rc_auth}",
    )
    statuses, left_dirs = cleanup_job(sup, key)
    check(
        "real_codex_job_dirs_removed",
        statuses == [0] * 5 and not left_dirs,
        f"clear status={statuses} left={left_dirs}",
    )


def hostile_job(sup, sup_pid):
    """An agent that SIGSTOPs everything of its uid in a loop, ignores SIGTERM and runs a fork loop: the supervisor
    refuses helpers meanwhile; an agent command started anyway is stopped, hits its deadline, and the supervisor
    sweep ends all of it."""
    key = "hostile-1"
    home, job, _, _ = prepare_job(sup, key, {"config.toml": "# hostile\n"})
    args = {"runner_pid": os.getpid(), "supervisor_pid": sup_pid}
    hostile = spawn(sup, [PY, AGENT, "hostile"], key, (json.dumps(args) + "\n").encode(), keep_stdin=True)
    tried = json.loads(hostile.line(20) or "{}")
    time.sleep(1)
    stopped = sum(1 for v in agent_live().values() if v[1] == "T")
    try:
        spawn(sup, [PY, SUP, "list", home], key)
        refused = None
    except RuntimeError as error:
        refused = str(error)[:160]
    check(
        "supervisor_refuses_helper_while_agent_alive",
        refused is not None and "helpers run only after a sweep" in refused,
        f"helper spawn while the hostile agent runs: {refused!r}",
    )
    started = time.monotonic()
    helper = run(sup, ["/bin/sleep", "30"], key, deadline=2)
    elapsed = time.monotonic() - started
    left = [p for p, v in processes().items() if v[0] == AGENT_UID]
    check(
        "hung_agent_command_hits_deadline_then_swept",
        helper.timed_out and helper.state_at_timeout == "T" and helper.status == -9 and helper.sweep.get("ok"),
        f"command state at the 2 s deadline={helper.state_at_timeout!r} status={helper.status} "
        f"returned after {elapsed:.2f} s",
    )
    states = {who: status(pid)["State"] for who, pid in (("runner", "self"), ("supervisor", sup_pid))}
    check(
        "hostile_agent_swept_within_deadline",
        stopped > 0
        and helper.sweep.get("ok")
        and helper.sweep.get("ms", 1e9) < 5000
        and not left
        and all(v.startswith("denied") for v in tried.values())
        and len(tried) == 2
        and not any(v.startswith("T") for v in states.values()),
        f"stopped agent processes before={stopped} sweep={helper.sweep} uid-agent left={len(left)} "
        f"hostile SIGSTOP to runner/supervisor={tried} states={states}",
    )
    statuses, left_dirs = cleanup_job(sup, key)
    if statuses != [0] * 5 or left_dirs:
        check("hostile_job_dirs_removed", False, f"clear status={statuses} left={left_dirs}")


def pid_exhaustion_job(sup):
    """A fork loop that ignores SIGTERM fills pids_limit; spawning fails, the in-process sweep still works."""
    key = "forkbomb-1"
    prepare_job(sup, key, {})
    bomb = "trap '' TERM HUP; while :; do /bin/sleep 1000 & done"  # holds every free pid
    spawn(sup, ["/bin/bash", "-c", bomb], key)  # bash, unlike dash, survives a failed fork
    refused, peak, deadline = None, 0, time.monotonic() + 20
    while refused is None and time.monotonic() < deadline:
        with open("/sys/fs/cgroup/pids.current") as f:
            peak = max(peak, int(f.read()))
        try:
            proc = spawn(sup, ["/bin/true"], key)
            proc.drain(1)
        except RuntimeError as error:
            refused = str(error)[:120]
        time.sleep(0.2)
    swept = kill_all(sup)
    after = run(sup, ["/bin/true"], key, deadline=5)
    check(
        "pid_exhaustion_sweep_still_works",
        refused is not None and swept.get("killed", 0) > 400 and after.status == 0 and not agent_threads(),
        f"peak container pids={peak} (pids_limit 512) spawn refused={refused!r} sweep={swept} spawn afterwards={after.status}",
    )
    cleanup_job(sup, key)


def codex_home_exhaustion_job(sup):
    """The agent fills its bounded per-job CODEX_HOME (bytes, then inodes): the job fails, runner state is fine."""
    key = "fill-1"
    home, _, _, _ = prepare_job(sup, key, {"auth.json": f'{{"canary":"{C}-job-auth-fill"}}'})
    fill = run(sup, [PY, AGENT, "fill"], key, deadline=120)
    report = json.loads(fill.out or b"{}")
    kill_all(sup)
    probe = os.path.join(STATE, "runner-write-probe")
    write_private(probe, "x" * (1 << 20))
    runner_ok = os.path.getsize(probe) == 1 << 20
    os.remove(probe)
    auth = copy_out(sup, key, "auth.json")
    statuses, left = cleanup_job(sup, key)
    fs = os.statvfs(HOMES)
    check(
        "codex_home_exhaustion_fails_job_cleanly",
        report.get("bytes_error") == "ENOSPC"
        and report.get("files_error") == "ENOSPC"
        and runner_ok
        and auth[0] == 0
        and statuses == [0] * 5
        and not left
        and fs.f_bfree > fs.f_blocks * 0.9,
        f"agent: {report}; runner wrote 1 MiB to its state dir={runner_ok}; auth.json copy-out={auth[0]}; "
        f"cleanup={statuses}; codex-homes free after cleanup={fs.f_bfree}/{fs.f_blocks} blocks",
    )


def sweep_failure_scenario(sup):
    """Run by run.sh in a second container with fault injection: the failing sweep must end the container."""
    key = "fail-1"
    prepare_job(sup, key, {})
    spawn(sup, ["/bin/sleep", "1000"], key)
    print("SCENARIO sweep-failure: sweep requested", flush=True)
    reply = sup.call("sweep")
    print(f"CHECK sweep_failure_terminates_container FAIL the supervisor answered {reply}", flush=True)
    raise SystemExit(1)


def closed_streams_job(sup):
    """An agent command closes stdout and stderr, ignores SIGTERM and keeps running: EOF arrives at once, so only a
    deadline covering the wait reply as well ends it (sweep)."""
    key = "linger-1"
    prepare_job(sup, key, {})
    proc = spawn(sup, [PY, AGENT, "linger"], key)
    time.sleep(0.5)
    signalled = signal_group(sup, proc.pid, 15).get("signalled")
    started = time.monotonic()
    result = finish(sup, proc, 2)
    elapsed = time.monotonic() - started
    check(
        "closed_streams_sigterm_ignored_hits_deadline_then_swept",
        result.timed_out and result.status == -9 and result.sweep.get("ok") and not agent_threads() and elapsed < 10,
        f"SIGTERM delivered to {signalled} thread group(s), ignored; state at the 2 s deadline="
        f"{result.state_at_timeout!r} status={result.status} returned after {elapsed:.2f} s",
    )
    cleanup_job(sup, key)


def zombie_leader_job(sup):
    """A process whose main thread called pthread_exit() while another thread runs must be swept too."""
    key = "zombie-1"
    prepare_job(sup, key, {})
    proc = spawn(sup, [PY, AGENT, "zombie-leader"], key, keep_stdin=True)
    leader = json.loads(proc.line(20) or "{}").get("leader")
    time.sleep(0.5)
    leader_state = processes().get(leader, (None, "gone"))[1]
    before = [t for t in agent_threads() if t[0] == leader]
    swept = kill_all(sup)
    time.sleep(0.3)
    after = agent_threads()
    check(
        "zombie_leader_thread_group_swept",
        leader_state == "Z" and len(before) >= 1 and swept.get("ok") and not after,
        f"leader {leader} state={leader_state!r} live threads before={before} sweep={swept} "
        f"live uid-agent threads after={after}",
    )
    cleanup_job(sup, key)


def main():
    sup = Supervisor()
    if os.environ.get("R1_SCENARIO") == "sweep-failure":
        sweep_failure_scenario(sup)
    assert JOB_TOKEN  # held in memory for the /proc/<runner>/mem read attempt
    sup_pid, me = identity_checks()
    layout()
    key, home = stand_in_job(sup, sup_pid, me)
    copy_back_checks(sup, key, home)
    zombie_leader_job(sup)
    closed_streams_job(sup)
    hostile_job(sup, sup_pid)
    pid_exhaustion_job(sup)
    codex_home_exhaustion_job(sup)
    if os.environ.get("R1_SKIP_CODEX") != "1":
        real_codex_job(sup, sup_pid)
    print(f"RESULT pass={sum(results)} fail={len(results) - sum(results)}", flush=True)
    raise SystemExit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
