#!/usr/bin/env bash
# Egress proxy test (threat model T4/T4b/T9, plan §9 WP3-OPS): a client on an internal network whose only way out
# is the proxy. Prints `CHECK <name> PASS|FAIL <detail>`; exit 0 only when every check passes. Needs Internet
# access from the host (example.com). A second proxy container on the egress network stands in for MemCan
# (any HTTP answer from it proves the exception).
#
# Usage: deploy/egress-proxy/test.sh [LEDGER_FILE]
# Env:   PROXY_IMAGE  image under test (default smart-review-egress-proxy:local; built unless PROXY_SKIP_BUILD=1)
#        CLIENT_IMAGE client with bash (default: the proxy image's base, ubuntu)
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
run_id="$(date -u +%Y%m%dt%H%M%S)-$$"
ledger="${1:-/dev/null}"
image="${PROXY_IMAGE:-smart-review-egress-proxy:local}"
client_image="${CLIENT_IMAGE:-$(sed -n 's/^FROM \([^ ]*\).*/\1/p' "$here/Dockerfile" | head -n 1)}"
internal="sr-egress-test-int-$run_id" egress="sr-egress-test-ext-$run_id"
proxy="sr-egress-test-proxy-$run_id" memcan="sr-egress-test-memcan-$run_id" client="sr-egress-test-client-$run_id"
proxy_ip=172.30.91.3 memcan_ip=172.30.92.9

cleanup() {
  docker rm -f "$proxy" "$memcan" "$client" > /dev/null 2>&1 || true
  docker network rm "$internal" "$egress" > /dev/null 2>&1 || true
}
trap cleanup EXIT

[[ "${PROXY_SKIP_BUILD:-0}" == 1 ]] || docker build -q -t "$image" "$here" > /dev/null
docker network create --internal --subnet 172.30.91.0/24 "$internal" > /dev/null
docker network create --subnet 172.30.92.0/24 "$egress" > /dev/null
# hardened NAME IP NETWORK [args]: the proxy's production options (deploy/compose.yaml).
hardened() {
  local name="$1" ip="$2" network="$3"
  shift 3
  docker create --name "$name" --init --read-only --cap-drop ALL --security-opt no-new-privileges \
    --memory 256m --pids-limit 128 --tmpfs /run/squid:uid=13,gid=13,mode=0750,size=1m \
    --network "$network" --ip "$ip" "$@" "$image" > /dev/null
}
hardened "$memcan" "$memcan_ip" "$egress"
hardened "$proxy" 172.30.92.2 "$egress" -e "SR_MEMCAN_URL=http://$memcan_ip:3128/"
docker network connect --ip "$proxy_ip" "$internal" "$proxy"
docker start "$memcan" "$proxy" > /dev/null
docker run -d --name "$client" --network "$internal" --ip 172.30.91.4 --user 10002:10002 --cap-drop ALL \
  --security-opt no-new-privileges --read-only --entrypoint sleep "$client_image" 600 > /dev/null
for _ in $(seq 60); do
  [[ "$(docker inspect -f '{{.State.Health.Status}}' "$proxy")" == healthy ]] && break
  sleep 1
done

results=()
check() {
  local name="$1" ok="$2" detail="$3"
  local r=FAIL
  if [[ "$ok" == 1 ]]; then r=PASS; fi
  results+=("$r")
  echo "CHECK $name $r $detail" | tee -a "$ledger"
}
# via REQUEST_LINE [HEADERS]: first status line of the proxy's answer to one request from the client.
via() {
  docker exec -e "REQ=$1" -e "HDR=${2:-}" "$client" bash -c \
    'exec 3<>/dev/tcp/'"$proxy_ip"'/3128; printf "%s\r\nHost: x\r\n%sConnection: close\r\n\r\n" "$REQ" "$HDR" >&3
     timeout 15 head -n 1 <&3 | tr -d "\r"' 2>&1 || true
}
direct() {
  docker exec "$client" bash -c "timeout 10 bash -c 'exec 3<>/dev/tcp/$1/$2' && echo CONNECTED" 2>&1 | tail -n 1 || true
}

health="$(docker inspect -f '{{.State.Health.Status}} user={{.Config.User}}' "$proxy")"
check proxy_healthy_unprivileged "$([[ "$health" == "healthy user=proxy" ]] && echo 1)" "$health"
out="$(direct 1.1.1.1 443) / $(direct example.com 80)"
check direct_egress_blocked "$([[ "$out" != *CONNECTED* ]] && echo 1)" "client without proxy: $out"
public="$(via 'CONNECT example.com:443 HTTP/1.1')"
check public_host_allowed "$([[ "$public" == "HTTP/1.1 200"* ]] && echo 1)" "CONNECT example.com:443 -> $public"
denied=""
for target in 'CONNECT 10.0.0.1:443' 'GET http://192.168.1.1/' 'GET http://169.254.169.254/latest/meta-data/' \
  'GET http://127.0.0.1:3128/' "GET http://$memcan_ip:80/" 'CONNECT [::1]:443'; do
  denied+="$target -> $(via "$target HTTP/1.1"); "
done
check private_destinations_denied "$([[ "$(grep -o '403 Forbidden' <<< "$denied" | wc -l)" == 6 ]] && echo 1)" "$denied"
memcan_answer="$(via "GET http://$memcan_ip:3128/ HTTP/1.1")"
canary="SR""CANARY$run_id"
via "GET http://example.com/${canary}path?q=${canary}query HTTP/1.1" "User-Agent: ${canary}ua\r\n" > /dev/null
sleep 2
log="$(docker logs "$proxy" 2>&1)"
access="$(grep -E '^[0-9]{10}\.[0-9]{3} ' <<< "$log" || true)"
logged=1
for pattern in ' TCP_TUNNEL/200 [0-9]+ [0-9]+ CONNECT example.com:443 ' ' TCP_DENIED/403 .* CONNECT 10.0.0.1:443 -$' \
  ' TCP_DENIED/403 .* GET 169.254.169.254:80 -$' " TCP_MISS/[0-9]+ .* GET $memcan_ip:3128 $memcan_ip$"; do
  grep -Eq "$pattern" <<< "$access" || logged=0
done
memcan_line="$(grep -E " TCP_MISS/[0-9]+ .* GET $memcan_ip:3128 $memcan_ip$" <<< "$access" || true)"
check memcan_exception_reachable "$([[ -n "$memcan_line" ]] && echo 1)" \
  "GET http://$memcan_ip:3128/ -> $memcan_answer; forwarded to the MemCan stand-in: $memcan_line"
check destinations_logged "$logged" "access log: $(tr '\n' ';' <<< "$access")"
check proxy_log_canary "$([[ "$log" != *"$canary"* && "$access" == *"GET example.com:80"* ]] && echo 1)" \
  "canary in URL path, query and User-Agent of a plain-HTTP request: $(grep -c "$canary" <<< "$log" || true) log lines contain it"
# Fields of the T4b format only: time, duration, client, result/status, bytes in, bytes out, method, host:port,
# upstream address.
check log_format_t4b "$(awk 'NF != 9 { bad = 1 } END { exit bad }' <<< "$access" && echo 1)" "every access line has 9 fields"

fails=0
for r in "${results[@]}"; do [[ "$r" == PASS ]] || fails=$((fails + 1)); done
echo "# egress proxy test: ${#results[@]} checks, $fails failed" | tee -a "$ledger"
((fails == 0))
