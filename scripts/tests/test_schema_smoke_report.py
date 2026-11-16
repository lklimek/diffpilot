"""schema-smoke-report.py must never read through agent-writable symlinks (container -> host file disclosure)."""
import json
import os
import subprocess
import sys
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "schema-smoke-report.py"
SECRET = "SYNTHETIC-HOST-SECRET-d41d8cd9"
SCHEMA = {"type": "object", "properties": {"answer": {"type": "string"}}, "required": ["answer"],
          "additionalProperties": False}


def run(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run([sys.executable, "-I", str(SCRIPT), *args], capture_output=True, text=True, check=False)


def make_run(tmp_path: Path) -> tuple[Path, Path, Path]:
    secret = tmp_path / "host-secret.txt"
    secret.write_text(SECRET)
    work = tmp_path / "work"
    job = work / "jobs" / "ask"
    job.mkdir(parents=True)
    (work / "in").mkdir()
    schema = work / "in" / "ask.schema.json"
    schema.write_text(json.dumps(SCHEMA))
    (job / "stdout.jsonl").write_text('{"type":"turn.completed"}\n')
    return work, job, secret


def test_archive_skips_symlinks_to_host_files(tmp_path):
    work, job, secret = make_run(tmp_path)
    (job / "output.json").write_text(json.dumps({"answer": "x"}))
    os.symlink(secret, job / "stderr.txt")
    os.symlink(secret, job / "leak.txt")
    dst = tmp_path / "archive"
    result = run("archive", str(work), str(dst))
    assert result.returncode == 0, result.stderr
    archived = [p for p in dst.rglob("*") if p.is_file()]
    assert archived, "regular files are archived"
    for path in archived:
        assert SECRET not in path.read_text(), path
    assert not (dst / "jobs" / "ask" / "leak.txt").exists(), "unexpected names are not archived"
    assert (dst / "jobs" / "ask" / "stdout.jsonl").exists()


def test_archive_skips_symlinked_job_directory(tmp_path):
    work, _, secret = make_run(tmp_path)
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "output.json").write_text(SECRET)
    os.symlink(outside, work / "jobs" / "plan")
    dst = tmp_path / "archive"
    assert run("archive", str(work), str(dst)).returncode == 0
    for path in dst.rglob("*"):
        if path.is_file():
            assert SECRET not in path.read_text(), path


def test_verdict_does_not_follow_a_symlinked_output(tmp_path):
    work, job, _ = make_run(tmp_path)
    valid_outside = tmp_path / "valid.json"
    valid_outside.write_text(json.dumps({"answer": "from the host"}))
    os.symlink(valid_outside, job / "output.json")
    result = run("verdict", str(work / "in" / "ask.schema.json"), str(job))
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() != "valid"
    assert "not a regular file" in result.stdout


def test_verdict_of_a_regular_valid_output(tmp_path):
    work, job, _ = make_run(tmp_path)
    (job / "output.json").write_text(json.dumps({"answer": "x"}))
    assert run("verdict", str(work / "in" / "ask.schema.json"), str(job)).stdout.strip() == "valid"
