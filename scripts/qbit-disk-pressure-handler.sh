#!/bin/bash
# qbit-disk-pressure-handler.sh — degradation gracieuse du seed quand le disque sature
# Strategy:
#   < 95%  : nothing (log only)
#   95-98% : "hard" — supprime du qBit les torrents deja en pause les plus anciens
#                     avec deleteFiles=true (libere /downloads, hardlink /media survit)
#   > 98%  : critical — log alerte, pas d'action auto
#
# IMPORTANT: ne touche jamais un torrent en seeding actif (uploading/stalledUP/forcedUP) —
# seuls les torrents deja en pause (pausedUP/stoppedUP) sont candidats a la suppression.
#
# Lance toutes les 15 min par qbit-disk-pressure-handler.timer.

set -euo pipefail

QBIT_URL="http://localhost:8080"
TARGET_PATH="/opt/homelab"

HARD_THRESHOLD=95
CRIT_THRESHOLD=98

MAX_ACTIONS_PER_RUN=5

LOG_FILE="/opt/homelab/logs/qbit-disk-pressure-handler.log"
LOCK_FILE="/tmp/qbit-disk-pressure-handler.lock"
LOCK_STALE_AFTER=1800

mkdir -p "$(dirname "$LOG_FILE")"

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

USE_PCT=$(df -P "$TARGET_PATH" | awk 'NR==2 {gsub("%",""); print $5}')
[ -n "$USE_PCT" ] || { log "error\treason=df_failed"; exit 1; }

if [ "$USE_PCT" -lt "$HARD_THRESHOLD" ]; then
  log "ok\tuse_pct=${USE_PCT}\tno_action"
  exit 0
fi

if ! curl -fsS "$QBIT_URL/api/v2/app/version" >/dev/null 2>&1; then
  log "error\treason=qbit_unreachable\tuse_pct=${USE_PCT}"
  exit 1
fi

TORRENTS=$(curl -fsS "$QBIT_URL/api/v2/torrents/info" 2>/dev/null) || {
  log "error\treason=torrents_info_failed\tuse_pct=${USE_PCT}"
  exit 1
}

if [ "$USE_PCT" -ge "$CRIT_THRESHOLD" ]; then
  log "critical\tuse_pct=${USE_PCT}\tno_auto_action_required\thint=manual_cleanup_needed"
  exit 0
fi

ACTIONS_TAKEN=0

if [ "$USE_PCT" -ge "$HARD_THRESHOLD" ]; then
  log "hard\tuse_pct=${USE_PCT}\tphase=delete_paused_torrents"

  HASHES_TO_DELETE=$(echo "$TORRENTS" | jq -r --argjson n "$MAX_ACTIONS_PER_RUN" '
    [.[] | select(.state == "stoppedUP")]
    | sort_by(.completion_on)
    | .[0:$n]
    | .[]
    | "\(.hash)\t\(.name)\t\(.size)\t\(.completion_on)"
  ')

  if [ -z "$HASHES_TO_DELETE" ]; then
    log "hard\treason=no_paused_torrents_to_delete"
  else
    HASH_LIST=$(echo "$HASHES_TO_DELETE" | awk -F'\t' '{print $1}' | paste -sd '|')
    if curl -fsS -X POST "$QBIT_URL/api/v2/torrents/delete" \
         --data-urlencode "hashes=$HASH_LIST" \
         --data-urlencode "deleteFiles=true" >/dev/null 2>&1; then
      while IFS=$'\t' read -r hash name size completed; do
        ACTIONS_TAKEN=$(( ACTIONS_TAKEN + 1 ))
        log "action=deleted\thash=$hash\tsize=$size\tcompleted=$completed\tname=$name"
      done <<< "$HASHES_TO_DELETE"
    else
      log "error\treason=delete_call_failed\thashes=$HASH_LIST"
    fi
  fi
fi

log "run_done\tuse_pct=${USE_PCT}\tactions=$ACTIONS_TAKEN"
