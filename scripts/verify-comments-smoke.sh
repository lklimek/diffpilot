#!/usr/bin/env bash
# verify_comments smoke (spike S13-3): one real `codex exec --output-schema` of a rendered verify_comments prompt in
# the S1a spike container (unprivileged, read-only root fs), laid out as the runner lays out a job: cwd = job dir,
# the read-only checkout at ./repo, no skills (the job kind needs none), the runner's per-job config (no GitHub or
# MemCan MCP: no installation token is minted here).
# The credential is handled as in schema-smoke.sh (per-job CODEX_HOME, compare-and-swap write-back); CODEX_HOME lives
# outside the agent's working directory, and the container reports whether a credential value reached the job output.
#
# Usage: scripts/verify-comments-smoke.sh PROMPT_FILE REPO_DIR
#   PROMPT_FILE  a prompt rendered as the server renders it (crates/server/src/review_loop/assess/prompt.rs)
#   REPO_DIR     a git checkout of the head named in the prompt, with the base commit present (both checked)
# Env:   SMOKE_IMAGE (diffpilot-spike-codex:0.167.0),
#        SMOKE_VOLUME (sr-spike-codex-home), SMOKE_MODEL (gpt-6.1-sol), SMOKE_EFFORT (high),
#        SMOKE_ARCHIVE (/data/artifacts/diffpilot/<today>/verify-comments-smoke)
# Prints one summary line: rc, seconds, schema verdict and the measures (tokens, items, output bytes).
# Needs: docker, git, python3 with jsonschema (apt python3-jsonschema).
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Host-side reads of agent-writable job directories go only through this helper (no symlink following).
report="${repo_root}/scripts/schema-smoke-report.py"
image="${SMOKE_IMAGE:-diffpilot-spike-codex:0.167.0}"
volume="${SMOKE_VOLUME:-sr-spike-codex-home}"
model="${SMOKE_MODEL:-gpt-6.1-sol}"
effort="${SMOKE_EFFORT:-high}"
kind=verify_comments
stamp="$(date -u +%Y%m%dT%H%M%SZ)"
work="/data/tmp/verify-comments-smoke/${stamp}"
archive="${SMOKE_ARCHIVE:-/data/artifacts/diffpilot/$(date -u +%F)/verify-comments-smoke}/${stamp}"

die() { echo "verify-comments-smoke: $*" >&2; exit 1; }
[[ $# -eq 2 ]] || die "usage: $0 PROMPT_FILE REPO_DIR"
prompt="$1"
checkout="$(cd "$2" && pwd)"
[[ -f "${prompt}" ]] || die "prompt ${prompt} not found"
git -C "${checkout}" rev-parse --verify -q HEAD >/dev/null || die "${checkout} is not a git checkout"
head="$(grep -m1 -oE 'head commit [0-9a-f]{40}' "${prompt}" | cut -d' ' -f3 || true)"
base="$(grep -m1 -oE 'base commit [0-9a-f]{40}' "${prompt}" | cut -d' ' -f3 || true)"
[[ -n "${head}" && -n "${base}" ]] || die "the prompt names no base and head commit"
[[ "$(git -C "${checkout}" rev-parse HEAD)" == "${head}" ]] || die "${checkout} is not at the prompt's head ${head}"
git -C "${checkout}" cat-file -e "${base}^{commit}" 2>/dev/null || die "the prompt's base ${base} is missing in ${checkout}"
for tool in docker git python3; do command -v "${tool}" >/dev/null || die "${tool} not found"; done
python3 -I -c 'import jsonschema' 2>/dev/null || die "python3-jsonschema missing (apt install python3-jsonschema)"
docker image inspect "${image}" >/dev/null 2>&1 || die "image ${image} not found (S1a harness build.sh)"
docker volume inspect "${volume}" >/dev/null 2>&1 || die "volume ${volume} not found (S1a codex-login.sh)"
case "${effort}" in low | medium | high) ;; *) die "SMOKE_EFFORT must be low, medium or high" ;; esac

out="${work}/jobs/${kind}"
mkdir -p "${work}/in" "${out}"
chmod 777 "${out}"
echo "verify-comments-smoke: work dir ${work}"
cp "${prompt}" "${work}/in/prompt.txt"
cp "${repo_root}/schemas/agent/${kind}.schema.json" "${work}/in/${kind}.schema.json"

# The runner's per-job config (crates/runner/src/provider/codex/config.rs) without MCP servers.
cat >"${work}/in/config.toml" <<TOML
model = "${model}"
model_reasoning_effort = "${effort}"
model_auto_compact_token_limit = 400000
sandbox_mode = "danger-full-access"
web_search = "disabled"
approval_policy = "never"
project_doc_max_bytes = 0

[analytics]
enabled = false

[skills.bundled]
enabled = false

[features]
plugins = false
apps = false
remote_plugin = false
plugin_discovery_prefetch = false
image_generation = false
browser_use = false
computer_use = false
hooks = false
tool_suggest = false
goals = false
daemon_auto_start = false
multi_agent = false
TOML

cat >"${work}/in/job.sh" <<'JOB'
#!/bin/bash
# In the container: the runner's job layout and agent environment, codex exec, compare-and-swap write-back, and a
# check that no credential value reached /out (the values never leave the container).
set -u
home=/codex/home
mkdir -m 700 "${home}" /job/home /job/tmp
install -m 600 /in/config.toml "${home}/config.toml"
install -m 600 "/in/${KIND}.schema.json" "${home}/output-schema.json"
install -m 600 /master/auth.json "${home}/auth.json"
master_before="$(sha256sum /master/auth.json | cut -d' ' -f1)"
# The credential values of the given files, one per line; checked against /out before and after any rotation.
credential_values() {
  grep -ohE '"(id_token|access_token|refresh_token|OPENAI_API_KEY)"[[:space:]]*:[[:space:]]*"[^"]{16,}"' "$@" \
    2>/dev/null | sed -E 's/^.*"([^"]+)"$/\1/'
}
values=/codex/values
credential_values /master/auth.json >"${values}"
cd /job
env -i HOME=/job/home PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin CODEX_HOME="${home}" \
  TMPDIR=/job/tmp LANG=C.UTF-8 GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=safe.directory GIT_CONFIG_VALUE_0=/job/repo \
  codex exec --json --skip-git-repo-check --ignore-rules --output-schema "${home}/output-schema.json" \
  -o /out/output.json - </in/prompt.txt >/out/stdout.jsonl 2>/out/stderr.txt &
pid=$!
trap 'kill -TERM "${pid}"' TERM INT
wait "${pid}"; rc=$?
if ! cmp -s "${home}/auth.json" /master/auth.json; then
  if [ "$(sha256sum /master/auth.json | cut -d' ' -f1)" = "${master_before}" ]; then
    install -m 600 "${home}/auth.json" /master/auth.json.new && mv -f /master/auth.json.new /master/auth.json
    state="credential rotated; written back to master"
  else
    state="CONFLICT: master changed during the job; not written back"
  fi
else
  state="credential unchanged"
fi
credential_values /master/auth.json "${home}/auth.json" >"${values}.after"
if [ ! -s "${values}" ] || [ ! -s "${values}.after" ]; then
  leak="output not checked: no credential value found"
elif grep -rqF -f "${values}" -f "${values}.after" /out; then
  leak="CREDENTIAL VALUE IN THE JOB OUTPUT"
else
  leak="no credential value in the job output"
fi
rm -f "${values}" "${values}.after" "${home}/auth.json" /out/credential.txt /out/rc
echo "${state}; ${leak}" >/out/credential.txt
echo "${rc}" >/out/rc
exit "${rc}"
JOB

cp "${work}/in/prompt.txt" "${out}/prompt.txt"
args=(--rm --init --name "sr-verify-comments-smoke-${stamp,,}" --user 10001:10001 --cap-drop ALL
  --security-opt no-new-privileges --read-only
  --tmpfs "/tmp:rw,exec,nosuid,nodev,size=512m"
  --tmpfs "/job:rw,exec,nosuid,nodev,size=1g,uid=10001,gid=10001,mode=0700"
  --tmpfs "/codex:rw,exec,nosuid,nodev,size=256m,uid=10001,gid=10001,mode=0700"
  --mount "type=volume,source=${volume},target=/master"
  --mount "type=bind,source=${work}/in,target=/in,readonly"
  --mount "type=bind,source=${checkout},target=/job/repo,readonly"
  --mount "type=bind,source=${out},target=/out"
  -e "KIND=${kind}")
started=$(date +%s)
rc=0
docker run "${args[@]}" "${image}" bash /in/job.sh >"${work}/docker.txt" 2>&1 </dev/null || rc=$?
seconds=$(($(date +%s) - started))
verdict="$(python3 -I "${report}" verdict "${work}/in/${kind}.schema.json" "${out}")"
credential="$(python3 -I "${report}" credential "${out}")"
printf 'rc=%s secs=%s effort=%s output=%s measures=%s (%s)\n' "${rc}" "${seconds}" "${effort}" "${verdict}" \
  "$(python3 -I "${report}" measures "${out}")" "${credential}" | tee -a "${work}/summary.txt"
[[ "${credential}" == *"; no credential value in the job output" ]] ||
  die "the credential check did not pass (${credential}); not archived, inspect ${work} by hand"

# --- sanitised archive: expected regular files only, no symlinks; JWTs, account ids, e-mails redacted ----
mkdir -p "${archive}"
python3 -I "${report}" archive "${work}" "${archive}"
# Second guard: a credential key in JSON form (also JSON-escaped) in what the agent wrote.
if grep -qsE '"(refresh_token|access_token|id_token)\\?"[[:space:]]*:' "${archive}/jobs/${kind}/stdout.jsonl" \
  "${archive}/jobs/${kind}/stderr.txt" "${archive}/jobs/${kind}/output.json"; then
  rm -rf "${archive}"
  die "a credential key reached the job output; the archive was removed, inspect ${work} by hand"
fi
echo "verify-comments-smoke: archived to ${archive}"
[[ "${rc}" == 0 && "${verdict}" == valid ]] || die "the job failed or its output is not valid"
