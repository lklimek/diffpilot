#!/usr/bin/env bash
# Checks of smoke-publish.sh with FAKE credentials in a throwaway Docker volume (needs Docker and the smoke image).
# Usage: deploy/dev/smoke-publish-check.sh   (SR_SMOKE_IMAGE as for real-codex-smoke.sh;
#        SR_SMOKE_PUBLISH=<script> runs the checks against another implementation)
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
publish="${SR_SMOKE_PUBLISH:-$here/smoke-publish.sh}"
image="${SR_SMOKE_IMAGE:-diffpilot-spike-codex:0.167.0}"
mkdir -p /data/tmp
tmp="$(mktemp -d /data/tmp/smoke-publish-check.XXXXXX)"
# shellcheck disable=SC2317  # invoked by the EXIT trap
cleanup() {
  docker volume ls -q --filter "name=sr-publish-check-$$-" | xargs -r docker volume rm -f > /dev/null 2>&1 || true
  chmod -R u+rwX "$tmp" 2> /dev/null || true
  rm -rf "$tmp"
}
trap cleanup EXIT
failed=0
result() { if [[ "$2" == ok ]]; then echo "ok   $1"; else echo "FAIL $1: $2"; failed=1; fi; }

# Fake credential values; SYNTHLEAK marks values an agent could smuggle into marker names.
good_auth='{"auth_mode":"chatgpt","tokens":{"id_token":"fake-id-token-0001","access_token":"fake-access-token-0002","refresh_token":"fake-refresh-token-0003","account_id":"fake-account"}}'
host_secret=fake-host-secret-value-1

# volume NAME AUTH_JSON [SHELL]: a volume like the runner state, credential at credentials/default/auth.json.
volume() {
  local name="sr-publish-check-$$-$1"
  docker volume create "$name" > /dev/null
  docker run --rm -i --network none --user 10001:10001 --mount "type=volume,source=$name,target=/codex-home" \
    "$image" sh -c "install -d -m 700 /codex-home/credentials/default && cat > /codex-home/credentials/default/auth.json
      cd /codex-home/credentials/default && ${3:-:}" <<< "$2"
  echo "$name"
}
# case_dir NAME: a stage with clean files and a screenshot, a host value list, and an absent OUT dir.
case_dir() {
  local dir="$tmp/$1"
  mkdir -p "$dir/stage"
  echo "started" > "$dir/stage/timeline.txt"
  echo '{"state":"finished"}' > "$dir/stage/analysis.json"
  printf 'PNG fake image' > "$dir/stage/tour-step1.png"
  echo "$host_secret" > "$dir/values"
  echo "$dir"
}
run() { "$publish" "$1/stage" "$1/out" "$2" "$1/values" > "$1/output.txt" 2>&1; }
published() { find "$1/out" -type f -printf '%f\n' 2> /dev/null | sort | tr '\n' ' '; }

# 1. Marker names from the agent-writable volume never reach the terminal or OUT_DIR; counts do.
dir="$(case_dir markers)"
vol="$(volume markers "$good_auth" 'touch auth.json.blocked auth.json.rotated-SYNTHLEAKA1B2C3D4 && mkdir SYNTHLEAKE5F6G7H8-kept')"
if ! run "$dir" "$vol"; then
  result "markers reported as counts only" "publication failed: $(tail -n 1 "$dir/output.txt")"
elif grep -rq SYNTHLEAK "$dir/output.txt" "$dir/out"; then
  result "markers reported as counts only" "a marker name was printed or archived"
elif ! grep -qx 'blocked=1 rotated=1 kept=1' "$dir/out/credential-markers.txt" 2> /dev/null; then
  result "markers reported as counts only" "credential-markers.txt lacks the counts"
else
  result "markers reported as counts only" ok
fi

# 2. A leaked credential or host value withholds its file and every screenshot; clean files are published.
dir="$(case_dir leak)"
vol="$(volume leak "$good_auth")"
echo 'x fake-refresh-token-0003 y' > "$dir/stage/documents.json"
echo "y $host_secret" > "$dir/stage/runner.log"
if ! run "$dir" "$vol"; then
  result "leak withholds file and screenshots" "publication failed: $(tail -n 1 "$dir/output.txt")"
elif [[ "$(published "$dir")" != "analysis.json credential-markers.txt publish.txt timeline.txt " ]]; then
  result "leak withholds file and screenshots" "published: $(published "$dir")"
else
  result "leak withholds file and screenshots" ok
fi

# 3-6. An incomplete scan publishes nothing.
expect_abort() {
  if run "$1" "$2"; then
    result "$3" "published although the scan was incomplete: $(published "$1")"
  elif [[ -n "$(published "$1")" ]]; then
    result "$3" "failed but published: $(published "$1")"
  elif ! grep -q 'nothing published:' "$1/output.txt"; then
    result "$3" "failed without the abort message"
  else
    result "$3" ok
    sed -n 's/^smoke-publish: nothing published: /       reason: /p' "$1/output.txt"
  fi
}
dir="$(case_dir unparsable)"
expect_abort "$dir" "$(volume unparsable '{"auth_mode":')" "unparsable credential aborts"
dir="$(case_dir fields)"
expect_abort "$dir" "$(volume fields '{"auth_mode":"chatgpt","tokens":{"account_id":"fake-account"}}')" \
  "credential without tokens aborts"
dir="$(case_dir unreadable)"
echo secret > "$dir/stage/server.log"
chmod 000 "$dir/stage/server.log"
expect_abort "$dir" "$(volume unreadable "$good_auth")" "unreadable stage file aborts"
dir="$(case_dir empty-values)"
: > "$dir/values"
expect_abort "$dir" "$(volume empty-values "$good_auth")" "empty host value list aborts"

exit "$failed"
