#!/bin/sh
# Exécuté PAR gluetun (busybox) via VPN_PORT_FORWARDING_UP_COMMAND à chaque renouvellement
# du port forwardé ProtonVPN. Pousse le port dans qBittorrent (même netns → 127.0.0.1).
# Doit rester en shell POSIX : pas de bash, pas de curl, pas de jq dans l'image gluetun.
set -eu

PORT_LIST="${1:-}"
[ -n "$PORT_LIST" ] || { echo "qbit-update-port: no port argument" >&2; exit 1; }
PORT=$(echo "$PORT_LIST" | cut -d',' -f1)
QBIT_URL="http://127.0.0.1:8080"

log() { printf '%s qbit-update-port %s\n' "$(date -Iseconds)" "$*"; }

log "trigger port=$PORT"

i=0
while [ $i -lt 30 ]; do
  wget -q --spider "$QBIT_URL/api/v2/app/version" 2>/dev/null && break
  sleep 2
  i=$((i+1))
done
if ! wget -q --spider "$QBIT_URL/api/v2/app/version" 2>/dev/null; then
  log "error qbit_unreachable_after_60s port=$PORT"
  exit 1
fi

JSON=$(printf '{"listen_port":%s,"random_port":false,"upnp":false}' "$PORT")
if ! wget -qO- --post-data="json=$JSON" "$QBIT_URL/api/v2/app/setPreferences" >/dev/null 2>&1; then
  log "error set_preferences_failed port=$PORT"
  exit 1
fi
log "applied listen_port=$PORT"
