#!/bin/sh
# Runner preflight (ADR-013, "codex-homes must be persistent and bounded"), run as uid runner by the supervisor
# before the runner: refuses to start (exit 78) unless the per-job CODEX_HOME root is its own bounded filesystem
# mounted nosuid, nodev, noexec.
#   SR_CODEX_HOMES_MAX_BYTES   upper bound of its capacity (default 17179869184 = 16 GiB)
#   SR_CODEX_HOMES_MAX_INODES  upper bound of its inodes (default 1048576)
#   SR_CODEX_HOMES_ALLOW_TMPFS 1 accepts a tmpfs (local stack only: a tmpfs loses a rotated credential on a crash,
#                              ADR-003), and only with SR_CODEX_HOMES_TMPFS_PUBLIC_URL = the server's public URL
#                              on localhost; production accepts only ext4
# Then executes its arguments (the runner), keeping stdin, the supervisor's control socket.
set -eu
state="${SR_RUNNER_STATE_DIR:-/var/lib/smart-review-runner}"
homes="${state}/codex-homes"
max_bytes="${SR_CODEX_HOMES_MAX_BYTES:-17179869184}"
max_inodes="${SR_CODEX_HOMES_MAX_INODES:-1048576}"

refuse() {
  echo "runner preflight: $1; the runner does not start (docs/runbook.md, \"Bounded codex-homes\")" >&2
  exit 78
}

mountpoint -q "$homes" || refuse "$homes is not a mount point"
[ "$(stat -c %d "$homes")" != "$(stat -c %d "$state")" ] || refuse "$homes is on the state volume's filesystem"
fstype="$(stat -f -c %T "$homes")"
case "$fstype" in
  ext2/ext3) ;;
  tmpfs)
    [ "${SR_CODEX_HOMES_ALLOW_TMPFS:-0}" = 1 ] || refuse "$homes is a tmpfs (not persistent)"
    printf '%s' "${SR_CODEX_HOMES_TMPFS_PUBLIC_URL:-}" | grep -Eq '^http://(localhost|127\.0\.0\.1|\[::1\])(:[0-9]+)?/?$' ||
      refuse "$homes is a tmpfs and the public URL '${SR_CODEX_HOMES_TMPFS_PUBLIC_URL:-}' is not on localhost"
    ;;
  *) refuse "$homes is $fstype, not the bounded ext4 filesystem" ;;
esac
options="$(awk -v m="$homes" '$5 == m { o = $6 } END { print o }' /proc/self/mountinfo)"
for option in nosuid nodev noexec; do
  case ",$options," in
    *",$option,"*) ;;
    *) refuse "$homes is mounted without $option (options: $options)" ;;
  esac
done
bytes=$(($(stat -f -c %b "$homes") * $(stat -f -c %S "$homes")))
[ "$bytes" -le "$max_bytes" ] || refuse "$homes holds $bytes bytes, more than SR_CODEX_HOMES_MAX_BYTES=$max_bytes"
inodes="$(stat -f -c %c "$homes")"
[ "$inodes" -le "$max_inodes" ] || refuse "$homes has $inodes inodes, more than SR_CODEX_HOMES_MAX_INODES=$max_inodes"
echo "runner preflight: $homes is a bounded $fstype filesystem ($bytes bytes, $inodes inodes, $options)" >&2
exec "$@"
