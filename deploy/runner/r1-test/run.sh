#!/usr/bin/env bash
# R1 container test (ADR-013, threat model T1/T4/T8b/T13): proves the runner process model in a container started
# with the production security options. Prints `CHECK <name> PASS|FAIL <detail>` lines and a RESULT line; exit 0
# only when every check passes. No real credential is used: canaries and fake keys only, network none.
#
# Usage: run.sh [LEDGER_FILE]     (default /data/tmp/r1-test/<timestamp>.log)
# Env:   R1_SKIP_BUILD=1          reuse the images built earlier
#        R1_SKIP_CODEX=1          skip the real-Codex job
# A second container (fault injection R1_FAULT_SWEEP_FAILS=1) proves that a failing sweep ends the container.
# codex-homes is a size- and inode-capped tmpfs here: a stand-in for the bounded persistent filesystem of ADR-013.
# Heavy job: builds two images (one at a time); do not run while a cargo job of this session runs.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
run_id="$(date -u +%Y%m%dt%H%M%S)"
ledger="${1:-/data/tmp/r1-test/${run_id}.log}"
mkdir -p "$(dirname "$ledger")"
skeleton=smart-review-runner:skeleton
image=smart-review-runner-r1-test:local
container="sr-r1-test-${run_id}"
failing="sr-r1-fail-${run_id}"
state="sr-r1-state-${run_id}"

cleanup() {
  docker rm -f "$container" "$failing" > /dev/null 2>&1 || true
  docker volume rm "$state" > /dev/null 2>&1 || true
}

# start NAME [docker run args...]: the production security options (ADR-013) plus the test mounts.
start() {
  local name="$1"
  shift
  docker run -d --name "$name" --init --network none --ipc none \
    --cap-drop ALL --cap-add CHOWN --cap-add SETUID --cap-add SETGID --cap-add KILL \
    --security-opt no-new-privileges --read-only \
    --pids-limit 512 --memory 2g --memory-swap 2g --cpus 2 \
    --tmpfs /work:rw,nosuid,nodev,size=512m,uid=10001,gid=10001,mode=0711 \
    --tmpfs /tmp:rw,nosuid,nodev,size=64m,uid=10001,gid=10001,mode=0700 \
    --mount "type=volume,source=$state,target=/var/lib/smart-review-runner" \
    --tmpfs /var/lib/smart-review-runner/codex-homes:rw,nosuid,nodev,size=16m,nr_inodes=2048,uid=10001,gid=10001,mode=0711 \
    "$@" "$image" > /dev/null
}
trap cleanup EXIT

if [[ "${R1_SKIP_BUILD:-0}" != 1 ]]; then
  docker build -q -t "$skeleton" "$here/.." > /dev/null
  docker build -q --build-arg "RUNNER_IMAGE=$skeleton" -t "$image" "$here" > /dev/null
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

docker volume create "$state" > /dev/null
SR_RUNNER_SECRET="SR""CANARY-runner-secret-$(head -c 8 /dev/urandom | od -An -tx1 | tr -d ' \n')"
export SR_RUNNER_SECRET
start "$container" -e SR_RUNNER_SECRET
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
start "$failing" -e R1_SCENARIO=sweep-failure -e R1_FAULT_SWEEP_FAILS=1 -e SR_SUPERVISOR_SWEEP_DEADLINE_MS=1000
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
