#!/bin/sh
# Offline image checks; no account, login or credential-store access.
set -eu
name=v13w3-worker-smoke
if docker container inspect "$name" >/dev/null 2>&1; then
    echo 'test container already exists' >&2
    exit 1
fi
trap 'docker rm -f "$name" >/dev/null 2>&1 || true' EXIT
docker run --name "$name" --network none --read-only --cap-drop ALL --security-opt no-new-privileges:true \
    --user 10002:10002 --memory 512m --cpus 2 --pids-limit 128 --tmpfs /tmp:uid=10002,gid=10002,mode=0700,size=16m \
    --entrypoint /bin/sh v13w3-worker:test -ec '
    test "$(id -u)" = 10002
    test -z "${GH_TOKEN:-}"
    test "$CLAUDE_PLUGIN_ROOT" = /etc/codex/claudius
    ! command -v cargo
    ! command -v rustc
    ! command -v github-mcp-server
    codex --version
    sr-check format >/dev/null
    git config --system --get-all safe.directory | rg "^\*$"
    python3 /etc/codex/claudius/scripts/validate_report.py /etc/codex/claudius/tests/fixtures/reports/v4-comment-assessments-host.json
    test -r /etc/codex/skills/report-format/references/comment-assessments.md
    test -r /etc/codex/skills/severity/SKILL.md
    echo "IM3 offline image checks PASS"
'
