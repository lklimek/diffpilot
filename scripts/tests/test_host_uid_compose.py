"""Render local ownership overrides without inspecting or changing the running stack."""
import json
import os
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def test_host_uid_volume_keeps_prepared_ownership():
    env = {key: value for key, value in os.environ.items() if not key.startswith("SR_")}
    env.update(SR_SERVER_UID="1234", SR_SERVER_GID="2345")
    result = subprocess.run(
        ["docker", "compose", "--env-file", str(ROOT / ".env.example"),
         "-f", str(ROOT / "deploy/compose.yaml"),
         "-f", str(ROOT / "deploy/compose.local.yaml"),
         "-f", str(ROOT / "deploy/dev/compose.host-uid.yaml"), "config", "--format", "json"],
        env=env, capture_output=True, text=True, check=True,
    )
    server = json.loads(result.stdout)["services"]["server"]
    assert server["user"] == "1234:2345"
    data = [mount for mount in server["volumes"] if mount["target"] == "/var/lib/smart-review"]
    assert len(data) == 1
    assert data[0]["source"] == "server-data"
    assert data[0].get("volume", {}).get("nocopy") is True
    assert any(mount["target"] == "/run/secrets/github-app.pem" for mount in server["volumes"])
