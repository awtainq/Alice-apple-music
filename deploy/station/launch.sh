#!/bin/sh
set -eu
BASE=/data/opt/alice-music
SUP=/tmp/alice-music-supervisor.pid
APP=/tmp/alice-music.pid
export PATH=/opt/bin:/opt/sbin:/sbin:/usr/sbin:/bin:/usr/bin
export PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1
export SSL_CERT_FILE=/opt/etc/ssl/certs/ca-certificates.crt
umask 077
case "${1:-status}" in
 start)
  if [ -s "$SUP" ] && kill -0 "$(cat "$SUP")" 2>/dev/null; then exit 0; fi
  mkdir -p /tmp/alice-music-cache
  nohup "$BASE/launch.sh" supervise </dev/null >/dev/null 2>&1 &
  echo "$!" > "$SUP"
  ;;
 supervise)
  child=
  cleanup() { [ -z "$child" ] || kill "$child" 2>/dev/null || true; rm -f "$SUP" "$APP"; }
  trap 'cleanup; exit 0' TERM INT
  trap cleanup EXIT
  while :; do
   /opt/bin/python3 "$BASE/station_runner.py" &
   child=$!;echo "$child" > "$APP"
   wait "$child" || true
   child=
   sleep 3
  done
  ;;
 stop)
  if [ -s "$SUP" ]; then kill "$(cat "$SUP")" 2>/dev/null || true; fi
  if [ -s "$APP" ]; then kill "$(cat "$APP")" 2>/dev/null || true; fi
  ;;
 status)
  if [ -s "$APP" ] && kill -0 "$(cat "$APP")" 2>/dev/null; then echo running; else echo stopped; fi
  ;;
 *) echo 'usage: launch.sh start|stop|status';exit 2 ;;
esac
