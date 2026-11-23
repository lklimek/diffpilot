#!/bin/sh
# Offline image checks; no account, login or credential-store access.
set -eu
name=v13w3-worker-smoke
work_volume=$name-work
image=${1:-v13w3-worker:test}
if docker container inspect "$name" >/dev/null 2>&1 || docker volume inspect "$work_volume" >/dev/null 2>&1; then
    echo 'test container already exists' >&2
    exit 1
fi
cleanup() {
    docker rm -f "$name" >/dev/null 2>&1 || true
    docker volume rm "$work_volume" >/dev/null 2>&1 || true
}
trap cleanup EXIT
docker run --name "$name" --network none --read-only --cap-drop ALL --security-opt no-new-privileges:true \
    --user 10002:10002 --memory 512m --cpus 2 --pids-limit 128 --tmpfs /tmp:uid=10002,gid=10002,mode=0700,size=16m \
    --entrypoint /bin/sh "$image" -ec '
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

docker rm "$name" >/dev/null
docker volume create --label dp.managed=v13w3-worker-smoke "$work_volume" >/dev/null
docker run -d --name "$name" --network none --init --read-only --cap-drop ALL \
    --security-opt no-new-privileges:true --user 10002:10002 --memory 512m --memory-swap 512m --cpus 2 --pids-limit 128 \
    --tmpfs /tmp:uid=10002,gid=10002,mode=0700,size=64m \
    --tmpfs /run/dp:uid=10002,gid=10002,mode=0700,size=1m \
    --mount "source=$work_volume,target=/work" \
    -e HOME=/work/home -e CODEX_HOME=/work/codex "$image" cs1 >/dev/null
# Entry waits for the launcher's first reset. No provider credentials or network.
docker exec "$name" /bin/sh -ec 'test ! -e /run/dp/app.sock'
docker exec -e DP_MEMCAN_URL=http://memcan.invalid/mcp -e DP_MEMCAN_TOKEN=fake-A \
    "$name" python3 /usr/local/bin/worker-config.py cs1
for attempt in $(seq 1 100); do
    if docker exec "$name" test -S /run/dp/app.sock; then break; fi
    if [ "$attempt" = 100 ]; then docker logs "$name"; exit 1; fi
    sleep 0.1
done
docker exec "$name" /bin/sh -ec '
    test -z "${DP_MEMCAN_TOKEN:-}"
    mkdir -p "$CODEX_HOME/sessions" "$CODEX_HOME/skills" "$HOME/.agents/skills"
    echo thread-history > "$CODEX_HOME/sessions/test.jsonl"
    echo planted > "$CODEX_HOME/AGENTS.md"
    echo planted > "$HOME/AGENTS.md"
'
docker exec -e DP_MEMCAN_URL=http://memcan.invalid/mcp -e DP_MEMCAN_TOKEN=fake-B \
    "$name" python3 /usr/local/bin/worker-config.py cs1
docker exec "$name" /bin/sh -ec '
    test "$(readlink "$CODEX_HOME/config.toml")" = /run/dp/config.toml
    test ! -e "$CODEX_HOME/AGENTS.md"
    test ! -e "$CODEX_HOME/skills"
    test ! -e "$HOME/AGENTS.md"
    test ! -e "$HOME/.agents"
    test "$(cat "$CODEX_HOME/sessions/test.jsonl")" = thread-history
    rg -q fake-B /run/dp/config.toml
    ! rg -q fake-A /run/dp/config.toml
    ! find /work -type f -exec grep -l fake-B {} + | rg .
    echo "IM3 job reset, runtime token rotation and persistent thread state PASS"
'
