#!/usr/bin/env bash
# Relance des applis de la SEEDBOX (2026-10-02). Installé sur la seedbox dans ~/.local/bin/, lancé par le crontab
# de la seedbox : `@reboot` (toutes les applis, 2 min après le démarrage) et toutes les 5 min (celles qui ne
# répondent plus deux fois de suite).
#
# Pourquoi : le 01/10 vers 20:50, l'hôte partagé de la seedbox a redémarré ; qBittorrent (service utilisateur) est
# reparti, mais Sonarr, Radarr, Bazarr, Jackett, FlareSolverr, autobrr et unpackerr sont restés arrêtés 16 h (ni
# import ni recherche). `app-<x> start` est sans effet sur une appli déjà lancée : le relancer est sans risque.
#
#   homelab-apps-watch.sh            vérifie et relance ce qui ne répond plus (2 échecs de suite)
#   homelab-apps-watch.sh --boot     après un démarrage : tout relancer
#   homelab-apps-watch.sh --dry-run  affiche ce qui serait fait, ne relance rien
set -u
MODE="${1:-}"
STATE="$HOME/.local/state/homelab-apps-watch"
LOG="$STATE/watch.log"
mkdir -p "$STATE"

log() { printf '%s %s\n' "$(date '+%F %T')" "$*" >> "$LOG"; [ "$MODE" = "--dry-run" ] && echo "$*"; }

# nom | commande de relance | URL de sonde (vide = pas de sonde HTTP : relancée seulement au démarrage)
APPS="
sonarr|app-sonarr start|http://127.0.0.1:16126/sonarr/ping
radarr|app-radarr start|http://127.0.0.1:16127/radarr/ping
bazarr|app-bazarr start|http://127.0.0.1:16131/bazarr/
jackett|app-jackett start|http://127.0.0.1:16129/
flaresolverr|app-flaresolverr start|http://172.17.0.1:16111/
autobrr|app-autobrr start|http://127.0.0.1:16123/
qbittorrent|systemctl --user start qbittorrent.service|http://127.0.0.1:16141/
unpackerr|app-unpackerr start|
"

up() { # toute réponse HTTP (même 30x/401) = l'appli tourne ; 000 = rien n'écoute
  local code
  code=$(curl -s -m 10 -o /dev/null -w '%{http_code}' "$1" 2>/dev/null)
  [ -n "$code" ] && [ "$code" != "000" ]
}

if [ "$MODE" = "--boot" ]; then
  sleep 120 # laisser l'hôte finir de démarrer (Docker, réseau)
fi

echo "$APPS" | while IFS='|' read -r name cmd url; do
  [ -z "$name" ] && continue
  fails_file="$STATE/$name.fails"
  if [ "$MODE" = "--boot" ]; then
    log "démarrage de l'hôte : $name → $cmd"
    eval "$cmd" >/dev/null 2>&1 || log "ÉCHEC $name : $cmd"
    rm -f "$fails_file"
    continue
  fi
  [ -z "$url" ] && continue
  if up "$url"; then
    if [ -s "$fails_file" ]; then log "$name répond de nouveau"; fi
    rm -f "$fails_file"
    continue
  fi
  n=$(( $(cat "$fails_file" 2>/dev/null || echo 0) + 1 ))
  echo "$n" > "$fails_file"
  if [ "$n" -lt 2 ]; then
    log "$name ne répond pas (1er échec) : nouvelle vérification au prochain passage"
    continue
  fi
  if [ "$MODE" = "--dry-run" ]; then
    log "[à blanc] $name ne répond pas ($n échecs) : relancerait « $cmd »"
    continue
  fi
  log "$name ne répond pas ($n échecs) : $cmd"
  eval "$cmd" >/dev/null 2>&1 || log "ÉCHEC $name : $cmd"
done

# journal borné (~2000 lignes)
if [ -f "$LOG" ] && [ "$(wc -l < "$LOG")" -gt 2000 ]; then
  tail -n 1000 "$LOG" > "$LOG.tmp" && mv "$LOG.tmp" "$LOG"
fi
exit 0
