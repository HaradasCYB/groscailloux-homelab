#!/bin/bash
# tba-import-bypass.sh — bypass le garde-fou Sonarr "Episode has a TBA title and
# recently aired" pour les episodes anime qui viennent de sortir.
#
# Quand TVDB n'a pas encore le titre officiel d'un episode, Sonarr refuse en
# permanent l'auto-import (DownloadedEpisodesScan declenche par auto-import.sh
# echoue silencieusement). On scanne /downloads via /api/v3/manualimport, on
# filtre les fichiers dont la SEULE rejection est celle-la, puis on POST un
# ManualImport pour bypass.
#
# Conditions strictes pour declencher :
#   - rejections == exactement 1 entree
#   - reason == "Episode has a TBA title and recently aired"
#   - une serie + une episode identifiees
#   - pas deja un fichier en lib pour cet episode
#
# Lance toutes les 5 min par tba-import-bypass.timer.

set -euo pipefail
[ -f /opt/homelab/.env ] && { set -a; . /opt/homelab/.env; set +a; }

SONARR_URL="http://localhost:8989"
SONARR_API_KEY="${SONARR_API_KEY}"

DOWNLOADS_DIR="/downloads"   # path tel que vu par Sonarr (mount)
IMPORT_MODE="auto"

LOG_FILE="/opt/homelab/logs/tba-import-bypass.log"
LOCK_FILE="/tmp/tba-import-bypass.lock"
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

if ! PREVIEW=$(curl -fsS -G -H "X-Api-Key: $SONARR_API_KEY" \
     --data-urlencode "folder=$DOWNLOADS_DIR" \
     --data-urlencode "filterExistingFiles=true" \
     "$SONARR_URL/api/v3/manualimport" 2>/dev/null); then
  log "error\treason=manualimport_unreachable"
  exit 1
fi

# Filter: exactly 1 rejection AND reason starts with "Episode has a TBA title"
# AND series.id + episodes[0].id present AND episodeFileId == 0
CANDIDATES=$(echo "$PREVIEW" | jq -c '
  .[]
  | select(
      (.rejections | length) == 1
      and (.rejections[0].reason | startswith("Episode has a TBA title"))
      and (.series.id // null) != null
      and ((.episodes // []) | length) >= 1
      and ((.episodes[0].episodeFileId // 0) == 0)
    )
')

[ -z "$CANDIDATES" ] && { log "run_done\tcandidates=0"; exit 0; }

ACTIONS=0
ERRORS=0

while IFS= read -r item; do
  [ -z "$item" ] && continue

  PATH_=$(echo "$item" | jq -r '.path')
  TITLE=$(echo "$item" | jq -r '.series.title')
  SE=$(echo "$item" | jq -r '"S\(.seasonNumber|tostring|("0"+.|.[-2:]))E\(.episodes[0].episodeNumber|tostring|("0"+.|.[-2:]))"')

  FILE_OBJ=$(echo "$item" | jq -c '{
    path: .path,
    folderName: ((.path | sub("/[^/]+$"; "")) // "/downloads"),
    seriesId: .series.id,
    episodeIds: [.episodes[].id],
    releaseGroup: .releaseGroup,
    quality: .quality,
    languages: .languages,
    indexerFlags: (.indexerFlags // 0),
    releaseType: (.releaseType // "singleEpisode"),
    episodeFileId: 0,
    downloadId: null
  }')

  CMD=$(jq -n --argjson f "$FILE_OBJ" --arg mode "$IMPORT_MODE" \
        '{name:"ManualImport", files:[$f], importMode:$mode}')

  RESP=$(curl -fsS -X POST -H "X-Api-Key: $SONARR_API_KEY" -H "Content-Type: application/json" \
         -d "$CMD" "$SONARR_URL/api/v3/command" 2>/dev/null) || {
    log "error\treason=post_failed\ttitle=$TITLE\tep=$SE\tpath=$PATH_"
    ERRORS=$(( ERRORS + 1 ))
    continue
  }

  CMD_ID=$(echo "$RESP" | jq -r '.id // empty')
  log "bypass_triggered\ttitle=$TITLE\tep=$SE\tcmd_id=$CMD_ID\tpath=$PATH_"
  ACTIONS=$(( ACTIONS + 1 ))
done <<< "$CANDIDATES"

log "run_done\tcandidates=$(echo "$CANDIDATES" | wc -l)\tactions=$ACTIONS\terrors=$ERRORS"
