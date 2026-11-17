#!/bin/sh
# Removes the package manager and the interpreters Codex does not need from the runner image (threat model T4:
# runner_image_has_no_toolchains). Run as the last step of an image; it cannot be undone inside the image.
# Arguments: tools allowed to remain (the container test image keeps python3).
set -eu
rm -rf \
  /usr/bin/apt /usr/bin/apt-* /usr/lib/apt /etc/apt /var/lib/apt /var/cache/apt /var/log/apt \
  /usr/bin/dpkg /usr/bin/dpkg-* /usr/sbin/dpkg-* /usr/lib/dpkg /etc/dpkg /var/lib/dpkg /var/log/dpkg.log \
  /usr/bin/debconf* /usr/share/debconf /var/cache/debconf \
  /usr/bin/perl* /usr/lib/x86_64-linux-gnu/perl* /usr/share/perl*
for tool in apt apt-get dpkg perl python python3 gcc cc c++ clang rustc cargo make node npm git-lfs; do
  case " $* " in *" $tool "*) continue ;; esac
  if command -v "$tool" > /dev/null 2>&1; then
    echo "strip-image: $tool is still present" >&2
    exit 1
  fi
done
