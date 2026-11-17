#!/usr/bin/env python3
"""R1 prototype of the runner supervisor (ADR-013). Test only; production is a Rust binary (WP3-RUN).

serve RUNNER_ARGV...   root entrypoint (caps SETUID/SETGID/CHOWN only): forks the runner as uid `runner` with an
                       inherited SOCK_SEQPACKET control socket (fd in SR_SUPERVISOR_FD) and serves its requests.
Requests (JSON, one per datagram), the whole privileged surface:
  spawn{argv, env, cwd} + 3 fds (stdin, stdout, stderr) -> {pid}: new session, uid/gid agent, no groups, exact env.
  wait{pid} -> {status} once reaped.
  handover{path} -> chown an empty runner-owned 0700 dir `<state>/codex-homes/<key>` or `<work>/<key>/{home,tmp}`
  to agent.
Helpers, run by the runner as uid `agent` through `spawn` (refused as root):
  sweep                  kill(-1, SIGKILL) until /proc shows no other uid-agent process; JSON {rounds, survivors}.
  signal PGID SIG        kill(-PGID, SIG).
  copy-in ROOT REL       stdin -> ROOT/REL (O_EXCL, 0600, parents 0700), no symlink anywhere below ROOT.
  copy-out ROOT REL MAX  ROOT/REL -> stdout: regular file, one link, at most MAX bytes, no symlink below ROOT.
  list ROOT              JSON list of the entries below ROOT (symlinks not followed).
  clear ROOT             removes everything below ROOT (symlinks not followed); ROOT itself stays.
"""

import ctypes
import errno
import json
import os
import pwd
import re
import select
import shutil
import signal
import socket
import stat
import struct
import sys
import time

PR_SET_CHILD_SUBREAPER = 36
SYS_OPENAT2 = 437
RESOLVE_NO_XDEV, RESOLVE_NO_MAGICLINKS, RESOLVE_NO_SYMLINKS, RESOLVE_BENEATH = 0x01, 0x02, 0x04, 0x08
STRICT = RESOLVE_NO_XDEV | RESOLVE_NO_MAGICLINKS | RESOLVE_NO_SYMLINKS | RESOLVE_BENEATH
NAME = re.compile(r"^[A-Za-z0-9._-]+$")

_libc = ctypes.CDLL(None, use_errno=True)


def openat2(dirfd, path, flags, mode=0, resolve=STRICT):
    how = struct.pack("QQQ", flags | os.O_CLOEXEC, mode, resolve)
    fd = _libc.syscall(SYS_OPENAT2, dirfd, os.fsencode(path), how, len(how))
    if fd < 0:
        err = ctypes.get_errno()
        raise OSError(err, os.strerror(err), path)
    return fd


def user(name):
    entry = pwd.getpwnam(name)
    return entry.pw_uid, entry.pw_gid


def drop_to(uid, gid):
    """setgroups/setresgid/setresuid: root loses every capability on the uid change (no KEEPCAPS)."""
    os.setgroups([])
    os.setresgid(gid, gid, gid)
    os.setresuid(uid, uid, uid)
    if os.getresuid() != (uid, uid, uid) or os.getresgid() != (gid, gid, gid):
        raise PermissionError("uid switch incomplete")


def run_as(ids, fn):
    """Runs fn() in a forked child with the given uid; returns its JSON result."""
    rfd, wfd = os.pipe2(os.O_CLOEXEC)
    pid = os.fork()
    if pid == 0:
        try:
            os.close(rfd)
            drop_to(*ids)
            out = json.dumps(fn()).encode()
            os.write(wfd, out)
            os._exit(0)
        except BaseException as error:  # noqa: BLE001 - reported to the parent
            os.write(wfd, json.dumps({"error": repr(error)}).encode())
            os._exit(1)
    os.close(wfd)
    chunks = []
    while chunk := os.read(rfd, 65536):
        chunks.append(chunk)
    os.close(rfd)
    os.waitpid(pid, 0)
    return json.loads(b"".join(chunks) or b"{}")


def proc_entries():
    """(pid, real uid, state) of every process visible in /proc."""
    for name in os.listdir("/proc"):
        if not name.isdigit():
            continue
        try:
            with open(f"/proc/{name}/status") as f:
                fields = dict(line.split(":", 1) for line in f if ":" in line)
            yield int(name), int(fields["Uid"].split()[0]), fields["State"].split()[0]
        except (OSError, KeyError, ValueError):
            continue


def sweep(agent_uid):
    """As uid agent: kill(-1, SIGKILL) hits exactly the processes of this uid except the caller; repeat until
    /proc shows no live one. The kernel fails a fork racing with the pending SIGKILL, so one round normally
    suffices; the loop is the proof."""
    me = os.getpid()
    alive = []
    for rounds in range(1, 101):
        try:
            os.kill(-1, signal.SIGKILL)
        except ProcessLookupError:
            pass
        alive = [p for p, uid, state in proc_entries() if uid == agent_uid and p != me and state not in "ZX"]
        if not alive:
            return {"rounds": rounds, "survivors": []}
        time.sleep(0.02)
    return {"rounds": 100, "survivors": alive}


class Supervisor:
    def __init__(self, runner_argv):
        self.runner_ids = user("runner")
        self.agent_ids = user("agent")
        self.state_dir = os.environ.get("SR_RUNNER_STATE_DIR", "/var/lib/smart-review-runner")
        self.codex_homes = os.path.join(self.state_dir, "codex-homes")
        self.work_dir = os.environ.get("SR_RUNNER_WORK_DIR", "/work")
        self.spawned = {}  # pid -> exit status or None
        self.waiters = {}  # pid -> [request id]
        self.runner_argv = runner_argv

    def start(self):
        if os.geteuid() != 0:
            sys.exit("supervisor: must start as root")
        _libc.prctl(PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0)
        for root in (self.state_dir, self.codex_homes, self.work_dir):
            st = os.lstat(root)
            if stat.S_ISDIR(st.st_mode) and st.st_uid != self.runner_ids[0]:
                os.lchown(root, *self.runner_ids)  # fresh volume roots; modes are the runner's job
        self.sock, child_end = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
        self.runner_pid = os.fork()
        if self.runner_pid == 0:
            try:
                self.sock.close()
                drop_to(*self.runner_ids)
                os.set_inheritable(child_end.fileno(), True)
                env = dict(os.environ, SR_SUPERVISOR_FD=str(child_end.fileno()))
                os.execve(self.runner_argv[0], self.runner_argv, env)
            except BaseException:  # noqa: BLE001 - the child must never return into the supervisor
                os._exit(127)
        child_end.close()
        self.wake_r, wake_w = os.pipe2(os.O_NONBLOCK | os.O_CLOEXEC)
        signal.set_wakeup_fd(wake_w)
        signal.signal(signal.SIGCHLD, lambda *_: None)
        signal.signal(signal.SIGTERM, lambda *_: run_as(self.runner_ids, lambda: os.kill(self.runner_pid, 15)))

    def loop(self):
        poller = select.poll()
        poller.register(self.sock, select.POLLIN)
        poller.register(self.wake_r, select.POLLIN)
        while True:
            for fd, _ in poller.poll():
                if fd == self.wake_r:
                    try:
                        os.read(self.wake_r, 4096)
                    except BlockingIOError:
                        pass
                elif fd == self.sock.fileno():
                    self.handle()
            status = self.reap()
            if status is not None:
                run_as(self.agent_ids, lambda: sweep(self.agent_ids[0]))
                os._exit(os.waitstatus_to_exitcode(status) & 0xFF)

    def reap(self):
        runner_status = None
        while True:
            try:
                pid, status = os.waitpid(-1, os.WNOHANG)
            except ChildProcessError:
                return runner_status
            if pid == 0:
                return runner_status
            if pid == self.runner_pid:
                runner_status = status
            elif pid in self.spawned:
                self.spawned[pid] = os.waitstatus_to_exitcode(status)
                for rid in self.waiters.pop(pid, []):
                    self.reply(rid, status=self.spawned[pid])

    def reply(self, rid, ok=True, **fields):
        try:
            self.sock.send(json.dumps({"id": rid, "ok": ok, **fields}).encode())
        except BrokenPipeError:
            pass

    def handle(self):
        try:
            data, fds, _, _ = socket.recv_fds(self.sock, 1 << 20, 3)
        except ConnectionResetError:
            data, fds = b"", []
        if not data:  # runner closed the socket; its exit is reaped next
            time.sleep(0.05)
            return
        request = json.loads(data)
        rid, op = request.get("id"), request.get("op")
        try:
            if op == "spawn":
                self.reply(rid, pid=self.spawn(request, fds))
            elif op == "wait":
                pid = request["pid"]
                if pid not in self.spawned:
                    raise ValueError("not a spawned pid")
                if self.spawned[pid] is None:
                    self.waiters.setdefault(pid, []).append(rid)
                else:
                    self.reply(rid, status=self.spawned[pid])
            elif op == "handover":
                self.handover(request["path"])
                self.reply(rid)
            else:
                raise ValueError(f"unknown op {op!r}")
        except (OSError, ValueError, KeyError, TypeError) as error:
            self.reply(rid, ok=False, error=repr(error))
        finally:
            for fd in fds:
                os.close(fd)

    def spawn(self, request, fds):
        argv, env, cwd = request["argv"], request["env"], request["cwd"]
        if len(fds) != 3 or not argv or not os.path.isabs(argv[0]) or not os.path.isabs(cwd):
            raise ValueError("spawn needs 3 fds, an absolute program and an absolute cwd")
        env = dict(entry.split("=", 1) for entry in env)
        pid = os.fork()
        if pid == 0:
            try:
                signal.set_wakeup_fd(-1)
                os.setsid()
                drop_to(*self.agent_ids)
                os.umask(0o077)
                os.chdir(cwd)
                high = [os.dup(fd) for fd in fds]
                for target, fd in enumerate(high):
                    os.dup2(fd, target, inheritable=True)
                os.closerange(3, 65536)
                for sig in range(1, signal.NSIG):
                    try:
                        signal.signal(sig, signal.SIG_DFL)
                    except (OSError, ValueError):
                        pass
                signal.pthread_sigmask(signal.SIG_SETMASK, [])
                os.execve(argv[0], argv, env)
            except BaseException:  # noqa: BLE001 - the child must never return into the supervisor
                os._exit(127)
        self.spawned[pid] = None
        return pid

    def handover(self, path):
        parent, name = os.path.split(path)
        allowed = parent == self.codex_homes or (os.path.dirname(parent) == self.work_dir and name in ("home", "tmp"))
        if os.path.normpath(path) != path or not allowed or not NAME.match(name):
            raise ValueError("not a per-job directory")
        fd = openat2(-100, path, os.O_PATH | os.O_DIRECTORY | os.O_NOFOLLOW, resolve=RESOLVE_NO_SYMLINKS)
        try:
            st = os.fstat(fd)
            if st.st_uid != self.runner_ids[0] or stat.S_IMODE(st.st_mode) != 0o700:
                raise ValueError("not a runner-owned 0700 directory")
            seen = run_as(self.runner_ids, lambda: _inspect_dir(path))  # root cannot list a 0700 directory
            if seen.get("ino") != [st.st_dev, st.st_ino] or not seen.get("empty"):
                raise ValueError("directory not empty or replaced")
            os.chown(f"/proc/self/fd/{fd}", *self.agent_ids)
        finally:
            os.close(fd)


def _inspect_dir(path):
    fd = openat2(-100, path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, resolve=RESOLVE_NO_SYMLINKS)
    try:
        st = os.fstat(fd)
        return {"ino": [st.st_dev, st.st_ino], "empty": not os.listdir(fd)}
    finally:
        os.close(fd)


def _walk_fd(dirfd, prefix, out):
    for name in os.listdir(dirfd):
        st = os.stat(name, dir_fd=dirfd, follow_symlinks=False)
        rel = f"{prefix}{name}"
        out.append({"path": rel, "mode": oct(st.st_mode), "size": st.st_size})
        if stat.S_ISDIR(st.st_mode):
            sub = openat2(dirfd, name, os.O_RDONLY | os.O_DIRECTORY)
            try:
                _walk_fd(sub, rel + "/", out)
            finally:
                os.close(sub)


def helper(argv):
    mode = argv[0]
    if mode == "sweep":
        result = sweep(os.getuid())
        print(json.dumps(result))
        sys.exit(1 if result["survivors"] else 0)
    if mode == "signal":
        os.kill(-int(argv[1]), int(argv[2]))
        return
    rootfd = os.open(argv[1], os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    if mode == "copy-in":
        parts = argv[2].split("/")
        if any(not NAME.match(p) or p in (".", "..") for p in parts):
            sys.exit("copy-in: bad path")
        dirfd = rootfd
        for part in parts[:-1]:
            try:
                os.mkdir(part, 0o700, dir_fd=dirfd)
            except FileExistsError:
                pass
            dirfd = openat2(dirfd, part, os.O_RDONLY | os.O_DIRECTORY)
        fd = openat2(dirfd, parts[-1], os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "wb") as out:
            shutil.copyfileobj(sys.stdin.buffer, out)
    elif mode == "copy-out":
        limit = int(argv[3])
        try:
            fd = openat2(rootfd, argv[2], os.O_RDONLY | os.O_NONBLOCK)
        except OSError as error:
            print(f"copy-out: open refused: {errno.errorcode.get(error.errno, error.errno)}", file=sys.stderr)
            sys.exit(2)
        st = os.fstat(fd)
        if not stat.S_ISREG(st.st_mode) or st.st_nlink != 1:
            print("copy-out: not a regular file with one link", file=sys.stderr)
            sys.exit(3)
        data = b""
        while st.st_size <= limit and len(data) <= limit and (chunk := os.read(fd, limit + 1 - len(data))):
            data += chunk
        if st.st_size > limit or len(data) > limit:
            print("copy-out: larger than the limit", file=sys.stderr)
            sys.exit(4)
        sys.stdout.buffer.write(data)
    elif mode == "list":
        out = []
        _walk_fd(rootfd, "", out)
        print(json.dumps(out))
    elif mode == "clear":
        for name in os.listdir(rootfd):
            st = os.stat(name, dir_fd=rootfd, follow_symlinks=False)
            if stat.S_ISDIR(st.st_mode):
                shutil.rmtree(name, dir_fd=rootfd)  # fd-based, does not follow symlinks
            else:
                os.unlink(name, dir_fd=rootfd)
    else:
        sys.exit(f"unknown helper {mode}")


def main():
    if len(sys.argv) >= 3 and sys.argv[1] == "serve":
        supervisor = Supervisor(sys.argv[2:])
        supervisor.start()
        supervisor.loop()
    elif len(sys.argv) >= 2:
        if os.geteuid() == 0:
            sys.exit("helpers never run as root")
        helper(sys.argv[1:])
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
