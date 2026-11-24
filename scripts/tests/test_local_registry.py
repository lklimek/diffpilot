"""Exercise prepare with fake commands; never access the owner's Docker stack."""

import json
import os
from pathlib import Path
import subprocess

import pytest

ROOT = Path(__file__).resolve().parents[2]
PIN = "registry:2.8.3@sha256:a3d8aaa63ed8681a604f1dea0aa03f100d5895b6a58ace528858a7b332415373"
NAME = "test-registry"


def prepare(tmp_path, container=None, network=None, image=None):
    commands = tmp_path / "bin"
    commands.mkdir()
    fixtures = tmp_path / "fixtures.json"
    fixtures.write_text(json.dumps({"container": container, "network": network}))
    docker = commands / "docker"
    docker.write_text("""#!/usr/bin/python3
import json, os, sys
from pathlib import Path
args = sys.argv[1:]
with open(os.environ['CALLS'], 'a') as log:
    log.write(json.dumps(args) + '\\n')
fixtures = json.loads(Path(os.environ['FIXTURES']).read_text())
if args[:2] in (['container', 'inspect'], ['network', 'inspect']):
    kind = args[0]
    value = fixtures[kind]
    if value is None:
        sys.exit(1)
    print(json.dumps([value]))
elif args[:2] == ['image', 'inspect']:
    print(json.dumps(['localhost:15000/test-worker@sha256:' + 'a'*64,
                      'localhost:15000/test-fetch@sha256:' + 'b'*64]))
""")
    git = commands / "git"
    git.write_text("""#!/bin/bash
if [[ "$*" == *'rev-parse HEAD'* ]]; then
    echo 71096d7d4481977added8761b2cfd63cd84852b4
elif [[ "$*" == *' archive '* ]]; then
    tar -cf - --files-from /dev/null
fi
""")
    curl = commands / "curl"
    curl.write_text("#!/bin/sh\nexit 0\n")
    for command in (docker, git, curl):
        command.chmod(0o755)
    calls = tmp_path / "calls.jsonl"
    env = {**os.environ, "PATH": f"{commands}:{os.environ['PATH']}",
           "SR_LOCAL_STATE_DIR": str(tmp_path / "state"),
           "SR_LOCAL_CLAUDIUS_SOURCE": str(tmp_path / "fake-source"),
           "SR_LOCAL_TMPDIR": str(tmp_path), "SR_LOCAL_IMAGE_PREFIX": "test",
           "CALLS": str(calls),
           "FIXTURES": str(fixtures)}
    env.pop("SR_LOCAL_REGISTRY_IMAGE", None)
    if image is not None:
        env["SR_LOCAL_REGISTRY_IMAGE"] = image
    result = subprocess.run([str(ROOT / "deploy/dev/local-containers.sh"), "prepare"],
                            env=env, capture_output=True, text=True, check=False)
    return result, [json.loads(line) for line in calls.read_text().splitlines()] if calls.exists() else []


def registry(running=True):
    return {"Id": "registry-id", "State": {"Running": running},
            "Config": {"Image": PIN, "Labels": {"diffpilot.local-registry": "1"}},
            "HostConfig": {"PortBindings": {"5000/tcp": [{"HostIp": "127.0.0.1", "HostPort": "15000"}]},
                           "ReadonlyRootfs": True, "CapDrop": ["ALL"],
                           "SecurityOpt": ["no-new-privileges:true"]},
            "NetworkSettings": {"Networks": {NAME: {}}}}


def registry_network(members=None):
    return {"Driver": "bridge", "Internal": False, "EnableIPv6": False,
            "Options": {"com.docker.network.bridge.gateway_mode_ipv4": "nat",
                        "com.docker.network.bridge.enable_icc": "false"},
            "Containers": members or {}}


def test_prepare_creates_dedicated_registry_and_pins_image(tmp_path):
    result, calls = prepare(tmp_path)
    assert result.returncode == 0, result.stderr
    create = next(c for c in calls if c[:2] == ["network", "create"])
    assert "--internal" not in create and create[-1] == NAME
    assert "com.docker.network.bridge.gateway_mode_ipv4=nat" in create
    assert "com.docker.network.bridge.enable_icc=false" in create
    run = next(c for c in calls if c[0] == "run")
    assert run[run.index("--network") + 1] == NAME
    assert run[-1] == PIN
    for flag, value in [("--cap-drop", "ALL"), ("--security-opt", "no-new-privileges:true"),
                        ("-p", "127.0.0.1:15000:5000")]:
        assert run[run.index(flag) + 1] == value
    assert "--read-only" in run
    assert any("/var/lib/registry" in a for a in run)
    assert sum(c[0] == "push" for c in calls) == 2


def test_running_legacy_registry_is_kept_without_mutations(tmp_path):
    old = registry()
    old["Config"]["Image"] = "registry:2.8.3"
    old["NetworkSettings"]["Networks"] = {"bridge": {}}
    result, calls = prepare(tmp_path, old)
    assert result.returncode == 0, result.stderr
    assert "keeping running registry" in result.stderr
    assert not any(c[0] in ("start", "stop", "rm", "run", "network") for c in calls)


@pytest.mark.parametrize("running", [True, False])
def test_matching_registry_is_reused(tmp_path, running):
    result, calls = prepare(tmp_path, registry(running), registry_network({"registry-id": {}}))
    assert result.returncode == 0, result.stderr
    assert "keeping running registry" not in result.stderr
    assert (["start", NAME] in calls) is not running
    assert not any(c[0] in ("stop", "rm", "run") for c in calls)


@pytest.mark.parametrize("bad", ["foreign-member", "internal", "driver", "ipv6", "stopped-legacy", "public-port", "icc", "routed"])
def test_prepare_refuses_unsafe_registry_reuse(tmp_path, bad):
    container = registry()
    network = registry_network({"registry-id": {}})
    if bad == "foreign-member":
        network["Containers"]["other-container"] = {}
    elif bad == "internal":
        network["Internal"] = True
    elif bad == "icc":
        network["Options"]["com.docker.network.bridge.enable_icc"] = "true"
    elif bad == "routed":
        network["Options"]["com.docker.network.bridge.gateway_mode_ipv4"] = "routed"
    elif bad == "driver":
        network["Driver"] = "overlay"
    elif bad == "ipv6":
        network["EnableIPv6"] = True
    elif bad == "stopped-legacy":
        container = registry(False)
        container["Config"]["Image"] = "registry:2.8.3"
    elif bad == "public-port":
        container["HostConfig"]["PortBindings"]["5000/tcp"][0]["HostIp"] = "0.0.0.0"
    result, calls = prepare(tmp_path, container, network)
    assert result.returncode != 0
    assert not any(c[0] in ("start", "stop", "rm", "run", "push") for c in calls)


def test_tag_only_registry_override_is_rejected(tmp_path):
    result, calls = prepare(tmp_path, image="registry:2.8.3")
    assert result.returncode != 0
    assert "digest" in result.stderr
    assert not calls


@pytest.mark.parametrize("foreign_member", [True, False])
def test_prepare_checks_existing_network_before_creating_registry(tmp_path, foreign_member):
    network = registry_network({"other-container": {}} if foreign_member else {})
    result, calls = prepare(tmp_path, network=network)
    assert (result.returncode != 0) == foreign_member, result.stderr
    assert not any(c[:2] == ["network", "create"] for c in calls)
    assert any(c[0] == "run" for c in calls) is not foreign_member
