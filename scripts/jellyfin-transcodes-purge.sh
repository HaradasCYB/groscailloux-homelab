#!/usr/bin/env bash
# Purge du tmpfs de transcodage de Jellyfin (/cache/transcodes, 2 Go) : Jellyfin laisse des segments derrière lui et,
# plein, ffmpeg écrit des segments vides → « chargement infini » sur tout ce qui transcode (2026-09-20).
#
#   jellyfin-transcodes-purge.sh --check     état : usage, fichiers, jobs actifs / orphelins (aucune suppression)
#   jellyfin-transcodes-purge.sh             routine (timer chaque minute) : jobs sans ffmpeg actif vieux de > 2 min ;
#                                            bascule seule en urgence si usage >= URGENT_PCT (85)
#   jellyfin-transcodes-purge.sh --urgent    tout ce qui n'a pas de ffmpeg actif, puis les segments > 10 min des jobs actifs
#   jellyfin-transcodes-purge.sh --force     vide tout — refusé s'il reste un ffmpeg actif (couperait une lecture)
#
# Code retour : 0 ok, 2 = toujours >= URGENT_PCT après purge, 3 = --force refusé.
# Alerte Discord admin (DISCORD_WEBHOOK_ADMIN dans .env, jamais affichée) quand l'urgence se déclenche ou échoue.
# Un job ACTIF plus gros que le tmpfs (remux haut débit ≈ 480 s × débit) ne se purge pas : seule la taille du tmpfs aide.
set -euo pipefail
CONTAINER=${CONTAINER:-jellyfin}
DIR=/cache/transcodes
URGENT_PCT=${URGENT_PCT:-85}
ROUTINE_MIN=${ROUTINE_MIN:-2}      # un job terminé n'est jamais réutilisé ; 2 min couvrent le démarrage d'un job
ACTIVE_MIN=${ACTIVE_MIN:-10}
MODE=routine
case "${1:-}" in --check) MODE=check ;; --urgent) MODE=urgent ;; --force) MODE=force ;; "") ;; *) echo "usage: $0 [--check|--urgent|--force]" >&2; exit 1 ;; esac

log() { echo "[$(date '+%F %T')] $*"; }
alert() {  # message court sur le salon Discord admin ; silencieux si le webhook manque
  local url; url=$(grep -E '^DISCORD_WEBHOOK_ADMIN=' /opt/homelab/.env 2>/dev/null | cut -d= -f2- | tr -d '"')
  [ -n "$url" ] || return 0
  curl -s -m 10 -o /dev/null -H 'Content-Type: application/json' \
    --data "$(printf '%s' "$1" | python3 -c 'import json,sys; print(json.dumps({"content": sys.stdin.read()[:1900]}))')" "$url" || true
}
usage_pct() { docker exec "$CONTAINER" df --output=pcent "$DIR" | tail -1 | tr -dc '0-9'; }
nfiles() { docker exec "$CONTAINER" sh -c "ls -1 $DIR 2>/dev/null | wc -l"; }
# préfixes (hash de job) des ffmpeg en cours : Jellyfin passe le chemin de la playlist « /cache/transcodes/<hash>.m3u8 »
active_jobs() { ps -eo args | grep '[j]ellyfin-ffmpeg/ffmpeg' | grep -oE "$DIR/[0-9a-f]{32}" | sed "s#$DIR/##" | sort -u; }

ACTIVE=$(active_jobs || true)
before=$(usage_pct); nb=$(nfiles)
log "tmpfs $DIR : ${before}% utilisé, $nb fichier(s), jobs ffmpeg actifs : $(echo "$ACTIVE" | grep -c . || true)"

if [ "$MODE" = check ]; then
  docker exec "$CONTAINER" sh -c "cd $DIR && ls -1 | sed -E 's/(-?[0-9]+)\.mp4$//; s/\.m3u8$//; s/\.ts$//' | sort | uniq -c" | while read -r n p; do
    st=orphelin; echo "$ACTIVE" | grep -qx "$p" && st=ACTIF
    sz=$(docker exec "$CONTAINER" sh -c "du -ch $DIR/${p}* 2>/dev/null | tail -1 | cut -f1")
    echo "  $sz  $n fichier(s)  $st  $p"
  done | sort -h
  exit 0
fi

if [ "$MODE" = routine ] && [ "$before" -ge "$URGENT_PCT" ]; then
  log "URGENT : $before% >= $URGENT_PCT% — purge d'urgence"
  MODE=urgent
  URGENT_AUTO=1
fi
URGENT_AUTO=${URGENT_AUTO:-0}

# liste des fichiers à effacer, construite dans le conteneur ; « -maxdepth 1 -type f » : jamais ailleurs que dans $DIR
keep_expr=""
for p in $ACTIVE; do keep_expr="$keep_expr ! -name '${p}*'"; done
case "$MODE" in
  routine) docker exec "$CONTAINER" sh -c "find $DIR -maxdepth 1 -type f -mmin +$ROUTINE_MIN $keep_expr -delete" ;;
  urgent)
    docker exec "$CONTAINER" sh -c "find $DIR -maxdepth 1 -type f $keep_expr -delete"
    for p in $ACTIVE; do docker exec "$CONTAINER" sh -c "find $DIR -maxdepth 1 -type f -name '${p}*.mp4' -mmin +$ACTIVE_MIN -delete; find $DIR -maxdepth 1 -type f -name '${p}*.ts' -mmin +$ACTIVE_MIN -delete"; done ;;
  force)
    if [ -n "$ACTIVE" ]; then log "--force refusé : $(echo "$ACTIVE" | grep -c .) ffmpeg actif(s), une lecture serait coupée (utiliser --urgent)"; exit 3; fi
    docker exec "$CONTAINER" sh -c "find $DIR -maxdepth 1 -type f -delete" ;;
esac
after=$(usage_pct)
log "$MODE : ${before}% → ${after}%, $(nfiles) fichier(s) restant(s)"
if [ "$after" -ge "$URGENT_PCT" ]; then
  log "URGENT : toujours ${after}% après purge (jobs actifs trop gros ?)"
  alert "⚠️ Jellyfin : tmpfs de transcodage toujours à ${after}% après purge d'urgence (${before}% avant, $(echo "$ACTIVE" | grep -c . || true) job(s) actif(s)). Un job actif dépasse le tmpfs : lectures en transcodage menacées."
  exit 2
fi
[ "$URGENT_AUTO" = 1 ] && alert "🧹 Jellyfin : tmpfs de transcodage saturé (${before}%), purge d'urgence faite → ${after}%."
exit 0
