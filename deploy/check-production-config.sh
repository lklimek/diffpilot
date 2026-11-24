#!/usr/bin/env bash
# Rejects development providers/legacy bypasses in production files and rendered Compose configuration.
# Only bounded service /tmp and Squid configuration tmpfs mounts are allowed; worker templates own job tmpfs.
# With --env-file, checks required settings and private service secret files without printing their contents.
# Usage: deploy/check-production-config.sh [--env-file FILE [--local | --secret-files]] [--no-render]
#        deploy/check-production-config.sh --self-test [--no-render]
#   (no option)       the files, production rendered with .env.example, local rendered with it on localhost
#   --env-file FILE   production rendered with FILE (for example the deployment's .env), and FILE's values
#   --local           also render the local variant; check runtime files/settings, allow HTTP and host uid/gid
#   --secret-files    secrets as files (compose.secrets.yaml): rendered with it; the four secret variables must be
#                     empty and their files in SR_SECRETS_DIR present (run as root to read them)
#   --no-render       no rendering (the files and FILE's values only)
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
forbidden='SR_RUNNER_DEV_NO_ISOLATION|SR_RUNNER_DEV_HOST_GATEWAY|SR_SUPERVISOR_TEST_FAULTS|SR_SUPERVISOR_UNPRIVILEGED|R1_FAULT_[A-Z_]+|fake-provider|SR_RUNNER_PROVIDER'
# Files that build or run the production stack (the local variant runs the same images).
files=(
  deploy/compose.yaml deploy/compose.local.yaml .env.example
  deploy/server/Dockerfile deploy/server/healthcheck.sh deploy/server/entrypoint.sh
  deploy/containers/runner.Dockerfile
  deploy/egress-proxy/Dockerfile deploy/egress-proxy/entrypoint.sh deploy/egress-proxy/squid.conf
)
# The only tmpfs allowed in RAM (service and target): the proxy's configuration, written at every start.
ram_allowed='^(egress-proxy /run/squid|(runner|launcher|codex-gateway) /tmp)$'

# scan ROOT: prints every offending line, returns 1 if there is one.
scan() {
  local root="$1" found=0 file
  for file in "${files[@]}"; do
    [[ -f "$root/$file" ]] || { echo "check-production-config: missing $file" >&2; found=1; continue; }
    if grep -nEH "$forbidden" "$root/$file"; then found=1; fi
  done
  return "$found"
}

secrets=(SR_SECRET_KEY SR_RUNNER_SECRET SR_GITHUB_CLIENT_SECRET SR_MEMCAN_TOKEN)
declare -A secret_file_names=(
  [SR_SECRET_KEY]=sr-secret-key [SR_RUNNER_SECRET]=sr-runner-secret
  [SR_GITHUB_CLIENT_SECRET]=sr-github-client-secret [SR_MEMCAN_TOKEN]=sr-memcan-token
)
# Values that must differ from .env.example's.
own_values=(SR_PUBLIC_URL SR_GITHUB_APP_ID SR_GITHUB_CLIENT_ID SR_MEMCAN_URL
  SR_DOCKER_GID SR_WORKER_IMAGE SR_FETCH_IMAGE "${secrets[@]}")
declare -A gateway_files=(
  [SR_GATEWAY_SECRET_KEY_PATH]=gateway-secret-key
  [SR_GATEWAY_CREDENTIALS_TOKEN_PATH]=gateway-credentials-token
  [SR_GATEWAY_KEYS_TOKEN_PATH]=gateway-keys-token
  [SR_RUNNER_GATEWAY_KEYS_TOKEN_PATH]=runner-gateway-keys-token
)
# Variables that only the local or development overrides use.
dev_only='^(SR_SERVER_UID|SR_SERVER_GID)$'


# load_env FILE NAME: fills the associative array NAME as docker compose reads an env file (last assignment wins,
# `export ` prefix, quotes, ` #` comments); keys whose value compose would interpolate (`$` outside single quotes)
# go to the global set `interpolated`.
declare -A interpolated=()
load_env() {
  local -n into="$2"
  local line key value
  into=()
  interpolated=()
  while IFS= read -r line || [[ -n "$line" ]]; do
    line="${line#"${line%%[![:space:]]*}"}"
    [[ -z "$line" || "$line" == \#* || "$line" != *=* ]] && continue
    line="${line#export }"
    key="${line%%=*}"
    key="${key%"${key##*[![:space:]]}"}"
    value="${line#*=}"
    value="${value#"${value%%[![:space:]]*}"}"
    if [[ "$value" =~ ^\'(.*)\'[[:space:]]*(#.*)?$ ]]; then
      value="${BASH_REMATCH[1]}"
    else
      if [[ "$value" =~ ^\"(.*)\"[[:space:]]*(#.*)?$ ]]; then
        value="${BASH_REMATCH[1]}"
      else
        value="${value%%[[:space:]]#*}"
        value="${value%"${value##*[![:space:]]}"}"
      fi
      if [[ "$value" == *\$* ]]; then interpolated["$key"]=1; fi
    fi
    # shellcheck disable=SC2034 # written through the nameref
    into["$key"]="$value"
  done < "$1"
}

# Match the gateway's whitespace trimming without printing values outside command substitutions.
secret_file_value() {
  local content
  content="$(cat -- "$1")"
  content="${content#"${content%%[![:space:]]*}"}"
  printf '%s' "${content%"${content##*[![:space:]]}"}"
}

# check_secret_file NAME PATH: a private, non-empty, non-placeholder service secret file; the runner
# secret at least 32 characters. Optional uid/gid/mode describe the consumer. Prints problems by name; never the content.
check_secret_file() {
  local name="$1" path="$2" uid="${3:-10010}" gid="${4:-}" mode="${5:-400}" found=0 actual_uid actual_gid actual_mode content decoded_bytes canonical
  if [[ ! -f "$path" ]]; then
    echo "check-production-config: $name: the file $path is missing (secrets as files)" >&2
    return 1
  fi
  if ! read -r actual_uid actual_gid actual_mode < <(stat -c '%u %g %a' "$path"); then
    echo "check-production-config: $name: cannot stat $path" >&2
    return 1
  fi
  if [[ "$actual_uid" != "$uid" || "$actual_mode" != "$mode" || (-n "$gid" && "$actual_gid" != "$gid") ]]; then
    echo "check-production-config: $name: $path requires owner $uid${gid:+:$gid} mode $mode for its consumers" >&2
    found=1
  fi
  if [[ ! -r "$path" ]]; then
    echo "check-production-config: $name: cannot read $path to check it; run as root" >&2
    return 1
  fi
  content="$(secret_file_value "$path")"
  if [[ -z "$content" ]]; then
    echo "check-production-config: $name: $path is empty" >&2
    found=1
  elif [[ "$content" == *REPLACE* ]]; then
    echo "check-production-config: $name: $path still holds a placeholder" >&2
    found=1
  elif [[ "$name" == SR_RUNNER_SECRET ]] && ((${#content} < 32)); then
    echo "check-production-config: $name: $path holds fewer than 32 characters (openssl rand -hex 32)" >&2
    found=1
  fi
  if [[ "$name" == SR_GATEWAY_SECRET_KEY_PATH ]]; then
    if ! decoded_bytes="$(printf '%s' "$content" | base64 --decode 2>/dev/null | wc -c)" || [[ "$decoded_bytes" != 32 ]]; then
      echo "check-production-config: $name: $path must be base64 encoding of exactly 32 bytes (openssl rand -base64 32)" >&2
      found=1
    else
      canonical="$(printf '%s' "$content" | base64 --decode | base64 -w0)"
      if [[ "$canonical" != "$content" ]]; then
        echo "check-production-config: $name: $path must use canonical base64 encoding" >&2
        found=1
      fi
    fi
  elif [[ "$name" == SR_GATEWAY_*_TOKEN_PATH || "$name" == SR_RUNNER_GATEWAY_KEYS_TOKEN_PATH ]]; then
    if (($(printf '%s' "$content" | wc -c) < 32)); then
      echo "check-production-config: $name: $path holds fewer than 32 bytes (openssl rand -hex 32)" >&2
      found=1
    fi
  fi
  return "$found"
}

# check_env FILE EXAMPLE SECRET_FILES(0|1) LOCAL(0|1): the production values of FILE; prints problems by variable name only.
check_env() {
  local -A values=() example=()
  local found=0 key value secrets_dir path gateway_keys runner_keys credentials digest nibble local_mode="${4:-0}"
  if [[ ! -f "$1" ]]; then
    echo "check-production-config: env file $1 is missing" >&2
    return 1
  fi
  say() { echo "check-production-config: $*" >&2; found=1; }
  load_env "$1" values
  for key in "${!interpolated[@]}"; do
    say "$key uses \$ interpolation, so this check cannot see the value compose uses; write it literally (single quotes keep a \$)"
  done
  load_env "$2" example
  for key in "${!values[@]}"; do
    if [[ "${values[$key]}" == *REPLACE* ]]; then say "$key still holds a placeholder (REPLACE...)"; fi
    if [[ "$key" =~ ^($forbidden)$ || ($local_mode == 0 && "$key" =~ $dev_only) ]]; then
      say "$key is a local or development setting; remove it from a production env file"
    fi
  done
  for key in "${own_values[@]}"; do
    value="${values[$key]:-}"
    if [[ -n "$value" && "$value" != *REPLACE* && "$value" == "${example[$key]:-}" ]]; then
      say "$key still holds the .env.example value"
    fi
  done
  if [[ "$local_mode" == 0 && "${values[COMPOSE_FILE]:-}" =~ compose\.local|/dev/ ]]; then
    say "COMPOSE_FILE names the local or a development override"
  fi
  if [[ "$local_mode" == 0 && ! "${values[SR_PUBLIC_URL]:-}" =~ ^https://[^/?#]+ ]]; then
    say "SR_PUBLIC_URL must be an https:// URL (the reverse proxy's public name; Secure cookies)"
  fi
  if [[ -z "${values[SR_ALLOWED_LOGINS]:-}" ]]; then
    say "SR_ALLOWED_LOGINS is empty: list the GitHub logins allowed to sign in, comma-separated (the server does not start without one)"
  fi
  if [[ -z "${values[SR_TRUSTED_PROXIES]:-}" ]]; then
    echo "check-production-config: warning: SR_TRUSTED_PROXIES is empty: behind the reverse proxy every client" \
      "then shares one address, so failed access-token attempts of all clients count against one limit;" \
      "set the proxy's address (the Docker gateway) and let the proxy append X-Forwarded-For" >&2
  fi
  if [[ ! "${values[SR_DOCKER_GID]:-}" =~ ^[0-9]+$ ]]; then
    say "SR_DOCKER_GID must be the Docker socket's numeric group id"
  fi
  for key in SR_WORKER_IMAGE SR_FETCH_IMAGE; do
    value="${values[$key]:-}"
    if [[ ! "$value" =~ ^[^[:space:]@]+@sha256:[a-fA-F0-9]{64}$ ]]; then
      say "$key must pin a repository@sha256 digest"
    else
      digest="${value##*@sha256:}"
      nibble="${digest:0:1}"
      if [[ -z "${digest//"$nibble"/}" ]]; then say "$key holds a placeholder digest"; fi
    fi
  done
  for key in "${!gateway_files[@]}"; do
    path="${values[$key]:-/srv/diffpilot/secrets/${gateway_files[$key]}}"
    if [[ "$key" == SR_RUNNER_GATEWAY_KEYS_TOKEN_PATH ]]; then
      check_secret_file "$key" "$path" 10002 || found=1
    else
      check_secret_file "$key" "$path" 10010 || found=1
    fi
  done
  gateway_keys="${values[SR_GATEWAY_KEYS_TOKEN_PATH]:-/srv/diffpilot/secrets/gateway-keys-token}"
  runner_keys="${values[SR_RUNNER_GATEWAY_KEYS_TOKEN_PATH]:-/srv/diffpilot/secrets/runner-gateway-keys-token}"
  credentials="${values[SR_GATEWAY_CREDENTIALS_TOKEN_PATH]:-/srv/diffpilot/secrets/gateway-credentials-token}"
  if [[ -f "$credentials" && -r "$credentials" && -f "$gateway_keys" && -r "$gateway_keys" ]] &&
      [[ "$(secret_file_value "$credentials")" == "$(secret_file_value "$gateway_keys")" ]]; then
    say "SR_GATEWAY_CREDENTIALS_TOKEN_PATH and SR_GATEWAY_KEYS_TOKEN_PATH must contain different scope tokens"
  fi
  if ! cmp -s -- "$gateway_keys" "$runner_keys"; then
    say "gateway and runner keys-scope files must contain identical bytes (run as root)"
  fi
  if ((${3})); then
    secrets_dir="${values[SR_SECRETS_DIR]:-/srv/diffpilot/secrets}"
    for key in "${secrets[@]}"; do
      if [[ -n "${values[$key]:-}" ]]; then say "$key must be empty with secrets as files (the file is used)"; fi
      if [[ "$key" == SR_RUNNER_SECRET ]]; then
        check_secret_file "$key" "$secrets_dir/${secret_file_names[$key]}" 10002 10010 440 || found=1
      else
        check_secret_file "$key" "$secrets_dir/${secret_file_names[$key]}" || found=1
      fi
    done
  else
    for key in "${secrets[@]}"; do
      if [[ -z "${values[$key]:-}" ]]; then say "$key is empty (or use --secret-files with compose.secrets.yaml)"; fi
    done
    value="${values[SR_RUNNER_SECRET]:-}"
    if [[ -n "$value" && "$value" != *REPLACE* ]] && ((${#value} < 32)); then
      say "SR_RUNNER_SECRET is shorter than 32 characters (openssl rand -hex 32)"
    fi
  fi
  return "$found"
}

env_file="" local=0 render=1 self_test=0 secret_files=0
while (($#)); do
  case "$1" in
    --env-file) env_file="${2:?--env-file needs a file}"; shift 2 ;;
    --local) local=1; shift ;;
    --secret-files) secret_files=1; shift ;;
    --no-render) render=0; shift ;;
    --self-test) self_test=1; shift ;;
    *) echo "check-production-config: unknown argument $1 (see the header)" >&2; exit 2 ;;
  esac
done

# can_render: 0 when rendering is wanted and possible; 1 when --no-render; exits 1 when compose or jq is missing.
can_render() {
  ((render)) || return 1
  if ! docker compose version > /dev/null 2>&1; then
    echo "check-production-config: docker compose is not available, so the rendered configuration cannot be checked; install it or pass --no-render" >&2
    exit 1
  fi
  if ! command -v jq > /dev/null 2>&1; then
    echo "check-production-config: jq is not available, so the rendered configuration cannot be checked for RAM mounts; install jq or pass --no-render" >&2
    exit 1
  fi
}

# rendered ROOT ENV_FILE [OVERRIDE]: `docker compose config` of the production stack has no switch and keeps
# nothing in RAM (OVERRIDE: a file in ROOT/deploy, or an absolute path).
rendered() {
  local out mounts ram files=(-f "$1/deploy/compose.yaml")
  case "${3:-}" in
    "") ;;
    /*) files+=(-f "$3") ;;
    *) files+=(-f "$1/deploy/$3") ;;
  esac
  # Bind mount sources only have to exist for `up`; `config` does not check them.
  out="$(docker compose --env-file "$2" "${files[@]}" config --format json 2>&1)" || { echo "$out" >&2; return 1; }
  if grep -nE "$forbidden" <<< "$out"; then return 1; fi
  # shellcheck disable=SC2016 # jq program
  mounts="$(jq -r '.services | to_entries[] | .key as $s
    | ((.value.tmpfs // [])[] | "\($s) \(split(":")[0])"),
      ((.value.volumes // [])[] | select(.type == "tmpfs") | "\($s) \(.target)")' <<< "$out")" || {
    echo "check-production-config: cannot list the tmpfs mounts of the rendered configuration (jq failed)" >&2
    return 1
  }
  ram="$(grep -vE "$ram_allowed" <<< "$mounts")" || true
  if [[ -n "$ram" ]]; then
    echo "check-production-config: keeps scratch in RAM (tmpfs), use a tmp-* volume instead: $(tr '\n' ';' <<< "$ram")" >&2
    return 1
  fi
  return 0
}

# rendered_local ROOT ENV_FILE: the local variant has no switch either and keeps nothing in RAM.
rendered_local() {
  rendered "$1" "$2" compose.local.yaml
}

if ((self_test)); then
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  for file in "${files[@]}"; do
    mkdir -p "$tmp/$(dirname "$file")"
    cp "$repo/$file" "$tmp/$file"
  done
  scan "$tmp" > /dev/null || { echo "self-test: clean copy rejected" >&2; exit 1; }
  echo 'ENV SR_RUNNER_DEV_NO_ISOLATION=1' >> "$tmp/deploy/containers/runner.Dockerfile"
  if scan "$tmp" > /dev/null; then echo "self-test: dev switch in the runner Dockerfile not caught" >&2; exit 1; fi
  cp "$repo/deploy/containers/runner.Dockerfile" "$tmp/deploy/containers/runner.Dockerfile"
  # Env values: a complete production env passes, each defect alone fails, no value is ever printed.
  sed -e 's|^SR_PUBLIC_URL=.*|SR_PUBLIC_URL=https://review.test.invalid|' \
    -e 's|^SR_SECRET_KEY=.*|SR_SECRET_KEY=CANARY-secret-key-0123456789abcdefghijklmnopqrstu|' \
    -e 's|^SR_RUNNER_SECRET=.*|SR_RUNNER_SECRET=CANARY-runner-secret-0123456789abcdef0123456789|' \
    -e 's|^SR_GITHUB_APP_ID=.*|SR_GITHUB_APP_ID=42|' -e 's|^SR_GITHUB_CLIENT_ID=.*|SR_GITHUB_CLIENT_ID=Iv23liTEST|' \
    -e 's|^SR_GITHUB_CLIENT_SECRET=.*|SR_GITHUB_CLIENT_SECRET=CANARY-client-secret|' \
    -e 's|^SR_MEMCAN_URL=.*|SR_MEMCAN_URL=http://memcan.test.invalid:8190/mcp|' \
    -e 's|^SR_MEMCAN_TOKEN=.*|SR_MEMCAN_TOKEN=CANARY-memcan-token|' \
    -e 's|^SR_ALLOWED_LOGINS=.*|SR_ALLOWED_LOGINS=octocat,hubot|' \
    -e 's|^SR_DOCKER_GID=.*|SR_DOCKER_GID=997|' \
    -e 's|^SR_WORKER_IMAGE=.*|SR_WORKER_IMAGE=worker@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef|' \
    -e 's|^SR_FETCH_IMAGE=.*|SR_FETCH_IMAGE=fetch@sha256:fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210|' \
    -e "s|^SR_GATEWAY_SECRET_KEY_PATH=.*|SR_GATEWAY_SECRET_KEY_PATH=$tmp/gateway-secret-key|" \
    -e "s|^SR_GATEWAY_CREDENTIALS_TOKEN_PATH=.*|SR_GATEWAY_CREDENTIALS_TOKEN_PATH=$tmp/gateway-credentials-token|" \
    -e "s|^SR_GATEWAY_KEYS_TOKEN_PATH=.*|SR_GATEWAY_KEYS_TOKEN_PATH=$tmp/gateway-keys-token|" \
    -e "s|^SR_RUNNER_GATEWAY_KEYS_TOKEN_PATH=.*|SR_RUNNER_GATEWAY_KEYS_TOKEN_PATH=$tmp/runner-gateway-keys-token|" "$repo/.env.example" > "$tmp/good.env"
  # Synthetic ownership keeps self-tests unprivileged; file modes and contents use real temp files.
  declare -A fixture_uids=() fixture_gids=()
  stat() {
    if [[ "${2:-}" == '%u %g %a' && "$3" == "$tmp/"* ]]; then
      printf '%s %s %s\n' "${fixture_uids[$3]:-10010}" "${fixture_gids[$3]:-10010}" "$(command stat -c '%a' "$3")"
    else
      command stat "$@"
    fi
  }
  declare -A fixture_values=(
    [gateway-secret-key]="$(printf '%032d' 0 | base64 -w0)"
    [gateway-credentials-token]=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
    [gateway-keys-token]=fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210
    [runner-gateway-keys-token]=fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210
  )
  for name in "${!fixture_values[@]}"; do
    printf '%s\n' "${fixture_values[$name]}" > "$tmp/$name"
    chmod 0400 "$tmp/$name"
  done
  fixture_uids["$tmp/runner-gateway-keys-token"]=10002
  # env_case NAME pass|fail SECRET_FILES SED_ARGS...: check_env of the good env edited by sed.
  env_case() {
    local name="$1" want="$2" as_files="$3" got=pass out secret
    shift 3
    sed "$@" "$tmp/good.env" > "$tmp/case.env"
    out="$(check_env "$tmp/case.env" "$repo/.env.example" "$as_files" "${env_local:-0}" 2>&1)" || got=fail
    [[ "$got" == "$want" ]] || { echo "self-test: env case '$name': expected $want, got $got: $out" >&2; exit 1; }
    if grep -q CANARY <<< "$out"; then echo "self-test: env case '$name' printed a secret value" >&2; exit 1; fi
    for secret in "${fixture_values[@]}"; do
      if grep -qF -- "$secret" <<< "$out"; then
        echo "self-test: env case '$name' printed a runtime key" >&2
        exit 1
      fi
    done
    if [[ -n "${want_text:-}" ]] && ! grep -qF -- "$want_text" <<< "$out"; then
      echo "self-test: env case '$name' did not say '$want_text': $out" >&2
      exit 1
    fi
  }
  runtime_env_case() {
    env_case "$@"
    env_local=1 env_case "local $1" "${@:2}"
  }
  env_case "placeholder secret" fail 0 -e 's|^SR_SECRET_KEY=.*|SR_SECRET_KEY=REPLACE_WITH_SECRET|'
  env_case "example App id" fail 0 -e 's|^SR_GITHUB_APP_ID=.*|SR_GITHUB_APP_ID=123456|'
  env_case "example public URL" fail 0 -e 's|^SR_PUBLIC_URL=.*|SR_PUBLIC_URL=https://review.example.com|'
  env_case "http public URL" fail 0 -e 's|^SR_PUBLIC_URL=.*|SR_PUBLIC_URL=http://review.test.invalid|'
  env_case "empty secret" fail 0 -e 's|^SR_SECRET_KEY=.*|SR_SECRET_KEY=|'
  env_case "no allowed login" fail 0 -e 's|^SR_ALLOWED_LOGINS=.*|SR_ALLOWED_LOGINS=|'
  env_case "allowed logins unset" fail 0 -e '/^SR_ALLOWED_LOGINS=/d'
  env_case "allowed logins placeholder" fail 0 -e 's|^SR_ALLOWED_LOGINS=.*|SR_ALLOWED_LOGINS=REPLACE_WITH_LOGINS|'
  env_case "short runner secret" fail 0 -e 's|^SR_RUNNER_SECRET=.*|SR_RUNNER_SECRET=short|'
  for key in SR_DOCKER_GID SR_WORKER_IMAGE SR_FETCH_IMAGE; do
    runtime_env_case "$key unset" fail 0 -e "/^$key=/d"
    runtime_env_case "$key empty" fail 0 -e "s|^$key=.*|$key=|"
    runtime_env_case "$key placeholder" fail 0 -e "s|^$key=.*|$key=REPLACE_WITH_VALUE|"
  done
  runtime_env_case "invalid socket group" fail 0 -e 's|^SR_DOCKER_GID=.*|SR_DOCKER_GID=root|'
  for key in SR_WORKER_IMAGE SR_FETCH_IMAGE; do
    runtime_env_case "$key unpinned" fail 0 -e "s|^$key=.*|$key=image:latest|"
    runtime_env_case "$key fake digest" fail 0 -e "s|^$key=.*|$key=image@sha256:$(printf '%064d' 0)|"
  done
  for name in gateway-secret-key gateway-credentials-token gateway-keys-token runner-gateway-keys-token; do
    mv "$tmp/$name" "$tmp/saved-key"
    runtime_env_case "$name missing" fail 0 -e ''
    mv "$tmp/saved-key" "$tmp/$name"
    fixture_uids["$tmp/$name"]=10001
    runtime_env_case "$name wrong owner" fail 0 -e ''
    if [[ "$name" == runner-gateway-keys-token ]]; then fixture_uids["$tmp/$name"]=10002
    else fixture_uids["$tmp/$name"]=10010; fi
    chmod 0444 "$tmp/$name"
    runtime_env_case "$name public mode" fail 0 -e ''
    chmod 0600 "$tmp/$name"
    : > "$tmp/$name"
    chmod 0400 "$tmp/$name"
    runtime_env_case "$name empty" fail 0 -e ''
    chmod 0600 "$tmp/$name"
    echo 'REPLACE_CANARY' > "$tmp/$name"
    chmod 0400 "$tmp/$name"
    runtime_env_case "$name placeholder" fail 0 -e ''
    chmod 0600 "$tmp/$name"
    printf '%s\n' "${fixture_values[$name]}" > "$tmp/$name"
    chmod 0400 "$tmp/$name"
  done
  chmod 0600 "$tmp/runner-gateway-keys-token"
  echo 'CANARY-different' > "$tmp/runner-gateway-keys-token"
  chmod 0400 "$tmp/runner-gateway-keys-token"
  runtime_env_case "keys-scope mismatch" fail 0 -e ''
  chmod 0600 "$tmp/runner-gateway-keys-token"
  printf '%s\n' "${fixture_values[runner-gateway-keys-token]}" > "$tmp/runner-gateway-keys-token"
  chmod 0400 "$tmp/runner-gateway-keys-token"
  for name in gateway-credentials-token gateway-keys-token; do
    chmod 0600 "$tmp/$name"
    echo 'CANARY-short' > "$tmp/$name"
    chmod 0400 "$tmp/$name"
    if [[ "$name" == gateway-keys-token ]]; then
      chmod 0600 "$tmp/runner-gateway-keys-token"
      cp "$tmp/$name" "$tmp/runner-gateway-keys-token"
      chmod 0400 "$tmp/runner-gateway-keys-token"
    fi
    want_text="fewer than 32 bytes" runtime_env_case "$name short" fail 0 -e ''
    chmod 0600 "$tmp/$name" "$tmp/runner-gateway-keys-token"
    printf '%s\n' "${fixture_values[$name]}" > "$tmp/$name"
    printf '%s\n' "${fixture_values[runner-gateway-keys-token]}" > "$tmp/runner-gateway-keys-token"
    chmod 0400 "$tmp/$name" "$tmp/runner-gateway-keys-token"
  done
  chmod 0600 "$tmp/gateway-credentials-token"
  printf ' \t%s \n' "${fixture_values[gateway-keys-token]}" > "$tmp/gateway-credentials-token"
  chmod 0400 "$tmp/gateway-credentials-token"
  runtime_env_case "identical trimmed admin scopes" fail 0 -e ''
  chmod 0600 "$tmp/gateway-credentials-token"
  cp "$tmp/gateway-keys-token" "$tmp/gateway-credentials-token"
  chmod 0400 "$tmp/gateway-credentials-token"
  runtime_env_case "identical admin scopes" fail 0 -e ''
  chmod 0600 "$tmp/gateway-credentials-token"
  printf '%s\n' "${fixture_values[gateway-credentials-token]}" > "$tmp/gateway-credentials-token"
  chmod 0400 "$tmp/gateway-credentials-token"
  for key_value in 'CANARY-not-base64' "$(printf '%031d' 0 | base64 -w0)" "$(printf '%033d' 0 | base64 -w0)"; do
    chmod 0600 "$tmp/gateway-secret-key"
    printf '%s\n' "$key_value" > "$tmp/gateway-secret-key"
    chmod 0400 "$tmp/gateway-secret-key"
    want_text=SR_GATEWAY_SECRET_KEY_PATH runtime_env_case "invalid encryption key" fail 0 -e ''
  done
  chmod 0600 "$tmp/gateway-secret-key"
  printf '%s\n' "${fixture_values[gateway-secret-key]}" > "$tmp/gateway-secret-key"
  chmod 0400 "$tmp/gateway-secret-key"
  env_local=1 env_case "complete local env" pass 0 \
    -e 's|^SR_PUBLIC_URL=.*|SR_PUBLIC_URL=http://localhost:3000|' \
    -e "\$a COMPOSE_FILE=deploy/compose.yaml:deploy/compose.local.yaml" \
    -e "\$a SR_SERVER_UID=1000" -e "\$a SR_SERVER_GID=1000"
  if "$repo/deploy/check-production-config.sh" --env-file "$tmp/missing.env" --local --no-render > "$tmp/local-check.log" 2>&1; then
    echo "self-test: --local skipped env-file validation" >&2
    exit 1
  fi
  env_case "complete production env" pass 0 -e ''
  env_case "quoted values" pass 0 -e 's|^SR_PUBLIC_URL=.*|SR_PUBLIC_URL="https://review.test.invalid"|'
  want_text="warning: SR_TRUSTED_PROXIES" env_case "no trusted proxy (warning only)" pass 0 \
    -e 's|^SR_TRUSTED_PROXIES=.*|SR_TRUSTED_PROXIES=|'
  want_text="warning: SR_TRUSTED_PROXIES" env_case "trusted proxy unset (warning only)" pass 0 \
    -e '/^SR_TRUSTED_PROXIES=/d'
  env_case "quoted value with a comment" pass 0 \
    -e 's|^SR_PUBLIC_URL=.*|SR_PUBLIC_URL="https://review.test.invalid" # public name|'
  env_case "single-quoted value is literal" pass 0 -e "s|^SR_MEMCAN_TOKEN=.*|SR_MEMCAN_TOKEN='CANARY-\\\${literal}'|"
  # shellcheck disable=SC2016 # a literal ${...} for the env file
  env_case "interpolated secret" fail 0 -e 's|^SR_SECRET_KEY=.*|SR_SECRET_KEY=${SR_UNSET_IN_SELF_TEST}|'
  # shellcheck disable=SC2016 # a literal ${...} for the env file
  env_case "interpolated public URL" fail 0 -e 's|^SR_PUBLIC_URL=.*|SR_PUBLIC_URL="https://${SR_UNSET_IN_SELF_TEST}"|'
  env_case "host-uid override variable" fail 0 -e "\$a SR_SERVER_UID=1000"
  env_case "dev switch" fail 0 -e "\$a SR_RUNNER_DEV_NO_ISOLATION=1"
  env_case "local override in COMPOSE_FILE" fail 0 -e "\$a COMPOSE_FILE=deploy/compose.yaml:deploy/compose.local.yaml"
  cp "$repo/.env.example" "$tmp/example.env"
  for key in "${!gateway_files[@]}"; do
    printf '%s=%s/%s\n' "$key" "$tmp" "${gateway_files[$key]}" >> "$tmp/example.env"
  done
  if check_env "$tmp/example.env" "$repo/.env.example" 0 > /dev/null 2>&1; then
    echo "self-test: .env.example accepted as a production env" >&2
    exit 1
  fi
  # Secrets as files.
  mkdir -m 0700 "$tmp/secrets"
  for name in sr-secret-key sr-github-client-secret sr-memcan-token; do echo "CANARY-$name" > "$tmp/secrets/$name"; done
  echo "CANARY-runner-secret-0123456789abcdef0123456789" > "$tmp/secrets/sr-runner-secret"
  chmod 0400 "$tmp/secrets"/*
  chmod 0440 "$tmp/secrets/sr-runner-secret"
  fixture_uids["$tmp/secrets/sr-runner-secret"]=10002
  files_env=(-e 's/^\(SR_SECRET_KEY\|SR_RUNNER_SECRET\|SR_GITHUB_CLIENT_SECRET\|SR_MEMCAN_TOKEN\)=.*/\1=/'
    -e "\$a SR_SECRETS_DIR=$tmp/secrets")
  env_case "secrets as files" pass 1 "${files_env[@]}"
  for name in "${secret_file_names[@]}"; do
    fixture_uids["$tmp/secrets/$name"]=10001
    env_case "$name wrong consumer" fail 1 "${files_env[@]}"
    if [[ "$name" == sr-runner-secret ]]; then fixture_uids["$tmp/secrets/$name"]=10002
    else fixture_uids["$tmp/secrets/$name"]=10010; fi
  done
  fixture_gids["$tmp/secrets/sr-runner-secret"]=10002
  env_case "runner secret wrong server group" fail 1 "${files_env[@]}"
  fixture_gids["$tmp/secrets/sr-runner-secret"]=10010
  env_case "secret both in env and file" fail 1 -e "\$a SR_SECRETS_DIR=$tmp/secrets"
  chmod 0444 "$tmp/secrets/sr-memcan-token"
  env_case "secret file readable by everyone" fail 1 "${files_env[@]}"
  chmod 0400 "$tmp/secrets/sr-memcan-token"
  chmod 0600 "$tmp/secrets/sr-runner-secret"
  echo "CANARY-short" > "$tmp/secrets/sr-runner-secret"
  chmod 0440 "$tmp/secrets/sr-runner-secret"
  env_case "short runner secret file" fail 1 "${files_env[@]}"
  rm "$tmp/secrets/sr-runner-secret"
  env_case "missing secret file" fail 1 "${files_env[@]}"
  if can_render; then
    sed "${files_env[@]}" "$tmp/good.env" > "$tmp/files.env"
    out="$(docker compose --env-file "$tmp/files.env" -f "$repo/deploy/compose.yaml" \
      -f "$repo/deploy/compose.secrets.yaml" config 2>&1)" || { echo "self-test: compose.secrets.yaml does not render: $out" >&2; exit 1; }
    if grep -qE '^ +SR_(SECRET_KEY|RUNNER_SECRET|GITHUB_CLIENT_SECRET|MEMCAN_TOKEN):' <<< "$out" ||
      [[ "$(grep -c '_FILE: /run/secrets/sr-' <<< "$out")" != 5 ]]; then
      echo "self-test: compose.secrets.yaml does not replace the four secrets by files" >&2
      exit 1
    fi
    rendered "$repo" "$tmp/files.env" compose.secrets.yaml > /dev/null || { echo "self-test: secrets variant rejected" >&2; exit 1; }
    sed 's|^SR_PUBLIC_URL=.*|SR_PUBLIC_URL=http://localhost:3000|' "$tmp/good.env" > "$tmp/local.env"
    rendered_local "$repo" "$tmp/local.env" > /dev/null 2>&1 || { echo "self-test: localhost local variant rejected" >&2; exit 1; }
    rendered "$repo" "$tmp/good.env" > /dev/null 2>&1 || { echo "self-test: production stack rejected" >&2; exit 1; }
    # Nothing in RAM: a tmpfs scratch area (short or long syntax) is refused; the proxy's config dir is the exception.
    for ram in 'runner: {tmpfs: ["/scratch:size=1g"]}' 'server: {volumes: [{type: tmpfs, target: /scratch}]}'; do
      printf 'services:\n  %s\n' "$ram" > "$tmp/deploy/ram.yaml"
      out="$(rendered "$repo" "$tmp/good.env" "$tmp/deploy/ram.yaml" 2>&1)" || true
      if ! grep -q 'keeps scratch in RAM' <<< "$out"; then
        echo "self-test: a tmpfs scratch area ($ram) not caught" >&2
        exit 1
      fi
    done
    # Without jq the RAM check cannot run: refused up front, and a failing jq fails the rendering (fail closed).
    mkdir "$tmp/nojq"
    ln -s "$(command -v docker)" "$tmp/nojq/docker"
    if out="$(PATH="$tmp/nojq" can_render 2>&1)" || ! grep -q 'jq is not available' <<< "$out"; then
      echo "self-test: a missing jq not refused: $out" >&2
      exit 1
    fi
    if out="$(jq() { return 2; }; rendered "$repo" "$tmp/good.env" 2>&1)" || ! grep -q 'jq failed' <<< "$out"; then
      echo "self-test: a failing jq passed the RAM check: $out" >&2
      exit 1
    fi
  fi
  echo "check-production-config: self-test passed"
  exit 0
fi

if ((local && secret_files)); then
  echo "check-production-config: --local and --secret-files exclude each other" >&2
  exit 2
fi
status=0
scan "$repo" || status=1
if [[ -n "$env_file" ]]; then check_env "$env_file" "$repo/.env.example" "$secret_files" "$local" || status=1; fi
if can_render; then
  if [[ -n "$env_file" ]]; then
    if ((secret_files)); then
      rendered "$repo" "$env_file" compose.secrets.yaml || status=1
    else
      rendered "$repo" "$env_file" || status=1
    fi
    if ((local)); then rendered_local "$repo" "$env_file" || status=1; fi
  else
    rendered "$repo" "$repo/.env.example" || status=1
    local_env="$(mktemp)"
    sed 's|^SR_PUBLIC_URL=.*|SR_PUBLIC_URL=http://localhost:3000|' "$repo/.env.example" > "$local_env"
    rendered_local "$repo" "$local_env" || status=1
    rm -f "$local_env"
  fi
fi
if ((status)); then
  echo "check-production-config: failed (lines above)" >&2
else
  echo "check-production-config: no development or test switch in the production files${env_file:+; $env_file passed}"
fi
exit "$status"
