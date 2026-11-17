#!/usr/bin/env python3
"""Stand-in agent for the R1 container test (ADR-013): what a hostile Codex command could try as uid `agent`.

Arguments arrive as one JSON line on stdin (never argv: /proc/*/cmdline is world-readable).
probe             one JSON line of findings on stdout, then leaves processes behind (see `leave_behind`) and
                  stays alive as the group leader, like Codex.
plant             plants copy-back traps in the per-job CODEX_HOME; one JSON line of what was planted.
environ-scan PREFIX  JSON: which /proc/*/environ were readable and which contained PREFIX.
hostile           ignores SIGTERM, starts a fork loop, tries to SIGSTOP runner and supervisor (one JSON line), then
                  SIGSTOPs every other process of its uid in a tight loop.
fill              fills $CODEX_HOME: one file until ENOSPC, then empty files until ENOSPC; one JSON line.
"""

import errno
import json
import os
import signal
import subprocess
import sys
import time


def status(pid="self"):
    with open(f"/proc/{pid}/status") as f:
        return {k: v.strip() for k, v in (line.split(":", 1) for line in f if ":" in line)}


def try_read(path):
    """'denied: <errno>' or 'READ <n> bytes' (a read is a finding)."""
    try:
        if os.path.isdir(path):
            return f"READ dir {len(os.listdir(path))} entries"
        with open(path, "rb") as f:
            return f"READ {len(f.read(4096))} bytes"
    except OSError as error:
        return f"denied: {error.strerror}"


def walk_all():
    """os.walk from / plus the agent's own dirs, which sit below 0711 parents that / cannot list."""
    for root in ("/", os.environ["CODEX_HOME"], os.environ["HOME"], os.environ["TMPDIR"]):
        yield from os.walk(root, onerror=lambda _: None)


def canary_sweep(prefix):
    """Every readable regular file outside /proc, /sys, /dev, plus every readable /proc/*/environ and cmdline."""
    hits, scanned = set(), 0
    needle = prefix.encode()
    for top, dirs, files in walk_all():
        if top in ("/proc", "/sys", "/dev"):
            dirs[:] = []
            continue
        for name in files:
            path = os.path.join(top, name)
            try:
                if not os.path.isfile(path) or os.path.islink(path):
                    continue
                with open(path, "rb") as f:
                    scanned += 1
                    if needle in f.read(8 << 20):
                        hits.add(path)
            except OSError:
                continue
    for pid in filter(str.isdigit, os.listdir("/proc")):
        for leaf in ("environ", "cmdline"):
            try:
                with open(f"/proc/{pid}/{leaf}", "rb") as f:
                    if needle in f.read():
                        hits.add(f"/proc/{pid}/{leaf}")
            except OSError:
                continue
    return {"hits": sorted(hits), "files_scanned": scanned}


def writable_paths():
    """Directories and regular files the agent may write, outside /proc and /sys (devices excluded)."""
    found = set()
    for top, dirs, files in walk_all():
        if top in ("/proc", "/sys"):
            dirs[:] = []
            continue
        if os.access(top, os.W_OK):
            found.add(top)
            dirs[:] = []  # everything below a writable directory is the agent's anyway
            continue
        for name in files:
            path = os.path.join(top, name)
            if os.path.isfile(path) and not os.path.islink(path) and os.access(path, os.W_OK):
                found.add(path)
    return sorted(found)


def open_fds():
    """Inherited descriptors (the listing's own directory fd excluded)."""
    links = {}
    for fd in os.listdir("/proc/self/fd"):
        try:
            links[fd] = os.readlink(f"/proc/self/fd/{fd}")
        except OSError:
            continue  # the listing's own fd, closed by now
    return {fd: t for fd, t in links.items() if t != f"/proc/{os.getpid()}/fd"}


def run(argv):
    result = subprocess.run(argv, capture_output=True, text=True)
    return {"rc": result.returncode, "out": (result.stdout + result.stderr).strip()[-200:]}


def leave_behind():
    """What a hostile command leaves: same-group child, setsid double-fork daemon, SIGTERM-ignoring and SIGSTOPped
    processes in a new session, a fork loop in a new session, a POSIX message queue and a SysV shm segment."""
    subprocess.Popen(["/bin/sleep", "1001"])
    subprocess.Popen(["/bin/sh", "-c", "(setsid /bin/sleep 1002 &) ; exit 0"]).wait()
    subprocess.Popen(["/bin/sh", "-c", "trap '' TERM HUP; exec /bin/sleep 1003"], start_new_session=True)
    stopped = subprocess.Popen(["/bin/sleep", "1004"], start_new_session=True)
    os.kill(stopped.pid, signal.SIGSTOP)
    subprocess.Popen(
        ["/bin/sh", "-c", "trap '' TERM; while :; do /bin/sleep 1005 & sleep 0.05; done"], start_new_session=True
    )
    import ctypes  # late: ctypes keeps a libffi descriptor open, which would blur the inherited-fd check

    libc = ctypes.CDLL(None, use_errno=True)
    libc.mq_open(b"/sr-left-behind", os.O_CREAT | os.O_RDWR, 0o600, None)  # IPC objects outlive processes
    libc.shmget(0x5352, 4096, 0o1000 | 0o600)


def probe(args):
    me = status()
    findings = {
        "identity": {
            k: me[k] for k in ("Uid", "Gid", "Groups", "CapInh", "CapPrm", "CapEff", "CapAmb", "NoNewPrivs", "Seccomp")
        },
        "apparmor": open("/proc/self/attr/current").read().strip(),
        "oom_score_adj": open("/proc/self/oom_score_adj").read().strip(),
        "env_keys": sorted(os.environ),
        "reads": {path: try_read(path) for path in args["deny"]},
        "fds": open_fds(),
        "signal": {},
        "escalation": {
            "setuid_binary_euid": run([args["setuid_probe"], "-u"]),
            "setpriv_root": run(["/usr/bin/setpriv", "--reuid=0", "--regid=0", "--clear-groups", "/bin/true"]),
            "setpriv_runner": run(["/usr/bin/setpriv", "--reuid=10001", "/bin/true"]),
            "unshare_userns": run(["/usr/bin/unshare", "--user", "--map-root-user", "/bin/true"]),
        },
        "canaries": canary_sweep(args["canary_prefix"]),
        "writable": writable_paths(),
        "own_files": {
            path: oct(os.stat(path).st_mode & 0o7777) + f" uid={os.stat(path).st_uid}" for path in args["own"]
        },
    }
    for name, pid in (("runner", args["runner_pid"]), ("supervisor", args["supervisor_pid"]), ("pid1", 1)):
        try:
            os.kill(pid, 0)
            findings["signal"][name] = "SENT"
        except OSError as error:
            findings["signal"][name] = f"denied: {error.strerror}"
    print(json.dumps(findings), flush=True)
    leave_behind()
    print(json.dumps({"left_behind": True}), flush=True)
    while True:
        time.sleep(60)


def plant(args):
    home, master, foreign = args["codex_home"], args["master_auth"], args["foreign_session"]
    day = os.path.join(home, "sessions/2026/11/17")
    os.makedirs(day, exist_ok=True)
    done = {}
    os.replace(os.path.join(home, "auth.json"), os.path.join(home, "auth.json.real"))
    os.symlink(master, os.path.join(home, "auth.json"))
    done["auth_symlink_to_master"] = "auth.json"
    os.mkfifo(os.path.join(day, "rollout-fifo-THREAD.jsonl"))
    done["fifo"] = "sessions/2026/11/17/rollout-fifo-THREAD.jsonl"
    os.symlink(os.path.dirname(foreign), os.path.join(home, "sessions/2026/11/18"))
    done["dir_symlink_to_foreign"] = "sessions/2026/11/18/" + os.path.basename(foreign)
    with open(os.path.join(day, "rollout-big-THREAD.jsonl"), "wb") as f:
        f.write(b"x" * (2 << 20))
    done["oversize"] = "sessions/2026/11/17/rollout-big-THREAD.jsonl"
    try:
        os.link(master, os.path.join(home, "hardlink-master"))
        done["hardlink_master"] = "CREATED"
    except OSError as error:
        done["hardlink_master"] = f"denied: {error.strerror}"
    with open(os.path.join(day, "rollout-good-THREAD.jsonl"), "w") as f:
        f.write('{"type":"session_meta","payload":{"id":"THREAD"}}\n')
    done["good"] = "sessions/2026/11/17/rollout-good-THREAD.jsonl"
    with open(os.path.join(home, "linked-source"), "w") as f:
        f.write("{}\n")
    os.link(os.path.join(home, "linked-source"), os.path.join(day, "rollout-linked-THREAD.jsonl"))
    done["second_link"] = "sessions/2026/11/17/rollout-linked-THREAD.jsonl"
    done["escape"] = "../../credentials/default/auth.json"
    print(json.dumps(done), flush=True)


def hostile(args):
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    subprocess.Popen(
        ["/bin/sh", "-c", "trap '' TERM; while :; do /bin/sleep 1020 & sleep 0.05; done"], start_new_session=True
    )
    tried = {}
    for name in ("runner_pid", "supervisor_pid"):
        try:
            os.kill(args[name], signal.SIGSTOP)
            tried[name] = "SENT"
        except OSError as error:
            tried[name] = f"denied: {error.strerror}"
    print(json.dumps(tried), flush=True)
    while True:
        try:
            os.kill(-1, signal.SIGSTOP)
        except OSError:
            pass


def fill():
    home = os.environ["CODEX_HOME"]
    report = {"bytes_written": 0}
    try:
        with open(os.path.join(home, "big"), "wb") as f:
            while True:
                f.write(b"\0" * (1 << 20))
                f.flush()
                report["bytes_written"] += 1 << 20
    except OSError as error:
        report["bytes_error"] = errno.errorcode.get(error.errno, str(error.errno))
    report["files_created"] = 0
    try:
        while True:
            open(os.path.join(home, f"f{report['files_created']}"), "x").close()
            report["files_created"] += 1
    except OSError as error:
        report["files_error"] = errno.errorcode.get(error.errno, str(error.errno))
    print(json.dumps(report), flush=True)


def environ_scan(prefix):
    readable, hits = [], []
    for pid in filter(str.isdigit, os.listdir("/proc")):
        try:
            with open(f"/proc/{pid}/environ", "rb") as f:
                data = f.read()
        except OSError:
            continue
        readable.append(int(pid))
        if prefix.encode() in data:
            hits.append(int(pid))
    print(json.dumps({"readable": readable, "hits": hits}), flush=True)


if __name__ == "__main__":
    mode = sys.argv[1]
    if mode == "probe":
        probe(json.loads(sys.stdin.readline()))
    elif mode == "plant":
        plant(json.loads(sys.stdin.readline()))
    elif mode == "hostile":
        hostile(json.loads(sys.stdin.readline()))
    elif mode == "fill":
        fill()
    elif mode == "environ-scan":
        environ_scan(sys.argv[2])
