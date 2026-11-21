#!/usr/bin/env bash
# Egress proxy test (threat model T4/T4b/T9, plan §9 WP3-OPS): a client on an internal network whose only way out
# is the proxy. Prints `CHECK <name> PASS|FAIL <detail>`; exit 0 only when every check passes. Needs Internet
# access from the host (example.com).
# Test doubles (test-fake-net.pl, in the proxy image): a DNS server the proxies use, answering memcan.test with the
# MemCan stand-in's address and evil*.test with that address plus a decoy's; HTTP stand-ins for MemCan and the
# decoy that log every request they get. Two proxies: MemCan configured by name and by IP literal.
#
# Usage: deploy/egress-proxy/test.sh [LEDGER_FILE]
# Env:   PROXY_IMAGE  image under test (default diffpilot-egress-proxy:local; built unless PROXY_SKIP_BUILD=1)
#        CLIENT_IMAGE client with bash (default: the proxy image's base, ubuntu)
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
run_id="$(date -u +%Y%m%dt%H%M%S)-$$"
ledger="${1:-/dev/null}"
image="${PROXY_IMAGE:-diffpilot-egress-proxy:local}"
client_image="${CLIENT_IMAGE:-$(sed -n 's/^FROM \([^ ]*\).*/\1/p' "$here/Dockerfile" | head -n 1)}"
p="sr-egress-test-$run_id"
internal="$p-int" egress="$p-ext"
proxy="$p-proxy" proxy_lit="$p-proxy-literal" dns="$p-dns" memcan="$p-memcan" decoy="$p-decoy" client="$p-client"
proxy_ip=172.30.91.3 proxy_lit_ip=172.30.91.5 dns_ip=172.30.92.53 memcan_ip=172.30.92.9 decoy_ip=172.30.92.10
tmp="$(mktemp -d)"

cleanup() {
  docker rm -f "$proxy" "$proxy_lit" "$dns" "$memcan" "$decoy" "$client" > /dev/null 2>&1 || true
  docker network rm "$internal" "$egress" > /dev/null 2>&1 || true
  rm -rf "$tmp"
}
trap cleanup EXIT

[[ "${PROXY_SKIP_BUILD:-0}" == 1 ]] || docker build -q --build-context "workspace=$here/../.." -t "$image" "$here" > /dev/null
docker network create --internal --subnet 172.30.91.0/24 "$internal" > /dev/null
docker network create --subnet 172.30.92.0/24 "$egress" > /dev/null
echo "nameserver $dns_ip" > "$tmp/resolv.conf"
chmod 0644 "$tmp/resolv.conf"
cp "$here/test-fake-net.pl" "$tmp/fake-net.pl"
chmod 0755 "$tmp" "$tmp/fake-net.pl"

# fake NAME IP ARGS...: a test double from the proxy image.
fake() {
  local name="$1" ip="$2"
  shift 2
  docker run -d --name "$name" --network "$egress" --ip "$ip" --read-only --cap-drop ALL \
    -v "$tmp/fake-net.pl:/fake-net.pl:ro" --entrypoint perl "$@" > /dev/null
}
fake "$dns" "$dns_ip" --user 0 --cap-add NET_BIND_SERVICE "$image" /fake-net.pl dns \
  "memcan.test=$memcan_ip" "evil.test=$memcan_ip,$decoy_ip" "evil2.test=$decoy_ip,$memcan_ip"
fake "$memcan" "$memcan_ip" "$image" /fake-net.pl http 8190 MEMCAN
fake "$decoy" "$decoy_ip" "$image" /fake-net.pl http 8190 DECOY
# hardened NAME EGRESS_IP INTERNAL_IP MEMCAN_URL: a proxy with the production options (deploy/compose.yaml) and
# the test resolver.
hardened() {
  docker create --name "$1" --init --read-only --cap-drop ALL --security-opt no-new-privileges \
    --memory 256m --pids-limit 128 --tmpfs /run/squid:uid=13,gid=13,mode=0750,size=1m \
    -v "$tmp/resolv.conf:/etc/resolv.conf:ro" --network "$egress" --ip "$2" -e "SR_MEMCAN_URL=$4" "$image" > /dev/null
  docker network connect --ip "$3" "$internal" "$1"
  docker start "$1" > /dev/null
}
hardened "$proxy" 172.30.92.2 "$proxy_ip" "http://memcan.test:8190/mcp"
hardened "$proxy_lit" 172.30.92.3 "$proxy_lit_ip" "http://$memcan_ip:8190/mcp"
docker run -d --name "$client" --network "$internal" --ip 172.30.91.4 --user 10002:10002 --cap-drop ALL \
  --security-opt no-new-privileges --read-only --entrypoint sleep "$client_image" 600 > /dev/null
for _ in $(seq 60); do
  [[ "$(docker inspect -f '{{.State.Health.Status}}' "$proxy" "$proxy_lit" | sort -u)" == healthy ]] && break
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
# via [-p PROXY_IP] REQUEST_LINE [HEADERS]: the proxy's answer (status line and body, CR removed) to one request.
via() {
  local ip="$proxy_ip"
  if [[ "$1" == -p ]]; then ip="$2"; shift 2; fi
  docker exec -e "REQ=$1" -e "HDR=${2:-}" -e "IP=$ip" "$client" bash -c \
    'exec 3<>"/dev/tcp/$IP/3128"; printf "%s\r\nHost: x\r\n%bConnection: close\r\n\r\n" "$REQ" "$HDR" >&3
     if [[ "$REQ" == CONNECT* ]]; then timeout 15 head -n 1 <&3; else timeout 15 cat <&3; fi | tr -d "\r" | head -c 4000' \
    2>&1 || true
}
status() { head -n 1 <<< "$1"; }
direct() {
  docker exec "$client" bash -c "timeout 10 bash -c 'exec 3<>/dev/tcp/$1/$2' && echo CONNECTED" 2>&1 | tail -n 1 || true
}

health="$(docker inspect -f '{{.State.Health.Status}} user={{.Config.User}}' "$proxy" "$proxy_lit" | sort -u)"
check proxy_healthy_unprivileged "$([[ "$health" == "healthy user=proxy" ]] && echo 1)" "$health"
out="$(direct 1.1.1.1 443) / $(direct example.com 80)"
check direct_egress_blocked "$([[ "$out" != *CONNECTED* ]] && echo 1)" "client without proxy: $out"
public="$(status "$(via 'CONNECT example.com:443 HTTP/1.1')")"
check public_host_allowed "$([[ "$public" == "HTTP/1.1 200"* ]] && echo 1)" "CONNECT example.com:443 -> $public"
denied=""
for target in 'CONNECT 10.0.0.1:443' 'GET http://192.168.1.1/' 'GET http://169.254.169.254/latest/meta-data/' \
  'GET http://127.0.0.1:3128/' "GET http://$memcan_ip:80/" 'CONNECT [::1]:443'; do
  denied+="$target -> $(status "$(via "$target HTTP/1.1")"); "
done
check private_destinations_denied "$([[ "$(grep -o '403 Forbidden' <<< "$denied" | wc -l)" == 6 ]] && echo 1)" "$denied"

by_name="$(via 'GET http://memcan.test:8190/mcp HTTP/1.1')"
by_literal="$(via -p "$proxy_lit_ip" "GET http://$memcan_ip:8190/mcp HTTP/1.1")"
check memcan_exception_reachable \
  "$([[ "$by_name" == *"MEMCAN GET /mcp"* && "$by_literal" == *"MEMCAN GET /mcp"* ]] && echo 1)" \
  "configured by name: $(status "$by_name") / $(tail -n 1 <<< "$by_name"); by IP literal: $(status "$by_literal") / $(tail -n 1 <<< "$by_literal")"
# Only the configured name (or literal), port and address pass; a name whose DNS answer also holds MemCan's
# address, MemCan's address under another name, and another port fall to the private-range deny.
bypass=""
for request in 'GET http://evil.test:8190/x1' 'GET http://evil2.test:8190/x2' "GET http://$memcan_ip:8190/x3" \
  'GET http://memcan.test:8191/x4' 'CONNECT evil.test:8190'; do
  bypass+="name-configured $request -> $(status "$(via "$request HTTP/1.1")"); "
done
for request in 'GET http://evil.test:8190/y1' 'GET http://evil2.test:8190/y2' 'GET http://memcan.test:8190/y3'; do
  bypass+="literal-configured $request -> $(status "$(via -p "$proxy_lit_ip" "$request HTTP/1.1")"); "
done
sleep 1
reached="$(docker logs "$memcan" 2>&1 | grep -Ev ' /mcp |/garbage' || true)$(docker logs "$decoy" 2>&1 || true)"
check memcan_exception_bound_to_name_port_address \
  "$([[ "$(grep -o '403 Forbidden' <<< "$bypass" | wc -l)" == 8 && -z "$reached" ]] && echo 1)" \
  "$bypass stand-ins reached by these: [$(tr '\n' ';' <<< "$reached")]"

# T4b: no request data in any log, access or diagnostic (stdout and stderr), also on error paths.
canary="SR""CANARY$run_id"
via "GET http://example.com/${canary}path?q=${canary}query HTTP/1.1" "User-Agent: ${canary}ua\r\n" > /dev/null
via "PRI http://example.com/${canary}pri?q=${canary}pri HTTP/1.1" > /dev/null
via "PRI * HTTP/1.1" "X-Canary: ${canary}pri2\r\n" > /dev/null
via "GET http://memcan.test:8190/garbage/${canary}bad?q=${canary}bad HTTP/1.1" "User-Agent: ${canary}bad\r\n" > /dev/null
via "CONNECT example.com:443 HTTP/1.1" "User-Agent: ${canary}connect\r\nProxy-Authorization: Basic $(printf '%s:x' "$canary" | base64)\r\nX-Encoded: %53R${canary#SR}\r\n" > /dev/null
via "GET http://example.com/%${canary}%zz?${canary}=%G0 HTTP/1.1" > /dev/null
via "GET http://exa mple.com/${canary}space HTTP/1.1" > /dev/null
sleep 2
stdout="$(docker logs "$proxy" 2> /dev/null)"
stderr="$(docker logs "$proxy" 2>&1 > /dev/null)"
access="$(grep -E '^[0-9]{10}\.[0-9]{3} ' <<< "$stdout" || true)"
logged=1
for pattern in ' TCP_TUNNEL/200 [0-9]+ [0-9]+ CONNECT example.com:443 ' ' TCP_DENIED/403 .* CONNECT 10.0.0.1:443 -$' \
  ' TCP_DENIED/403 .* GET 169.254.169.254:80 -$' " TCP_MISS/200 .* GET memcan.test:8190 $memcan_ip$" \
  ' TCP_DENIED/403 .* GET evil.test:8190 -$'; do
  grep -Eq "$pattern" <<< "$access" || logged=0
done
check destinations_logged "$logged" "access log: $(tr '\n' ';' <<< "$access")"
in_logs="$( (grep -c "$canary" <<< "$stdout$stderr" || true) | head -n 1)"
check proxy_log_canary "$([[ "$in_logs" == 0 && "$access" == *"GET example.com:80"* ]] && echo 1)" \
  "canary in path, query, headers of GET, PRI, a garbage upstream answer, CONNECT with encoded headers, malformed URLs: $in_logs log lines contain it (stdout $(grep -c "$canary" <<< "$stdout" || true), stderr $(grep -c "$canary" <<< "$stderr" || true)) [$(grep "$canary" <<< "$stdout$stderr" | head -n 3 | cut -c 1-200 | tr '\n' ';')]; stderr: $(tr '\n' ';' <<< "$stderr" | head -c 1500)"
# Fields of the T4b format only: time, duration, client, result/status, bytes in, bytes out, method, host:port,
# upstream address; nothing else on stdout.
check log_format_t4b "$(awk 'NF != 9 { bad = 1 } END { exit bad }' <<< "$access" && [[ "$access" == "$stdout" ]] && echo 1)" \
  "every stdout line is an access line with 9 fields"

fails=0
for r in "${results[@]}"; do [[ "$r" == PASS ]] || fails=$((fails + 1)); done
echo "# egress proxy test: ${#results[@]} checks, $fails failed" | tee -a "$ledger"
((fails == 0))
