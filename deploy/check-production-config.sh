#!/usr/bin/env bash
# Rejects development providers/legacy bypasses in production files and rendered Compose configuration.
# Only bounded service /tmp and Squid configuration tmpfs mounts are allowed; worker templates own job tmpfs.
# With --env-file, checks required settings and private service secret files without printing their contents.
# Usage: deploy/check-production-config.sh [--env-file FILE [--local | --secret-files]] [--no-render]
#        deploy/check-production-config.sh --self-test [--no-render]
#   (no option)       the files, production rendered with .env.example, local rendered with it on localhost
#   --env-file FILE   production rendered with FILE (for example the deployment's .env), and FILE's values
#   --local           also the local variant rendered with FILE
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
own_values=(SR_PUBLIC_URL SR_GITHUB_APP_ID SR_GITHUB_CLIENT_ID SR_MEMCAN_URL "${secrets[@]}")
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

# check_secret_file NAME PATH: a private, non-empty, non-placeholder service secret file; the runner
# secret at least 32 characters. Prints problems by name; never the content.
check_secret_file() {
  local name="$1" path="$2" found=0 mode content
  if [[ ! -f "$path" ]]; then
    echo "check-production-config: $name: the file $path is missing (secrets as files)" >&2
    return 1
  fi
  read -r mode < <(stat -c '%a' "$path")
  if (((8#$mode & 8#007) != 0)); then
    echo "check-production-config: $name: $path has permissions for other users (mode $mode)" >&2
    found=1
  fi
  if [[ ! -r "$path" ]]; then
    echo "check-production-config: $name: cannot read $path to check it; run as root" >&2
    return 1
  fi
  content="$(cat -- "$path")"
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
  return "$found"
}

# check_env FILE EXAMPLE SECRET_FILES(0|1): the production values of FILE; prints problems by variable name only.
check_env() {
  local -A values=() example=()
  local found=0 key value secrets_dir
  say() { echo "check-production-config: $*" >&2; found=1; }
  load_env "$1" values
  for key in "${!interpolated[@]}"; do
    say "$key uses \$ interpolation, so this check cannot see the value compose uses; write it literally (single quotes keep a \$)"
  done
  load_env "$2" example
  for key in "${!values[@]}"; do
    if [[ "${values[$key]}" == *REPLACE* ]]; then say "$key still holds a placeholder (REPLACE...)"; fi
    if [[ "$key" =~ $dev_only || "$key" =~ ^($forbidden)$ ]]; then
      say "$key is a local or development setting; remove it from a production env file"
    fi
  done
  for key in "${own_values[@]}"; do
    value="${values[$key]:-}"
    if [[ -n "$value" && "$value" != *REPLACE* && "$value" == "${example[$key]:-}" ]]; then
      say "$key still holds the .env.example value"
    fi
  done
  if [[ "${values[COMPOSE_FILE]:-}" =~ compose\.local|/dev/ ]]; then
    say "COMPOSE_FILE names the local or a development override"
  fi
  if [[ ! "${values[SR_PUBLIC_URL]:-}" =~ ^https://[^/?#]+ ]]; then
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
  if ((${3})); then
    secrets_dir="${values[SR_SECRETS_DIR]:-/srv/diffpilot/secrets}"
    for key in "${secrets[@]}"; do
      if [[ -n "${values[$key]:-}" ]]; then say "$key must be empty with secrets as files (the file is used)"; fi
      check_secret_file "$key" "$secrets_dir/${secret_file_names[$key]}" || found=1
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
    -e "s|^SR_RUNNER_CLAUDIUS_REF=.*|SR_RUNNER_CLAUDIUS_REF=$(printf '%040d' 7)|" "$repo/.env.example" > "$tmp/good.env"
  # env_case NAME pass|fail SECRET_FILES SED_ARGS...: check_env of the good env edited by sed.
  env_case() {
    local name="$1" want="$2" as_files="$3" got=pass out
    shift 3
    sed "$@" "$tmp/good.env" > "$tmp/case.env"
    out="$(check_env "$tmp/case.env" "$repo/.env.example" "$as_files" 2>&1)" || got=fail
    [[ "$got" == "$want" ]] || { echo "self-test: env case '$name': expected $want, got $got: $out" >&2; exit 1; }
    if grep -q CANARY <<< "$out"; then echo "self-test: env case '$name' printed a secret value" >&2; exit 1; fi
    if [[ -n "${want_text:-}" ]] && ! grep -qF -- "$want_text" <<< "$out"; then
      echo "self-test: env case '$name' did not say '$want_text': $out" >&2
      exit 1
    fi
  }
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
  if check_env "$repo/.env.example" "$repo/.env.example" 0 > /dev/null 2>&1; then
    echo "self-test: .env.example accepted as a production env" >&2
    exit 1
  fi
  # Secrets as files.
  mkdir -m 0700 "$tmp/secrets"
  for name in sr-secret-key sr-github-client-secret sr-memcan-token; do echo "CANARY-$name" > "$tmp/secrets/$name"; done
  echo "CANARY-runner-secret-0123456789abcdef0123456789" > "$tmp/secrets/sr-runner-secret"
  chmod 0400 "$tmp/secrets"/*
  files_env=(-e 's/^\(SR_SECRET_KEY\|SR_RUNNER_SECRET\|SR_GITHUB_CLIENT_SECRET\|SR_MEMCAN_TOKEN\)=.*/\1=/'
    -e "\$a SR_SECRETS_DIR=$tmp/secrets")
  env_case "secrets as files" pass 1 "${files_env[@]}"
  env_case "secret both in env and file" fail 1 -e "\$a SR_SECRETS_DIR=$tmp/secrets"
  chmod 0444 "$tmp/secrets/sr-memcan-token"
  env_case "secret file readable by everyone" fail 1 "${files_env[@]}"
  chmod 0400 "$tmp/secrets/sr-memcan-token"
  chmod 0600 "$tmp/secrets/sr-runner-secret"
  echo "CANARY-short" > "$tmp/secrets/sr-runner-secret"
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
    sed 's|^SR_PUBLIC_URL=.*|SR_PUBLIC_URL=http://localhost:3000|' "$repo/.env.example" > "$tmp/local.env"
    rendered_local "$repo" "$tmp/local.env" > /dev/null 2>&1 || { echo "self-test: localhost local variant rejected" >&2; exit 1; }
    rendered "$repo" "$repo/.env.example" > /dev/null 2>&1 || { echo "self-test: production stack rejected" >&2; exit 1; }
    # Nothing in RAM: a tmpfs scratch area (short or long syntax) is refused; the proxy's config dir is the exception.
    for ram in 'runner: {tmpfs: ["/scratch:size=1g"]}' 'server: {volumes: [{type: tmpfs, target: /scratch}]}'; do
      printf 'services:\n  %s\n' "$ram" > "$tmp/deploy/ram.yaml"
      out="$(rendered "$repo" "$repo/.env.example" "$tmp/deploy/ram.yaml" 2>&1)" || true
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
    if out="$(jq() { return 2; }; rendered "$repo" "$repo/.env.example" 2>&1)" || ! grep -q 'jq failed' <<< "$out"; then
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
if [[ -n "$env_file" ]] && ((!local)); then check_env "$env_file" "$repo/.env.example" "$secret_files" || status=1; fi
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
