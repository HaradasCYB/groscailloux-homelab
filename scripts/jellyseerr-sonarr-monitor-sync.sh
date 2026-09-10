#!/bin/bash
# jellyseerr-sonarr-monitor-sync.sh — reconcilie Sonarr seasons[].monitored avec
# les season_request de Jellyseerr.
#
# Pourquoi : Jellyseerr POST /series sans addOptions.monitor, donc Sonarr applique
# son defaut "all" et toutes les saisons finissent monitored, meme celles non
# cochees par l'user (ex. Re:Zero S01 grab du pack SHiNiGAMi).
#
# Logique : pour chaque serie Sonarr ayant un mapping Jellyseerr
# (media.externalServiceId), set seasons[].monitored = (saisonNumber demandee
# dans Jellyseerr) OR (saison a deja des fichiers). S00 (specials) jamais touche.
# Les series ajoutees directement dans Sonarr (sans entree Jellyseerr) ne sont
# pas modifiees.
#
# DRY_RUN=1 pour voir le diff sans PUT.
#
# Lance toutes les 10 min par jellyseerr-sonarr-monitor-sync.timer.

set -euo pipefail
[ -f /opt/homelab/.env ] && { set -a; . /opt/homelab/.env; set +a; }

SONARR_URL="http://localhost:8989"
SONARR_API_KEY="${SONARR_API_KEY}"

JELLYSEERR_DB="/opt/homelab/jellyseerr/config/db/db.sqlite3"

DRY_RUN="${DRY_RUN:-0}"

LOG_FILE="/opt/homelab/logs/jellyseerr-sonarr-monitor-sync.log"
LOCK_FILE="/tmp/jellyseerr-sonarr-monitor-sync.lock"
LOCK_STALE_AFTER=900

mkdir -p "$(dirname "$LOG_FILE")"

log() { printf '%s\t%b\n' "$(date -Iseconds)" "$*" >> "$LOG_FILE"; }

if [ -e "$LOCK_FILE" ]; then
  LOCK_AGE=$(( $(date +%s) - $(stat -c %Y "$LOCK_FILE" 2>/dev/null || echo 0) ))
  if [ "$LOCK_AGE" -lt "$LOCK_STALE_AFTER" ]; then
    exit 0
  fi
  log "warn\treason=stale_lock_removed\tage=${LOCK_AGE}s"
  rm -f "$LOCK_FILE"
fi
echo $$ > "$LOCK_FILE"
trap 'rm -f "$LOCK_FILE"' EXIT

[ -r "$JELLYSEERR_DB" ] || { log "error\treason=db_unreadable\tpath=$JELLYSEERR_DB"; exit 1; }

# Map sonarrSeriesId -> [seasonNumbers requested]. Filter out DECLINED(3) / FAILED(4).
REQUESTED_MAP=$(python3 - "$JELLYSEERR_DB" <<'PYEOF'
import sqlite3, json, sys
con = sqlite3.connect(sys.argv[1])
out = {}
for sid, sn in con.execute("""
    SELECT m.externalServiceId, sr.seasonNumber
    FROM season_request sr
    JOIN media_request mr ON mr.id = sr.requestId
    JOIN media m ON m.id = mr.mediaId
    WHERE m.mediaType = 'tv'
      AND m.externalServiceId IS NOT NULL
      AND sr.status NOT IN (3, 4)
"""):
    out.setdefault(sid, set()).add(sn)
print(json.dumps({str(k): sorted(v) for k, v in out.items()}))
PYEOF
)

if [ -z "$REQUESTED_MAP" ] || [ "$REQUESTED_MAP" = "{}" ]; then
  log "skip\treason=no_jellyseerr_requests"
  exit 0
fi

if ! ALL_SERIES=$(curl -fsS -H "X-Api-Key: $SONARR_API_KEY" "$SONARR_URL/api/v3/series" 2>/dev/null); then
  log "error\treason=sonarr_unreachable"
  exit 1
fi

CHANGES=0

while IFS= read -r series; do
  [ -z "$series" ] && continue
  SERIES_ID=$(echo "$series" | jq -r '.id')
  TITLE=$(echo "$series" | jq -r '.title')
  REQUESTED=$(echo "$REQUESTED_MAP" | jq -c --arg sid "$SERIES_ID" '.[$sid] // empty')

  # Skip series not managed by Jellyseerr
  [ -z "$REQUESTED" ] && continue

  # Compute new seasons array
  NEW_SERIES=$(echo "$series" | jq --argjson req "$REQUESTED" '
    .seasons |= map(
      if .seasonNumber == 0 then .
      elif ([.seasonNumber] | inside($req)) then .monitored = true
      elif ((.statistics.episodeFileCount // 0) > 0) then .monitored = true
      else .monitored = false
      end
    )
  ')

  # Compare before/after
  BEFORE=$(echo "$series"     | jq -c '[.seasons[] | select(.monitored) | .seasonNumber]')
  AFTER=$(echo "$NEW_SERIES"  | jq -c '[.seasons[] | select(.monitored) | .seasonNumber]')

  [ "$BEFORE" = "$AFTER" ] && continue

  if [ "$DRY_RUN" = "1" ]; then
    log "dry_run\tseries_id=$SERIES_ID\ttitle=$TITLE\trequested=$REQUESTED\tbefore=$BEFORE\tafter=$AFTER"
    CHANGES=$(( CHANGES + 1 ))
    continue
  fi

  if curl -fsS -X PUT -H "X-Api-Key: $SONARR_API_KEY" -H "Content-Type: application/json" \
       --data "$NEW_SERIES" "$SONARR_URL/api/v3/series/$SERIES_ID" >/dev/null 2>&1; then
    log "synced\tseries_id=$SERIES_ID\ttitle=$TITLE\trequested=$REQUESTED\tbefore=$BEFORE\tafter=$AFTER"
    CHANGES=$(( CHANGES + 1 ))
  else
    log "error\treason=put_failed\tseries_id=$SERIES_ID\ttitle=$TITLE"
  fi
done < <(echo "$ALL_SERIES" | jq -c '.[]')

log "run_done\tdry_run=$DRY_RUN\tchanges=$CHANGES"
