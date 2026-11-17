#!/bin/sh
# Writes the MemCan exception of the SSRF guard from SR_MEMCAN_URL (the full MCP endpoint, e.g.
# http://memcan.lan:8190/mcp), then runs Squid in the foreground. SR_MEMCAN_URL unset or empty: no exception, every
# private destination stays denied.
# The exception needs all three: the configured host name (or IP literal) as written in the request, its port,
# and the one address the name resolved to here at start. That address is pinned in Squid's hosts file, so Squid
# connects to nothing else for that name; any other name, even one whose DNS answer holds MemCan's address, falls
# to the private-range deny.
set -eu

conf_dir=/run/squid
memcan_conf="${conf_dir}/memcan.conf"
hosts="${conf_dir}/hosts"
: > "${memcan_conf}"
cat /etc/hosts > "${hosts}"

if [ -n "${SR_MEMCAN_URL:-}" ]; then
  rest="${SR_MEMCAN_URL#*://}"
  scheme="${SR_MEMCAN_URL%%://*}"
  hostport="${rest%%/*}"
  case "${hostport}" in
    *:*) host="${hostport%:*}"; port="${hostport##*:}" ;;
    *) host="${hostport}"; if [ "${scheme}" = https ]; then port=443; else port=80; fi ;;
  esac
  if ! printf '%s' "${scheme}" | grep -Eq '^https?$' || ! printf '%s' "${host}" | grep -Eq '^[A-Za-z0-9.-]+$' \
    || ! printf '%s' "${port}" | grep -Eq '^[0-9]{1,5}$'; then
    echo "egress-proxy: SR_MEMCAN_URL must be http(s)://host[:port][/path]; refusing to start" >&2
    exit 64
  fi
  if printf '%s' "${host}" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$'; then
    addr="${host}"
  else
    addr="$(getent ahostsv4 "${host}" | awk 'NR == 1 { print $1 }')"
    if [ -z "${addr}" ]; then
      echo "egress-proxy: cannot resolve the MemCan host ${host}; refusing to start" >&2
      exit 65
    fi
    printf '%s %s\n' "${addr}" "${host}" >> "${hosts}"
  fi
  printf '%s\n' \
    "acl memcan_name dstdomain -n ${host}" \
    "acl memcan_port port ${port}" \
    "acl memcan_addr dst ${addr}" \
    "http_access allow memcan_name memcan_port memcan_addr" > "${memcan_conf}"
  echo "egress-proxy: MemCan exception for ${host}:${port} at ${addr}" >&2
fi

echo "egress-proxy: starting Squid (diagnostics: critical and fatal only)" >&2
exec squid -N -f "${SQUID_CONF:-/etc/squid/squid.conf}" "$@"
