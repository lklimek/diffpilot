#!/usr/bin/env bash
# Prepares the bounded filesystem for the runner's per-job CODEX_HOMEs on the Docker host (ADR-013): a
# preallocated ext4 image of fixed size and inode count, loop-mounted through /etc/fstab (nosuid, nodev, noexec;
# the runner's preflight requires all three), root owned by the runner uid 10001, mode 0711, without lost+found
# (the runner treats every entry as a job's home). No boot-time fsck (pass 0), so lost+found is not recreated; the
# content is per-job scratch. Persistent across container crashes (ADR-003), bounded so the agent cannot fill the
# host disk. Idempotent: an existing image is never reformatted or resized.
#
# Usage (root):  sudo SR_CODEX_HOMES_SIZE=8G deploy/host/prepare-codex-homes.sh
# Env:  SR_CODEX_HOMES_SIZE    size of a new image (default 8G; fallocate syntax). Must stay below the runner's
#                              SR_CODEX_HOMES_MAX_BYTES (default 16 GiB).
#       SR_CODEX_HOMES_INODES  inodes of a new image (default 262144; below SR_CODEX_HOMES_MAX_INODES)
#       SR_CODEX_HOMES_IMAGE   image file (default /srv/smart-review/codex-homes.img)
#       SR_CODEX_HOMES_DIR     mount point, bind-mounted into the runner (default /srv/smart-review/codex-homes)
set -euo pipefail

size="${SR_CODEX_HOMES_SIZE:-8G}"
inodes="${SR_CODEX_HOMES_INODES:-262144}"
image="${SR_CODEX_HOMES_IMAGE:-/srv/smart-review/codex-homes.img}"
dir="${SR_CODEX_HOMES_DIR:-/srv/smart-review/codex-homes}"
runner_uid=10001

die() { echo "prepare-codex-homes: $*" >&2; exit 1; }

[[ $EUID -eq 0 ]] || die "run as root (sudo)"
[[ "$image" == /* && "$dir" == /* ]] || die "SR_CODEX_HOMES_IMAGE and SR_CODEX_HOMES_DIR must be absolute paths"
[[ "$image$dir" != *[[:space:]]* ]] || die "paths must not contain white space (fstab)"
for tool in fallocate mkfs.ext4 blkid mountpoint findmnt; do
  command -v "$tool" > /dev/null || die "$tool is required"
done

install -d -m 0755 "$(dirname "$image")" "$dir"

if [[ -e "$image" ]]; then
  [[ "$(blkid -p -s TYPE -o value "$image")" == ext4 ]] || die "$image exists but is not an ext4 image; move it away"
  echo "prepare-codex-homes: keeping the existing image $image (not resized)"
else
  fallocate -l "$size" "$image"
  chmod 0600 "$image"
  mkfs.ext4 -q -F -N "$inodes" -m 0 -L sr-codex-homes "$image"
  echo "prepare-codex-homes: created $image ($size, $inodes inodes)"
fi

entry="$image $dir ext4 loop,nosuid,nodev,noexec,nofail 0 0"
if ! awk -v d="$dir" '$2 == d { found = 1 } END { exit !found }' /etc/fstab; then
  printf '%s\n' "$entry" >> /etc/fstab
  echo "prepare-codex-homes: added to /etc/fstab: $entry"
elif ! grep -qxF "$entry" /etc/fstab; then
  die "/etc/fstab already mounts $dir differently; expected: $entry"
fi

mountpoint -q "$dir" || mount "$dir"
[[ "$(findmnt -no FSTYPE --mountpoint "$dir")" == ext4 ]] || die "$dir is not the ext4 image after mounting"
[[ "$(findmnt -no SOURCE --mountpoint "$dir")" == /dev/loop* ]] || die "$dir is not loop-mounted"
rmdir "$dir/lost+found" 2> /dev/null || true
chown "$runner_uid:$runner_uid" "$dir"
chmod 0711 "$dir"
df -h "$dir"
df -i "$dir"
echo "prepare-codex-homes: ready; set SR_CODEX_HOMES_DIR=$dir in .env"
