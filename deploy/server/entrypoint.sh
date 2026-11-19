#!/bin/bash
# Server entrypoint: empties /tmp (the disposable volume tmp-server, docs/runbook.md "Temporary volumes"), so
# nothing of a previous run survives a restart, then runs the server.
set -euo pipefail
if ! find /tmp -mindepth 1 -delete; then
  echo "server entrypoint: cannot empty /tmp; stop the server and remove its temporary volume (docs/runbook.md, \"Temporary volumes\")" >&2
  exit 1
fi
exec /usr/local/bin/diffpilot-server "$@"
