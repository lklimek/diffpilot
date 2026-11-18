#!/usr/bin/env bash
# Real-Codex smoke (plan §8): ONE analysis of a public PR through the dev stack with the real Codex provider.
# Server and fake GitHub run on the host (stack.sh --without runner); the runner and Codex run in their own
# container. Sign-in stays on fake GitHub; the PR metadata comes from anonymous REST recordings; the clone base
# is https://github.com (public clone). The agent gets no GitHub token and no MCP server.
#
# Usage: real-codex-smoke.sh PR_URL RECORDINGS_DIR OUT_DIR
#   RECORDINGS_DIR  fake-GitHub recordings of the PR (rest/pull.json, pull-commits.json, pull-files.json,
#                   check-runs.json; rest/user.json and oauth/access-token.json copied from fixtures/pr-482/github)
#   OUT_DIR         archive (absent or empty): timeline, analysis, jobs (outcome, duration, usage), documents,
#                   tour, logs, screenshots, credential marker counts. Everything is collected in a private stage
#                   (/data/tmp/smart-review-smoke.XXXXXX/stage) and published only by smoke-publish.sh after a
#                   complete secret scan; nothing is published when the scan is incomplete or the run fails early.
#   The terminal shows fixed text and validated values only (ids, states, counts); server answers stay in the
#   private timeline.
#
# Inputs:
#   SR_DEV_SERVER_BIN, SR_DEV_FAKE_GITHUB_BIN, SR_DEV_RUNNER_BIN (all required; the runner runs inside the image,
#                         so it must be built against the image's libc; any build with the Codex provider)
#   SR_SMOKE_IMAGE        Codex image (default smart-review-spike-codex:0.167.0; Codex at /usr/local/bin/codex,
#                         user 10001, /codex-home owned by it)
#   SR_SMOKE_CODEX_VOLUME volume holding the Codex login auth.json (default sr-spike-codex-home)
#   SR_SMOKE_STATE_VOLUME an existing runner state volume that already holds the credential
#                         (credentials/default/auth.json), e.g. a stopped local compose stack's runner-state: used
#                         as the runner's state, nothing is copied, so only one live copy of the credential exists;
#                         unset: a new volume sr-smoke-runner-<run> with a copy of SR_SMOKE_CODEX_VOLUME
#   SR_SMOKE_CAP_SECONDS  bound for the analysis (default 2700 = 45 min)
#   SR_SMOKE_PROVIDER     codex (default) or fake: plumbing dry run without an LLM call (runner built with
#                         feature fake-provider; replays the pr-482 recordings, so validation may fail)
#   SR_SMOKE_ISOLATION    none (default; described below) or supervisor: the production runner image
#                         (SR_SMOKE_RUNNER_IMAGE, default smart-review-runner:local) under its root supervisor with
#                         the ADR-013 options of deploy/compose.yaml, behind the egress proxy
#                         (SR_SMOKE_PROXY_IMAGE, default smart-review-egress-proxy:local) on the internal network
#                         `sr-smoke-int` (172.31.251.0/24, removed at the end); the image's own runner unless
#                         SR_SMOKE_PROVIDER=fake (then SR_DEV_RUNNER_BIN is mounted over it); per-job CODEX_HOME root
#                         on a capped tmpfs (SR_CODEX_HOMES_ALLOW_TMPFS=1, development only); the internal network keeps
#                         a host address for the host's server, so the preflight's isolation check is skipped
#                         (SR_RUNNER_DEV_HOST_GATEWAY=1, development only); the proxy's host-name
#                         log is archived as egress-proxy.log
#
# Runner container: own PID namespace with --init, 4 GiB memory, 1024 pids, all capabilities dropped,
# no-new-privileges, read-only root, no restart policy; network `sr-smoke` (172.31.250.0/24), which reaches the
# internet and the server's runner listener on the bridge address 172.31.250.1 only. Its private state volume
# `sr-smoke-runner-<run>` receives a copy of the credential volume to volume (never displayed); the volume is kept
# afterwards, because a rotated credential would exist only there. The container is stopped and removed when the
# analysis ends, fails, hits the cap or the script exits. Environment of the runner:
#   SR_RUNNER_SERVER_URL=http://172.31.250.1:<port>  SR_RUNNER_SECRET (this run's, via the docker CLI environment)
#   SR_RUNNER_ID=smoke-runner  SR_RUNNER_STATE_DIR=/var/lib/smart-review-runner  SR_RUNNER_PROVIDER=<provider>
#   SR_RUNNER_CODEX_BIN=/usr/local/bin/codex  RUST_LOG=info  NO_COLOR=1  SR_RUNNER_DEV_NO_ISOLATION=1 (no supervisor)
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
pr_url="${1:?usage: real-codex-smoke.sh PR_URL RECORDINGS_DIR OUT_DIR}"
recordings="$(cd "${2:?recordings dir}" && pwd)"
out="$(realpath -m "${3:?out dir}")"
[[ ! -e "$out" || -z "$(ls -A "$out")" ]] || { echo "real-codex-smoke.sh: $out is not empty" >&2; exit 1; }
: "${SR_DEV_SERVER_BIN:?}" "${SR_DEV_FAKE_GITHUB_BIN:?}" "${SR_DEV_RUNNER_BIN:?}"
image="${SR_SMOKE_IMAGE:-smart-review-spike-codex:0.167.0}"
codex_volume="${SR_SMOKE_CODEX_VOLUME:-sr-spike-codex-home}"
cap="${SR_SMOKE_CAP_SECONDS:-2700}"
provider="${SR_SMOKE_PROVIDER:-codex}"
network=sr-smoke gateway=172.31.250.1
isolation="${SR_SMOKE_ISOLATION:-none}"
case "$isolation" in
  none | supervisor) ;;
  *) echo "real-codex-smoke.sh: SR_SMOKE_ISOLATION must be none or supervisor" >&2; exit 2 ;;
esac
inner=sr-smoke-int proxy_ip=172.31.251.3
if [[ "$isolation" == supervisor ]]; then gateway=172.31.251.1; fi
run="$(date -u +%Y%m%dt%H%M%S)"
container="sr-smoke-runner-$run" volume="${SR_SMOKE_STATE_VOLUME:-sr-smoke-runner-$run}" proxy="sr-smoke-proxy-$run"

mkdir -p /data/tmp
private="$(mktemp -d /data/tmp/smart-review-smoke.XXXXXX)"
stage="$private/stage"
mkdir "$stage"
# log: private timeline only (published after the scan); say: also the terminal, for fixed text and validated
# values; fail: a fixed reason on the terminal, details only in the timeline.
log() { printf '%s %s\n' "$(date -u +%FT%TZ)" "$*" >> "$stage/timeline.txt"; }
say() {
  log "$*"
  printf '%s %s\n' "$(date -u +%FT%TZ)" "$*" >&2
}
fail() {
  say "FAILED: $1 (nothing published; private stage $stage)"
  if [[ -n "${2:-}" ]]; then log "details: $2"; fi
  exit 1
}

docker network inspect "$network" > /dev/null 2>&1 ||
  docker network create --driver bridge --subnet 172.31.250.0/24 --gateway 172.31.250.1 "$network" > /dev/null
if [[ "$isolation" == supervisor ]]; then
  # Internal: no route out; the host (server's runner listener) is reachable on the gateway address only.
  docker network inspect "$inner" > /dev/null 2>&1 ||
    docker network create --driver bridge --internal --subnet 172.31.251.0/24 --gateway "$gateway" "$inner" > /dev/null
fi
runner_port="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"

stack_pid=""
# shellcheck disable=SC2317  # invoked by the EXIT trap
cleanup() {
  trap - EXIT INT TERM
  if docker container inspect "$container" > /dev/null 2>&1; then
    docker stop -t 30 "$container" > /dev/null 2>&1 || true
    docker logs "$container" > "$stage/runner.log" 2>&1 || true
    docker rm -f "$container" > /dev/null 2>&1 || true
    say "runner container stopped and removed"
  fi
  if [[ -n "$stack_pid" ]]; then
    kill -TERM "$stack_pid" 2> /dev/null || true
    wait "$stack_pid" 2> /dev/null || true
    stack_pid=""
  fi
  if docker container inspect "$proxy" > /dev/null 2>&1; then
    docker logs "$proxy" > "$stage/egress-proxy.log" 2>&1 || true
    docker rm -f "$proxy" > /dev/null 2>&1 || true
    say "egress proxy stopped and removed"
  fi
  if [[ "$isolation" == supervisor ]]; then docker network rm "$inner" > /dev/null 2>&1 || true; fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# --- stack on the host, runner listener on the smoke bridge ---------------------------------------------------
say "starting the stack (no runner)"
SR_DEV_GITHUB_RECORDINGS="$recordings" SR_DEV_CLONE_URL=https://github.com \
  SR_DEV_RUNNER_LISTEN_ADDR="$gateway:$runner_port" \
  "$here/stack.sh" --without runner > "$private/stack.out" 2> "$stage/stack.err" &
stack_pid=$!
for _ in $(seq 300); do
  grep -q '^SR_DEV_STATE_DIR=' "$private/stack.out" && break
  kill -0 "$stack_pid" 2> /dev/null || fail "the stack did not start"
  sleep 0.2
done
url="$(sed -n 's/^SR_DEV_URL=//p' "$private/stack.out")"
state="$(sed -n 's/^SR_DEV_STATE_DIR=//p' "$private/stack.out")"
[[ -n "$url" && -n "$state" ]] || fail "the stack did not report its URL"
say "stack ready at $url, state $state"

# --- runner container -----------------------------------------------------------------------------------------
if [[ -n "${SR_SMOKE_STATE_VOLUME:-}" ]]; then
  docker volume inspect "$volume" > /dev/null 2>&1 || fail "SR_SMOKE_STATE_VOLUME $volume does not exist"
  docker run --rm --init --network none --user 10001:10001 --cap-drop ALL --security-opt no-new-privileges \
    --mount "type=volume,source=$volume,target=/codex-home,readonly" "$image" sh -c '
      jq -e "(.tokens.account_id // \"\") | length > 0" /codex-home/credentials/default/auth.json > /dev/null' ||
    fail "the state volume has no credential with an account_id"
  say "existing state volume $volume used (credential present with account_id; nothing copied)"
else
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
  say "credential copied into volume $volume (account_id present)"
fi

SR_RUNNER_SECRET="$(sed -n 's/^SR_RUNNER_SECRET=//p' "$state/stack.env" | head -n 1)"
export SR_RUNNER_SECRET
if [[ "$isolation" == supervisor ]]; then
  # Egress proxy: the runner network's only way out (no MemCan exception: the dev stack has no MemCan).
  docker run -d --name "$proxy" --init --read-only --cap-drop ALL --security-opt no-new-privileges \
    --memory 256m --pids-limit 128 --tmpfs /run/squid:uid=13,gid=13,mode=0750,size=1m \
    --network "$network" "${SR_SMOKE_PROXY_IMAGE:-smart-review-egress-proxy:local}" > /dev/null
  docker network connect --ip "$proxy_ip" "$inner" "$proxy"
  for _ in $(seq 60); do
    [[ "$(docker inspect -f '{{.State.Health.Status}}' "$proxy")" == healthy ]] && break
    sleep 1
  done
  [[ "$(docker inspect -f '{{.State.Health.Status}}' "$proxy")" == healthy ]] || fail "the egress proxy is not healthy"
  bin_mount=()
  if [[ "$provider" == fake ]]; then
    bin_mount=(--mount "type=bind,source=$(realpath "$SR_DEV_RUNNER_BIN"),target=/usr/local/bin/smart-review-runner,readonly")
  fi
  # ADR-013 options as in deploy/compose.yaml; the image's entrypoint is the supervisor, its CMD the preflight.
  docker run -d --name "$container" --init --restart no --ipc none \
    --memory 4g --memory-swap 4g --pids-limit 1024 \
    --cap-drop ALL --cap-add CHOWN --cap-add SETUID --cap-add SETGID --cap-add KILL \
    --security-opt no-new-privileges --read-only \
    --tmpfs /work:rw,nosuid,nodev,size=4g,uid=10001,gid=10001,mode=0711 \
    --tmpfs /tmp:rw,nosuid,nodev,size=256m,uid=10001,gid=10001,mode=0700 \
    --network "$inner" "${bin_mount[@]}" \
    --mount "type=volume,source=$volume,target=/var/lib/smart-review-runner" \
    --tmpfs /var/lib/smart-review-runner/codex-homes:rw,nosuid,nodev,noexec,size=1g,nr_inodes=131072,uid=10001,gid=10001,mode=0711 \
    -e SR_RUNNER_SERVER_URL="http://$gateway:$runner_port" -e SR_RUNNER_SECRET -e SR_RUNNER_ID=smoke-runner \
    -e SR_RUNNER_PROVIDER="$provider" -e SR_RUNNER_EGRESS_PROXY="http://$proxy_ip:3128" \
    -e HTTPS_PROXY="http://$proxy_ip:3128" -e https_proxy="http://$proxy_ip:3128" \
    -e NO_PROXY="localhost,127.0.0.1,$gateway" -e no_proxy="localhost,127.0.0.1,$gateway" \
    -e SR_CODEX_HOMES_ALLOW_TMPFS=1 -e SR_CODEX_HOMES_TMPFS_PUBLIC_URL="$url" -e SR_RUNNER_DEV_HOST_GATEWAY=1 \
    -e RUST_LOG=info -e NO_COLOR=1 \
    "${SR_SMOKE_RUNNER_IMAGE:-smart-review-runner:local}" > /dev/null
else
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
    -e SR_RUNNER_CODEX_BIN=/usr/local/bin/codex -e RUST_LOG=info -e NO_COLOR=1 -e SR_RUNNER_DEV_NO_ISOLATION=1 \
    "$image" smart-review-runner > /dev/null
fi
unset SR_RUNNER_SECRET
say "runner container $container started (provider $provider, isolation $isolation)"

# --- one analysis ---------------------------------------------------------------------------------------------
jar="$state/smoke.jar"
login="$(curl -sf -c "$jar" -b "$jar" -o /dev/null -w '%{redirect_url}' "$url/auth/github/login")"
callback="$(curl -sf -o /dev/null -w '%{redirect_url}' "$login")"
curl -sf -c "$jar" -b "$jar" -o /dev/null "$callback"
csrf="$(curl -sf -b "$jar" "$url/api/v1/me" | jq -r .csrf_token)"
[[ -n "$csrf" && "$csrf" != null ]] || fail "sign-in failed"
api() { curl -s -b "$jar" -H "X-CSRF-Token: $csrf" -H 'Content-Type: application/json' "$@"; }
resolved="$(api -d "$(jq -n --arg url "$pr_url" '{url: $url}')" "$url/api/v1/pr/resolve")"
jq -e .pr > /dev/null <<< "$resolved" || fail "the PR could not be resolved" "$resolved"
created="$(api -d "$(jq '{pr: {repo: .pr.repo, number: .pr.number}, head_sha: .pr.head_sha}' <<< "$resolved")" \
  "$url/api/v1/analyses")"
id="$(jq -r '.id // empty' <<< "$created")"
[[ "$id" =~ ^[0-9a-f-]{36}$ ]] || fail "the analysis could not be created" "$created"
say "analysis $id requested"
# A new analysis analyses flows on request; the smoke runs every flow, so background analysis goes on.
background="$(api -X PUT -d '{"enabled":true}' "$url/api/v1/analyses/$id/background")"
[[ "$(jq -r .background_analysis <<< "$background")" == true ]] ||
  fail "background analysis could not be switched on" "$background"

last="" started="$(date +%s)"
while :; do
  current="$(api "$url/api/v1/analyses/$id" | jq -c .state)"
  if [[ "$current" != "$last" ]]; then
    log "state $current"
    # Only the state name and the counters reach the terminal.
    say "state $(jq -r '[.state, (.done // empty), (.total // empty)] | map(tostring | select(test("^[a-z_0-9]+$")))
      | join(" ")' <<< "$current")"
  fi
  last="$current"
  case "$(jq -r .state <<< "$current")" in finished | failed | cancelled) break ;; esac
  if (($(date +%s) - started > cap)); then
    say "cap of $cap s reached; stopping"
    break
  fi
  [[ "$(docker container inspect -f '{{.State.Running}}' "$container" 2> /dev/null)" == true ]] ||
    { say "the runner container is not running"; break; }
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
    await browser.close();' 2>> "$stage/timeline.txt") || say "screenshots failed (details in the timeline)"
fi
cleanup
cp "$state/logs/server.log" "$state/logs/fake-github.log" "$stage/"

# --- scan, then publish -----------------------------------------------------------------------------------------
# Values known on the host: generated secrets, fixture OAuth values, the session cookie and CSRF token.
{
  sed -n 's/^\(SR_SECRET_KEY\|SR_RUNNER_SECRET\|SR_GITHUB_CLIENT_SECRET\|SR_MEMCAN_TOKEN\)=//p' "$state/stack.env"
  jq -r '.response | .access_token, .refresh_token' "$recordings/oauth/access-token.json"
  jq -r '.request.body.code' "$recordings/oauth/access-token.json"
  awk '{ sub(/^#HttpOnly_/, "") } !/^#/ && NF >= 7 { print $7 }' "$jar"
  echo "$csrf"
} | awk 'length >= 8' | sort -u > "$private/host-values"
# 4 generated secrets, 3 fixture values, the session cookie and the CSRF token.
(($(wc -l < "$private/host-values") >= 9)) || fail "fewer host values than expected for the scan"
log "analysis $id ended; publishing after the secret scan"
SR_SMOKE_IMAGE="$image" "$here/smoke-publish.sh" "$stage" "$out" "$volume" "$private/host-values" ||
  fail "the secret scan was incomplete"
say "published to $out; runner state volume $volume kept; private stage $private"
