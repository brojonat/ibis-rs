#!/bin/bash
# Local Postgres for conformance tests: unix socket only, data in .pg/.
# C locale so string ordering is bytewise (matches DuckDB and the goldens).
set -euo pipefail
dir="$(cd "$(dirname "$0")/.." && pwd)/.pg"
port="${IBIS_PG_PORT:-54330}"
case "${1:-}" in
  start)
    if [ ! -d "$dir/data" ]; then
      initdb -D "$dir/data" -U postgres -A trust --no-locale -E UTF8 >/dev/null
    fi
    if ! pg_ctl -D "$dir/data" status >/dev/null 2>&1; then
      pg_ctl -D "$dir/data" -l "$dir/server.log" -w \
        -o "-p $port -k $dir -c listen_addresses=''" start >/dev/null
    fi
    psql -h "$dir" -p "$port" -U postgres -d postgres -qtAc \
      "SELECT 1 FROM pg_database WHERE datname = 'ibis_testing'" | grep -q 1 ||
      psql -h "$dir" -p "$port" -U postgres -d postgres -qc "CREATE DATABASE ibis_testing" ;;
  stop) pg_ctl -D "$dir/data" -m fast stop >/dev/null ;;
  *) echo "usage: $0 start|stop" >&2; exit 2 ;;
esac
