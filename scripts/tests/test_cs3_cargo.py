"""The container test can emit its runtime template on a clean CI host."""

import os
from pathlib import Path
import subprocess


def test_cs3_uses_the_selected_cargo_command_without_host_credentials(tmp_path):
    root = Path(__file__).resolve().parents[2]
    commands = tmp_path / "bin"
    commands.mkdir()
    docker = commands / "docker"
    docker.write_text("""#!/bin/sh
case "$1 $2" in
  'container inspect'|'network inspect'|'volume inspect') exit 1 ;;
  'inspect --format') echo healthy ;;
esac
exit 0
""")
    cargo = commands / "ci-cargo"
    marker = tmp_path / "cargo-args"
    scratch_marker = tmp_path / "scratch-path"
    cargo.write_text(
        '#!/bin/sh\nprintf "%s\\n" "$@" > "$CARGO_MARKER"\n'
        'printf "%s\\n" "$DOCKER_CONFIG" > "$CARGO_SCRATCH_MARKER"\nexit 42\n'
    )
    docker.chmod(0o755)
    cargo.chmod(0o755)
    result = subprocess.run(
        ["bash", str(root / "deploy/containers/test-cs3-squid.sh")],
        cwd=root,
        env={
            **os.environ,
            "PATH": f"{commands}:{os.environ['PATH']}",
            "SR_CARGO_COMMAND": str(cargo),
            "CARGO_MARKER": str(marker),
            "SR_CS3_TMPDIR": str(tmp_path),
            "CARGO_SCRATCH_MARKER": str(scratch_marker),
        },
        capture_output=True,
        text=True,
        timeout=15,
    )
    assert result.returncode == 42, result.stderr
    assert marker.read_text().splitlines() == [
        "run",
        "-p",
        "runner",
        "--features",
        "fake-provider",
        "--example",
        "cs3-template",
    ]
    assert Path(scratch_marker.read_text().strip()).parent.parent == tmp_path
