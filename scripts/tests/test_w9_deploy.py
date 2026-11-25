"""Deployment regressions use fake Docker and synthetic secret-file metadata only."""

import base64
import json
import os
from pathlib import Path
import subprocess

import pytest

ROOT = Path(__file__).resolve().parents[2]


def executable(path, content):
    path.write_text(content)
    path.chmod(0o755)


def test_stack_respects_host_scratch(tmp_path):
    commands = tmp_path / "bin"
    commands.mkdir()
    executable(
        commands / "mktemp",
        '#!/bin/sh\nprintf "%s\\n" "$@" > "$SCRATCH_ARGS"\nexec /usr/bin/mktemp "$@"\n',
    )
    executable(
        commands / "docker",
        """#!/bin/sh
python3 - "$@" <<'INNER'
import json, os, sys
config = json.load(open(os.environ["STACK_CONFIG"]))
project = sys.argv[sys.argv.index("-p") + 1]
config["services"]["launcher"]["environment"].update(SR_CONTAINER_PREFIX=project, SR_WORKER_NETWORK=project+"-worker", SR_JUDGE_NETWORK=project+"-judge")
print(json.dumps(config))
INNER
""",
    )
    config = {
        "services": {
            "launcher": {
                "network_mode": "none",
                "volumes": [{"target": "/var/run/docker.sock"}],
                "environment": {
                    "SR_WORKER_IMAGE": "fake@sha256:abc",
                    "SR_FETCH_IMAGE": "fake@sha256:def",
                },
            },
            "runner": {
                "user": "10002:10002",
                "cap_drop": ["ALL"],
                "environment": {"SR_RUNNER_MIRRORS_DIR": "/mirror"},
                "volumes": [{"target": "/mirror", "source": "mirror"}],
            },
            "codex-gateway": {"networks": {"worker": {}, "judge": {}, "runner": {}}},
            "egress-proxy": {"networks": {}, "volumes": []},
            "server": {"volumes": []},
        },
        "networks": {"worker": {"internal": True}, "judge": {"internal": True}},
    }
    config_file = tmp_path / "config.json"
    config_file.write_text(json.dumps(config))
    for selected in ("SR_STACK_TMPDIR", "RUNNER_TEMP", "TMPDIR"):
        marker = tmp_path / "scratch-args"
        env = {
            k: v
            for k, v in os.environ.items()
            if k not in {"SR_STACK_TMPDIR", "RUNNER_TEMP", "TMPDIR"}
        }
        result = subprocess.run(
            ["bash", "deploy/test-stack.sh"],
            cwd=ROOT,
            env={
                **env,
                selected: str(tmp_path),
                "PATH": f"{commands}:{os.environ['PATH']}",
                "SCRATCH_ARGS": str(marker),
                "STACK_CONFIG": str(config_file),
            },
            capture_output=True,
            text=True,
            timeout=15,
        )
        assert result.returncode == 0, result.stderr
        assert str(tmp_path) in marker.read_text(), selected
        assert not list(tmp_path.glob("diffpilot-stack-config.*"))


@pytest.fixture
def production(tmp_path):
    commands = tmp_path / "bin"
    commands.mkdir()
    executable(
        commands / "stat",
        """#!/bin/sh
if [ "$2" = '%u %g %a' ]; then
  uid=10010; gid=10010
  case "$3" in */sr-runner-secret) uid=10002 ;; */runner-gateway-keys-token) uid=10002; gid=10002 ;; esac
  case "$3" in *"${WRONG_OWNER:-never-match}"*) uid=10001 ;; esac
  printf '%s %s %s\\n' "$uid" "$gid" "$(/usr/bin/stat -c '%a' "$3")"
else
  exec /usr/bin/stat "$@"
fi
""",
    )
    values = dict(
        line.split("=", 1)
        for line in (ROOT / ".env.example").read_text().splitlines()
        if line and not line.startswith("#") and "=" in line
    )
    values.update(
        SR_PUBLIC_URL="https://production.invalid",
        SR_ALLOWED_LOGINS="octocat",
        SR_GITHUB_APP_ID="42",
        SR_GITHUB_CLIENT_ID="Iv23test",
        SR_MEMCAN_URL="http://memcan.invalid:8190/mcp",
        SR_SECRET_KEY="CANARY-encryption",
        SR_RUNNER_SECRET="CANARY-runner-0123456789abcdef0123456789",
        SR_GITHUB_CLIENT_SECRET="CANARY-github",
        SR_MEMCAN_TOKEN="CANARY-memcan",
        SR_DOCKER_GID="997",
        SR_WORKER_IMAGE="worker@sha256:" + "0123456789abcdef" * 4,
        SR_FETCH_IMAGE="fetch@sha256:" + "fedcba9876543210" * 4,
    )
    for key, name in [
        ("SR_GATEWAY_SECRET_KEY_PATH", "gateway-secret-key"),
        ("SR_GATEWAY_CREDENTIALS_TOKEN_PATH", "gateway-credentials-token"),
        ("SR_GATEWAY_KEYS_TOKEN_PATH", "gateway-keys-token"),
        ("SR_RUNNER_GATEWAY_KEYS_TOKEN_PATH", "runner-gateway-keys-token"),
    ]:
        path = tmp_path / name
        if name == "gateway-secret-key":
            value = base64.b64encode(b"CANARY" + b"x" * 26).decode()
        else:
            value = "CANARY-keys" if "keys-token" in name else "CANARY-credentials"
            value += "-synthetic-token-0123456789abcdef"
        path.write_text(value)
        path.chmod(0o400)
        values[key] = str(path)
    secret_dir = tmp_path / "secrets"
    secret_dir.mkdir()
    values["SR_SECRETS_DIR"] = str(secret_dir)
    for key, name in [
        ("SR_SECRET_KEY", "sr-secret-key"),
        ("SR_RUNNER_SECRET", "sr-runner-secret"),
        ("SR_GITHUB_CLIENT_SECRET", "sr-github-client-secret"),
        ("SR_MEMCAN_TOKEN", "sr-memcan-token"),
    ]:
        path = secret_dir / name
        path.write_text(values[key])
        path.chmod(0o440 if key == "SR_RUNNER_SECRET" else 0o400)
    return tmp_path, commands, values


def guard(production, *, as_files=False, wrong_owner=None, overlays=()):
    tmp_path, commands, values = production
    values = dict(values)
    if as_files:
        for key in (
            "SR_SECRET_KEY",
            "SR_RUNNER_SECRET",
            "SR_GITHUB_CLIENT_SECRET",
            "SR_MEMCAN_TOKEN",
        ):
            values[key] = ""
    env_file = tmp_path / "production.env"
    env_file.write_text("".join(f"{key}={value}\n" for key, value in values.items()))
    result = subprocess.run(
        [
            "bash",
            "deploy/check-production-config.sh",
            "--no-render",
            "--env-file",
            str(env_file),
            *(part for overlay in overlays for part in ("--env-file", str(overlay))),
            *(["--secret-files"] if as_files else []),
        ],
        cwd=ROOT,
        env={
            **os.environ,
            "PATH": f"{commands}:{os.environ['PATH']}",
            "WRONG_OWNER": wrong_owner or "never-match",
        },
        capture_output=True,
        text=True,
        timeout=15,
    )
    assert "CANARY" not in result.stdout + result.stderr
    return result


def test_guard_accepts_consumer_files(production):
    assert guard(production).returncode == 0
    assert guard(production, as_files=True).returncode == 0


def test_guard_rejects_missing_overlay_without_rendering(production):
    assert guard(production, overlays=[production[0] / "missing.env"]).returncode != 0


@pytest.mark.parametrize(
    "variable", ["SR_DOCKER_GID", "SR_WORKER_IMAGE", "SR_FETCH_IMAGE"]
)
@pytest.mark.parametrize("bad", ["", "REPLACE_ME", "example"])
def test_guard_rejects_runtime_defaults(production, variable, bad):
    values = production[2]
    values[variable] = (
        dict(
            line.split("=", 1)
            for line in (ROOT / ".env.example").read_text().splitlines()
            if line.startswith(variable + "=")
        )[variable]
        if bad == "example"
        else bad
    )
    assert guard(production).returncode != 0


@pytest.mark.parametrize(
    "name",
    [
        "gateway-secret-key",
        "gateway-credentials-token",
        "gateway-keys-token",
        "runner-gateway-keys-token",
    ],
)
@pytest.mark.parametrize("defect", ["missing", "empty", "placeholder", "mode", "owner"])
def test_guard_checks_each_gateway_file(production, name, defect):
    path = production[0] / name
    if defect == "missing":
        path.unlink()
    elif defect in {"empty", "placeholder"}:
        path.chmod(0o600)
        path.write_text("" if defect == "empty" else "REPLACE_CANARY")
        path.chmod(0o400)
    elif defect == "mode":
        path.chmod(0o444)
    assert (
        guard(production, wrong_owner=name if defect == "owner" else None).returncode
        != 0
    )


def test_guard_rejects_mismatched_keys(production):
    path = production[0] / "runner-gateway-keys-token"
    path.chmod(0o600)
    path.write_text("CANARY-different")
    path.chmod(0o400)
    assert guard(production).returncode != 0


@pytest.mark.parametrize(
    "name",
    ["sr-runner-secret", "sr-secret-key", "sr-github-client-secret", "sr-memcan-token"],
)
def test_guard_checks_each_service_consumer(production, name):
    assert guard(production, as_files=True, wrong_owner=name).returncode != 0


def test_legacy_cleanup_is_explicit_idempotent_and_refuses_attached_containers(
    tmp_path,
):
    commands = tmp_path / "bin"
    commands.mkdir()
    executable(
        commands / "docker",
        """#!/bin/sh
printf '%s\\n' "$*" >> "$DOCKER_CALLS"
case "$1 $2" in
 'volume ls') [ "${EMPTY:-0}" = 1 ] || printf '%s\\n' w9_runner-state w9_tmp-runner-codex-homes unrelated ;;
 'ps -aq') [ "${BUSY:-0}" = 0 ] || echo legacy-container ;;
 'volume rm') shift 2; printf '%s\\n' "$@" ;;
 *) exit 42 ;;
esac
""",
    )
    marker = tmp_path / "calls"
    env = {
        **os.environ,
        "PATH": f"{commands}:{os.environ['PATH']}",
        "DOCKER_CALLS": str(marker),
    }
    script = ["bash", "deploy/cleanup-legacy-volumes.sh", "w9"]
    for args, busy, code in [
        (script, "0", 2),
        (script + ["--after-successful-deploy"], "1", 1),
        (script + ["--after-successful-deploy"], "0", 0),
    ]:
        marker.write_text("")
        result = subprocess.run(
            args,
            cwd=ROOT,
            env={**env, "BUSY": busy},
            capture_output=True,
            text=True,
            timeout=15,
        )
        assert result.returncode == code, result.stderr
        calls = marker.read_text()
        assert ("volume rm" in calls) == (code == 0)
        assert "unrelated" not in result.stdout
        assert " run " not in calls and " inspect " not in calls
    marker.write_text("")
    result = subprocess.run(
        script + ["--after-successful-deploy"],
        cwd=ROOT,
        env={**env, "EMPTY": "1"},
        capture_output=True,
        text=True,
        timeout=15,
    )
    assert result.returncode == 0, result.stderr
    assert not result.stdout
    assert "volume rm" not in marker.read_text()
