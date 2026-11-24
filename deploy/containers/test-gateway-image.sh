#!/usr/bin/env bash
# Empty fake-only gateway startup: no network and no linked credential, auth or upstream request.
set -euo pipefail
image=${1:?server image required}
directory=${2:?new synthetic runtime secrets directory required}
status=0
docker run --rm --network none --read-only --cap-drop ALL --security-opt no-new-privileges:true \
  --tmpfs /tmp:uid=10010,gid=10010,mode=0700,size=16m \
  --tmpfs /state:uid=10010,gid=10010,mode=0700,size=16m \
  --mount "type=bind,source=$directory/gateway-secret-key,target=/run/secrets/key,readonly" \
  --mount "type=bind,source=$directory/gateway-credentials-token,target=/run/secrets/credentials,readonly" \
  --mount "type=bind,source=$directory/gateway-keys-token,target=/run/secrets/keys,readonly" \
  -e SR_GATEWAY_MODEL_ADDR=127.0.0.2:8090 -e SR_GATEWAY_ADMIN_ADDR=127.0.0.3:8091 \
  -e SR_GATEWAY_STORE=/state -e SR_GATEWAY_SECRET_KEY_FILE=/run/secrets/key \
  -e SR_GATEWAY_CREDENTIALS_TOKEN_FILE=/run/secrets/credentials -e SR_GATEWAY_KEYS_TOKEN_FILE=/run/secrets/keys \
  --entrypoint timeout "$image" --kill-after=10s 3s /usr/local/bin/diffpilot-gateway || status=$?
[[ "$status" == 124 ]] || { echo "gateway startup failed (exit $status)" >&2; exit 1; }
echo 'gateway image: empty encrypted store and local shared credentials-file permissions PASS'
