#!/usr/bin/env bash
# Checks of the dev stack launcher (stack.sh) and the e2e global setup (e2e/stack.ts):
#   1. inherited GIT_DIR / GIT_WORK_TREE / GIT_INDEX_FILE leave a control repository untouched;
#   2. a relative SR_DEV_STATE_DIR works;
#   3. a start timeout during the build leaves no build process behind.
# Usage: deploy/dev/stack-check.sh   (SR_DEV_*_BIN as for stack.sh; without them checks 1-2 build first)
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
mkdir -p /data/tmp
tmp="$(mktemp -d /data/tmp/stack-check.XXXXXX)"
trap 'rm -rf "$tmp"' EXIT
cd "$tmp"
failed=0
result() { if [[ "$2" == ok ]]; then echo "ok   $1"; else echo "FAIL $1: $2"; failed=1; fi; }

# start_and_stop NAME ENV...: runs the launcher (no runner) with ENV until it is ready, then stops it.
start_and_stop() {
  local name="$1" pid ready=1
  shift
  env "$@" "$here/stack.sh" --without runner > "$name.out" 2> "$name.err" &
  pid=$!
  for _ in $(seq 600); do
    grep -q '^SR_DEV_URL=' "$name.out" && { ready=0; break; }
    kill -0 "$pid" 2> /dev/null || break
    sleep 0.2
  done
  kill -TERM "$pid" 2> /dev/null || true
  wait "$pid" || true
  return "$ready"
}

# 1. Git variables of the caller must not reach the fixture repository build.
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
control="$tmp/control"
git init -q "$control"
echo control > "$control/file"
git -C "$control" add file
git -C "$control" -c user.name=c -c user.email=c@example.com commit -q -m control
snapshot() {
  git -C "$control" rev-parse HEAD
  git -C "$control" for-each-ref
  sha256sum "$control/.git/index" | cut -d' ' -f1
  (cd "$control" && find . -path ./.git -prune -o -type f -print | sort)
}
before="$(snapshot)"
started=ok
start_and_stop git GIT_DIR="$control/.git" GIT_WORK_TREE="$control" GIT_INDEX_FILE="$control/.git/index" \
  SR_DEV_STATE_DIR="$tmp/git-state" || started="the stack did not start: $(tail -n 1 git.err)"
if [[ "$(snapshot)" != "$before" ]]; then
  result "git variables isolated" "the control repository changed"
else
  result "git variables isolated" "$started"
fi

# 2. A relative state dir is resolved against the caller's working directory.
if start_and_stop relative SR_DEV_STATE_DIR=relative-state && [[ -s "$tmp/relative-state/logs/server.log" ]]; then
  result "relative SR_DEV_STATE_DIR" ok
else
  result "relative SR_DEV_STATE_DIR" "the stack did not start: $(tail -n 1 relative.err)"
fi

# 3. Global setup start timeout while the launcher builds: a fake cargo that never ends must be gone afterwards.
marker="7777.$$"
mkdir bin
printf '#!/usr/bin/env bash\nsleep %s\n' "$marker" > bin/cargo
chmod +x bin/cargo
env -u SR_DEV_SERVER_BIN -u SR_DEV_RUNNER_BIN -u SR_DEV_FAKE_GITHUB_BIN PATH="$tmp/bin:$PATH" \
  SR_E2E_START_TIMEOUT_MS=5000 SR_DEV_STATE_DIR="$tmp/timeout-state" \
  node --input-type=module -e "
    const { default: setup } = await import('$repo/e2e/stack.ts');
    try { const stop = await setup(); await stop(); console.log('setup did not time out'); }
    catch (error) { console.log('setup failed as expected: ' + error.message); }" > timeout.out 2> timeout.err
if ! grep -q 'failed as expected' timeout.out; then
  result "build stopped on start timeout" "$(cat timeout.out)"
elif pgrep -f "sleep $marker" > /dev/null; then
  pkill -f "sleep $marker" || true
  result "build stopped on start timeout" "the build process survived the global setup"
else
  result "build stopped on start timeout" ok
fi

exit "$failed"
