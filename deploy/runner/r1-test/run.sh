#!/usr/bin/env bash
# R1 container test (ADR-013, threat model T1/T4/T8b/T13): proves the runner process model in a container started
# with the production security options. Prints `CHECK <name> PASS|FAIL <detail>` lines and a RESULT line; exit 0
# only when every check passes. No real credential is used: canaries and fake keys only, network none.
#
# Usage: run.sh [LEDGER_FILE]     (default /data/tmp/r1-test/<timestamp>.log); the production image is also tagged
#                                 smart-review-runner:local (the compose default, used by deploy/test-stack.sh)
# Env:   R1_SKIP_BUILD=1          reuse the images built earlier
#        R1_SKIP_CODEX=1          skip the real-Codex job
#        R1_SUPERVISOR=rust       run the checks against the Rust smart-review-supervisor (default: python prototype)
#        R1_{BASE_IMAGE,SKELETON_IMAGE,TEST_IMAGE,RUNNER_TAG}  isolated image tags (defaults below)
#        R1_WORKSPACE            explicit source context (e.g. a secret-free staging directory)
# A second container (fault injection R1_FAULT_SWEEP_FAILS=1) proves that a failing sweep ends the container.
# /work and /tmp are this run's Docker volumes, as the tmp-* volumes of deploy/compose.yaml. codex-homes is a
# size- and inode-capped tmpfs here, a test-only stand-in for a full disk (codex_home_exhaustion_fails_job_cleanly;
# production has no size bound, ADR-013). The production image (target `runner`) is checked for toolchains and for
# its preflight (network isolation); the test image derives from target `base`.
# Heavy job: builds two images (one at a time); do not run while a cargo job of this session runs.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
run_id="$(date -u +%Y%m%dt%H%M%S)"
workspace="${R1_WORKSPACE:-$here/../../..}"
ledger="${1:-/data/tmp/r1-test/${run_id}.log}"
mkdir -p "$(dirname "$ledger")"
skeleton="${R1_SKELETON_IMAGE:-smart-review-runner:skeleton}"
base="${R1_BASE_IMAGE:-smart-review-runner:base}"
image="${R1_TEST_IMAGE:-smart-review-runner-r1-test:local}"
runner_tag="${R1_RUNNER_TAG:-smart-review-runner:local}"
container="sr-r1-test-${run_id}"
failing="sr-r1-fail-${run_id}"
state="sr-r1-state-${run_id}"
preflight="sr-r1-preflight-${run_id}"
pf_state="sr-r1-pfstate-${run_id}"
pf_nets=("sr-r1-internal-${run_id}" "sr-r1-isolated-${run_id}" "sr-r1-bridge-${run_id}")
# Disposable volumes (created on first mount): the test containers' and the preflight containers' /work, /tmp and
# (preflight only) codex-homes.
scratch=("sr-r1-work-${run_id}" "sr-r1-tmp-${run_id}" "sr-r1-pfwork-${run_id}" "sr-r1-pftmp-${run_id}"
  "sr-r1-pfhomes-${run_id}")

cleanup() {
  docker rm -f "$container" "$failing" "$preflight" > /dev/null 2>&1 || true
  docker volume rm "$state" "$pf_state" "${scratch[@]}" > /dev/null 2>&1 || true
  docker network rm "${pf_nets[@]}" > /dev/null 2>&1 || true
}

# start NAME [docker run args...]: the production security options (ADR-013) plus the test mounts.
start() {
  local name="$1"
  shift
  docker run -d --name "$name" --init --network none --ipc none \
    --cap-drop ALL --cap-add CHOWN --cap-add SETUID --cap-add SETGID --cap-add KILL \
    --security-opt no-new-privileges --read-only \
    --pids-limit 512 --memory 2g --memory-swap 2g --cpus 2 \
    --mount "type=volume,source=${scratch[0]},target=/work" --mount "type=volume,source=${scratch[1]},target=/tmp" \
    --mount "type=volume,source=$state,target=/var/lib/smart-review-runner" \
    --tmpfs /var/lib/smart-review-runner/codex-homes:rw,nosuid,nodev,noexec,size=16m,nr_inodes=2048,uid=10001,gid=10001,mode=0711 \
    -e "R1_SUPERVISOR=${R1_SUPERVISOR:-python}" "$@" "$image" > /dev/null
}
trap cleanup EXIT

if [[ "${R1_SKIP_BUILD:-0}" != 1 ]]; then
  docker build -q --build-context "workspace=$workspace" --target base -t "$base" "$here/.." > /dev/null
  docker build -q --build-context "workspace=$workspace" -t "$skeleton" -t "$runner_tag" "$here/.." \
    > /dev/null
  docker build -q --build-arg "RUNNER_IMAGE=$base" -t "$image" "$here" > /dev/null
fi

{
  echo "# R1 container test ${run_id}: image $(docker image inspect -f '{{.Id}}' "$image")"
  echo "# docker $(docker version -f '{{.Server.Version}}'), kernel $(uname -r)"
} > "$ledger"

versions="$(docker run --rm --network none --entrypoint /bin/sh "$skeleton" -c \
  'codex --version; github-mcp-server --version 2>&1 | grep Version; git --version; rg --version | head -n 1;
   id runner; id agent; find / -xdev -type f -perm /6000 | wc -l')"
codex_ok=0; grep -q '^codex-cli 0\.167\.0$' <<< "$versions" && grep -q '2\.1\.0' <<< "$versions" && codex_ok=1
suid="$(tail -n 1 <<< "$versions")"
if [[ $codex_ok == 1 && $suid == 0 ]] && grep -q 'uid=10001(runner) gid=10001(runner) groups=10001(runner)' <<< "$versions" \
   && grep -q 'uid=10002(agent) gid=10002(agent) groups=10002(agent)' <<< "$versions"; then r=PASS; else r=FAIL; fi
echo "CHECK runner_image_pins_and_uids $r $(tr '\n' ';' <<< "$versions" | sed 's/;$//') (last: setuid/setgid files)" | tee -a "$ledger"

# T4: no package manager, compiler or interpreter beyond Codex's needs; no development or test switch baked in.
present="$(docker run --rm --network none --entrypoint /bin/sh "$skeleton" -c \
  'for t in apt apt-get dpkg perl python python3 gcc cc c++ clang rustc cargo make node npm git-lfs; do
     command -v "$t" > /dev/null 2>&1 && printf "%s " "$t"; done; true')"
env_baked="$(docker image inspect -f '{{range .Config.Env}}{{println .}}{{end}}' "$skeleton" \
  | grep -E '^(SR_RUNNER_DEV_NO_ISOLATION|SR_RUNNER_DEV_HOST_GATEWAY|SR_SUPERVISOR_TEST_FAULTS|SR_SUPERVISOR_UNPRIVILEGED|R1_FAULT_[A-Z_]*|SR_CODEX_HOMES_ALLOW_TMPFS)=' || true)"
entry="$(docker image inspect -f '{{json .Config.Entrypoint}} {{json .Config.User}}' "$skeleton")"
if [[ -z "$present" && -z "$env_baked" && "$entry" == '["/usr/local/bin/smart-review-supervisor","serve","--"] ""' ]]; then r=PASS; else r=FAIL; fi
echo "CHECK runner_image_has_no_toolchains $r present=[${present% }] test/dev switches in image env=[${env_baked}] entrypoint/user=$entry" | tee -a "$ledger"

# Preflight: the production entrypoint refuses to start the runner unless the network is isolated.
# run_preflight NAME [docker run args]: exit code and log of the production image with the runner's options.
# Network: none, or $pf_network when set.
run_preflight() {
  docker rm -f "$preflight" > /dev/null 2>&1 || true
  docker run -d --name "$preflight" --init --network "${pf_network:-none}" --ipc none \
    --cap-drop ALL --cap-add CHOWN --cap-add SETUID --cap-add SETGID --cap-add KILL \
    --security-opt no-new-privileges --read-only --pids-limit 128 --memory 512m \
    --mount "type=volume,source=${scratch[2]},target=/work" --mount "type=volume,source=${scratch[3]},target=/tmp" \
    --mount "type=volume,source=$pf_state,target=/var/lib/smart-review-runner" \
    --mount "type=volume,source=${scratch[4]},target=/var/lib/smart-review-runner/codex-homes" \
    "$@" "$skeleton" > /dev/null
  local code
  code="$(timeout 60 docker wait "$preflight" || echo timeout)"
  printf '%s|%s' "$code" "$(docker logs "$preflight" 2>&1 | grep 'runner preflight' | tr '\n' '/' | sed 's|/$||')"
}

# Network isolation (ADR-013): an internal network without the isolated gateway mode (what an Engine older than
# 28.0 silently makes of the production network), a bridge with a default route, and the production setting.
docker network create --internal --subnet 172.30.95.0/24 "${pf_nets[0]}" > /dev/null
docker network create --internal --subnet 172.30.96.0/24 -o com.docker.network.bridge.gateway_mode_ipv4=isolated \
  -o com.docker.network.bridge.gateway_mode_ipv6=isolated "${pf_nets[1]}" > /dev/null
docker network create --subnet 172.30.97.0/24 "${pf_nets[2]}" > /dev/null
net_internal="$(pf_network="${pf_nets[0]}" run_preflight --ip 172.30.95.4)"
net_bridge="$(pf_network="${pf_nets[2]}" run_preflight)"
net_isolated="$(pf_network="${pf_nets[1]}" run_preflight --ip 172.30.96.4)"
net_dev="$(pf_network="${pf_nets[0]}" run_preflight -e SR_RUNNER_DEV_HOST_GATEWAY=1)"
if [[ "$net_internal" == "78|"*"not isolated: gateway candidate 172.30.95.1 answered"*"Docker Engine 28.0"* \
      && "$net_bridge" == "78|"*"not isolated: IPv4 default route via 172.30.97.1"* \
      && "$net_isolated" != "78|"* && "$net_isolated" == *"network isolated (no default route; no answer from gateway candidates: 172.30.96.1)"* \
      && "$net_dev" != "78|"* && "$net_dev" == *"NOT checked"* ]]; then r=PASS; else r=FAIL; fi
echo "CHECK runner_preflight_refuses_unisolated_network $r internal without isolated: $net_internal; bridge: $net_bridge; internal+isolated (runner then stops on its config): $net_isolated; dev switch: $net_dev" | tee -a "$ledger"
docker rm -f "$preflight" > /dev/null 2>&1 || true

docker volume create "$state" > /dev/null
SR_RUNNER_SECRET="SR""CANARY-runner-secret-$(head -c 8 /dev/urandom | od -An -tx1 | tr -d ' \n')"
export SR_RUNNER_SECRET
# Test-only: `clear` of the quarantine check's directory fails (Rust supervisor; needs the test flag).
start "$container" -e SR_RUNNER_SECRET -e R1_SKIP_CODEX -e SR_SUPERVISOR_TEST_FAULTS=1 -e R1_FAULT_CLEAR_FAILS=quarantine-1
unset SR_RUNNER_SECRET

hardening="$(docker inspect -f 'CapDrop={{.HostConfig.CapDrop}} CapAdd={{.HostConfig.CapAdd}} SecurityOpt={{.HostConfig.SecurityOpt}} ReadonlyRootfs={{.HostConfig.ReadonlyRootfs}} Privileged={{.HostConfig.Privileged}} IpcMode={{.HostConfig.IpcMode}} PidsLimit={{.HostConfig.PidsLimit}} Memory={{.HostConfig.Memory}} Init={{.HostConfig.Init}} User={{.Config.User}}' "$container")"
mounts="$(docker inspect -f '{{range .Mounts}}{{.Type}}:{{.Source}}->{{.Destination}} {{end}}' "$container")"
if grep -q 'CapDrop=\[ALL\]' <<< "$hardening" && grep -Eq 'CapAdd=\[CAP_CHOWN CAP_KILL CAP_SETGID CAP_SETUID\]' <<< "$hardening" \
   && grep -q 'SecurityOpt=\[no-new-privileges\]' <<< "$hardening" && grep -q 'ReadonlyRootfs=true' <<< "$hardening" \
   && grep -q 'Privileged=false' <<< "$hardening" && grep -q 'IpcMode=none' <<< "$hardening" \
   && grep -Eq 'PidsLimit=[1-9]' <<< "$hardening" \
   && ! grep -q 'unconfined' <<< "$hardening" && ! grep -q 'docker.sock' <<< "$mounts"; then r=PASS; else r=FAIL; fi
echo "CHECK runner_container_hardening $r $hardening Mounts=[$mounts]" | tee -a "$ledger"

exit_code="$(timeout 600 docker wait "$container" || echo timeout)"
docker logs "$container" 2>&1 | tee -a "$ledger"
echo "# container exit: $exit_code" | tee -a "$ledger"

# Fail closed: a sweep that fails (injected) must end the container within the deadline, never hang.
docker rm -f "$container" > /dev/null
started=$SECONDS
# The Rust supervisor honours the fault switch only with its explicit test flag.
start "$failing" -e R1_SCENARIO=sweep-failure -e R1_FAULT_SWEEP_FAILS=1 -e SR_SUPERVISOR_TEST_FAULTS=1 -e SR_SUPERVISOR_SWEEP_DEADLINE_MS=1000
fail_exit="$(timeout 60 docker wait "$failing" || echo timeout)"
elapsed=$((SECONDS - started))
fail_log="$(docker logs "$failing" 2>&1)"
if [[ "$fail_exit" == 70 && $elapsed -lt 30 ]] && grep -q 'sweep failed, terminating the container' <<< "$fail_log" \
   && ! grep -q '^CHECK' <<< "$fail_log"; then r=PASS; else r=FAIL; fi
echo "CHECK sweep_failure_terminates_container $r container exit=$fail_exit after ${elapsed}s (sweep deadline 1 s); log: $(tr '\n' ' ' <<< "$fail_log")" | tee -a "$ledger"
fails="$(grep -c '^CHECK .* FAIL' "$ledger" || true)"
passes="$(grep -c '^CHECK .* PASS' "$ledger" || true)"
echo "# total: pass=$passes fail=$fails ledger=$ledger" | tee -a "$ledger"
[[ "$exit_code" == 0 && "$fails" == 0 ]]
