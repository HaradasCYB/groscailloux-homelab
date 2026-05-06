#!/bin/bash
# qbit-stuck-handler.sh — detecte les torrents bloques (stalled / metaDL) en queue
# Sonarr+Radarr depuis >=10 min et les remplace : retire de qBit, blocklist la
# release, declenche un nouveau search.
#
# Lance toutes les 5 min par qbit-stuck-handler.timer.

set -euo pipefail

# shellcheck disable=SC1091
[ -f /opt/homelab/.env ] && set -a && . /opt/homelab/.env && set +a

SONARR_URL="${SONARR_URL:-http://localhost:8989}"
SONARR_API_KEY="${SONARR_API_KEY:?missing SONARR_API_KEY}"

RADARR_URL="${RADARR_URL:-http://localhost:7878}"
RADARR_API_KEY="${RADARR_API_KEY:?missing RADARR_API_KEY}"

STALL_THRESHOLD=28800
MAX_ACTIONS_PER_RUN=5
STUCK_REGEX='stalled|metadata|no connections'

LOG_FILE="/opt/homelab/logs/qbit-stuck-handler.log"
STATE_FILE="/opt/homelab/logs/qbit-stuck-handler.state"
LOCK_FILE="/tmp/qbit-stuck-handler.lock"
LOCK_STALE_AFTER=900

mkdir -p "$(dirname "$LOG_FILE")"
touch "$STATE_FILE"

log() {
  printf '%s\t%b\n' "$(date -Iseconds)" "$*" >> "$LOG_FILE"
}

if [ -e "$LOCK_FILE" ]; then
  LOCK_AGE=$(( $(date +%s) - $(stat -c %Y "$LOCK_FILE" 2>/dev/null || echo 0) ))
  if [ "$LOCK_AGE" -lt "$LOCK_STALE_AFTER" ]; then
    log "skip\treason=lock_held\tage=${LOCK_AGE}s"
    exit 0
  fi
  log "warn\treason=stale_lock_removed\tage=${LOCK_AGE}s"
  rm -f "$LOCK_FILE"
fi
echo $$ > "$LOCK_FILE"
trap 'rm -f "$LOCK_FILE"' EXIT

NOW=$(date +%s)
ACTIONS_TAKEN=0
NEW_STATE=$(mktemp)
trap 'rm -f "$LOCK_FILE" "$NEW_STATE"' EXIT

process_service() {
  local service="$1" url="$2" key="$3"

  local queue
  if ! queue=$(curl -fsS -H "X-Api-Key: $key" "$url/api/v3/queue?pageSize=200&includeUnknownSeriesItems=false" 2>/dev/null); then
    log "warn\tservice=$service\treason=queue_unreachable"
    grep -P "^${service}\t" "$STATE_FILE" >> "$NEW_STATE" || true
    return 0
  fi

  local stuck_items
  stuck_items=$(echo "$queue" | jq -c --arg re "$STUCK_REGEX" '
    .records // []
    | map(select(.errorMessage != null and (.errorMessage | test($re; "i"))))
    | map({id, downloadId, title, errorMessage})
    | .[]
  ' 2>/dev/null || echo "")

  declare -A current_stuck=()

  while IFS= read -r item; do
    [ -z "$item" ] && continue
    local qid did title err
    qid=$(echo "$item" | jq -r '.id')
    did=$(echo "$item" | jq -r '.downloadId')
    title=$(echo "$item" | jq -r '.title')
    err=$(echo "$item" | jq -r '.errorMessage')
    [ -z "$did" ] || [ "$did" = "null" ] && continue

    current_stuck["$did"]=1

    local first_seen
    first_seen=$(awk -F'\t' -v s="$service" -v d="$did" '$1==s && $2==d { print $3; exit }' "$STATE_FILE")

    if [ -z "$first_seen" ]; then
      printf '%s\t%s\t%s\t%s\n' "$service" "$did" "$NOW" "$title" >> "$NEW_STATE"
      log "tracking\tservice=$service\thash=$did\ttitle=$title\terror=$err"
      continue
    fi

    local age=$(( NOW - first_seen ))

    if [ "$age" -lt "$STALL_THRESHOLD" ]; then
      printf '%s\t%s\t%s\t%s\n' "$service" "$did" "$first_seen" "$title" >> "$NEW_STATE"
      log "still_stuck\tservice=$service\thash=$did\tage=${age}s\ttitle=$title"
      continue
    fi

    if [ "$ACTIONS_TAKEN" -ge "$MAX_ACTIONS_PER_RUN" ]; then
      printf '%s\t%s\t%s\t%s\n' "$service" "$did" "$first_seen" "$title" >> "$NEW_STATE"
      log "deferred\tservice=$service\thash=$did\tage=${age}s\treason=rate_limit\ttitle=$title"
      continue
    fi

    local del_url="$url/api/v3/queue/$qid?removeFromClient=true&blocklist=true&skipRedownload=false&changeCategory=false"
    if curl -fsS -X DELETE -H "X-Api-Key: $key" "$del_url" >/dev/null 2>&1; then
      ACTIONS_TAKEN=$(( ACTIONS_TAKEN + 1 ))
      log "action=replaced\tservice=$service\tqueue_id=$qid\thash=$did\tage=${age}s\ttitle=$title"
    else
      printf '%s\t%s\t%s\t%s\n' "$service" "$did" "$first_seen" "$title" >> "$NEW_STATE"
      log "error\tservice=$service\treason=delete_failed\tqueue_id=$qid\thash=$did\ttitle=$title"
    fi
  done <<< "$stuck_items"

  while IFS=$'\t' read -r s did first_seen title; do
    [ "$s" = "$service" ] || continue
    if [ -z "${current_stuck[$did]:-}" ]; then
      log "cleared\tservice=$service\thash=$did\ttitle=$title"
    fi
  done < "$STATE_FILE"
}

process_service "sonarr" "$SONARR_URL" "$SONARR_API_KEY"
process_service "radarr" "$RADARR_URL" "$RADARR_API_KEY"

mv "$NEW_STATE" "$STATE_FILE"
trap 'rm -f "$LOCK_FILE"' EXIT

log "run_done\tactions=$ACTIONS_TAKEN\tstate_size=$(wc -l < "$STATE_FILE")"
