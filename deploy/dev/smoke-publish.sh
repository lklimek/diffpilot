#!/usr/bin/env bash
# Publishes the stage of a smoke run (real-codex-smoke.sh) to OUT_DIR only after a complete secret scan:
#   1. credential markers in the runner state volume are counted by fixed category inside a container; no name
#      from that agent-writable volume ever leaves it;
#   2. every value of HOST_VALUES (one per line) is searched in every stage file on the host;
#   3. the credential's token values are compared inside a container (the stage is streamed in with tar); they
#      never leave it, only their count and the names of matching stage files do;
#   4. a file holding a value is withheld, and then every screenshot (*.png) is withheld too: images show the
#      same data and cannot be searched.
# A scanner error, an unreadable file, an empty value list or a credential without its tokens publishes nothing.
#
# Usage: smoke-publish.sh STAGE OUT_DIR VOLUME HOST_VALUES   (OUT_DIR absent or empty; SR_SMOKE_IMAGE as for
# real-codex-smoke.sh). Prints fixed text, counts and stage file names only (also kept as OUT_DIR/publish.txt).
# Exit 0: published, possibly with withheld files; exit 1: nothing published.
set -euo pipefail

stage="${1:?usage: smoke-publish.sh STAGE OUT_DIR VOLUME HOST_VALUES}"
out="${2:?out dir}" volume="${3:?volume}" values="${4:?host value list}"
image="${SR_SMOKE_IMAGE:-smart-review-spike-codex:0.167.0}"
container_args=(--rm --network none --user 10001:10001 --cap-drop ALL --security-opt no-new-privileges
  --mount "type=volume,source=$volume,target=/s,readonly")

abort() {
  echo "smoke-publish: nothing published: $*" >&2
  exit 1
}
[[ ! -e "$out" || -z "$(ls -A "$out")" ]] || abort "OUT_DIR is not empty"

# The stage holds only files this tooling wrote, with plain names.
names=()
for file in "$stage"/*; do
  name="${file##*/}"
  [[ -f "$file" && ! -L "$file" && "$name" =~ ^[A-Za-z0-9._-]+$ ]] || abort "unexpected stage entry"
  names+=("$name")
done
((${#names[@]})) || abort "the stage is empty"

# 1. Marker counts; find's stderr would carry names, so only its status is used.
markers="$(docker run "${container_args[@]}" "$image" bash -c '
  set -euo pipefail
  count() { find /s -maxdepth 4 -name "$1" -print0 2> /dev/null > /tmp/found; tr -cd "\0" < /tmp/found | wc -c; }
  blocked="$(count auth.json.blocked)" rotated="$(count "auth.json.rotated-*")" kept="$(count "*-kept")"
  echo "blocked=$blocked rotated=$rotated kept=$kept"' \
  2> /dev/null)" || abort "cannot count the credential markers"
[[ "$markers" =~ ^blocked=[0-9]+\ rotated=[0-9]+\ kept=[0-9]+$ ]] || abort "unexpected marker count output"

# 2. Host-known values.
host_values="$(awk 'length >= 8' "$values" | sort -u)" || abort "cannot read the host value list"
[[ -n "$host_values" ]] || abort "the host value list is empty"
leaks=()
for name in "${names[@]}"; do
  if grep -qF -f <(printf '%s\n' "$host_values") -- "$stage/$name"; then
    leaks+=("$name")
  elif (($? != 1)); then
    abort "cannot read the stage file $name"
  fi
done

# 3. Credential values: the master must yield its access and refresh token; status lines only come back.
scan="$(tar -C "$stage" -cf - -- "${names[@]}" | docker run -i "${container_args[@]}" "$image" bash -c '
  set -euo pipefail
  mkdir /tmp/scan
  tar -xf - -C /tmp/scan
  master=/s/credentials/default/auth.json
  jq -e "(.tokens.access_token | strings | length > 0) and (.tokens.refresh_token | strings | length > 0)" \
    "$master" > /dev/null 2>&1 || { echo "credential=unreadable"; exit 0; }
  find /s \( -name auth.json -o -name "auth.json.rotated-*" \) -type f -print0 2> /dev/null > /tmp/files
  : > /tmp/values
  while IFS= read -r -d "" file; do
    jq -r "(.tokens | objects | to_entries[] | select(.key != \"account_id\") | .value | strings),
      (.OPENAI_API_KEY | strings)" "$file" >> /tmp/values 2> /dev/null || { echo "credential=unparsable"; exit 0; }
  done < /tmp/files
  awk "length >= 8" /tmp/values | sort -u > /tmp/tokens
  for file in /tmp/scan/*; do
    status=0
    grep -qF -f /tmp/tokens -- "$file" || status=$?
    case "$status" in 0) echo "leak ${file##*/}" ;; 1) ;; *) echo "scan=error"; exit 0 ;; esac
  done
  echo "values=$(wc -l < /tmp/tokens)"
  echo "scan=complete"' 2> /dev/null)" || abort "the credential scan did not run"
[[ "$(tail -n 1 <<< "$scan")" == scan=complete ]] || abort "the credential scan is incomplete"
token_count="$(sed -n 's/^values=\([0-9]\+\)$/\1/p' <<< "$scan")"
((token_count >= 2)) || abort "the credential scan read fewer than 2 token values"
while read -r kind name; do
  [[ "$kind" == leak ]] || continue
  [[ " ${names[*]} " == *" $name "* ]] || abort "the credential scan named an unknown file"
  [[ " ${leaks[*]} " == *" $name "* ]] || leaks+=("$name")
done <<< "$scan"

# 4. Publish.
report=("credential markers: $markers" "credential token values compared: $token_count")
mkdir -p "$out"
count=0
for name in "${names[@]}"; do
  if [[ " ${leaks[*]} " == *" $name "* ]]; then
    report+=("withheld: $name (contains a secret value)")
  elif ((${#leaks[@]})) && [[ "$name" == *.png ]]; then
    report+=("withheld: $name (a text file contained a secret value)")
  else
    cp -- "$stage/$name" "$out/"
    count=$((count + 1))
  fi
done
echo "$markers" > "$out/credential-markers.txt"
report+=("published: $count stage files to $out")
printf '%s\n' "${report[@]}" | tee "$out/publish.txt"
