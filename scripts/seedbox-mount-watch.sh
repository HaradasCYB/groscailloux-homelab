#!/usr/bin/env bash
# Chien de garde du montage rclone de la seedbox (2026-09-27).
#
# Un fichier supprimé sur la seedbox (remplacement, suppression, renommage par un Arr) peut rester dans le cache de
# répertoires de rclone ; si Jellyfin l'ouvre à ce moment, l'ouverture SFTP ne rend jamais la main : le ffprobe de
# Jellyfin reste bloqué dans le noyau (état D, impossible à tuer) et garde le fichier « vivant » dans rclone. Le
# 2026-09-26, huit ffprobe sur l'ancien BLACK TORCH S01E01 ont tenu 19 h : plus aucun titre nouveau n'apparaissait dans
# Jellyfin (file de rafraîchissement bloquée) et l'analyse de la médiathèque restait figée à 91 %.
#
#   seedbox-mount-watch.sh --check   état seulement
#   seedbox-mount-watch.sh           (timer, 5 min) ffprobe de plus de STUCK_MIN min : fichier oublié dans rclone
#                                    (vfs/forget) ; toujours bloqué : montage redémarré s'il n'y a aucune lecture en cours
#                                    depuis la seedbox, ou d'office au-delà de HARD_MIN min. Alerte Discord admin.
set -euo pipefail
STUCK_MIN=${STUCK_MIN:-15}
HARD_MIN=${HARD_MIN:-60}
RC=http://127.0.0.1:5572
MODE=act
case "${1:-}" in --check) MODE=check ;; "") ;; *) echo "usage: $0 [--check]" >&2; exit 1 ;; esac

log() { echo "[$(date '+%F %T')] $*"; }
env_get() { grep -E "^$1=" /opt/homelab/.env 2>/dev/null | cut -d= -f2- | tr -d '"'; }
alert() {
  local url; url=$(env_get DISCORD_WEBHOOK_ADMIN)
  [ -n "$url" ] || return 0
  curl -s -m 10 -o /dev/null -H 'Content-Type: application/json' \
    --data "$(printf '%s' "$1" | python3 -c 'import json,sys; print(json.dumps({"content": sys.stdin.read()[:1900]}))')" "$url" || true
}
# « pid minutes chemin » des ffprobe de Jellyfin plus vieux que STUCK_MIN minutes, sur un fichier de la seedbox
stuck() {
  ps -eo pid=,etimes=,args= | while read -r pid secs args; do
    case "$args" in *jellyfin-ffmpeg/ffprobe*file:/seedbox/media/*) ;; *) continue ;; esac
    [ "$secs" -ge $((STUCK_MIN * 60)) ] || continue
    path=$(printf '%s' "$args" | sed -E 's#.* -i file:/seedbox/media/(.*) -threads.*#\1#; s#.* -i file:/seedbox/media/(.*)$#\1#')
    echo "$pid $((secs / 60)) $path"
  done
}
# lectures en cours d'un fichier de la seedbox (chemins Jellyfin), via l'API
seedbox_playing() {
  local key; key=$(env_get JELLYFIN_API_KEY)
  curl -s -m 10 -H "X-Emby-Token: $key" "http://127.0.0.1:8096/Sessions?activeWithinSeconds=300" | python3 -c '
import json,sys
try: s=json.load(sys.stdin)
except Exception: print(1); sys.exit()
print(sum(1 for x in s if (x.get("NowPlayingItem") or {}).get("Path","").startswith("/seedbox/")))' 2>/dev/null || echo 1
}

LIST=$(stuck || true)
if [ -z "$LIST" ]; then
  [ "$MODE" = check ] && log "aucun ffprobe bloqué (> $STUCK_MIN min)"
  exit 0
fi
log "ffprobe bloqués (> $STUCK_MIN min) :"; echo "$LIST" | sed 's/^/  /'
[ "$MODE" = check ] && exit 0

# 1. oublier les fichiers dans rclone (suffit si le fichier n'est plus tenu ouvert)
echo "$LIST" | awk '{ $1=""; $2=""; sub(/^  /,""); print }' | sort -u | while read -r rel; do
  curl -s -m 30 -X POST "$RC/vfs/forget" --data-urlencode "file=$rel" >/dev/null || true
  log "rclone : oublié « $rel »"
done
sleep 15
LIST=$(stuck || true)
[ -z "$LIST" ] && { log "débloqué sans redémarrage"; exit 0; }

# 2. toujours bloqué : redémarrer le montage (coupe les lectures seedbox en cours) — seulement sans lecture, ou d'office
oldest=$(echo "$LIST" | awk '{print $2}' | sort -n | tail -1)
playing=$(seedbox_playing)
files=$(echo "$LIST" | awk '{ $1=""; $2=""; sub(/^  /,""); print }' | sort -u | head -3 | tr '\n' ';')
if [ "$playing" -gt 0 ] && [ "$oldest" -lt "$HARD_MIN" ]; then
  log "toujours bloqué ($oldest min) ; $playing lecture(s) seedbox en cours : redémarrage au prochain passage"
  exit 0
fi
log "redémarrage du montage (bloqué depuis $oldest min, lectures seedbox en cours : $playing)"
systemctl restart homelab-seedbox-mount
sleep 10
left=$(stuck | grep -c . || true)
msg="Montage seedbox redémarré : ffprobe de Jellyfin bloqué depuis ${oldest} min sur ${files} (fichier supprimé resté dans le cache rclone). Lectures seedbox coupées : ${playing}. Restant bloqués : ${left}."
log "$msg"
alert "$msg"
