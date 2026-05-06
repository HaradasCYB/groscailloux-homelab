#!/bin/bash
# jellyseerr-user-poller.sh — detecte les users locaux Jellyseerr crees via UI
# (par exemple "Add User" depuis l'admin) et les remplace par un onboarding
# unifie via homelab-onboard-user.sh (Jellyfin + Jellyseerr-imported + mail custom).
#
# Lance toutes les 60s par jellyseerr-user-poller.timer.

set -euo pipefail

# shellcheck disable=SC1091
[ -f /opt/homelab/.env ] && set -a && . /opt/homelab/.env && set +a

JELLYSEERR_URL="${JELLYSEERR_URL:-http://localhost:5055}"
JELLYSEERR_API_KEY="${JELLYSEERR_API_KEY:?missing JELLYSEERR_API_KEY}"

JELLYFIN_URL="${JELLYFIN_URL:-http://localhost:8096}"
JELLYFIN_API_KEY="${JELLYFIN_API_KEY:?missing JELLYFIN_API_KEY}"

ONBOARD_SCRIPT="/opt/homelab/scripts/homelab-onboard-user.sh"

LOG_FILE="/opt/homelab/logs/jellyseerr-user-poller.log"
STATE_FILE="/opt/homelab/logs/jellyseerr-user-poller.state"
LOCK_FILE="/tmp/jellyseerr-user-poller.lock"
LOCK_STALE_AFTER=600

USERTYPE_LOCAL=2
ADMIN_USER_ID=1
MAX_AGE_SECONDS=600

mkdir -p "$(dirname "$LOG_FILE")"
touch "$STATE_FILE"

log() {
  printf '%s\t%b\n' "$(date -Iseconds)" "$*" >> "$LOG_FILE"
}

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

[ -x "$ONBOARD_SCRIPT" ] || { log "error\treason=onboard_script_missing"; exit 1; }

if ! curl -fsS -H "X-Api-Key: $JELLYSEERR_API_KEY" "$JELLYSEERR_URL/api/v1/status" >/dev/null 2>&1; then
  log "skip\treason=jellyseerr_unreachable"
  exit 0
fi

USERS=$(curl -fsS -H "X-Api-Key: $JELLYSEERR_API_KEY" "$JELLYSEERR_URL/api/v1/user?take=200") || {
  log "error\treason=user_list_failed"
  exit 1
}

NOW=$(date +%s)
MIN_CREATED=$(( NOW - MAX_AGE_SECONDS ))

CANDIDATES=$(echo "$USERS" | jq -c --argjson admin "$ADMIN_USER_ID" --argjson minc "$MIN_CREATED" --argjson lt "$USERTYPE_LOCAL" '
  .results[]
  | select(
      .userType == $lt
      and .id != $admin
      and ((.email // "") | test("^[^@\\s]+@[^@\\s]+\\.[^@\\s]+$"))
      and ((.createdAt | sub("\\.[0-9]+Z$"; "Z") | fromdateiso8601) >= $minc)
    )
  | {id, email, username, jellyfinUsername, createdAt}
')

[ -z "$CANDIDATES" ] && exit 0

while IFS= read -r user; do
  [ -z "$user" ] && continue

  ID=$(echo "$user" | jq -r '.id')
  EMAIL=$(echo "$user" | jq -r '.email')
  USERNAME=$(echo "$user" | jq -r '.username // .jellyfinUsername // empty')

  if grep -qF "$EMAIL" "$STATE_FILE" 2>/dev/null; then
    continue
  fi

  if [ -z "$USERNAME" ]; then
    USERNAME=$(echo "$EMAIL" | cut -d'@' -f1)
  fi

  if curl -fsS -H "X-Emby-Token: $JELLYFIN_API_KEY" "$JELLYFIN_URL/Users" \
     | jq -e --arg n "$USERNAME" '.[] | select((.Name | ascii_downcase) == ($n | ascii_downcase))' >/dev/null 2>&1; then
    log "skip\treason=jellyfin_user_already_exists\tusername=$USERNAME\tid=$ID"
    printf '%s\t%s\tskip_existing_jf\n' "$EMAIL" "$NOW" >> "$STATE_FILE"
    continue
  fi

  log "detected\tid=$ID\tusername=$USERNAME\temail=$EMAIL"

  if curl -fsS -X DELETE -H "X-Api-Key: $JELLYSEERR_API_KEY" "$JELLYSEERR_URL/api/v1/user/$ID" >/dev/null 2>&1; then
    log "deleted_local\tid=$ID"
  else
    log "error\treason=delete_local_failed\tid=$ID"
    continue
  fi

  if "$ONBOARD_SCRIPT" "$USERNAME" "$EMAIL" >> "$LOG_FILE" 2>&1; then
    log "onboarded\tusername=$USERNAME\temail=$EMAIL"
    printf '%s\t%s\tonboarded\n' "$EMAIL" "$NOW" >> "$STATE_FILE"
  else
    log "error\treason=onboard_failed\tusername=$USERNAME\temail=$EMAIL"
    printf '%s\t%s\tonboard_failed\n' "$EMAIL" "$NOW" >> "$STATE_FILE"
  fi
done <<< "$CANDIDATES"
