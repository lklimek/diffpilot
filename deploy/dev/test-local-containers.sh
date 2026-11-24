#!/usr/bin/env bash
# Run under the heavy flock. Production images, isolated Compose project, fake upstreams, no account auth.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
scratch=$(mktemp -d "${SR_LOCAL_TMPDIR:-${RUNNER_TEMP:-/data/tmp}}/v13w7-local.XXXXXX")
project=v13w7-local-$(python3 -c 'import secrets; print(secrets.token_hex(5))')
export SR_LOCAL_STATE_DIR=$scratch/state SR_LOCAL_IMAGE_PREFIX=$project
export SR_LOCAL_REGISTRY_NAME=$project-registry
export DOCKER_CONFIG=$scratch/docker
mkdir "$DOCKER_CONFIG"
free_port() { python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])'; }
SR_LOCAL_REGISTRY_PORT=$(free_port)
export SR_LOCAL_REGISTRY_PORT
http_port=$(free_port)
compose=(docker compose -p "$project" --env-file "$scratch/config.env" --env-file "$scratch/runtime.env"
  -f "$root/deploy/compose.yaml" -f "$root/deploy/compose.local.yaml"
  -f "$root/deploy/dev/compose.host-uid.yaml" -f "$scratch/fake.yaml")
cleanup() {
  status=$?
  trap - EXIT
  if [[ -f "$scratch/fake.yaml" && -f "$scratch/runtime.env" ]]; then
    if ((status)); then "${compose[@]}" logs --no-color --tail 40 >&2 || true; fi
    # The launcher and all generated workers must be gone before their networks and volumes.
    "${compose[@]}" stop runner launcher >/dev/null 2>&1 || true
    ids=$(docker ps -aq --filter "label=dp.managed=$project")
    if [[ -n "$ids" ]]; then xargs -r docker rm -f <<<"$ids" >/dev/null; fi
    "${compose[@]}" down -v --remove-orphans >/dev/null 2>&1 || true
    volumes=$(docker volume ls -q --filter "label=dp.managed=$project")
    if [[ -n "$volumes" ]]; then xargs -r docker volume rm <<<"$volumes" >/dev/null; fi
    # The host-uid seed creates server-data before Compose, so it has no Compose ownership label.
    for volume in "${project}_server-data" "$project-pr-7-3-work" "$project-c2"; do
      docker volume rm "$volume" >/dev/null 2>&1 || true
    done
  fi
  docker rm -f -v "$project-registry" >/dev/null 2>&1 || true
  docker volume rm "$project-registry-data" >/dev/null 2>&1 || true
  docker network rm "$project-registry" >/dev/null 2>&1 || true
  # New runtime secrets have foreign owners: remove only this throwaway directory via a root container.
  docker run --rm --network none --user 0:0 --entrypoint rm \
    --mount "type=bind,source=$scratch,target=/scratch" "$project-worker:local" -rf /scratch/runtime-secrets >/dev/null 2>&1 || true
  rm -rf "$scratch"
  if docker volume inspect "${project}_server-data" >/dev/null 2>&1; then
    echo 'test database volume remains after cleanup' >&2
    status=1
  fi
  exit "$status"
}
trap cleanup EXIT
on_error() {
  local rc=$?
  echo "test-local-containers: line $1 failed (exit $rc): $BASH_COMMAND" >&2
  exit "$rc"
}
trap 'on_error $LINENO' ERR
"$root/deploy/dev/local-containers.sh" prepare
# The second pass must reuse the same verified loopback registry and produce usable digest refs.
"$root/deploy/dev/local-containers.sh" prepare
"$root/deploy/containers/test-launcher-image.sh" "$project-runner:local"
"$root/deploy/containers/test-worker-image.sh" "$project-worker:local"
"$root/deploy/dev/local-containers.sh" secrets "$scratch/runtime-secrets"
"$root/deploy/dev/local-containers.sh" secrets "$scratch/runtime-secrets"
"$root/deploy/containers/test-gateway-image.sh" "$project-server:local" "$scratch/runtime-secrets"
"$root/deploy/dev/local-containers.sh" env "$scratch/runtime.env"
# Pick five unused /24s; never inspect or mount another application's state.
docker network ls -q | xargs -r docker network inspect >"$scratch/networks.json"
python3 - "$scratch" "$http_port" <<'PY'
import base64
import ipaddress
import json
import os
from pathlib import Path
import secrets
import sys
import random
directory = Path(sys.argv[1])
used = [ipaddress.ip_network(c['Subnet']) for n in json.loads((directory/'networks.json').read_text())
        for c in (n['IPAM']['Config'] or []) if 'Subnet' in c]
candidates = [ipaddress.ip_network(f'10.249.{i}.0/24') for i in range(1, 255)]
random.shuffle(candidates)
subnets = [n for n in candidates if not any(n.overlaps(u) for u in used if u.version == 4)][:5]
if len(subnets) != 5:
    raise SystemExit('no isolated test subnets available')
public, runner, egress, worker, judge = [str(n.network_address).rsplit('.',1)[0] for n in subnets]
values = {
    'COMPOSE_PROJECT_NAME': os.environ['SR_LOCAL_IMAGE_PREFIX'],
    'SR_PUBLIC_URL': 'http://localhost:'+sys.argv[2], 'SR_HTTP_PORT': sys.argv[2],
    'SR_ALLOWED_LOGINS': 'fake', 'SR_SECRET_KEY': base64.b64encode(secrets.token_bytes(32)).decode(),
    'SR_RUNNER_SECRET': secrets.token_hex(32), 'SR_GITHUB_APP_ID': '42',
    'SR_GITHUB_CLIENT_ID': 'fake-client', 'SR_GITHUB_CLIENT_SECRET': 'fake-client-secret',
    'SR_GITHUB_APP_PRIVATE_KEY_PATH': str(directory/'fake.pem'),
    'SR_GITHUB_API_URL': f'http://{runner}.5:8091', 'SR_GITHUB_OAUTH_URL': f'http://{runner}.5:8091',
    'SR_GITHUB_CLONE_URL': f'http://{runner}.5:8091',
    'SR_MEMCAN_URL': f'http://{runner}.5:8190/mcp', 'SR_MEMCAN_TOKEN': 'fake-memcan',
    'SR_SERVER_PUBLIC_ADDR': public+'.2', 'SR_SERVER_RUNNER_ADDR': runner+'.2',
    'SR_RUNNER_ADDR': runner+'.4', 'SR_PROXY_RUNNER_ADDR': runner+'.3',
    'SR_PROXY_EGRESS_ADDR': egress+'.2', 'SR_GATEWAY_ADMIN_ADDR': runner+'.5',
    'SR_GATEWAY_EGRESS_ADDR': egress+'.3', 'SR_PROXY_WORKER_ADDR': worker+'.3',
    'SR_GATEWAY_MODEL_ADDR': worker+'.2', 'SR_GATEWAY_JUDGE_ADDR': judge+'.2',
}
for name, subnet in zip(('PUBLIC','RUNNER','EGRESS','WORKER','JUDGE'), subnets):
    values[f'SR_{name}_SUBNET'] = str(subnet)
(directory/'config.env').write_text(''.join(f'{k}={v}\n' for k,v in values.items()))
(directory/'fake.pem').write_text('fake-key-unused-by-healthcheck')
(directory/'fake.pem').chmod(0o444)
PY
"$root/deploy/dev/local-containers.sh" check "$scratch/config.env" "$scratch/runtime.env"
cat >"$scratch/fake.yaml" <<YAML
services:
  codex-gateway:
    image: $project-worker:local
    entrypoint: [python3, /fixture/network.py, listen]
    volumes:
      - $root/deploy/containers/network-fixture.py:/fixture/network.py:ro
YAML
# Only the fresh test DB volume; the production data-volume ownership is an owner action in the runbook.
docker run --rm --network none --user 0:0 --entrypoint chown \
  --mount "type=volume,source=${project}_server-data,target=/data,volume-nocopy" \
  "$project-server:local" "$(id -u):$(id -g)" /data
"${compose[@]}" up -d --no-build --wait --wait-timeout 120
test "$(curl --noproxy '*' --fail --silent --show-error -o /dev/null -w '%{http_code}' "http://127.0.0.1:$http_port/healthz")" = 204
# Positive controls on the trusted runner network prove admin/runner ports are actually listening.
# shellcheck disable=SC2016 # Perl variables, not shell interpolation.
"${compose[@]}" exec -T runner perl -MIO::Socket::INET -e '
  for my $key (qw(SR_GATEWAY_ADMIN_URL SR_RUNNER_SERVER_URL)) {
    $ENV{$key} =~ m{^http://([^:]+):(\d+)$} or die "address";
    my $s = IO::Socket::INET->new(PeerAddr=>$1, PeerPort=>$2, Timeout=>3) or die "trusted listener unavailable";
  }
'
rpc() {
  # shellcheck disable=SC2016 # Perl variables, not shell interpolation.
  "${compose[@]}" exec -T launcher perl -MIO::Socket::UNIX -MJSON::PP -e '
    my $s = IO::Socket::UNIX->new(Peer => "/run/launcher/launcher.sock") or die "socket";
    print $s $ARGV[0], "\n";
    my $r = decode_json(scalar <$s>);
    die "launcher error" if $r->{error};
  ' "$1"
}
for shape in pr judge; do
  extra=''
  [[ "$shape" != pr ]] || extra=',"pull_request_id":7'
  rpc "{\"id\":\"probe\",\"action\":{\"operation\":\"ensure\",\"container\":{\"shape\":\"$shape\",\"payer\":\"3\"$extra},\"memcan\":null}}"
done
# Run probes in the real launcher's CS1/CS2 namespaces; neither probe invokes a model.
for pair in "pr:pr-7-3" "judge:user-3"; do
  "${compose[@]}" config --format json | python3 -c '
import json,sys
c=json.load(sys.stdin); s=c["services"]
print(json.dumps({"admin": s["server"]["environment"]["SR_GATEWAY_ADMIN_URL"],
 "runner": s["runner"]["environment"]["SR_RUNNER_SERVER_URL"],
 "memcan": s["server"]["environment"]["SR_MEMCAN_URL"],
 "model": "http://"+s["codex-gateway"]["environment"]["SR_GATEWAY_MODEL_ADDR"],
 "judge": "http://"+s["codex-gateway"]["environment"]["SR_GATEWAY_JUDGE_ADDR"]}))
' | docker exec -i "$project-${pair#*:}" python3 -c "$(cat "$root/deploy/containers/network-fixture.py")" "${pair%%:*}"
done
echo 'local installer and live SG5/SG7 isolation PASS (QA-404); no real auth'
