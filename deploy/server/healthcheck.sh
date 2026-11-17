#!/bin/bash
# Server healthcheck: GET /healthz on the public listener answers 204 (no curl in the image; bash /dev/tcp).
set -euo pipefail
addr="${SR_LISTEN_ADDR:-0.0.0.0:3000}"
host="${addr%:*}"
port="${addr##*:}"
[[ "$host" == 0.0.0.0 ]] && host=127.0.0.1
exec 3<> "/dev/tcp/${host}/${port}"
printf 'GET /healthz HTTP/1.0\r\nHost: localhost\r\n\r\n' >&3
IFS= read -r -t 4 status <&3
[[ "$status" == "HTTP/1."?" 204"* ]]
