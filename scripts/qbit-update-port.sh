#!/bin/sh
# qbit-update-port.sh — pousse le port forwarde par gluetun vers qBit listening port
#
# Execute par gluetun via VPN_PORT_FORWARDING_UP_COMMAND a chaque renouvellement.
# Argument $1 = port (ex: 49234) ou plusieurs ports separes par virgule (on prend le premier).
#
# Volume mount cote gluetun :
#   /opt/homelab/scripts:/gluetun/scripts:ro
# Donc gluetun peut executer ce script directement.

set -eu

PORT_LIST="${1:-}"
[ -z "$PORT_LIST" ] && { echo "qbit-update-port: no port argument" >&2; exit 1; }

PORT=$(echo "$PORT_LIST" | cut -d',' -f1)
QBIT_URL="http://127.0.0.1:8080"

LOG_FILE="/gluetun/qbit-port-forward.log"

log() {
  printf '%s\t%s\n' "$(date -Iseconds)" "$*" >> "$LOG_FILE" 2>/dev/null || true
}

log "trigger\tport=$PORT"

i=0
while [ $i -lt 30 ]; do
  if wget -q --spider "$QBIT_URL/api/v2/app/version" 2>/dev/null; then
    break
  fi
  sleep 2
  i=$((i+1))
done

if ! wget -q --spider "$QBIT_URL/api/v2/app/version" 2>/dev/null; then
  log "error\treason=qbit_unreachable_after_60s\tport=$PORT"
  exit 1
fi

JSON=$(printf '{"listen_port":%s,"random_port":false,"upnp":false}' "$PORT")
RESPONSE=$(wget -qO- --post-data="json=$JSON" "$QBIT_URL/api/v2/app/setPreferences" 2>&1) || RESPONSE="ERROR"

if [ "$RESPONSE" = "ERROR" ]; then
  log "error\treason=set_preferences_failed\tport=$PORT"
  exit 1
fi

log "applied\tlisten_port=$PORT"
