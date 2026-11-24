"""Render local ownership overrides without inspecting or changing the running stack."""
import json
import os
import shutil
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]


@pytest.mark.skipif(shutil.which("docker") is None, reason="docker CLI not installed")
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
    data = [mount for mount in server["volumes"] if mount["target"] == "/var/lib/diffpilot"]
    assert len(data) == 1
    assert data[0]["source"] == "server-data"
    assert data[0].get("volume", {}).get("nocopy") is True
    assert any(mount["target"] == "/run/secrets/github-app.pem" for mount in server["volumes"])


@pytest.mark.skipif(shutil.which("docker") is None, reason="docker CLI not installed")
def test_worker_network_services_reserve_distinct_static_addresses():
    env = {key: value for key, value in os.environ.items() if not key.startswith("SR_")}
    env.update(SR_GATEWAY_MODEL_ADDR="10.241.222.2", SR_PROXY_WORKER_ADDR="10.241.222.3",
               SR_PROXY_EGRESS_ADDR="10.241.223.2", SR_GATEWAY_EGRESS_ADDR="10.241.223.3")
    result = subprocess.run(
        ["docker", "compose", "--env-file", str(ROOT / ".env.example"),
         "-f", str(ROOT / "deploy/compose.yaml"), "config", "--format", "json"],
        env=env, capture_output=True, text=True, check=True,
    )
    services = json.loads(result.stdout)["services"]
    assert services["codex-gateway"]["networks"]["worker"]["ipv4_address"] == "10.241.222.2"
    assert services["egress-proxy"]["networks"]["worker"]["ipv4_address"] == "10.241.222.3"
    assert services["codex-gateway"]["networks"]["egress"]["ipv4_address"] == "10.241.223.3"
    assert services["egress-proxy"]["networks"]["egress"]["ipv4_address"] == "10.241.223.2"
