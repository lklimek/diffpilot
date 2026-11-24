#!/usr/bin/env bash
# Local deployment preparation only: never starts/stops the application stack.
# Run under flock /data/tmp/diffpilot-heavy.lock (builds and one-off containers are sequential).
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
state=${SR_LOCAL_STATE_DIR:-${XDG_STATE_HOME:-$HOME/.local/state}/diffpilot-containers}
prefix=${SR_LOCAL_IMAGE_PREFIX:-diffpilot-local}
registry=${SR_LOCAL_REGISTRY_NAME:-$prefix-registry}
port=${SR_LOCAL_REGISTRY_PORT:-15000}
registry_image=${SR_LOCAL_REGISTRY_IMAGE:-registry:2.8.3}
die() { echo "local-containers: $*" >&2; exit 1; }
[[ "$prefix" =~ ^[a-z0-9][a-z0-9-]*$ && "$registry" =~ ^[a-z0-9][a-z0-9-]*$ ]] || die 'invalid resource prefix'
if [[ ! "$port" =~ ^[0-9]+$ ]] || ((port <= 1024 || port >= 65536)); then die 'invalid registry port'; fi
mkdir -p "$state"
state=$(realpath "$state")
# Also serialize users of this state directory; partial keys pairs fail closed after interruption.
exec 8>"$state/prepare.lock"
flock 8

case "${1:-}" in
  prepare)
    scratch=$(mktemp -d "${SR_LOCAL_TMPDIR:-${RUNNER_TEMP:-/data/tmp}}/diffpilot-images.XXXXXX")
    trap 'rm -rf "$scratch"' EXIT
    if [[ -n "${SR_LOCAL_CLAUDIUS_SOURCE:-}" ]]; then
      source_tree=$SR_LOCAL_CLAUDIUS_SOURCE
    else
      source_tree=$scratch/claudius
      git clone --quiet --no-checkout https://github.com/lklimek/claudius.git "$source_tree"
      git -C "$source_tree" checkout --quiet --detach 71096d7d4481977added8761b2cfd63cd84852b4
    fi
    "$root/deploy/containers/prepare-claudius.sh" "$source_tree" "$scratch/curated"
    docker build -f "$root/deploy/server/Dockerfile" -t "$prefix-server:local" "$root"
    docker build -f "$root/deploy/containers/runner.Dockerfile" -t "$prefix-runner:local" "$root"
    docker build --build-context "workspace=$root" -t "$prefix-proxy:local" "$root/deploy/egress-proxy"
    docker build --build-context "claudius=$scratch/curated" -f "$root/deploy/worker/Dockerfile" -t "$prefix-worker:local" "$root"
    docker build -f "$root/deploy/fetch/Dockerfile" -t "$prefix-fetch:local" "$root"
    if docker container inspect "$registry" >"$scratch/registry.json" 2>/dev/null; then
      jq -e --arg port "$port" --arg image "$registry_image" '
        .[0] | .Config.Labels["diffpilot.local-registry"] == "1" and .Config.Image == $image and
        .HostConfig.PortBindings == {"5000/tcp": [{"HostIp":"127.0.0.1","HostPort":$port}]}
      ' "$scratch/registry.json" >/dev/null || die 'existing registry does not match the loopback-only template'
      docker start "$registry" >/dev/null
    else
      docker run -d --name "$registry" --label diffpilot.local-registry=1 --restart unless-stopped \
        -p "127.0.0.1:$port:5000" "$registry_image" >/dev/null
    fi
    for attempt in {1..30}; do
      if curl --noproxy '*' -fsS "http://127.0.0.1:$port/v2/" >/dev/null 2>&1; then break; fi
      [[ "$attempt" != 30 ]] || die 'registry did not become ready'
      sleep 1
    done
    refs=()
    for kind in worker fetch; do
      ref="localhost:$port/$prefix-$kind"
      docker tag "$prefix-$kind:local" "$ref:local"
      docker push "$ref:local"
      digest=$(docker image inspect "$ref:local" --format '{{json .RepoDigests}}' |
        jq -er --arg ref "$ref@sha256:" '.[] | select(startswith($ref))')
      refs+=("SR_${kind^^}_IMAGE=$digest")
    done
    printf '%s\n' "SR_SERVER_IMAGE=$prefix-server:local" "SR_RUNNER_IMAGE=$prefix-runner:local" \
      "SR_PROXY_IMAGE=$prefix-proxy:local" "${refs[@]}" >"$state/images.env.tmp"
    mv "$state/images.env.tmp" "$state/images.env"
    ;;
  secrets)
    directory=${2:?usage: local-containers.sh secrets NEW_RUNTIME_DIRECTORY}
    mkdir -p "$directory"
    directory=$(realpath "$directory")
    [[ "$directory" != *$'\n'* && "$directory" != *','* ]] || die 'unsupported directory name'
    # Mount only the explicit new-runtime directory, never the existing installation's secrets directory.
    docker run --rm --network none --user 0:0 --read-only --cap-drop ALL --cap-add CHOWN --cap-add DAC_OVERRIDE \
      --security-opt no-new-privileges:true --entrypoint python3 \
      --mount "type=bind,source=$directory,target=/secrets" \
      --mount "type=bind,source=$root/deploy/dev/local-runtime-secrets.py,target=/prepare.py,readonly" \
      "$prefix-worker:local" /prepare.py /secrets "${SR_SERVER_GID:-$(id -g)}"
    printf '%s' "$directory" >"$state/secrets-dir"
    ;;
  env)
    target=${2:?usage: local-containers.sh env NEW_NON_SECRET_ENV_FILE}
    [[ -f "$state/images.env" && -f "$state/secrets-dir" ]] || die 'run prepare and secrets first'
    directory=$(cat "$state/secrets-dir")
    # Paths are literal Compose env values; reject characters that change their meaning.
    [[ "$directory" != *[[:space:]\$\"\'\#]* ]] || die 'runtime path must not contain env metacharacters'
    (set -o noclobber
      {
        cat "$state/images.env"
        printf '%s\n' "SR_DOCKER_GID=${SR_DOCKER_GID:-1000}" \
          "SR_SERVER_UID=${SR_SERVER_UID:-$(id -u)}" "SR_SERVER_GID=${SR_SERVER_GID:-$(id -g)}" \
          "SR_GATEWAY_SECRET_KEY_PATH=$directory/gateway-secret-key" \
          "SR_GATEWAY_CREDENTIALS_TOKEN_PATH=$directory/gateway-credentials-token" \
          "SR_GATEWAY_KEYS_TOKEN_PATH=$directory/gateway-keys-token" \
          "SR_RUNNER_GATEWAY_KEYS_TOKEN_PATH=$directory/runner-gateway-keys-token"
      } >"$target")
    ;;
  check)
    shift
    (($#)) || die 'usage: local-containers.sh check CONFIG_ENV... RUNTIME_ENV (same order as compose)'
    [[ -f "$state/secrets-dir" ]] || die 'run secrets first'
    directory=$(cat "$state/secrets-dir")
    mounts=(--mount "type=bind,source=$root,target=$root,readonly"
      --mount "type=bind,source=$directory,target=$directory,readonly"
      --mount "type=bind,source=$(command -v docker),target=/usr/local/bin/docker,readonly")
    compose=$(docker info --format '{{range .ClientInfo.Plugins}}{{if eq .Name "compose"}}{{.Path}}{{end}}{{end}}')
    [[ -f "$compose" ]] || die 'Docker Compose plugin path is unavailable'
    mounts+=(--mount "type=bind,source=$compose,target=/usr/local/lib/docker/cli-plugins/docker-compose,readonly")
    args=()
    for file in "$@"; do
      file=$(realpath "$file")
      mounts+=(--mount "type=bind,source=$file,target=$file,readonly")
      args+=(--env-file "$file")
    done
    # No engine socket or network: Compose config is local; the guard reads only explicit configuration/files.
    docker run --rm --network none --user 0:0 --read-only --cap-drop ALL --cap-add DAC_OVERRIDE \
      --security-opt no-new-privileges:true --tmpfs /tmp:mode=0700,size=16m \
      -e DOCKER_CONFIG=/tmp/docker --entrypoint bash "${mounts[@]}" "$prefix-worker:local" \
      "$root/deploy/check-production-config.sh" --local "${args[@]}"
    ;;
  *) die 'usage: local-containers.sh prepare | secrets DIR | env FILE | check CONFIG_ENV... RUNTIME_ENV' ;;
esac
