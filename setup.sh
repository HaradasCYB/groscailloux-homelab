#!/bin/bash
# setup.sh — bootstrap idempotent d'un hôte pour groscailloux-homelab.
#
#   git clone https://github.com/HaradasCYB/groscailloux-homelab.git /opt/homelab
#   cd /opt/homelab && cp .env.example .env && nano .env
#   sudo ./setup.sh [--from-source] [--no-start]
#
# Étapes : prérequis → .env → arborescence → telegraf.conf → binaires homelabd/homelabctl
# (release GitHub, ou cargo avec --from-source) → docker compose up → unités systemd.
set -euo pipefail

BASE=/opt/homelab
REPO_ROOT=$(cd "$(dirname "$0")" && pwd)
RELEASE_URL=https://github.com/HaradasCYB/groscailloux-homelab/releases/latest/download/homelab-x86_64-unknown-linux-musl.tar.gz
FROM_SOURCE=0
START=1
for a in "$@"; do
  case "$a" in
    --from-source) FROM_SOURCE=1 ;;
    --no-start) START=0 ;;
    *) echo "usage: sudo ./setup.sh [--from-source] [--no-start]" >&2; exit 2 ;;
  esac
done

ok()   { printf '\033[0;32m✓\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!\033[0m %s\n' "$*"; }
err()  { printf '\033[0;31m✗\033[0m %s\n' "$*" >&2; }
hdr()  { printf '\n==> %s\n' "$*"; }

[ "$EUID" -eq 0 ] || { err "à lancer avec sudo"; exit 1; }
[ "$REPO_ROOT" = "$BASE" ] || { err "le dépôt doit être cloné dans $BASE (ici : $REPO_ROOT)"; exit 1; }

hdr "1. Prérequis"
missing=()
for c in docker curl zstd tar unzip; do command -v "$c" >/dev/null || missing+=("$c"); done
command -v unrar >/dev/null || command -v 7z >/dev/null || missing+=("unrar|p7zip-full")
if [ ${#missing[@]} -gt 0 ]; then
  err "outils manquants : ${missing[*]}"
  echo "   apt install -y curl zstd unzip unrar p7zip-full ; docker : https://docs.docker.com/engine/install/"
  exit 1
fi
docker compose version >/dev/null 2>&1 || { err "docker compose v2 requis (docker-compose-plugin)"; exit 1; }
id -u deploy >/dev/null 2>&1 || { err "l'utilisateur 'deploy' (uid 1000, groupe docker) doit exister"; exit 1; }
ok "docker $(docker --version | awk '{print $3}' | tr -d ,), compose $(docker compose version --short)"

hdr "2. .env"
[ -f "$BASE/.env" ] || { err ".env manquant : cp .env.example .env puis remplir (voir SECRETS.md)"; exit 1; }
chmod 600 "$BASE/.env"; chown deploy:deploy "$BASE/.env"
empty=$(grep -cE '^[A-Z_]+=\s*(#.*)?$' "$BASE/.env" || true)
[ "$empty" -eq 0 ] || { warn "$empty variable(s) vide(s) dans .env :"; grep -nE '^[A-Z_]+=\s*(#.*)?$' "$BASE/.env" | cut -d= -f1; }
set -a; . "$BASE/.env"; set +a
ok ".env chargé"

hdr "3. Arborescence"
for d in library/downloads library/media/movies library/media/tvshows state backups diun \
         jellyfin/config jellyfin/cache jellyseerr/config radarr/config sonarr/config prowlarr/config \
         qbittorrent/config pyload/config filebrowser/database filebrowser/config homarr portainer gluetun \
         npm/data npm/letsencrypt telegraf/etc influxdb/data influxdb/config guacamole/mysql grafana; do
  mkdir -p "$BASE/$d"
done
for d in library state backups diun jellyfin jellyseerr radarr sonarr prowlarr qbittorrent pyload filebrowser portainer gluetun telegraf influxdb; do
  [ -z "$(find "$BASE/$d" -maxdepth 0 -user deploy 2>/dev/null)" ] && chown 1000:1000 "$BASE/$d" || true
done
chown 472:472 "$BASE/grafana" 2>/dev/null || true
ok "dossiers en place (npm/, homarr/, guacamole/ gardent leur propriétaire d'image)"

hdr "4. telegraf.conf"
if [ -n "${INFLUX_TOKEN:-}" ]; then
  sed "s|\${INFLUX_TOKEN}|$INFLUX_TOKEN|" "$BASE/telegraf/etc/telegraf.conf.tmpl" > "$BASE/telegraf/etc/telegraf.conf"
  chmod 640 "$BASE/telegraf/etc/telegraf.conf"; chown deploy:deploy "$BASE/telegraf/etc/telegraf.conf"
  ok "telegraf.conf généré"
else
  warn "INFLUX_TOKEN vide : telegraf.conf non généré (à faire après le premier démarrage d'InfluxDB)"
fi

hdr "5. Binaires homelabd / homelabctl"
if [ "$FROM_SOURCE" = 1 ]; then
  command -v cargo >/dev/null || { err "cargo requis (rustup) pour --from-source"; exit 1; }
  sudo -u deploy env PATH="/home/deploy/.cargo/bin:$PATH" cargo build --release --target x86_64-unknown-linux-musl -j4 --manifest-path "$BASE/Cargo.toml"
  install -m 755 "$BASE"/target/x86_64-unknown-linux-musl/release/{homelabd,homelabctl} /usr/local/bin/
else
  tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
  if curl -fsSL "$RELEASE_URL" -o "$tmp/homelab.tar.gz"; then
    tar -xzf "$tmp/homelab.tar.gz" -C "$tmp"
    install -m 755 "$tmp/homelabd" "$tmp/homelabctl" /usr/local/bin/
  else
    err "téléchargement de la release impossible ($RELEASE_URL) — relancer avec --from-source"; exit 1
  fi
fi
ok "$(homelabd --version) / $(homelabctl --version)"

hdr "6. Docker compose"
(cd "$BASE" && docker compose config --quiet) || { err "docker-compose.yml invalide"; exit 1; }
if [ "$START" = 1 ]; then
  (cd "$BASE" && docker compose pull --quiet && docker compose up -d --remove-orphans)
  ok "stack démarrée"
else
  warn "--no-start : compose non lancé"
fi

hdr "7. systemd"
homelabctl install
if [ "$START" = 1 ]; then
  systemctl restart homelabd.service && ok "homelabd démarré"
fi

hdr "8. Suite"
cat <<EOF
  - Vérifier : homelabctl check ; journalctl -u homelabd -f ; docker compose ps
  - Premier déploiement : configurer les services (DEPLOY.md), récupérer les clés API,
    compléter .env puis relancer sudo ./setup.sh
  - Onboarder un utilisateur : homelabctl onboard <user> <email>
  - NPM : proxifier onboarder.<domaine> vers 172.18.0.1:8766 (UI homelabd sur l'hôte)
EOF
ok "setup terminé"
