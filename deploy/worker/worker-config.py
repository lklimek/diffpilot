#!/usr/bin/python3
"""Reset job instructions and publish the user config on the runtime tmpfs."""

import json
import os
from pathlib import Path
import re
import shutil
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
# Only Codex thread transcripts and their index survive a job boundary.
for parent in (home, codex):
    if parent.is_symlink():
        remove(parent)
    parent.mkdir(parents=True, exist_ok=True)
    for child in parent.iterdir():
        thread_state = parent == codex and (
            child.name
            in {"sessions", "archived_sessions", "history.jsonl", "session_index.jsonl"}
            or re.fullmatch(r"state_\d+\.sqlite(?:-wal|-shm)?", child.name)
        )
        if not thread_state or child.is_symlink():
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
