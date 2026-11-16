#!/usr/bin/env bash
# Real-Codex smoke (plan §8): ONE analysis of a public PR through the dev stack with the real Codex provider.
# Server and fake GitHub run on the host (stack.sh --without runner); the runner and Codex run in their own
# container. Sign-in stays on fake GitHub; the PR metadata comes from anonymous REST recordings; the clone base
# is https://github.com (public clone). The agent gets no GitHub token and no MCP server.
#
# Usage: real-codex-smoke.sh PR_URL RECORDINGS_DIR OUT_DIR
#   RECORDINGS_DIR  fake-GitHub recordings of the PR (rest/pull.json, pull-commits.json, pull-files.json,
#                   check-runs.json; rest/user.json and oauth/access-token.json copied from fixtures/pr-482/github)
#   OUT_DIR         archive: timeline, analysis, jobs (outcome, duration, usage), documents, tour, logs,
#                   screenshots; every file is scanned for secrets first and left out if one is found
#
# Inputs:
#   SR_DEV_SERVER_BIN, SR_DEV_FAKE_GITHUB_BIN, SR_DEV_RUNNER_BIN (all required; the runner runs inside the image,
#                         so it must be built against the image's libc; any build with the Codex provider)
#   SR_SMOKE_IMAGE        Codex image (default smart-review-spike-codex:0.167.0; Codex at /usr/local/bin/codex,
#                         user 10001, /codex-home owned by it)
#   SR_SMOKE_CODEX_VOLUME volume holding the Codex login auth.json (default sr-spike-codex-home)
#   SR_SMOKE_CAP_SECONDS  bound for the analysis (default 2700 = 45 min)
#   SR_SMOKE_PROVIDER     codex (default) or fake: plumbing dry run without an LLM call (runner built with
#                         feature fake-provider; replays the pr-482 recordings, so validation may fail)
#
# Runner container: own PID namespace with --init, 4 GiB memory, 1024 pids, all capabilities dropped,
# no-new-privileges, read-only root, no restart policy; network `sr-smoke` (172.31.250.0/24), which reaches the
# internet and the server's runner listener on the bridge address 172.31.250.1 only. Its private state volume
# `sr-smoke-runner-<run>` receives a copy of the credential volume to volume (never displayed); the volume is kept
# afterwards, because a rotated credential would exist only there. The container is stopped and removed when the
# analysis ends, fails, hits the cap or the script exits. Environment of the runner:
#   SR_RUNNER_SERVER_URL=http://172.31.250.1:<port>  SR_RUNNER_SECRET (this run's, via the docker CLI environment)
#   SR_RUNNER_ID=smoke-runner  SR_RUNNER_STATE_DIR=/var/lib/smart-review-runner  SR_RUNNER_PROVIDER=<provider>
#   SR_RUNNER_CODEX_BIN=/usr/local/bin/codex  RUST_LOG=info  NO_COLOR=1
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
pr_url="${1:?usage: real-codex-smoke.sh PR_URL RECORDINGS_DIR OUT_DIR}"
recordings="$(cd "${2:?recordings dir}" && pwd)"
out="${3:?out dir}"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
: "${SR_DEV_SERVER_BIN:?}" "${SR_DEV_FAKE_GITHUB_BIN:?}" "${SR_DEV_RUNNER_BIN:?}"
image="${SR_SMOKE_IMAGE:-smart-review-spike-codex:0.167.0}"
codex_volume="${SR_SMOKE_CODEX_VOLUME:-sr-spike-codex-home}"
cap="${SR_SMOKE_CAP_SECONDS:-2700}"
provider="${SR_SMOKE_PROVIDER:-codex}"
network=sr-smoke gateway=172.31.250.1
run="$(date -u +%Y%m%dt%H%M%S)"
container="sr-smoke-runner-$run" volume="sr-smoke-runner-$run"

log() { printf '%s %s\n' "$(date -u +%FT%TZ)" "$*" | tee -a "$out/timeline.txt" >&2; }
fail() { log "FAILED: $*"; exit 1; }

docker network inspect "$network" > /dev/null 2>&1 ||
  docker network create --driver bridge --subnet 172.31.250.0/24 --gateway "$gateway" "$network" > /dev/null
runner_port="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"

stack_pid=""
stage=""
# shellcheck disable=SC2317  # invoked by the EXIT trap
cleanup() {
  trap - EXIT INT TERM
  if docker container inspect "$container" > /dev/null 2>&1; then
    docker stop -t 30 "$container" > /dev/null 2>&1 || true
    if [[ -n "$stage" ]]; then docker logs "$container" > "$stage/runner.log" 2>&1 || true; fi
    docker rm -f "$container" > /dev/null 2>&1 || true
    log "runner container stopped and removed"
  fi
  if [[ -n "$stack_pid" ]]; then
    kill -TERM "$stack_pid" 2> /dev/null || true
    wait "$stack_pid" 2> /dev/null || true
  fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# --- stack on the host, runner listener on the smoke bridge ---------------------------------------------------
log "starting the stack (no runner)"
SR_DEV_GITHUB_RECORDINGS="$recordings" SR_DEV_CLONE_URL=https://github.com \
  SR_DEV_RUNNER_LISTEN_ADDR="$gateway:$runner_port" \
  "$here/stack.sh" --without runner > "$out/.stack.out" 2> "$out/.stack.err" &
stack_pid=$!
for _ in $(seq 300); do
  grep -q '^SR_DEV_STATE_DIR=' "$out/.stack.out" && break
  kill -0 "$stack_pid" 2> /dev/null || fail "the stack did not start; see $out/.stack.err"
  sleep 0.2
done
url="$(sed -n 's/^SR_DEV_URL=//p' "$out/.stack.out")"
state="$(sed -n 's/^SR_DEV_STATE_DIR=//p' "$out/.stack.out")"
rm -f "$out/.stack.out"
[[ -n "$url" && -n "$state" ]] || fail "the stack did not report its URL"
stage="$state/smoke"
mkdir -p "$stage"
mv "$out/.stack.err" "$stage/stack.err"
log "stack ready at $url, state $state"

# --- runner container -----------------------------------------------------------------------------------------
docker volume create "$volume" > /dev/null
# Mounted at /codex-home first, the empty volume takes that dir's owner (10001) and mode (0700) from the image.
docker run --rm --init --network none --user 10001:10001 --cap-drop ALL --security-opt no-new-privileges \
  --mount "type=volume,source=$codex_volume,target=/src,readonly" \
  --mount "type=volume,source=$volume,target=/codex-home" "$image" sh -c '
    install -d -m 700 /codex-home/credentials/default &&
    cp /src/auth.json /codex-home/credentials/default/auth.json &&
    chmod 600 /codex-home/credentials/default/auth.json &&
    jq -e "(.tokens.account_id // \"\") | length > 0" /codex-home/credentials/default/auth.json > /dev/null' ||
  fail "the credential copy failed or has no account_id"
log "credential copied into volume $volume (account_id present)"

SR_RUNNER_SECRET="$(sed -n 's/^SR_RUNNER_SECRET=//p' "$state/stack.env" | head -n 1)"
export SR_RUNNER_SECRET
docker run -d --name "$container" --init --restart no \
  --memory 4g --memory-swap 4g --pids-limit 1024 \
  --user 10001:10001 --cap-drop ALL --security-opt no-new-privileges --read-only \
  --tmpfs /tmp:rw,nosuid,nodev,size=512m \
  --tmpfs /home/runner:rw,nosuid,nodev,size=64m,uid=10001,gid=10001,mode=0700 \
  --network "$network" \
  --mount "type=bind,source=$(realpath "$SR_DEV_RUNNER_BIN"),target=/usr/local/bin/smart-review-runner,readonly" \
  --mount "type=volume,source=$volume,target=/var/lib/smart-review-runner" \
  -e SR_RUNNER_SERVER_URL="http://$gateway:$runner_port" -e SR_RUNNER_SECRET -e SR_RUNNER_ID=smoke-runner \
  -e SR_RUNNER_STATE_DIR=/var/lib/smart-review-runner -e SR_RUNNER_PROVIDER="$provider" \
  -e SR_RUNNER_CODEX_BIN=/usr/local/bin/codex -e RUST_LOG=info -e NO_COLOR=1 \
  "$image" smart-review-runner > /dev/null
unset SR_RUNNER_SECRET
log "runner container $container started (provider $provider)"

# --- one analysis ---------------------------------------------------------------------------------------------
jar="$state/smoke.jar"
login="$(curl -sf -c "$jar" -b "$jar" -o /dev/null -w '%{redirect_url}' "$url/auth/github/login")"
callback="$(curl -sf -o /dev/null -w '%{redirect_url}' "$login")"
curl -sf -c "$jar" -b "$jar" -o /dev/null "$callback"
csrf="$(curl -sf -b "$jar" "$url/api/v1/me" | jq -r .csrf_token)"
[[ -n "$csrf" && "$csrf" != null ]] || fail "sign-in failed"
api() { curl -s -b "$jar" -H "X-CSRF-Token: $csrf" -H 'Content-Type: application/json' "$@"; }
resolved="$(api -d "$(jq -n --arg url "$pr_url" '{url: $url}')" "$url/api/v1/pr/resolve")"
jq -e .pr > /dev/null <<< "$resolved" || fail "resolve: $resolved"
created="$(api -d "$(jq '{pr: {repo: .pr.repo, number: .pr.number}, head_sha: .pr.head_sha}' <<< "$resolved")" \
  "$url/api/v1/analyses")"
id="$(jq -r '.id // empty' <<< "$created")"
[[ -n "$id" ]] || fail "create analysis: $created"
log "analysis $id requested for $pr_url"

last="" started="$(date +%s)"
while :; do
  current="$(api "$url/api/v1/analyses/$id" | jq -c .state)"
  [[ "$current" == "$last" ]] || log "state $current"
  last="$current"
  case "$(jq -r .state <<< "$current")" in finished | failed | cancelled) break ;; esac
  if (($(date +%s) - started > cap)); then
    log "cap of $cap s reached; stopping"
    break
  fi
  [[ "$(docker container inspect -f '{{.State.Running}}' "$container" 2> /dev/null)" == true ]] ||
    { log "the runner container is not running"; break; }
  sleep 10
done

# --- collect --------------------------------------------------------------------------------------------------
api "$url/api/v1/analyses/$id" | jq . > "$stage/analysis.json"
api "$url/api/v1/analyses/$id/tour" | jq . > "$stage/tour.json"
db="$state/server/smart-review.sqlite"
sqlite3 -json "$db" "SELECT kind, stage, state, attempt, generation, flow_id, created_at, claimed_at, finished_at,
    ROUND((julianday(finished_at) - julianday(claimed_at)) * 86400, 1) AS run_seconds,
    json_remove(json_extract(result, '$.outcome'), '$.output') AS outcome, json_extract(result, '$.usage') AS usage,
    json_extract(result, '$.llm_call') AS llm_call
  FROM jobs WHERE analysis_id = '$id' ORDER BY created_at" |
  jq 'map(.outcome |= (fromjson? // .) | .usage |= (fromjson? // .) | .llm_call |= (fromjson? // .))' > "$stage/jobs.json"
sqlite3 -json "$db" "SELECT kind, key, json(body) AS body FROM documents WHERE analysis_id = '$id'
  ORDER BY kind, key" | jq '[.[] | .body |= fromjson]' > "$stage/documents.json"
sqlite3 -json "$db" "SELECT failure_flow_id, failure_flow_title, failure_message, agent_thread_id IS NOT NULL
  AS has_thread FROM analyses WHERE id = '$id'" | jq . > "$stage/analysis-failure.json"
if [[ "$(jq -r .state.state "$stage/analysis.json")" == finished ]]; then
  # shellcheck disable=SC2016  # JavaScript template literals
  (cd "$repo/e2e" && SMOKE_URL="$url" SMOKE_ID="$id" SMOKE_DIR="$stage" node --input-type=module -e '
    const { chromium } = await import("@playwright/test");
    const browser = await chromium.launch();
    const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
    const { SMOKE_URL: url, SMOKE_ID: id, SMOKE_DIR: dir } = process.env;
    await page.goto(`${url}/auth/github/login`);
    await page.goto(`${url}/analyses/${id}`);
    const heading = page.getByRole("main").getByRole("heading", { level: 2 }).first();
    for (const name of ["pr-context", "flow-intro", "step1"]) {
      await heading.waitFor();
      if (name === "step1") await page.getByRole("region", { name: /, lines \d+ to \d+$/ }).waitFor();
      await page.screenshot({ path: `${dir}/tour-${name}.png`, fullPage: true });
      await page.keyboard.press("j");
    }
    await browser.close();') || log "screenshots failed"
fi
cleanup
cp "$state/logs/server.log" "$state/logs/fake-github.log" "$stage/"
docker run --rm --network none --user 10001:10001 --cap-drop ALL \
  --mount "type=volume,source=$volume,target=/s,readonly" "$image" \
  sh -c 'find /s -maxdepth 4 \( -name "auth.json.blocked" -o -name "auth.json.rotated-*" -o -name "*-kept" \) \
    -printf "%P\n"' > "$stage/credential-markers.txt"
log "credential markers in $volume: $(tr '\n' ' ' < "$stage/credential-markers.txt")"

# --- secret scan, then archive --------------------------------------------------------------------------------
# Values known on the host: generated secrets, fixture OAuth values, the session cookie and CSRF token.
{
  sed -n 's/^\(SR_SECRET_KEY\|SR_RUNNER_SECRET\|SR_GITHUB_CLIENT_SECRET\|SR_MEMCAN_TOKEN\)=//p' "$state/stack.env"
  jq -r '.response | .access_token, .refresh_token' "$recordings/oauth/access-token.json"
  jq -r '.request.body.code' "$recordings/oauth/access-token.json"
  awk '{ sub(/^#HttpOnly_/, "") } !/^#/ && NF >= 7 { print $7 }' "$jar"
  echo "$csrf"
} | awk 'length >= 8' | sort -u > "$state/scan-values"
leaks="$(grep -rlF -f "$state/scan-values" "$stage" || true)"
rm -f "$state/scan-values"
# Credential values are compared inside a container; only names of matching files come out.
leaks+=$'\n'"$(docker run --rm --network none --user 10001:10001 --cap-drop ALL \
  --mount "type=volume,source=$volume,target=/s,readonly" \
  --mount "type=bind,source=$stage,target=/scan,readonly" "$image" sh -c '
    find /s -name "auth.json*" -type f -exec jq -r "(.tokens | objects | to_entries[] | select(.key != \"account_id\") | .value | strings),
      (.OPENAI_API_KEY | strings)" {} + 2> /dev/null | awk "length >= 8" |
      sort -u > /tmp/values
    [ -s /tmp/values ] && grep -rlF -f /tmp/values /scan | sed "s|^/scan/||"; true')"
leaks="$(sed '/^$/d; s|^'"$stage"'/||' <<< "$leaks" | sort -u)"
for file in "$stage"/*; do
  name="${file##*/}"
  if grep -qx -- "$name" <<< "$leaks"; then
    log "NOT ARCHIVED: $name contains a secret value"
  else
    cp "$file" "$out/"
  fi
done
log "archived to $out; runner state volume $volume kept"
