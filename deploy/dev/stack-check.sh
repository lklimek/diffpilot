#!/usr/bin/env bash
# Checks of the dev stack launcher (stack.sh) and the e2e stack handling (e2e/global-setup.ts, e2e/support/stack.ts):
#   1. inherited GIT_DIR / GIT_WORK_TREE / GIT_INDEX_FILE leave a control repository untouched;
#   2. a relative SR_DEV_STATE_DIR works;
#   3. a start timeout while the launcher builds, and the e2e build timeout, leave no build process behind;
#   4. an e2e worker killed with SIGKILL leaves none of its stack's processes behind.
# Usage: deploy/dev/stack-check.sh   (SR_DEV_*_BIN as for stack.sh; without them checks 1-2 and 4 build first)
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

# 3. Build timeouts: a fake cargo that never ends must be gone afterwards (node runs the TypeScript directly).
marker="7777.$$"
mkdir bin
printf '#!/usr/bin/env bash\nsleep %s\n' "$marker" > bin/cargo
printf '#!/usr/bin/env bash\nexit 0\n' > bin/npm
chmod +x bin/cargo bin/npm
# timeout_check NAME SCRIPT: SCRIPT must print "failed as expected"; no `sleep $marker` may survive it.
timeout_check() {
  env -u SR_DEV_SERVER_BIN -u SR_DEV_RUNNER_BIN -u SR_DEV_FAKE_GITHUB_BIN -u SR_DEV_FAKE_MEMCAN_BIN \
    PATH="$tmp/bin:$PATH" SR_E2E_START_TIMEOUT_MS=5000 SR_E2E_BUILD_TIMEOUT_MS=5000 \
    SR_DEV_STATE_DIR="$tmp/timeout-state" node --input-type=module -e "$2" > "$1.out" 2> "$1.err" || true
  if ! grep -q 'failed as expected' "$1.out"; then
    result "$1" "$(cat "$1.out") $(tail -n 3 "$1.err")"
  elif sleep 1 && pgrep -f "sleep $marker" > /dev/null; then
    pkill -f "sleep $marker" || true
    result "$1" "the build process survived"
  else
    result "$1" ok
  fi
}
timeout_check "launcher build stopped on start timeout" "
  const { startStack } = await import('$repo/e2e/support/stack.ts');
  try { const { stop } = await startStack(); await stop(); console.log('the stack started'); }
  catch (error) { console.log('failed as expected: ' + error.message); }"
timeout_check "e2e build stopped on build timeout" "
  const { default: setup } = await import('$repo/e2e/global-setup.ts');
  try { await setup(); console.log('the build finished'); }
  catch (error) { console.log('failed as expected: ' + error.message); }"

# 4. A worker killed with SIGKILL: its stack guard stops the launcher, which stops every component.
node --input-type=module -e "
  const { startStack } = await import('$repo/e2e/support/stack.ts');
  await startStack();
  console.log('ready');
  setInterval(() => {}, 1000);" > worker.out 2> worker.err &
worker=$!
for _ in $(seq 600); do
  grep -q '^ready$' worker.out && break
  kill -0 "$worker" 2> /dev/null || break
  sleep 0.2
done
launcher="$(pgrep -P "$worker" -f 'deploy/dev/stack\.sh' | head -n 1 || true)"
if [[ -z "$launcher" ]] || ! grep -q '^ready$' worker.out; then
  kill -KILL "$worker" 2> /dev/null || true
  result "killed worker leaves no stack" "the stack did not start: $(tail -n 3 worker.err)"
else
  mapfile -t stack < <(echo "$launcher"; pgrep -P "$launcher" || true)
  kill -KILL "$worker"
  wait "$worker" 2> /dev/null || true
  left=("${stack[@]}")
  for _ in $(seq 150); do
    left=()
    for pid in "${stack[@]}"; do kill -0 "$pid" 2> /dev/null && left+=("$pid"); done
    [[ ${#left[@]} -eq 0 ]] && break
    sleep 0.2
  done
  if [[ ${#left[@]} -eq 0 ]]; then
    result "killed worker leaves no stack (${#stack[@]} processes)" ok
  else
    result "killed worker leaves no stack" "still running: $(IFS=,; ps -o pid=,stat=,args= -p "${left[*]}" | tr '\n' ' ')"
    kill -KILL "${left[@]}" 2> /dev/null || true
  fi
fi

exit "$failed"
