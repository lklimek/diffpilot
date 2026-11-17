#!/usr/bin/env python3
"""Stand-in runner for the R1 container test (ADR-013). Runs as uid `runner` under supervisor.py, drives a
stand-in job and a real-Codex job through the supervisor protocol and prints `CHECK <name> PASS|FAIL <detail>`.
Canary values start with the prefix C; SR_RUNNER_SECRET is one (set by run.sh)."""

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
CAPS_THREE = "00000000000000c1"  # CHOWN | SETGID | SETUID
ZERO = "0000000000000000"
C = "".join(("SR", "CANARY"))  # split so that this file itself is no canary hit
JOB_TOKEN = f"{C}-job-token-" + secrets.token_hex(8)  # lives only in this process's memory
MASTER = os.path.join(STATE, "credentials/default/auth.json")
THREAD_A, THREAD_B = "019a0000-aaaa-7000-8000-00000000000a", "019a0000-bbbb-7000-8000-00000000000b"
ROLLOUT = "sessions/2026/11/17/rollout-2026-11-17T00-00-00-{}.jsonl"
COPY_CAP = 1 << 20

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


def agent_live():
    return {p: v for p, v in processes().items() if v[0] == AGENT_UID and v[1] not in "ZX"}


class Supervisor:
    """Client of the supervisor protocol over the inherited SOCK_SEQPACKET socket."""

    def __init__(self):
        self.sock = socket.socket(fileno=int(os.environ["SR_SUPERVISOR_FD"]))
        self.next = 0
        self.early = {}

    def call(self, op, fds=(), **fields):
        self.next += 1
        rid = self.next
        msg = json.dumps({"id": rid, "op": op, **fields}).encode()
        socket.send_fds(self.sock, [msg], list(fds)) if fds else self.sock.send(msg)
        while rid not in self.early:
            reply = json.loads(self.sock.recv(1 << 20))
            self.early[reply["id"]] = reply
        return self.early.pop(rid)


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

    def drain(self):
        out, err = [self.buf], []
        for fd, sink in ((self.out, out), (self.err, err)):
            while chunk := os.read(fd, 65536):
                sink.append(chunk)
            os.close(fd)
        return b"".join(out), b"".join(err).decode(errors="replace").strip()


def agent_env(key):
    return [
        f"HOME={WORK}/{key}/home",
        f"TMPDIR={WORK}/{key}/tmp",
        f"CODEX_HOME={HOMES}/{key}",
        "PATH=/usr/local/bin:/usr/bin:/bin",
        "LANG=C.UTF-8",
    ]


def spawn(sup, argv, key, stdin=b"", keep_stdin=False):
    """Starts argv as uid agent via the supervisor; stdin is written and closed unless keep_stdin."""
    in_r, in_w = os.pipe()
    out_r, out_w = os.pipe()
    err_r, err_w = os.pipe()
    reply = sup.call("spawn", fds=(in_r, out_w, err_w), argv=argv, env=agent_env(key), cwd=f"{WORK}/{key}")
    for fd in (in_r, out_w, err_w):
        os.close(fd)
    if not reply["ok"]:
        raise RuntimeError(f"spawn refused: {reply}")
    if stdin:
        os.write(in_w, stdin)
    if not keep_stdin:
        os.close(in_w)
    return Proc(reply["pid"], out_r, err_r)


def run(sup, argv, key, stdin=b""):
    """(exit status, stdout bytes, stderr text) of an agent command."""
    proc = spawn(sup, argv, key, stdin)
    out, err = proc.drain()
    return sup.call("wait", pid=proc.pid)["status"], out, err


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
    statuses.append(run(sup, ["/usr/bin/ipcrm", "--all"], key)[0])
    for d in (home, f"{job}/home", f"{job}/tmp"):
        os.rmdir(d)  # empty agent-owned dir in a runner-owned parent
    os.remove(f"{job}/repo/README.md")
    os.rmdir(f"{job}/repo")
    os.rmdir(job)
    return statuses, [d for d in (home, job) if os.path.lexists(d)]


def kill_all(sup, key):
    """The sweep helper as uid agent: kill(-1, SIGKILL) reaches exactly the uid-agent processes."""
    status, out, _ = run(sup, [PY, SUP, "sweep"], key)
    return dict(json.loads(out or b"{}"), ok=status == 0)


def signal_group(sup, key, pgid, sig):
    return run(sup, [PY, SUP, "signal", str(pgid), str(sig)], key)[0]


def copy_out(sup, key, rel, cap=COPY_CAP):
    started = time.monotonic()
    rc, out, err = run(sup, [PY, SUP, "copy-out", os.path.join(HOMES, key), rel, str(cap)], key)
    return rc, out, err, time.monotonic() - started


def identity_checks():
    sup_pid = os.getppid()
    s, me = status(sup_pid), status("self")
    sup_ok = (
        s["Uid"].split()[0] == "0"
        and s["CapEff"] == s["CapPrm"] == s["CapBnd"] == CAPS_THREE
        and s["CapAmb"] == ZERO
        and s["NoNewPrivs"] == "1"
        and s["Seccomp"] == "2"
    )
    check(
        "supervisor_caps_only_three",
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
        f"NoNewPrivs={ident.get('NoNewPrivs')}",
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
    signal_group(sup, key, probe.pid, 15)
    probe_status = sup.call("wait", pid=probe.pid)["status"]
    time.sleep(0.5)
    after_group_kill = agent_live()
    check(
        "process_group_kill_misses_escapees",
        len(after_group_kill) >= 4,
        f"left_behind={left is not None} agent processes before={len(before)} probe_status={probe_status} "
        f"alive after SIGTERM to the group={len(after_group_kill)}: "
        f"{sorted(set(v[2] for v in after_group_kill.values()))}",
    )
    killed = kill_all(sup, key)
    time.sleep(0.3)
    remaining = {p: v for p, v in processes().items() if v[0] == AGENT_UID}
    check(
        "agent_processes_killed_at_job_end",
        killed["ok"] and not remaining and os.path.exists(f"/proc/{sup_pid}"),
        f"kill_all={ {k: killed.get(k) for k in ('ok', 'rounds', 'survivors')} } uid-agent processes left="
        f"{len(remaining)} supervisor alive={os.path.exists(f'/proc/{sup_pid}')}",
    )
    return key, home


def copy_back_checks(sup, key, home):
    plant_args = {
        "codex_home": home,
        "master_auth": MASTER,
        "foreign_session": os.path.join(STATE, f"sessions/{THREAD_B}/rollout-b.jsonl"),
    }
    rc, out, err = run(sup, [PY, AGENT, "plant"], key, (json.dumps(plant_args) + "\n").encode())
    planted = json.loads(out or b"{}")
    kill_all(sup, key)
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
    cat_rc, cat_out, cat_err = run(sup, ["/bin/cat", os.path.join(home, "auth.json")], key)
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
    rollout = None
    while time.monotonic() < deadline:
        line = codex.line(2)
        if line and thread is None and '"thread.started"' in line:
            thread = json.loads(line)["thread_id"]
        seen |= {v[2] for v in processes().values() if v[0] == AGENT_UID}
        cmds = " | ".join(v[2] for v in agent_live().values())
        if thread and "github-mcp-server" in cmds and "sleep 1010" in cmds and rollout is None:
            rc, out, _ = run(sup, [PY, SUP, "list", home], key)
            names = [e["path"] for e in json.loads(out or b"[]")]
            pattern = re.compile(rf"^sessions/\d{{4}}/\d{{2}}/\d{{2}}/rollout-[0-9T:-]+-{re.escape(thread)}\.jsonl$")
            rollout = next((n for n in names if pattern.match(n)), None)
        if rollout:
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
    rc, out, _ = run(sup, [PY, AGENT, "environ-scan", C], key)
    scan = json.loads(out or b"{}")
    check(
        "real_codex_env_has_no_runner_secret",
        codex.pid in scan.get("readable", []) and not scan.get("hits"),
        f"codex pid {codex.pid} environ readable to agent={codex.pid in scan.get('readable', [])} "
        f"environs with the canary={scan.get('hits')}",
    )
    signal_group(sup, key, codex.pid, 15)
    codex_status = sup.call("wait", pid=codex.pid)["status"]
    time.sleep(0.5)
    survivors = sorted(set(v[2] for v in agent_live().values()))
    check(
        "real_codex_escapee_survives_group_kill",
        any("sleep 1010" in c for c in survivors),
        f"codex status={codex_status}; alive after SIGTERM to Codex's group: {survivors}",
    )
    killed = kill_all(sup, key)
    time.sleep(0.3)
    left = [p for p, v in processes().items() if v[0] == AGENT_UID]
    check("real_codex_processes_killed", killed["ok"] and not left, f"rounds={killed.get('rounds')} left={left}")
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


def main():
    sup = Supervisor()
    assert JOB_TOKEN  # held in memory for the /proc/<runner>/mem read attempt
    sup_pid, me = identity_checks()
    layout()
    key, home = stand_in_job(sup, sup_pid, me)
    copy_back_checks(sup, key, home)
    if os.environ.get("R1_SKIP_CODEX") != "1":
        real_codex_job(sup, sup_pid)
    print(f"RESULT pass={sum(results)} fail={len(results) - sum(results)}", flush=True)
    raise SystemExit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
