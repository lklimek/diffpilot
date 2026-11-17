#!/bin/bash
# Runner preflight (ADR-013), run as uid runner by the supervisor before the runner; refuses to start (exit 78):
# 1. unless the per-job CODEX_HOME root is its own bounded filesystem mounted nosuid, nodev, noexec;
#   SR_CODEX_HOMES_MAX_BYTES   upper bound of its capacity (default 17179869184 = 16 GiB)
#   SR_CODEX_HOMES_MAX_INODES  upper bound of its inodes (default 1048576)
#   SR_CODEX_HOMES_ALLOW_TMPFS 1 accepts a tmpfs (local stack only: a tmpfs loses a rotated credential on a crash,
#                              ADR-003), and only with SR_CODEX_HOMES_TMPFS_PUBLIC_URL = the server's public URL
#                              on localhost; production accepts only ext4
# 2. unless the container's networks are isolated: no default route, and no gateway candidate (the first address of
#    every attached subnet, every route's next hop, IPv4 and IPv6) answers a TCP connect, not even with a refusal.
#    An Engine older than 28.0 ignores the runner network's `isolated` gateway mode and leaves the host's address
#    there. SR_RUNNER_DEV_HOST_GATEWAY=1 skips this (development smoke only; never in a deployment).
# Then executes its arguments (the runner), keeping stdin, the supervisor's control socket.
set -euo pipefail
state="${SR_RUNNER_STATE_DIR:-/var/lib/smart-review-runner}"
homes="${state}/codex-homes"
max_bytes="${SR_CODEX_HOMES_MAX_BYTES:-17179869184}"
max_inodes="${SR_CODEX_HOMES_MAX_INODES:-1048576}"
probe_ports=(22 53 80 443 3000 3001 8080)

refuse() {
  echo "runner preflight: $1; the runner does not start (docs/runbook.md, \"Exit codes and restarts\")" >&2
  exit 78
}

# --- 1. bounded codex-homes ---------------------------------------------------------------------------------------
mountpoint -q "$homes" || refuse "$homes is not a mount point"
[[ "$(stat -c %d "$homes")" != "$(stat -c %d "$state")" ]] || refuse "$homes is on the state volume's filesystem"
fstype="$(stat -f -c %T "$homes")"
case "$fstype" in
  ext2/ext3) ;;
  tmpfs)
    [[ "${SR_CODEX_HOMES_ALLOW_TMPFS:-0}" == 1 ]] || refuse "$homes is a tmpfs (not persistent)"
    [[ "${SR_CODEX_HOMES_TMPFS_PUBLIC_URL:-}" =~ ^http://(localhost|127\.0\.0\.1|\[::1\])(:[0-9]+)?/?$ ]] ||
      refuse "$homes is a tmpfs and the public URL '${SR_CODEX_HOMES_TMPFS_PUBLIC_URL:-}' is not on localhost"
    ;;
  *) refuse "$homes is $fstype, not the bounded ext4 filesystem" ;;
esac
options="$(awk -v m="$homes" '$5 == m { o = $6 } END { print o }' /proc/self/mountinfo)"
for option in nosuid nodev noexec; do
  [[ ",$options," == *",$option,"* ]] || refuse "$homes is mounted without $option (options: $options)"
done
bytes=$(($(stat -f -c %b "$homes") * $(stat -f -c %S "$homes")))
((bytes <= max_bytes)) || refuse "$homes holds $bytes bytes, more than SR_CODEX_HOMES_MAX_BYTES=$max_bytes"
inodes="$(stat -f -c %c "$homes")"
((inodes <= max_inodes)) || refuse "$homes has $inodes inodes, more than SR_CODEX_HOMES_MAX_INODES=$max_inodes"
echo "runner preflight: $homes is a bounded $fstype filesystem ($bytes bytes, $inodes inodes, $options)" >&2

# --- 2. network isolation -----------------------------------------------------------------------------------------
not_isolated() {
  refuse "the runner's network is not isolated: $1. Docker Engine 28.0 or newer is required, with the runner network option com.docker.network.bridge.gateway_mode_ipv4=isolated (and _ipv6) of deploy/compose.yaml"
}
# ipv4 HEX: dotted address of a little-endian /proc/net/route field.
ipv4() { printf '%d.%d.%d.%d' "0x${1:6:2}" "0x${1:4:2}" "0x${1:2:2}" "0x${1:0:2}"; }
# ipv6 HEX: colon form of a 32-digit /proc/net/ipv6_route field.
ipv6() { sed 's/.\{4\}/&:/g; s/:$//' <<< "$1"; }

if [[ "${SR_RUNNER_DEV_HOST_GATEWAY:-0}" == 1 ]]; then
  echo "runner preflight: network isolation NOT checked (SR_RUNNER_DEV_HOST_GATEWAY=1, development only)" >&2
else
  # Own addresses never count as a gateway.
  own=" $(awk '/\|--/ { a = $2 } /host LOCAL/ { print a }' /proc/net/fib_trie 2> /dev/null | sort -u | tr '\n' ' ')"
  own+="$(awk '{ print $1 }' /proc/net/if_inet6 2> /dev/null | while read -r a; do ipv6 "$a"; done | tr '\n' ' ') "
  candidates=()
  while read -r iface dest gateway _ _ _ _ mask _; do
    [[ "$iface" == Iface || "$iface" == lo ]] && continue
    [[ "$dest" == 00000000 && "$mask" == 00000000 ]] && not_isolated "IPv4 default route via $(ipv4 "$gateway") on $iface"
    [[ "$gateway" != 00000000 ]] && candidates+=("$(ipv4 "$gateway")")
    first=$(((0x${dest:6:2} << 24 | 0x${dest:4:2} << 16 | 0x${dest:2:2} << 8 | 0x${dest:0:2}) + 1))
    candidates+=("$((first >> 24 & 255)).$((first >> 16 & 255)).$((first >> 8 & 255)).$((first & 255))")
  done < /proc/net/route
  while read -r dest plen _ _ nexthop _ _ _ _ iface; do
    [[ "$iface" == lo || "$dest" == fe80* || "$dest" == ff* ]] && continue
    [[ "$dest" == 00000000000000000000000000000000 && "$plen" == 00 ]] && not_isolated "IPv6 default route on $iface"
    [[ "$nexthop" != 00000000000000000000000000000000 ]] && candidates+=("$(ipv6 "$nexthop")")
    candidates+=("$(ipv6 "${dest:0:28}0001")")
  done < <(cat /proc/net/ipv6_route 2> /dev/null || true)
  checked=()
  for candidate in $(printf '%s\n' "${candidates[@]}" | sort -u); do
    [[ "$own" == *" $candidate "* ]] && continue
    checked+=("$candidate")
    for port in "${probe_ports[@]}"; do
      # Unreachable, no route or no answer within 1 s: isolated. Connected or refused: an IP stack answers there.
      if answer="$(timeout 1 bash -c "exec 3<>/dev/tcp/$candidate/$port" 2>&1)"; then answer=connected; fi
      case "$answer" in
        *connected* | *"Connection refused"*)
          not_isolated "gateway candidate $candidate answered on port $port (${answer##*: })"
          ;;
      esac
    done
  done
  echo "runner preflight: network isolated (no default route; no answer from gateway candidates: ${checked[*]:-none})" >&2
fi
exec "$@"
