"""Host-side part of schema-smoke.sh and verify-comments-smoke.sh: judge each job's output, measure it and archive a
sanitised copy of the run.

The job directories are bind-mounted into the agent container, so the agent can plant symlinks there. Every
read walks the path component by component with O_NOFOLLOW, accepts only expected names and regular files,
and caps the size: nothing outside the run directory can reach the published archive (CWE-59).
"""
import json
import os
import re
import stat
import sys
from pathlib import Path

from jsonschema import Draft202012Validator

MAX_BYTES = 8 * 1024 * 1024
JOB_FILES = ("output.json", "stdout.jsonl", "stderr.txt", "rc", "credential.txt", "prompt.txt")
KIND = re.compile(r"^[a-z_]{1,32}$")
PATTERNS = [(re.compile(r"eyJ[A-Za-z0-9_-]{10,}(\.[A-Za-z0-9_-]+)*"), "<jwt-redacted>"),
            (re.compile(r"user-[A-Za-z0-9]{10,}"), "user-<redacted>"),
            (re.compile(r'"(account_id|user_id|email|credits|balance)"\s*:\s*"[^"]*"'), r'"\1": "<redacted>"'),
            (re.compile(r"[\w.+-]+@[\w-]+\.[\w.-]+"), "<email-redacted>")]


class NotRegular(Exception):
    """A path component is a symlink, missing, or not the expected file type."""


def read_safe(root: Path, *parts: str) -> str:
    """Reads `root/parts...` without following any symlink below `root`; regular files only."""
    fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY)
    try:
        for part in parts[:-1]:
            try:
                nxt = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
            except OSError as error:
                raise NotRegular(part) from error
            os.close(fd)
            fd = nxt
        try:
            file_fd = os.open(parts[-1], os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=fd)
        except OSError as error:
            raise NotRegular(parts[-1]) from error
    finally:
        os.close(fd)
    with os.fdopen(file_fd, "rb") as handle:
        info = os.fstat(handle.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_size > MAX_BYTES:
            raise NotRegular(parts[-1])
        return handle.read(MAX_BYTES).decode("utf-8", errors="replace")


def job_names(work: Path) -> list[str]:
    jobs = work / "jobs"
    if not jobs.is_dir() or jobs.is_symlink():
        return []
    return sorted(e.name for e in os.scandir(jobs) if KIND.match(e.name) and e.is_dir(follow_symlinks=False))


def verdict(schema_path: Path, job_dir: Path) -> str:
    schema = json.loads(schema_path.read_text())
    try:
        text = read_safe(job_dir, "output.json")
    except NotRegular:
        return "no output: output.json is not a regular file"
    for name in ("stdout.jsonl", "stderr.txt"):
        try:
            if "invalid_json_schema" in read_safe(job_dir, name):
                return "schema rejected by the provider"
        except NotRegular:
            pass
    try:
        output = json.loads(text)
    except ValueError:
        return "no JSON output"
    errors = [e.message for e in Draft202012Validator(schema).iter_errors(output)]
    return "valid" if not errors else "invalid: " + "; ".join(errors[:3])


def credential(job_dir: Path) -> str:
    try:
        return read_safe(job_dir, "credential.txt").strip().splitlines()[0][:120]
    except (NotRegular, IndexError):
        return "credential state unknown"


def measures(job_dir: Path) -> dict:
    """Token usage of the last `turn.completed` event, the output's item count and size."""
    found: dict = {}
    try:
        for line in read_safe(job_dir, "stdout.jsonl").splitlines():
            try:
                event = json.loads(line)
            except ValueError:
                continue
            usage = event.get("usage") if isinstance(event, dict) else None
            if isinstance(usage, dict) and event.get("type") == "turn.completed":
                found = {k: v for k, v in usage.items() if isinstance(v, int)}
    except NotRegular:
        pass
    found.update(items=None, output_bytes=None)
    try:
        text = read_safe(job_dir, "output.json")
        found["output_bytes"] = len(text.encode())
        output = json.loads(text)
    except (NotRegular, ValueError):
        return found
    if isinstance(output, dict) and isinstance(output.get("items"), list):
        found["items"] = len(output["items"])
    return found


def sanitise(text: str) -> str:
    for rx, rep in PATTERNS:
        text = rx.sub(rep, text)
    return text


def archive(work: Path, dst: Path) -> None:
    wanted = [("summary.txt",)] + [("in", e.name) for e in os.scandir(work / "in")
                                   if e.is_file(follow_symlinks=False) and e.name != "auth.json"]
    wanted += [("jobs", kind, name) for kind in job_names(work) for name in JOB_FILES]
    for parts in wanted:
        try:
            text = read_safe(work, *parts)
        except NotRegular:
            continue
        target = dst.joinpath(*parts)
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(sanitise(text))


def main(argv: list[str]) -> int:
    if len(argv) == 4 and argv[1] == "verdict":
        print(verdict(Path(argv[2]), Path(argv[3])))
    elif len(argv) == 3 and argv[1] == "credential":
        print(credential(Path(argv[2])))
    elif len(argv) == 3 and argv[1] == "measures":
        print(json.dumps(measures(Path(argv[2]))))
    elif len(argv) == 4 and argv[1] == "archive":
        archive(Path(argv[2]), Path(argv[3]))
    else:
        print("usage: schema-smoke-report.py verdict SCHEMA JOB_DIR | credential JOB_DIR | measures JOB_DIR"
              " | archive WORK DST",
              file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
