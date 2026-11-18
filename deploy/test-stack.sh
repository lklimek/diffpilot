#!/usr/bin/env bash
# Stack test (threat model T4/T7b/T9, ADR-013): brings up deploy/compose.yaml with deploy/compose.local.yaml (the
# override only publishes on loopback) from the images
# built before, with placeholder secrets and the subnets 172.29.81-83.0/24, then checks the running containers.
# Prints `CHECK <name> PASS|FAIL <detail>`; exit 0 only when every check passes; tears everything down.
# Host test doubles: a MemCan stand-in (SR_MEMCAN_URL = http://<host LAN address>:<port>/mcp) and a control
# service, both listening on 0.0.0.0 of the host. Needs Internet access (example.com, github.com).
#
# Usage: deploy/test-stack.sh [LEDGER_FILE]
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ledger="${1:-/dev/null}"
project="sr-stack-test-$$"
tmp="$(mktemp -d)"
host_ip="$(ip -4 route get 1.1.1.1 | sed -n 's/.* src \([0-9.]*\).*/\1/p')"
read -r http_port memcan_port control_port < <(python3 -c 'import socket
socks = [socket.socket() for _ in range(3)]
for s in socks:
    s.bind(("0.0.0.0", 0))
print(*(s.getsockname()[1] for s in socks))')
compose=(docker compose -p "$project" --env-file "$tmp/stack.env" -f "$repo/deploy/compose.yaml"
  -f "$repo/deploy/compose.local.yaml")
pids=()

cleanup() {
  "${compose[@]}" down -v --remove-orphans > /dev/null 2>&1 || true
  for pid in "${pids[@]}"; do kill "$pid" 2> /dev/null || true; done
  rm -rf "$tmp"
}
trap cleanup EXIT

# Host test doubles.
mkdir -p "$tmp/memcan" "$tmp/control"
echo MEMCAN-STANDIN > "$tmp/memcan/mcp"
echo CONTROL-SERVICE > "$tmp/control/index.html"
python3 -m http.server --bind 0.0.0.0 --directory "$tmp/memcan" "$memcan_port" > /dev/null 2>&1 &
pids+=("$!")
python3 -m http.server --bind 0.0.0.0 --directory "$tmp/control" "$control_port" > /dev/null 2>&1 &
pids+=("$!")

echo "placeholder: not a key" > "$tmp/app.pem"
chmod 0444 "$tmp/app.pem"
sed -e "s|^SR_SECRET_KEY=.*|SR_SECRET_KEY=$(head -c 32 /dev/urandom | base64)|" \
  -e "s|^SR_RUNNER_SECRET=.*|SR_RUNNER_SECRET=$(head -c 24 /dev/urandom | od -An -tx1 | tr -d ' \n')|" \
  -e "s|^SR_PUBLIC_URL=.*|SR_PUBLIC_URL=http://localhost:$http_port|" \
  -e "s|^SR_HTTP_PORT=.*|SR_HTTP_PORT=$http_port|" \
  -e "s|^SR_GITHUB_APP_PRIVATE_KEY_PATH=.*|SR_GITHUB_APP_PRIVATE_KEY_PATH=$tmp/app.pem|" \
  -e "s|^SR_MEMCAN_URL=.*|SR_MEMCAN_URL=http://$host_ip:$memcan_port/mcp|" \
  -e "s|172\.29\.73\.|172.29.83.|; s|172\.29\.71\.|172.29.81.|; s|172\.29\.72\.|172.29.82.|" \
  "$repo/.env.example" > "$tmp/stack.env"
"${compose[@]}" up -d --no-build --wait --wait-timeout 180 > "$tmp/up.log" 2>&1 || { cat "$tmp/up.log" >&2; exit 1; }

results=()
check() {
  local r=FAIL
  if [[ "$2" == 1 ]]; then r=PASS; fi
  results+=("$r")
  echo "CHECK $1 $r $3" | tee -a "$ledger"
}
runner="$project-runner-1" server="$project-server-1" proxy="$project-egress-proxy-1"
# tcp CONTAINER UID HOST PORT: CONNECTED or the error of a direct TCP connection from inside the container.
tcp() {
  docker exec -u "$2" "$1" bash -c "timeout 5 bash -c 'exec 3<>/dev/tcp/$3/$4' && echo CONNECTED" 2>&1 | tail -n 1 || true
}
# get CONTAINER UID HOST PORT URL: status line and last body line of a plain HTTP request to HOST:PORT.
get() {
  docker exec -u "$2" -e "H=$3" -e "P=$4" -e "U=$5" "$1" bash -c \
    'exec 3<>"/dev/tcp/$H/$P"; printf "%s HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n" "$U" >&3
     timeout 10 cat <&3 | tr -d "\r" | sed -n "1p;\$p" | tr "\n" " "' 2>&1 || true
}

health="$(docker inspect -f '{{.Name}}={{.State.Health.Status}}' "$server" "$runner" "$proxy" | tr '\n' ' ')"
check stack_healthy "$([[ "$(grep -o '=healthy' <<< "$health" | wc -l)" == 3 ]] && echo 1)" "$health"
limits="$(docker inspect -f 'NanoCpus={{.HostConfig.NanoCpus}} Memory={{.HostConfig.Memory}} PidsLimit={{.HostConfig.PidsLimit}} CapAdd={{.HostConfig.CapAdd}} CapDrop={{.HostConfig.CapDrop}} SecurityOpt={{.HostConfig.SecurityOpt}} ReadonlyRootfs={{.HostConfig.ReadonlyRootfs}} IpcMode={{.HostConfig.IpcMode}} Restart={{.HostConfig.RestartPolicy.Name}}' "$runner")"
check runner_limits_and_hardening "$([[ "$limits" == "NanoCpus=2000000000 Memory=6442450944 PidsLimit=1024 CapAdd=[CAP_CHOWN CAP_KILL CAP_SETGID CAP_SETUID] CapDrop=[ALL] SecurityOpt=[no-new-privileges:true] ReadonlyRootfs=true IpcMode=none Restart=unless-stopped" ]] && echo 1)" "$limits"
# Nothing in RAM (owner decision): scratch on the disposable tmp-* volumes, owned and moded as ADR-013 needs.
tmpfs="$(docker inspect -f '{{.Name}}={{json .HostConfig.Tmpfs}}' "$server" "$runner" | tr '\n' ' ')"
temporary="$(docker volume ls -q --filter label=smart-review.volume=temporary \
  --filter "label=com.docker.compose.project=$project" | sort | tr '\n' ' ')"
modes="$(docker exec "$runner" stat -c '%n=%a:%u' /tmp /work /var/lib/smart-review-runner/codex-homes | tr '\n' ' ')"
agent_tmp="$(docker exec -u 10002 "$runner" ls /tmp 2>&1 || true)"
check scratch_on_temporary_volumes "$([[ "$tmpfs" != *'"/'* \
  && "$temporary" == "${project}_tmp-runner ${project}_tmp-runner-codex-homes ${project}_tmp-runner-work ${project}_tmp-server " \
  && "$modes" == "/tmp=700:10001 /work=711:10001 /var/lib/smart-review-runner/codex-homes=711:10001 " \
  && "$agent_tmp" == *"Permission denied"* ]] && echo 1)" \
  "tmpfs: $tmpfs; temporary volumes: $temporary; runner modes: $modes; agent lists /tmp: $agent_tmp"

net="${project}_runner"
bridge="br-$(docker network inspect -f '{{.Id}}' "$net" | cut -c 1-12)"
bridge_addr="$(ip -4 -o addr show dev "$bridge" 2> /dev/null | awk '{ print $4 }' | tr '\n' ' ')"
check runner_network_has_no_host_address "$([[ -z "$bridge_addr" ]] && echo 1)" \
  "bridge $bridge addresses: [${bridge_addr}] options: $(docker network inspect -f '{{json .Options}} internal={{.Internal}}' "$net")"

# The runner network reaches nothing on the host or another network: not the host's services (gateway, LAN
# address, other bridges), not the server's public listener; as the agent and as the runner uid.
direct=""
for uid in 10002 10001; do
  for target in "172.29.81.1:$control_port" "$host_ip:$control_port" "$host_ip:$memcan_port" "172.17.0.1:$control_port" \
    "172.29.83.1:$control_port" "172.29.82.1:$control_port" 172.29.83.2:3000 1.1.1.1:443; do
    direct+="uid $uid -> $target: $(tcp "$runner" "$uid" "${target%:*}" "${target##*:}"); "
  done
done
for uid in 10002 10001; do
  direct+="uid $uid -> [2606:4700:4700::1111]:443: $(tcp "$runner" "$uid" 2606:4700:4700::1111 443); "
done
check runner_reaches_no_host_service "$([[ "$direct" != *CONNECTED* ]] && echo 1)" "$direct"
preflight="$(docker logs "$runner" 2>&1 | grep 'runner preflight: network' | tail -n 1)"
check runner_preflight_proved_isolation "$([[ "$preflight" == *"network isolated (no default route"* ]] && echo 1)" "$preflight"
# DNS through the embedded resolver (127.0.0.11): names of the runner network resolve, external names do not (on an
# internal network Docker does not forward queries out); the server, on a normal network, resolves them.
dns_internal="$(docker exec -u 10002 "$runner" getent hosts egress-proxy 2>&1 || echo failed)"
dns_external="$(docker exec -u 10002 "$runner" getent ahosts example.com 2>&1 || echo "failed (exit $?)")"
dns_server="$(docker exec "$server" getent ahosts example.com 2>&1 | head -n 1 || echo failed)"
check runner_dns_no_external_names "$([[ "$dns_internal" == 172.29.81.3* && "$dns_external" == failed* && -n "$dns_server" && "$dns_server" != failed* ]] && echo 1)" \
  "agent: egress-proxy -> $dns_internal; example.com -> $dns_external; server: example.com -> $dns_server"
inner="server runner listener: $(tcp "$runner" 10002 172.29.81.2 3001); proxy: $(tcp "$runner" 10002 172.29.81.3 3128)"
check runner_reaches_listener_and_proxy "$([[ "$(grep -o CONNECTED <<< "$inner" | wc -l)" == 2 ]] && echo 1)" "$inner"

proxied="$(get "$runner" 10002 172.29.81.3 3128 "CONNECT example.com:443" | cut -c 1-40)"
memcan_via_proxy="$(get "$runner" 10002 172.29.81.3 3128 "GET http://$host_ip:$memcan_port/mcp")"
control_via_proxy="$(get "$runner" 10002 172.29.81.3 3128 "GET http://$host_ip:$control_port/")"
check proxy_policy_from_runner "$([[ "$proxied" == "HTTP/1.1 200"* && "$memcan_via_proxy" == *MEMCAN-STANDIN* && "$control_via_proxy" == "HTTP/1.1 403"* ]] && echo 1)" \
  "CONNECT example.com:443 -> $proxied; MemCan -> $memcan_via_proxy; host control service -> $control_via_proxy"
memcan_from_server="$(get "$server" 10010 "$host_ip" "$memcan_port" "GET /mcp")"
check server_reaches_memcan "$([[ "$memcan_from_server" == *MEMCAN-STANDIN* ]] && echo 1)" "server -> MemCan: $memcan_from_server"
healthz="$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$http_port/healthz" || true)"
check server_published_on_loopback "$([[ "$healthz" == 204 ]] && echo 1)" "GET http://127.0.0.1:$http_port/healthz -> $healthz"

sleep 2
access="$(docker logs "$proxy" 2> /dev/null | grep -E '^[0-9]{10}\.[0-9]{3} ' || true)"
check proxy_logged_runner_traffic "$(grep -q 'TCP_TUNNEL/200 .* CONNECT example.com:443 ' <<< "$access" \
  && grep -q "TCP_MISS/200 .* GET $host_ip:$memcan_port " <<< "$access" \
  && grep -q "TCP_DENIED/403 .* GET $host_ip:$control_port " <<< "$access" && echo 1)" "$(tr '\n' ';' <<< "$access")"

fails=0
for r in "${results[@]}"; do [[ "$r" == PASS ]] || fails=$((fails + 1)); done
echo "# stack test: ${#results[@]} checks, $fails failed" | tee -a "$ledger"
((fails == 0))
