#!/usr/bin/env bash
# Purge du tmpfs de transcodage de Jellyfin (/cache/transcodes, 2 Go) : Jellyfin laisse des segments derrière lui et,
# plein, ffmpeg écrit des segments vides → « chargement infini » sur tout ce qui transcode (2026-09-20).
#
#   jellyfin-transcodes-purge.sh --check     état : usage, fichiers, jobs actifs / orphelins (aucune suppression)
#   jellyfin-transcodes-purge.sh             routine (timer 5 min) : jobs sans ffmpeg actif vieux de > 30 min ;
#                                            bascule seule en urgence si usage >= URGENT_PCT (85)
#   jellyfin-transcodes-purge.sh --urgent    tout ce qui n'a pas de ffmpeg actif, puis les segments > 10 min des jobs actifs
#   jellyfin-transcodes-purge.sh --force     vide tout — refusé s'il reste un ffmpeg actif (couperait une lecture)
#
# Code retour : 0 ok, 2 = toujours >= URGENT_PCT après purge, 3 = --force refusé.
set -euo pipefail
CONTAINER=${CONTAINER:-jellyfin}
DIR=/cache/transcodes
URGENT_PCT=${URGENT_PCT:-85}
ROUTINE_MIN=${ROUTINE_MIN:-30}
ACTIVE_MIN=${ACTIVE_MIN:-10}
MODE=routine
case "${1:-}" in --check) MODE=check ;; --urgent) MODE=urgent ;; --force) MODE=force ;; "") ;; *) echo "usage: $0 [--check|--urgent|--force]" >&2; exit 1 ;; esac

log() { echo "[$(date '+%F %T')] $*"; }
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
fi

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
if [ "$after" -ge "$URGENT_PCT" ]; then log "URGENT : toujours ${after}% après purge (jobs actifs trop gros ?)"; exit 2; fi
