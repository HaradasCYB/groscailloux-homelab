#!/bin/bash
# setup.sh — bootstrap groscailloux-homelab
#
# Verifie prerequis, valide .env, cree l'arborescence, lance docker compose,
# installe les unit files systemd.
#
# Idempotent. A relancer apres modifs .env ou docker-compose.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")" && pwd)"
TARGET_BASE="/opt/homelab"

# Couleurs basiques
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'
ok()   { echo -e "${GREEN}✓${NC} $*"; }
warn() { echo -e "${YELLOW}!${NC} $*"; }
err()  { echo -e "${RED}✗${NC} $*" >&2; }
hdr()  { echo ""; echo "==> $*"; }

[ "$EUID" -eq 0 ] || { err "Lance setup.sh avec sudo (besoin pour systemd units)."; exit 1; }

hdr "1. Prerequis"

MISSING=()
for cmd in docker jq curl openssl inotifywait unzip 7z; do
  if ! command -v "$cmd" >/dev/null 2>&1; then
    MISSING+=("$cmd")
  fi
done
if [ ${#MISSING[@]} -gt 0 ]; then
  err "Outils manquants : ${MISSING[*]}"
  echo "  Installer : sudo apt install -y ${MISSING[*]/inotifywait/inotify-tools} p7zip-full"
  exit 1
fi
ok "Prerequis OK"

if ! docker compose version >/dev/null 2>&1; then
  err "docker compose v2 requis. Installer docker-compose-plugin."
  exit 1
fi
ok "docker compose v2 OK"

hdr "2. .env validation"

ENV_FILE="$REPO_ROOT/.env"
if [ ! -f "$ENV_FILE" ]; then
  err ".env manquant. Faire : cp .env.example .env && nano .env"
  exit 1
fi

REMAINING=$(grep -c "CHANGE_ME" "$ENV_FILE" || true)
if [ "$REMAINING" -gt 0 ]; then
  warn "Il reste $REMAINING champ(s) CHANGE_ME dans .env"
  grep -n "CHANGE_ME" "$ENV_FILE" | head -10
  read -p "Continuer quand meme ? [yN] " -n 1 -r REPLY
  echo
  [[ $REPLY =~ ^[Yy]$ ]] || exit 1
fi
ok ".env OK"

# shellcheck disable=SC1090
set -a && . "$ENV_FILE" && set +a

hdr "3. Arborescence"

DIRS=(
  "$TARGET_BASE/downloads"
  "$TARGET_BASE/media/movies"
  "$TARGET_BASE/media/tvshows"
  "$TARGET_BASE/logs"
  "$TARGET_BASE/jellyfin/config"
  "$TARGET_BASE/jellyfin/cache"
  "$TARGET_BASE/jellyseerr/config"
  "$TARGET_BASE/radarr/config"
  "$TARGET_BASE/sonarr/config"
  "$TARGET_BASE/prowlarr/config"
  "$TARGET_BASE/jackett/config"
  "$TARGET_BASE/qbittorrent/config"
  "$TARGET_BASE/pyload/config"
  "$TARGET_BASE/filebrowser/database"
  "$TARGET_BASE/filebrowser/config"
  "$TARGET_BASE/homarr"
  "$TARGET_BASE/grafana"
  "$TARGET_BASE/influxdb/data"
  "$TARGET_BASE/influxdb/config"
  "$TARGET_BASE/portainer"
  "$TARGET_BASE/gluetun"
  "$TARGET_BASE/npm/data"
  "$TARGET_BASE/npm/letsencrypt"
  "$TARGET_BASE/telegraf/etc"
  "$TARGET_BASE/guacamole/mysql"
)

for d in "${DIRS[@]}"; do
  mkdir -p "$d"
done
chown -R 1000:1000 "$TARGET_BASE/downloads" "$TARGET_BASE/media" "$TARGET_BASE/logs" \
                   "$TARGET_BASE/jellyfin" "$TARGET_BASE/jellyseerr" "$TARGET_BASE/radarr" \
                   "$TARGET_BASE/sonarr" "$TARGET_BASE/prowlarr" "$TARGET_BASE/jackett" \
                   "$TARGET_BASE/qbittorrent" "$TARGET_BASE/pyload" "$TARGET_BASE/filebrowser" \
                   "$TARGET_BASE/homarr" "$TARGET_BASE/portainer" "$TARGET_BASE/gluetun" \
                   "$TARGET_BASE/npm" "$TARGET_BASE/telegraf" 2>/dev/null || true
chown -R 472:472 "$TARGET_BASE/grafana" 2>/dev/null || true
ok "Arborescence creee"

hdr "4. Sync repo files vers /opt/homelab"

# On ne copie PAS les fichiers existants des services (preservation state)
rsync -a --exclude='.git' --exclude='*.original' \
  --exclude='downloads/' --exclude='media/' --exclude='logs/' \
  "$REPO_ROOT/docker-compose.yml" "$REPO_ROOT/.env" \
  "$REPO_ROOT/scripts" "$REPO_ROOT/init" "$REPO_ROOT/templates" \
  "$TARGET_BASE/"

chmod 755 "$TARGET_BASE/scripts/"*.sh "$TARGET_BASE/init/"*.sh 2>/dev/null
chmod 600 "$TARGET_BASE/.env"
ok "Files synced"

hdr "5. docker compose up"

cd "$TARGET_BASE"
docker compose pull
docker compose up -d

ok "Services demarres"

hdr "6. systemd units"

UNITS_DIR="/etc/systemd/system"
for unit in "$TARGET_BASE/scripts/"*.{service,timer}; do
  [ -f "$unit" ] || continue
  name=$(basename "$unit")
  cp "$unit" "$UNITS_DIR/$name"
  ok "Installed: $name"
done

systemctl daemon-reload

# Enable les timers (pas les services oneshot directs — sauf auto-import qui est daemon)
for timer in "$TARGET_BASE/scripts/"*.timer; do
  [ -f "$timer" ] || continue
  name=$(basename "$timer")
  systemctl enable --now "$name" 2>/dev/null && ok "Enabled: $name"
done

# auto-import.service est un daemon (Type=simple), enable-now
if [ -f "$UNITS_DIR/auto-import.service" ]; then
  systemctl enable --now auto-import.service 2>/dev/null && ok "Enabled: auto-import.service"
fi

hdr "7. Recap"

echo ""
echo "Services UI :"
echo "  Jellyfin    http://localhost:8096"
echo "  Jellyseerr  http://localhost:5055"
echo "  Sonarr      http://localhost:8989"
echo "  Radarr      http://localhost:7878"
echo "  Prowlarr    http://localhost:9696"
echo "  qBittorrent http://localhost:8080"
echo "  Homarr      http://localhost:7575"
echo "  Grafana     http://localhost:3000"
echo "  Portainer   http://localhost:9000"
echo "  NPM         http://localhost:81"
echo ""
echo "Prochaines etapes :"
echo "  1. Configurer chaque service (suivre DEPLOY.md Phase 4)"
echo "  2. Recuperer les API keys et updater .env"
echo "  3. Relancer ./setup.sh pour propager les nouveaux secrets"
echo "  4. Lancer ./init/01-sonarr-init.sh, etc. pour appliquer le tuning FR"
echo "  5. Onboard ton premier user : ./scripts/homelab-onboard-user.sh <user> <email>"
echo ""
ok "Setup termine."
