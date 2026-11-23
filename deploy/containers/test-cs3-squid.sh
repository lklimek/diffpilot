#!/usr/bin/env bash
# Run under flock /data/tmp/diffpilot-heavy.lock; never touch another compose project.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
prefix=v13w3-cs3-test
scratch=$(mktemp -d /data/tmp/v13w3-cs3.XXXXXX)
export DOCKER_CONFIG="$scratch/docker"
mkdir "$DOCKER_CONFIG"
cleanup() {
  docker rm -f "$prefix-fetch" "$prefix-seed" "$prefix-proxy" >/dev/null 2>&1 || true
  docker network rm "$prefix-worker" "$prefix-egress" >/dev/null 2>&1 || true
  docker volume rm "$prefix-work" "$prefix-c2" >/dev/null 2>&1 || true
  rm -rf "$scratch"
}
trap cleanup EXIT
# Fail before creating anything if a previous independent run owns these names.
for kind in container network volume; do
  case "$kind" in
    container) names=("$prefix-fetch" "$prefix-seed" "$prefix-proxy") ;;
    network) names=("$prefix-worker" "$prefix-egress") ;;
    volume) names=("$prefix-work" "$prefix-c2") ;;
  esac
  for name in "${names[@]}"; do
    if docker "$kind" inspect "$name" >/dev/null 2>&1; then
      trap - EXIT
      echo "test resource already exists: $name" >&2
      exit 1
    fi
  done
done

docker build -t v13w3-fetch:test -f "$root/deploy/fetch/Dockerfile" "$root"
docker build -t v13w3-proxy:test --build-context "workspace=$root" "$root/deploy/egress-proxy"
docker network create --internal --opt com.docker.network.bridge.gateway_mode_ipv4=isolated "$prefix-worker"
docker network create "$prefix-egress"
docker volume create "$prefix-work"
docker volume create "$prefix-c2"
# The fixture has a build script that must never execute, and hostile repo-local Cargo configuration.
mkdir -p "$scratch/repo/src" "$scratch/repo/.cargo"
python3 - "$root/Cargo.lock" "$scratch/repo" <<'PY'
from pathlib import Path
import sys
import tomllib
lock = tomllib.loads(Path(sys.argv[1]).read_text())
pkg = next(p for p in lock['package'] if p['name'] == 'itoa')
repo = Path(sys.argv[2])
(repo / 'Cargo.toml').write_text('[package]\nname="cs3-fixture"\nversion="0.1.0"\nedition="2021"\n[dependencies]\nitoa="=' + pkg['version'] + '"\n')
(repo / 'Cargo.lock').write_text('version=4\n[[package]]\nname="cs3-fixture"\nversion="0.1.0"\ndependencies=["itoa"]\n[[package]]\nname="itoa"\nversion="' + pkg['version'] + '"\nsource="' + pkg['source'] + '"\nchecksum="' + pkg['checksum'] + '"\n')
(repo / 'src/lib.rs').write_text('pub fn fixture() {}\n')
(repo / 'build.rs').write_text('fn main() { panic!("CS3 executed PR code"); }\n')
(repo / '.cargo/config.toml').write_text('[http]\nproxy="http://127.0.0.1:1"\n[build]\nrustc-wrapper="/must-not-run"\n')
PY
# Seed only the synthetic fixture; never read a host credential store.
docker create --name "$prefix-seed" --network none --user 0:0 --cap-drop ALL --cap-add CHOWN --mount "source=$prefix-work,target=/work" \
  --mount "source=$prefix-c2,target=/c2" --entrypoint /bin/chown v13w3-fetch:test -R 10002:10002 /work/repo >/dev/null
docker cp "$scratch/repo" "$prefix-seed:/work/repo"
docker start -a "$prefix-seed"
docker rm "$prefix-seed" >/dev/null

docker create --name "$prefix-proxy" --network "$prefix-egress" --read-only --cap-drop ALL \
  --security-opt no-new-privileges:true --tmpfs /run/squid:uid=13,gid=13,mode=0750,size=1m \
  --memory 256m --cpus 0.5 --pids-limit 128 v13w3-proxy:test >/dev/null
docker network connect --alias egress-proxy "$prefix-worker" "$prefix-proxy"
docker start "$prefix-proxy" >/dev/null
for attempt in {1..30}; do
  status=$(docker inspect --format '{{.State.Health.Status}}' "$prefix-proxy")
  if [ "$status" = healthy ]; then break; fi
  if [ "$attempt" = 30 ]; then docker logs "$prefix-proxy"; exit 1; fi
  sleep 1
done
run_fetch() {
  docker run --name "$prefix-fetch" --network "$prefix-worker" --init --read-only --cap-drop ALL \
    --security-opt no-new-privileges:true --user 10002:10002 --memory 2g --memory-swap 2g --cpus 2 --pids-limit 1024 \
    --mount "source=$prefix-work,target=/work,readonly" --mount "source=$prefix-c2,target=/c2" \
    --tmpfs /tmp:uid=10002,gid=10002,mode=0700,size=512m \
    -e HTTP_PROXY=http://egress-proxy:3128 -e HTTPS_PROXY=http://egress-proxy:3128 \
    --entrypoint /bin/bash v13w3-fetch:test -ec '
      if timeout 3 bash -c "exec 3<>/dev/tcp/index.crates.io/443"; then
        echo "direct Internet egress unexpectedly succeeded" >&2; exit 1
      fi
      exec 3<>/dev/tcp/egress-proxy/3128
      printf "GET http://169.254.169.254/latest/meta-data/ HTTP/1.0\r\n\r\n" >&3
      IFS= read -r status <&3
      [[ "$status" == *" 403 "* ]]
      exec 3>&-
      /usr/local/bin/dp-fetch
      test ! -d /work/repo/target
      test -n "$(find /c2/registry/src -name lib.rs -print -quit)"
    '
  docker rm "$prefix-fetch" >/dev/null
}
run_fetch
# A second fetch reuses the extracted sources; a lock-free copy must stay in tmpfs.
docker run --name "$prefix-seed" --network none --user 10002:10002 --read-only --cap-drop ALL \
  --mount "source=$prefix-work,target=/work" --entrypoint /bin/sh v13w3-fetch:test \
  -ec 'cp /work/repo/Cargo.lock /work/expected.lock; rm /work/repo/Cargo.lock'
docker rm "$prefix-seed" >/dev/null
run_fetch
docker logs "$prefix-proxy" 2>&1 | tee "$scratch/proxy.log"
rg 'index.crates.io:443' "$scratch/proxy.log"
rg 'static.crates.io:443' "$scratch/proxy.log"
printf '%s\n' 'CS3 through Squid: locked and lock-free fetch PASS; direct egress and metadata denied; no PR build executed'
