"""Worker assets use fake binaries and isolated homes; never provider auth."""

import json
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import time
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]


class WorkerAssets(unittest.TestCase):
    def app_server(self, root):
        """Stand in for app-server: a listening control socket in the runtime dir."""
        (root / "runtime").mkdir(parents=True, exist_ok=True)
        server = socket.socket(socket.AF_UNIX)
        self.addCleanup(server.close)
        server.bind(str(root / "runtime/app.sock"))
        server.listen()

    def test_job_reset_returns_only_after_app_server_listens(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            env = {
                "PATH": "/usr/bin:/bin",
                "HOME": str(root / "home"),
                "CODEX_HOME": str(root / "codex"),
                "DP_RUNTIME_DIR": str(root / "runtime"),
            }
            timer = threading.Timer(0.5, self.app_server, [root])
            timer.start()
            self.addCleanup(timer.cancel)
            started = time.monotonic()
            subprocess.run(
                ["python3", ROOT / "deploy/worker/worker-config.py", "cs1"],
                env=env,
                check=True,
                timeout=10,
            )
            self.assertGreaterEqual(time.monotonic() - started, 0.5)
            self.assertTrue((root / "runtime/job-ready").is_file())

    def test_job_reset_fails_when_app_server_never_listens(self):
        for stale in (False, True):
            with self.subTest(stale=stale), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                if stale:
                    (root / "runtime").mkdir()
                    with socket.socket(socket.AF_UNIX) as dead:
                        dead.bind(str(root / "runtime/app.sock"))
                result = subprocess.run(
                    ["python3", ROOT / "deploy/worker/worker-config.py", "cs1"],
                    env={
                        "PATH": "/usr/bin:/bin",
                        "HOME": str(root / "home"),
                        "CODEX_HOME": str(root / "codex"),
                        "DP_RUNTIME_DIR": str(root / "runtime"),
                        "DP_APP_READY_SECS": "0.3",
                    },
                    capture_output=True,
                    text=True,
                    timeout=10,
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("app-server", result.stderr)

    def test_entry_replaces_stale_config_and_pins_critical_flags(self):
        for shape in ("cs1", "cs2"):
            with self.subTest(shape=shape), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                self.app_server(root)
                codex = root / "work/codex"
                home = root / "work/home"
                codex.mkdir(parents=True)
                home.mkdir()
                (codex / "config.toml").write_text(
                    '[projects."/work/repo"]\ntrust_level="trusted"\n'
                )
                fake = root / "codex"
                fake.write_text('#!/bin/sh\nprintf "%s\\n" "$@"\n')
                fake.chmod(0o755)
                env = {
                    "PATH": f"{root}:/usr/bin:/bin",
                    "HOME": str(home),
                    "CODEX_HOME": str(codex),
                    "DP_MEMCAN_URL": "http://memcan:8190/mcp",
                    "DP_MEMCAN_TOKEN": 'fake"\\token',
                    "DP_RUNTIME_DIR": str(root / "runtime"),
                }
                # A prior job can write instructions but must not teach the next job.
                for parent in (home, codex):
                    for name in ("AGENTS.md", "AGENTS.override.md", "CLAUDE.md"):
                        (parent / name).write_text("planted instruction")
                    for name in ("skills", "prompts", ".codex", ".claude", ".agents"):
                        (parent / name).mkdir()
                        (parent / name / "planted").write_text("planted instruction")
                (codex / "sessions").mkdir()
                (codex / "sessions/thread.jsonl").write_text("thread history")
                (codex / "state_5.sqlite").write_text("thread index")
                subprocess.run(
                    ["python3", ROOT / "deploy/worker/worker-config.py", shape],
                    env=env,
                    check=True,
                )
                result = subprocess.run(
                    [ROOT / "deploy/worker/dp-worker-entry", shape],
                    env=env,
                    capture_output=True,
                    text=True,
                    check=True,
                )
                config = tomllib.loads((codex / "config.toml").read_text())
                self.assertTrue((codex / "config.toml").is_symlink())
                self.assertEqual(
                    (codex / "config.toml").resolve().parent, root / "runtime"
                )
                self.assertEqual(config["model_auto_compact_token_limit"], 400000)
                for name in (
                    "AGENTS.md",
                    "AGENTS.override.md",
                    "CLAUDE.md",
                    "skills",
                    "prompts",
                    ".codex",
                    ".claude",
                    ".agents",
                ):
                    self.assertFalse((codex / name).exists(), name)
                    self.assertFalse((home / name).exists(), name)
                self.assertEqual(
                    (codex / "sessions/thread.jsonl").read_text(), "thread history"
                )
                self.assertEqual((codex / "state_5.sqlite").read_text(), "thread index")
                self.assertEqual(
                    config["projects"]["/work"]["trust_level"], "untrusted"
                )
                self.assertEqual(
                    config["projects"]["/work/repo"]["trust_level"], "untrusted"
                )
                self.assertIn("project_doc_max_bytes=0", result.stdout)
                self.assertIn("skills.include_instructions=false", result.stdout)
                self.assertIn("features.hooks=false", result.stdout)
                self.assertNotIn("skills.config", config)
                if shape == "cs2":
                    self.assertEqual(config["sandbox_mode"], "read-only")
                    self.assertNotIn("mcp_servers", config)
                    self.assertFalse(config["features"]["shell_tool"])
                else:
                    self.assertEqual(
                        config["mcp_servers"]["memcan"]["http_headers"][
                            "Authorization"
                        ],
                        'Bearer fake"\\token',
                    )

    def test_job_reset_rotates_runtime_credentials_and_does_not_follow_links(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            self.app_server(root)
            codex = root / "work/codex"
            home = root / "work/home"
            codex.mkdir(parents=True)
            home.mkdir()
            outside = root / "outside"
            outside.mkdir()
            (outside / "keep").write_text("untouched")
            env = {
                "PATH": "/usr/bin:/bin",
                "HOME": str(home),
                "CODEX_HOME": str(codex),
                "DP_RUNTIME_DIR": str(root / "runtime"),
                "DP_MEMCAN_URL": "http://memcan/mcp",
            }
            for token in ("fake-A", "fake-B"):
                (codex / "AGENTS.md").symlink_to(outside / "keep")
                (codex / "skills").symlink_to(outside, target_is_directory=True)
                (home / ".codex").symlink_to(codex, target_is_directory=True)
                subprocess.run(
                    ["python3", ROOT / "deploy/worker/worker-config.py", "cs1"],
                    env={**env, "DP_MEMCAN_TOKEN": token},
                    check=True,
                )
                self.assertFalse((codex / "AGENTS.md").exists())
                self.assertFalse((codex / "skills").exists())
                self.assertFalse((home / ".codex").exists())
                self.assertEqual((outside / "keep").read_text(), "untouched")
                config = tomllib.loads((codex / "config.toml").read_text())
                self.assertEqual(
                    config["mcp_servers"]["memcan"]["http_headers"]["Authorization"],
                    "Bearer " + token,
                )
                for path in (root / "work").rglob("*"):
                    if path.is_file() and not path.is_symlink():
                        self.assertNotIn(token, path.read_text())
            subprocess.run(
                ["python3", ROOT / "deploy/worker/worker-config.py", "cs1"],
                env={
                    key: value for key, value in env.items() if key != "DP_MEMCAN_URL"
                },
                check=True,
            )
            self.assertNotIn(
                "mcp_servers", tomllib.loads((codex / "config.toml").read_text())
            )

    def test_debian_bases_are_digest_pinned(self):
        for relative in (
            "deploy/worker/Dockerfile",
            "deploy/containers/runner.Dockerfile",
        ):
            for line in (ROOT / relative).read_text().splitlines():
                if line.startswith("FROM debian:bookworm-slim"):
                    self.assertRegex(
                        line,
                        r"^FROM debian:bookworm-slim@sha256:[0-9a-f]{64}( AS [a-z]+)?$",
                    )

    def test_warm_reset_preserves_codex_runtime_and_empties_home(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            self.app_server(root)
            codex = root / "work/codex"
            home = root / "work/home"
            home.mkdir(parents=True)
            runtime_files = [
                "tmp/arg0-linux/codex-arg0-test/apply_patch",
                "tmp/arg0-linux/codex-arg0-test/codex-linux-sandbox",
                "app-server-control/endpoint.json",
                "installation_id",
                "sessions/thread.jsonl",
                "archived_sessions/thread.jsonl",
                "history.jsonl",
                "session_index.jsonl",
            ]
            for database in (
                "state_5",
                "logs_2",
                "goals_1",
                "memories_1",
                "queue_1",
                "thread_history_1",
            ):
                for suffix in ("", "-wal", "-shm"):
                    runtime_files.append(database + ".sqlite" + suffix)
            for name in runtime_files:
                path = codex / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("live runtime data")
            inodes = {name: (codex / name).stat().st_ino for name in runtime_files}
            instruction_files = (
                "AGENTS.md",
                "AGENTS.override.md",
                "AGENTS.custom.md",
                "CLAUDE.md",
                "CLAUDE.local.md",
                "instructions.md",
                "config.toml",
            )
            instruction_dirs = (
                "skills",
                "prompts",
                "rules",
                "hooks",
                "plugins",
                ".agents",
                ".claude",
                ".codex",
                ".config",
            )
            env = {
                "PATH": "/usr/bin:/bin",
                "HOME": str(home),
                "CODEX_HOME": str(codex),
                "DP_RUNTIME_DIR": str(root / "runtime"),
            }
            for _ in range(2):
                for name in (
                    ".profile",
                    ".bashrc",
                    ".bash_profile",
                    ".gitconfig",
                    "other",
                ):
                    (home / name).write_text("planted startup data")
                (home / "nested").mkdir()
                (home / "nested/payload").write_text("planted startup data")
                for parent in (home, codex):
                    for name in instruction_files:
                        (parent / name).write_text("planted instruction")
                    for name in instruction_dirs:
                        (parent / name).mkdir()
                        (parent / name / "planted").write_text("planted instruction")
                subprocess.run(
                    ["python3", ROOT / "deploy/worker/worker-config.py", "cs1"],
                    env=env,
                    check=True,
                )
                self.assertEqual(list(home.iterdir()), [])
                for name in runtime_files:
                    path = codex / name
                    self.assertTrue(path.is_file(), name)
                    self.assertEqual(path.read_text(), "live runtime data", name)
                    self.assertEqual(path.stat().st_ino, inodes[name], name)
                for parent in (home, codex):
                    for name in (*instruction_files, *instruction_dirs):
                        if parent == codex and name == "config.toml":
                            self.assertTrue((parent / name).is_symlink())
                        else:
                            self.assertFalse(
                                (parent / name).exists(), str(parent / name)
                            )

    def test_read_only_gh_reloads_token_and_rejects_writes(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            gh = root / "gh-real"
            gh.write_text(
                '#!/usr/bin/python3\nimport os,sys,json\nprint(json.dumps([os.environ["GH_TOKEN"],sys.argv[1:]]))\n'
            )
            gh.chmod(0o755)
            # Replace fixed image paths only in the isolated copy.
            wrapper = root / "gh"
            wrapper.write_text(
                (ROOT / "deploy/worker/gh")
                .read_text()
                .replace("/usr/bin/gh", str(gh))
                .replace("/run/dp/github-token", str(root / "token"))
            )
            wrapper.chmod(0o755)
            env = {"PATH": "/usr/bin:/bin", "HOME": temp}
            for token in ("fake-A", "fake-B"):
                (root / "token").write_text(token)
                result = subprocess.run(
                    [wrapper, "pr", "view", "1"],
                    env=env,
                    capture_output=True,
                    text=True,
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(json.loads(result.stdout)[0], token)
            for args in (
                ["pr", "comment", "1"],
                ["api", "-X", "POST", "repos/o/r"],
                ["api", "graphql"],
                ["api", "--hostname", "evil.invalid", "user"],
                ["api", "https://evil.invalid"],
                ["auth", "login"],
                ["pr", "view", "1", "--repo", "evil.invalid/o/r"],
            ):
                result = subprocess.run([wrapper, *args], env=env, capture_output=True)
                self.assertNotEqual(result.returncode, 0, args)
            (root / "token").write_text("")
            result = subprocess.run(
                [wrapper, "pr", "view", "1"], env=env, capture_output=True
            )
            self.assertNotEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main()
