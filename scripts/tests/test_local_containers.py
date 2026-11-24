"""The local installer creates new runtime material without opening existing secrets."""

import base64
import importlib.util
import os
from pathlib import Path
import stat
import subprocess

import pytest

ROOT = Path(__file__).resolve().parents[2]


def installer():
    spec = importlib.util.spec_from_file_location(
        "local_runtime_secrets", ROOT / "deploy/dev/local-runtime-secrets.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_new_secrets_have_distinct_scopes_and_consumer_permissions(tmp_path, monkeypatch):
    module = installer()
    owners = {}
    monkeypatch.setattr(module.os, "fchown", lambda fd, uid, gid: owners.update(
        {Path(os.readlink(f"/proc/self/fd/{fd}")).name: (uid, gid)}
    ))
    module.prepare(tmp_path, 1000)
    assert len(base64.b64decode((tmp_path / "gateway-secret-key").read_bytes(), validate=True)) == 32
    credentials = (tmp_path / "gateway-credentials-token").read_text()
    keys = (tmp_path / "gateway-keys-token").read_text()
    assert len(bytes.fromhex(credentials)) == len(bytes.fromhex(keys)) == 32
    assert credentials != keys
    assert (tmp_path / "runner-gateway-keys-token").read_text() == keys
    assert owners == {
        "gateway-secret-key": (10010, 10010),
        "gateway-credentials-token": (10010, 1000),
        "gateway-keys-token": (10010, 10010),
        "runner-gateway-keys-token": (10002, 10002),
    }
    assert stat.S_IMODE((tmp_path / "gateway-credentials-token").stat().st_mode) == 0o440
    assert all(stat.S_IMODE(p.stat().st_mode) == 0o400 for p in tmp_path.iterdir()
               if p.name != "gateway-credentials-token")


def test_rerun_never_opens_or_changes_existing_files(tmp_path, monkeypatch):
    module = installer()
    for name in module.NAMES:
        (tmp_path / name).write_bytes(b"existing")
    before = {p.name: p.stat() for p in tmp_path.iterdir()}
    monkeypatch.setattr(module.os, "open", lambda *_a, **_kw: pytest.fail("opened existing secret"))
    module.prepare(tmp_path, 1000)
    assert {p.name: p.stat() for p in tmp_path.iterdir()} == before


@pytest.mark.parametrize("name", ["gateway-keys-token", "runner-gateway-keys-token"])
def test_partial_keys_pair_fails_before_creating_any_file(tmp_path, name):
    module = installer()
    (tmp_path / name).write_bytes(b"existing")
    with pytest.raises(ValueError, match="both keys-scope files"):
        module.prepare(tmp_path, 1000)
    assert [p.name for p in tmp_path.iterdir()] == [name]


def test_symlink_secret_is_rejected_without_following_it(tmp_path):
    module = installer()
    (tmp_path / "gateway-secret-key").symlink_to(tmp_path / "absent")
    with pytest.raises(ValueError, match="regular file"):
        module.prepare(tmp_path, 1000)
    assert not (tmp_path / "absent").exists()


def test_env_contains_only_runtime_configuration_and_refuses_overwrite(tmp_path):
    state = tmp_path / "state"
    state.mkdir()
    (state / "images.env").write_text(
        "SR_SERVER_IMAGE=local/server:test\nSR_RUNNER_IMAGE=local/runner:test\n"
        "SR_PROXY_IMAGE=local/proxy:test\n"
        f"SR_WORKER_IMAGE=localhost:5000/worker@sha256:{'ab' * 32}\n"
        f"SR_FETCH_IMAGE=localhost:5000/fetch@sha256:{'cd' * 32}\n"
    )
    secrets = tmp_path / "new-secrets"
    (state / "secrets-dir").write_text(str(secrets))
    target = tmp_path / "runtime.env"
    env = {**os.environ, "SR_LOCAL_STATE_DIR": str(state), "SR_DOCKER_GID": "1000"}
    command = [str(ROOT / "deploy/dev/local-containers.sh"), "env", str(target)]
    first = subprocess.run(command, env=env, capture_output=True, text=True, check=False)
    assert first.returncode == 0, first.stderr
    values = dict(line.split("=", 1) for line in target.read_text().splitlines())
    assert values["SR_DOCKER_GID"] == "1000"
    assert values["SR_SERVER_UID"] == str(os.getuid())
    assert values["SR_GATEWAY_CREDENTIALS_TOKEN_PATH"] == str(secrets / "gateway-credentials-token")
    assert not {"SR_SECRET_KEY", "SR_RUNNER_SECRET", "SR_MEMCAN_TOKEN"} & values.keys()
    second = subprocess.run(command, env=env, capture_output=True, text=True, check=False)
    assert second.returncode != 0
    assert "existing" in second.stderr
