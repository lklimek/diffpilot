"""Worker assets use fake binaries and isolated homes; never provider auth."""

import json
from pathlib import Path
import subprocess
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]


class WorkerAssets(unittest.TestCase):
    def test_entry_replaces_stale_config_and_pins_critical_flags(self):
        for shape in ("cs1", "cs2"):
            with self.subTest(shape=shape), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                (root / "config.toml").write_text(
                    '[projects."/work/repo"]\ntrust_level="trusted"\n'
                )
                fake = root / "codex"
                fake.write_text('#!/bin/sh\nprintf "%s\\n" "$@"\n')
                fake.chmod(0o755)
                env = {
                    "PATH": f"{root}:/usr/bin:/bin",
                    "HOME": temp,
                    "CODEX_HOME": temp,
                    "DP_MEMCAN_URL": "http://memcan:8190/mcp",
                    "DP_MEMCAN_TOKEN": 'fake"\\token',
                }
                result = subprocess.run(
                    [ROOT / "deploy/worker/dp-worker-entry", shape],
                    env=env,
                    capture_output=True,
                    text=True,
                    check=True,
                )
                config = tomllib.loads((root / "config.toml").read_text())
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
