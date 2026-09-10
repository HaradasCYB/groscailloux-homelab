#!/bin/bash
# vpn-toggle.sh — bascule qBittorrent VPN ON / OFF
# Usage:
#   sudo /opt/homelab/scripts/vpn-toggle.sh on    # qBit derrière gluetun (NordVPN)
#   sudo /opt/homelab/scripts/vpn-toggle.sh off   # qBit direct (sans VPN)
#   sudo /opt/homelab/scripts/vpn-toggle.sh status

set -e
[ -f /opt/homelab/.env ] && { set -a; . /opt/homelab/.env; set +a; }
COMPOSE=/opt/homelab/docker-compose.yml
SNAPSHOT_ON=/opt/homelab/.docker-compose.vpn-on.yml
SNAPSHOT_OFF=/opt/homelab/.docker-compose.vpn-off.yml
QBIT_CONF=/opt/homelab/qbittorrent/config/qBittorrent/qBittorrent.conf

# Active/désactive IPv6 dans qBit selon le mode VPN.
# Sous VPN (NordVPN/OpenVPN), tun0 est IPv4-only et eth0 du netns gluetun n'a pas
# d'IPv6 → toute annonce IPv6 sortante échoue avec "Operation not permitted".
# Hors VPN, le VPS a une IPv6 native, on en profite pour les trackers AAAA.
set_qbit_ipv6() {
  local val="$1"   # "true" ou "false"
  if grep -q '^Session\\IPv6Enabled=' "$QBIT_CONF"; then
    sed -i "s|^Session\\\\IPv6Enabled=.*|Session\\\\IPv6Enabled=$val|" "$QBIT_CONF"
  else
    sed -i "/^\[BitTorrent\]/a Session\\\\IPv6Enabled=$val" "$QBIT_CONF"
  fi
}

# Repoint le download client QBittorrent de Sonarr/Radarr vers $1 (gluetun|qbittorrent)
repoint_arrs() {
  local target="$1"
  for KEY_BASE in "${SONARR_API_KEY}|http://localhost:8989/api/v3" "${RADARR_API_KEY}|http://localhost:7878/api/v3"; do
    local KEY="${KEY_BASE%%|*}"
    local BASE="${KEY_BASE##*|}"
    local DC ID PAYLOAD
    DC=$(curl -s "$BASE/downloadclient" -H "X-Api-Key: $KEY")
    ID=$(echo "$DC" | python3 -c "import sys,json; print(next(c['id'] for c in json.load(sys.stdin) if c['implementation']=='QBittorrent'))")
    PAYLOAD=$(echo "$DC" | TARGET="$target" python3 -c '
import sys, json, os
target = os.environ["TARGET"]
data = json.load(sys.stdin)
client = next(c for c in data if c["implementation"] == "QBittorrent")
for field in client["fields"]:
    if field.get("name") == "host":
        field["value"] = target
print(json.dumps(client))
')
    curl -s -X PUT "$BASE/downloadclient/$ID?forceSave=true" \
      -H "X-Api-Key: $KEY" -H "Content-Type: application/json" \
      -d "$PAYLOAD" >/dev/null
  done
}

case "${1:-status}" in
  on)
    [ ! -f "$SNAPSHOT_ON" ] && { echo "Manque $SNAPSHOT_ON"; exit 1; }
    cp "$COMPOSE" "$SNAPSHOT_OFF"
    cp "$SNAPSHOT_ON" "$COMPOSE"
    set_qbit_ipv6 false
    cd /opt/homelab && docker compose up -d --force-recreate qbittorrent gluetun
    sleep 6
    repoint_arrs gluetun
    docker exec npm sh -c 'sed -i "s/set \$server         \"qbittorrent\";/set \$server         \"gluetun\";/" /data/nginx/proxy_host/5.conf && nginx -s reload' 2>/dev/null
    echo "VPN ON — qBit derrière gluetun (NordVPN)"
    sleep 3
    echo -n "IP publique qBit: "; docker exec qbittorrent wget -qO- https://ifconfig.me/ip 2>/dev/null; echo
    ;;
  off)
    [ ! -f "$SNAPSHOT_OFF" ] && { echo "Manque $SNAPSHOT_OFF"; exit 1; }
    cp "$COMPOSE" "$SNAPSHOT_ON"
    cp "$SNAPSHOT_OFF" "$COMPOSE"
    set_qbit_ipv6 true
    cd /opt/homelab && docker compose up -d --force-recreate qbittorrent gluetun
    sleep 6
    repoint_arrs qbittorrent
    docker exec npm sh -c 'sed -i "s/set \$server         \"gluetun\";/set \$server         \"qbittorrent\";/" /data/nginx/proxy_host/5.conf && nginx -s reload' 2>/dev/null
    echo "VPN OFF — qBit direct sur l'IP du VPS"
    sleep 3
    echo -n "IP publique qBit: "; docker exec qbittorrent wget -qO- https://ifconfig.me/ip 2>/dev/null; echo
    ;;
  status)
    NETMODE=$(docker inspect qbittorrent --format '{{.HostConfig.NetworkMode}}' 2>/dev/null)
    if echo "$NETMODE" | grep -q '^container:'; then
      echo "VPN ON  — qBit derrière gluetun"
    else
      echo "VPN OFF — qBit direct"
    fi
    echo -n "IP publique qBit: "; docker exec qbittorrent wget -qO- https://ifconfig.me/ip 2>/dev/null; echo
    ;;
  *)
    echo "Usage: $0 {on|off|status}"
    exit 1
    ;;
esac
