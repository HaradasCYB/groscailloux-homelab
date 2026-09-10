#!/bin/bash
# qbit-tracker-ratio-policy.sh — applique des share limits per-torrent selon le tracker
# Mapping :
#   *c411.org*               → ratio=-1, time=-1   (private prio max, seed indefini)
#   *yggleak* / *u2p*        → ratio=2.0, time=14d (private secondaire)
#   trackers publics connus  → ratio=2.0, time=14d
#   defaut                   → ratio=1.0, time=7d
#
# Auth qBit : aucune, whitelist 127.0.0.1/8 + 172.18.0.0/16 deja en place.
# Lance toutes les 30 min par qbit-tracker-ratio-policy.timer.

set -euo pipefail

QBIT_URL="http://localhost:8080"

LOG_FILE="/opt/homelab/logs/qbit-tracker-ratio-policy.log"
LOCK_FILE="/tmp/qbit-tracker-ratio-policy.lock"
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
  rm -f "$LOCK_FILE"
fi
echo $$ > "$LOCK_FILE"
trap 'rm -f "$LOCK_FILE"' EXIT

if ! curl -fsS "$QBIT_URL/api/v2/app/version" >/dev/null 2>&1; then
  log "error\treason=qbit_unreachable"
  exit 1
fi

TORRENTS=$(curl -fsS "$QBIT_URL/api/v2/torrents/info") || {
  log "error\treason=torrents_info_failed"
  exit 1
}

UNLIMITED_RATIO=-1
UNLIMITED_TIME=-1
PUBLIC_RATIO=2.0
PUBLIC_TIME=20160
DEFAULT_RATIO=1.0
DEFAULT_TIME=10080

CHANGES=0

while IFS=$'\t' read -r hash tracker name cur_ratio cur_time; do
  [ -z "$hash" ] && continue

  case "$tracker" in
    *c411.org*)
      target_ratio="$UNLIMITED_RATIO"; target_time="$UNLIMITED_TIME"; tier="c411"
      ;;
    *yggleak*|*u2p*|*ygg.gratis*)
      target_ratio="$PUBLIC_RATIO"; target_time="$PUBLIC_TIME"; tier="ygg"
      ;;
    *opentrackr*|*demonii*|*exodus.desync*|*open.stealth*|*tracker.torrent.eu*|*tracker.theoks*|*explodie.org*|*leet-tracker*|*tracker.dler*|*tracker.filemail*|*tracker.opentrackr*|*tracker.alaskantf*|*tracker-udp.gbitt*|*overflow.biz*|*open.dstud*|*tracker.srv00*|*tracker1.myporn*|*durukanbal*|*encrypt.net*|*corpscorp*|*6ahddutb*)
      target_ratio="$PUBLIC_RATIO"; target_time="$PUBLIC_TIME"; tier="public"
      ;;
    *)
      target_ratio="$DEFAULT_RATIO"; target_time="$DEFAULT_TIME"; tier="default"
      ;;
  esac

  cur_ratio_norm=$(printf '%.2f' "$cur_ratio" 2>/dev/null || echo "0")
  target_ratio_norm=$(printf '%.2f' "$target_ratio" 2>/dev/null || echo "0")

  if [ "$cur_ratio_norm" = "$target_ratio_norm" ] && [ "$cur_time" = "$target_time" ]; then
    continue
  fi

  if curl -fsS -X POST "$QBIT_URL/api/v2/torrents/setShareLimits" \
       --data-urlencode "hashes=$hash" \
       --data-urlencode "ratioLimit=$target_ratio" \
       --data-urlencode "seedingTimeLimit=$target_time" \
       --data-urlencode "inactiveSeedingTimeLimit=-1" >/dev/null 2>&1; then
    CHANGES=$(( CHANGES + 1 ))
    log "applied\ttier=$tier\tratio=$target_ratio\ttime=$target_time\thash=$hash\tname=${name:0:60}"
  else
    log "error\treason=set_limits_failed\thash=$hash\tname=${name:0:60}"
  fi
done < <(echo "$TORRENTS" | jq -r '.[] | [.hash, (.tracker // ""), .name, (.ratio_limit // -2), (.seeding_time_limit // -2)] | @tsv')

log "run_done\tchanges=$CHANGES\ttotal_torrents=$(echo "$TORRENTS" | jq 'length')"
