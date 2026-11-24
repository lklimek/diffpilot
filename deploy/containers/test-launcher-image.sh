#!/bin/sh
# Scoped Docker API smoke; no workers or provider/auth requests are created.
set -eu
name=v13w3-launcher-smoke
volume=v13w3-launcher-smoke-socket
image=${1:-v13w3-runner:test}
for resource in "$name" "$name-stat" "$volume"; do
    if docker container inspect "$resource" >/dev/null 2>&1 || docker volume inspect "$resource" >/dev/null 2>&1; then
        echo 'test resource already exists' >&2
        exit 1
    fi
done
for network in v13w3-smoke-worker v13w3-smoke-judge; do
    if docker network inspect "$network" >/dev/null 2>&1; then
        echo 'test network already exists' >&2
        exit 1
    fi
done
cleanup() {
    docker rm -f "$name" "$name-stat" >/dev/null 2>&1 || true
    docker volume rm "$volume" >/dev/null 2>&1 || true
    docker network rm v13w3-smoke-worker v13w3-smoke-judge >/dev/null 2>&1 || true
}
trap cleanup EXIT
for network in v13w3-smoke-worker v13w3-smoke-judge; do
    docker network create --internal --opt com.docker.network.bridge.gateway_mode_ipv4=isolated "$network" >/dev/null
done
# Inspect the daemon host's gid, not the sandbox's remapped nobody group.
socket_gid=$(docker run --rm --name "$name-stat" --network none --read-only --cap-drop ALL \
    --mount type=bind,source=/var/run/docker.sock,target=/var/run/docker.sock,readonly \
    --entrypoint stat "$image" -c %g /var/run/docker.sock)
docker run -d --name "$name" --network none --init --read-only --cap-drop ALL \
    --security-opt no-new-privileges:true --group-add "$socket_gid" \
    --memory 256m --cpus 1 --pids-limit 64 \
    --mount type=bind,source=/var/run/docker.sock,target=/var/run/docker.sock \
    --mount "source=$volume,target=/run/launcher" \
    -e SR_CONTAINER_PREFIX=v13w3-launcher-smoke \
    -e SR_WORKER_IMAGE=v13w3-worker@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
    -e SR_FETCH_IMAGE=v13w3-fetch@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb \
    -e SR_WORKER_NETWORK=v13w3-smoke-worker -e SR_JUDGE_NETWORK=v13w3-smoke-judge \
    -e SR_WORKER_PROXY=http://egress-proxy:3128 "$image" --launcher >/dev/null
for attempt in 1 2 3 4 5 6 7 8 9 10; do
    if docker exec "$name" test -S /run/launcher/launcher.sock; then break; fi
    if [ "$attempt" = 10 ]; then docker logs "$name"; exit 1; fi
    sleep 1
done
docker exec "$name" /bin/sh -ec '
    test "$(stat -c %a /run/launcher/launcher.sock)" = 600
    ! command -v codex
    ! command -v diffpilot-supervisor
'
docker exec "$name" perl -MIO::Socket::UNIX -MJSON::PP -e '
    my $s = IO::Socket::UNIX->new(Peer => "/run/launcher/launcher.sock") or die "socket";
    print $s qq({"id":"smoke","action":{"operation":"list"}}\n);
    my $r = decode_json(scalar <$s>);
    die "response" unless $r->{id} eq "smoke" && @{$r->{workers}} == 0;
'
docker stop -t 5 "$name" >/dev/null
[ "$(docker inspect --format '{{.State.ExitCode}}' "$name")" = 0 ]
printf '%s\n' 'IM2 launcher mode: private socket, scoped list and graceful shutdown PASS'
