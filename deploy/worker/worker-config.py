#!/usr/bin/python3
"""Replace the user layer; the provider repeats this before start/fork/resume."""

import json
import os
from pathlib import Path
import sys

shape = sys.argv[1]
if shape not in ("cs1", "cs2"):
    sys.exit("unknown worker shape")
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
    # JSON basic strings escape the same characters as TOML basic strings.
    preset += "\n[mcp_servers.memcan]\nurl = " + json.dumps(os.environ["DP_MEMCAN_URL"])
    preset += "\n[mcp_servers.memcan.http_headers]\nAuthorization = "
    preset += json.dumps("Bearer " + os.environ["DP_MEMCAN_TOKEN"]) + "\n"
path = Path(os.environ["CODEX_HOME"]) / "config.toml"
path.parent.mkdir(parents=True, exist_ok=True)
path.write_text(preset)
path.chmod(0o600)
