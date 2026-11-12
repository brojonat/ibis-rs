#!/bin/bash
# Throwaway local Postgres for the spike: unix socket only, data in .pg/.
set -euo pipefail
dir="$(cd "$(dirname "$0")/.." && pwd)/.pg"
case "${1:-}" in
  start)
    [ -d "$dir/data" ] || initdb -D "$dir/data" -A trust >/dev/null
    pg_ctl -D "$dir/data" status >/dev/null 2>&1 && exit 0
    pg_ctl -D "$dir/data" -l "$dir/server.log" -w \
      -o "-p 54329 -k $dir -c listen_addresses=''" start >/dev/null ;;
  stop) pg_ctl -D "$dir/data" -m fast stop >/dev/null ;;
  *) echo "usage: $0 start|stop" >&2; exit 2 ;;
esac
