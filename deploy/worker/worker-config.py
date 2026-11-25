#!/usr/bin/python3
"""Reset job instructions and publish the user config on the runtime tmpfs."""

import json
import os
from pathlib import Path
import shutil
import socket
import sys
import time

shape = sys.argv[1]
if shape not in ("cs1", "cs2"):
    sys.exit("unknown worker shape")
runtime = Path(os.environ.get("DP_RUNTIME_DIR", "/run/dp"))
if sys.argv[2:] == ["--wait"]:
    # The launcher supplies each job's config before app-server reads its user layer.
    while not (runtime / "job-ready").is_file():
        time.sleep(0.05)
    sys.exit(0)


def remove(path):
    if path.is_symlink() or not path.is_dir():
        path.unlink(missing_ok=True)
    else:
        shutil.rmtree(path)


os.umask(0o077)
codex = Path(os.environ["CODEX_HOME"])
home = Path(os.environ["HOME"])
# Empty HOME; preserve live app-server files while resetting CODEX_HOME instructions.
for parent in (home, codex):
    if parent.is_symlink():
        remove(parent)
    parent.mkdir(parents=True, exist_ok=True)
    for child in parent.iterdir():
        instruction_source = (
            child.name
            in {
                "skills",
                "prompts",
                "rules",
                "hooks",
                "plugins",
                ".agents",
                ".claude",
                ".codex",
                ".config",
                "instructions.md",
                "config.toml",
            }
            or child.match("AGENTS*.md")
            or child.match("CLAUDE*.md")
        )
        if parent == home or instruction_source:
            remove(child)

preset = Path(__file__).with_name("config.toml").read_text()
if shape == "cs2":
    preset = preset.replace(
        'sandbox_mode = "danger-full-access"', 'sandbox_mode = "read-only"'
    )
    preset = preset.replace(
        "features = {",
        "features = { shell_tool = false, unified_exec = false, view_image = false,",
    )
elif os.environ.get("DP_MEMCAN_URL"):
    preset += "\n[mcp_servers.memcan]\nurl = " + json.dumps(os.environ["DP_MEMCAN_URL"])
    preset += "\n[mcp_servers.memcan.http_headers]\nAuthorization = "
    preset += json.dumps("Bearer " + os.environ["DP_MEMCAN_TOKEN"]) + "\n"

runtime.mkdir(parents=True, exist_ok=True)
# Unlink first: agent-owned config paths may be symlinks to persistent files.
remove(runtime / "config.next")
(runtime / "config.next").write_text(preset)
(runtime / "config.next").replace(runtime / "config.toml")
remove(codex / "config.toml")
(codex / "config.toml").symlink_to(runtime / "config.toml")
remove(runtime / "job-ready")
(runtime / "job-ready").touch(mode=0o600)

# The launcher attaches right after this exec; return only once app-server accepts connections.
deadline = time.monotonic() + float(os.environ.get("DP_APP_READY_SECS", "9"))
while True:
    with socket.socket(socket.AF_UNIX) as probe:
        try:
            probe.connect(str(runtime / "app.sock"))
            break
        except OSError:
            if time.monotonic() >= deadline:
                sys.exit("app-server did not accept connections on its control socket")
    time.sleep(0.05)
