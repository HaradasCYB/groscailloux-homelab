#!/bin/bash
# 01-sonarr-init.sh — replay des Custom Formats + Quality Profile + Release Profiles
# de l'auteur original, via API Sonarr REST.
#
# Prerequis :
#   - Sonarr UP (port 8989)
#   - SONARR_API_KEY dans /opt/homelab/.env
#   - Series root folder configure dans Sonarr (Settings > Media Management > Root Folders)
#
# Idempotent : si un CF/profile existe deja, skip.

set -euo pipefail

# shellcheck disable=SC1091
[ -f /opt/homelab/.env ] && set -a && . /opt/homelab/.env && set +a

SONARR_URL="${SONARR_URL:-http://localhost:8989}"
SONARR_API_KEY="${SONARR_API_KEY:?missing SONARR_API_KEY}"

DATA_DIR="$(cd "$(dirname "$0")" && pwd)/data"

if ! curl -fsS -H "X-Api-Key: $SONARR_API_KEY" "$SONARR_URL/api/v3/system/status" >/dev/null; then
  echo "Erreur: Sonarr injoignable sur $SONARR_URL ou cle API invalide" >&2
  exit 1
fi

echo "==> 1. Custom Formats Sonarr"

while read -r cf; do
  NAME=$(echo "$cf" | jq -r '.name')
  EXISTING=$(curl -fsS -H "X-Api-Key: $SONARR_API_KEY" "$SONARR_URL/api/v3/customformat" \
    | jq -r --arg n "$NAME" '.[] | select(.name==$n) | .id // empty')
  if [ -n "$EXISTING" ]; then
    echo "    skip: $NAME (id=$EXISTING)"
  else
    RESP=$(curl -fsS -X POST -H "X-Api-Key: $SONARR_API_KEY" -H "Content-Type: application/json" \
      -d "$cf" "$SONARR_URL/api/v3/customformat" || echo '{}')
    if echo "$RESP" | jq -e '.id' >/dev/null 2>&1; then
      echo "    created: $NAME (id=$(echo "$RESP" | jq -r '.id'))"
    else
      echo "    error: $NAME"
    fi
  fi
done < <(jq -c '.[]' "$DATA_DIR/sonarr-customformats.json")

echo ""
echo "==> 2. Quality Profile 'FR-friendly H.264'"

# Build name->id map for current CFs
CF_MAP=$(curl -fsS -H "X-Api-Key: $SONARR_API_KEY" "$SONARR_URL/api/v3/customformat" \
  | jq 'map({key: .name, value: .id}) | from_entries')

# Update formatItems[].format with the local CF ids
PROFILE_PAYLOAD=$(jq --argjson map "$CF_MAP" '
  .formatItems |= map(.format = ($map[.name] // 0))
' "$DATA_DIR/sonarr-qualityprofile-6.json")

PROFILE_NAME=$(echo "$PROFILE_PAYLOAD" | jq -r '.name')
EXISTING_PROFILE=$(curl -fsS -H "X-Api-Key: $SONARR_API_KEY" "$SONARR_URL/api/v3/qualityprofile" \
  | jq -r --arg n "$PROFILE_NAME" '.[] | select(.name==$n) | .id // empty')

if [ -n "$EXISTING_PROFILE" ]; then
  PAYLOAD=$(echo "$PROFILE_PAYLOAD" | jq --argjson id "$EXISTING_PROFILE" '.id=$id')
  if curl -fsS -X PUT -H "X-Api-Key: $SONARR_API_KEY" -H "Content-Type: application/json" \
       -d "$PAYLOAD" "$SONARR_URL/api/v3/qualityprofile/$EXISTING_PROFILE" >/dev/null; then
    echo "    updated: $PROFILE_NAME (id=$EXISTING_PROFILE)"
  else
    echo "    error updating $PROFILE_NAME"
  fi
else
  RESP=$(curl -fsS -X POST -H "X-Api-Key: $SONARR_API_KEY" -H "Content-Type: application/json" \
    -d "$PROFILE_PAYLOAD" "$SONARR_URL/api/v3/qualityprofile" || echo '{}')
  if echo "$RESP" | jq -e '.id' >/dev/null 2>&1; then
    echo "    created: $PROFILE_NAME (id=$(echo "$RESP" | jq -r '.id'))"
  else
    echo "    error creating $PROFILE_NAME"
  fi
fi

echo ""
echo "==> 3. Release Profiles Sonarr"

while read -r rp; do
  NAME=$(echo "$rp" | jq -r '.name // "unnamed"')
  EXISTING=$(curl -fsS -H "X-Api-Key: $SONARR_API_KEY" "$SONARR_URL/api/v3/releaseprofile" \
    | jq -r --arg n "$NAME" '.[] | select(.name==$n) | .id // empty')
  if [ -n "$EXISTING" ]; then
    echo "    skip: $NAME (id=$EXISTING)"
  else
    RESP=$(curl -fsS -X POST -H "X-Api-Key: $SONARR_API_KEY" -H "Content-Type: application/json" \
      -d "$rp" "$SONARR_URL/api/v3/releaseprofile" || echo '{}')
    if echo "$RESP" | jq -e '.id' >/dev/null 2>&1; then
      echo "    created: $NAME"
    else
      echo "    skip (requires tag setup): $NAME"
    fi
  fi
done < <(jq -c '.[]' "$DATA_DIR/sonarr-releaseprofiles.json")

echo ""
echo "✓ Sonarr init done."
echo "  Verifier dans l'UI : Settings > Custom Formats / Quality Profiles"
