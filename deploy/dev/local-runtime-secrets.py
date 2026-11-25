#!/usr/bin/python3
"""Create only absent runtime secrets; existing secret bytes are never opened."""

import base64
import os
from pathlib import Path
import secrets
import stat
import sys

NAMES = ("gateway-secret-key", "gateway-credentials-token", "gateway-keys-token",
         "runner-gateway-keys-token")


def prepare(directory: Path, server_gid: int):
    """Create the gateway files and a runner copy of a newly generated keys token."""
    present = {}
    for name in NAMES:
        try:
            mode = (directory / name).lstat().st_mode
        except FileNotFoundError:
            present[name] = False
        else:
            if not stat.S_ISREG(mode):
                raise ValueError(f"{name} must be a regular file")
            present[name] = True
    if present[NAMES[2]] != present[NAMES[3]]:
        raise ValueError("both keys-scope files must exist or both must be absent; restore the pair manually")
    credentials = secrets.token_hex(32)
    keys = secrets.token_hex(32)
    while keys == credentials:
        keys = secrets.token_hex(32)
    values = (base64.b64encode(secrets.token_bytes(32)).decode(), credentials, keys, keys)
    owners = ((10010, 10010, 0o400), (10010, server_gid, 0o440),
              (10010, 10010, 0o400), (10002, 10002, 0o400))
    for name, value, (uid, gid, mode) in zip(NAMES, values, owners):
        if present[name]:
            continue
        fd = os.open(directory / name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        with os.fdopen(fd, "wb") as output:
            output.write(value.encode())
            output.flush()
            os.fchmod(output.fileno(), mode)
            os.fchown(output.fileno(), uid, gid)
            os.fsync(output.fileno())


if __name__ == "__main__":
    try:
        prepare(Path(sys.argv[1]), int(sys.argv[2]))
    except (ValueError, OSError) as error:
        sys.exit(f"runtime secrets: {error}")
