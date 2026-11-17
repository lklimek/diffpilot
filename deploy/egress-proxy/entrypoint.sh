#!/bin/sh
# Writes the MemCan exception of the SSRF guard from SR_MEMCAN_URL, then runs Squid in the foreground.
# SR_MEMCAN_URL unset or empty: no exception, every private destination stays denied.
set -eu

conf_dir=/run/squid
memcan_conf="${conf_dir}/memcan.conf"
: > "${memcan_conf}"

if [ -n "${SR_MEMCAN_URL:-}" ]; then
  rest="${SR_MEMCAN_URL#*://}"
  scheme="${SR_MEMCAN_URL%%://*}"
  hostport="${rest%%/*}"
  case "${hostport}" in
    *:*) host="${hostport%:*}"; port="${hostport##*:}" ;;
    *) host="${hostport}"; if [ "${scheme}" = https ]; then port=443; else port=80; fi ;;
  esac
  if ! printf '%s' "${host}" | grep -Eq '^[A-Za-z0-9.-]+$' || ! printf '%s' "${port}" | grep -Eq '^[0-9]{1,5}$'; then
    echo "egress-proxy: SR_MEMCAN_URL must be http(s)://host[:port][/path]; refusing to start" >&2
    exit 64
  fi
  printf 'acl memcan_dst dst %s\nacl memcan_port port %s\nhttp_access allow memcan_dst memcan_port\n' \
    "${host}" "${port}" > "${memcan_conf}"
  echo "egress-proxy: MemCan exception for ${host}:${port}" >&2
fi

exec squid -N -f "${SQUID_CONF:-/etc/squid/squid.conf}" "$@"
