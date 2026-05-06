#!/bin/bash
# 02-radarr-init.sh — replay des Custom Formats + Quality Profile FR-friendly
# de l'auteur original, via API Radarr REST.
#
# Prerequis :
#   - Radarr UP (port 7878)
#   - RADARR_API_KEY dans /opt/homelab/.env
#   - Movies root folder configure dans Radarr
#
# Idempotent : si un CF/profile existe deja, skip.

set -euo pipefail

# shellcheck disable=SC1091
[ -f /opt/homelab/.env ] && set -a && . /opt/homelab/.env && set +a

RADARR_URL="${RADARR_URL:-http://localhost:7878}"
RADARR_API_KEY="${RADARR_API_KEY:?missing RADARR_API_KEY}"

DATA_DIR="$(cd "$(dirname "$0")" && pwd)/data"

if ! curl -fsS -H "X-Api-Key: $RADARR_API_KEY" "$RADARR_URL/api/v3/system/status" >/dev/null; then
  echo "Erreur: Radarr injoignable sur $RADARR_URL ou cle API invalide" >&2
  exit 1
fi

echo "==> 1. Custom Formats Radarr"

while read -r cf; do
  NAME=$(echo "$cf" | jq -r '.name')
  EXISTING=$(curl -fsS -H "X-Api-Key: $RADARR_API_KEY" "$RADARR_URL/api/v3/customformat" \
    | jq -r --arg n "$NAME" '.[] | select(.name==$n) | .id // empty')
  if [ -n "$EXISTING" ]; then
    echo "    skip: $NAME (id=$EXISTING)"
  else
    RESP=$(curl -fsS -X POST -H "X-Api-Key: $RADARR_API_KEY" -H "Content-Type: application/json" \
      -d "$cf" "$RADARR_URL/api/v3/customformat" || echo '{}')
    if echo "$RESP" | jq -e '.id' >/dev/null 2>&1; then
      echo "    created: $NAME (id=$(echo "$RESP" | jq -r '.id'))"
    else
      echo "    error: $NAME"
    fi
  fi
done < <(jq -c '.[]' "$DATA_DIR/radarr-customformats.json")

echo ""
echo "==> 2. Quality Profile 'FR-friendly H.264'"

CF_MAP=$(curl -fsS -H "X-Api-Key: $RADARR_API_KEY" "$RADARR_URL/api/v3/customformat" \
  | jq 'map({key: .name, value: .id}) | from_entries')

while read -r profile; do
  PROFILE_PAYLOAD=$(echo "$profile" | jq --argjson map "$CF_MAP" '
    .formatItems |= map(.format = ($map[.name] // 0))
  ')
  PROFILE_NAME=$(echo "$PROFILE_PAYLOAD" | jq -r '.name')
  EXISTING_PROFILE=$(curl -fsS -H "X-Api-Key: $RADARR_API_KEY" "$RADARR_URL/api/v3/qualityprofile" \
    | jq -r --arg n "$PROFILE_NAME" '.[] | select(.name==$n) | .id // empty')

  if [ -n "$EXISTING_PROFILE" ]; then
    PAYLOAD=$(echo "$PROFILE_PAYLOAD" | jq --argjson id "$EXISTING_PROFILE" '.id=$id')
    if curl -fsS -X PUT -H "X-Api-Key: $RADARR_API_KEY" -H "Content-Type: application/json" \
         -d "$PAYLOAD" "$RADARR_URL/api/v3/qualityprofile/$EXISTING_PROFILE" >/dev/null; then
      echo "    updated: $PROFILE_NAME (id=$EXISTING_PROFILE)"
    else
      echo "    error updating $PROFILE_NAME"
    fi
  else
    RESP=$(curl -fsS -X POST -H "X-Api-Key: $RADARR_API_KEY" -H "Content-Type: application/json" \
      -d "$PROFILE_PAYLOAD" "$RADARR_URL/api/v3/qualityprofile" || echo '{}')
    if echo "$RESP" | jq -e '.id' >/dev/null 2>&1; then
      echo "    created: $PROFILE_NAME (id=$(echo "$RESP" | jq -r '.id'))"
    else
      echo "    error creating $PROFILE_NAME"
    fi
  fi
done < <(jq -c '.[]' "$DATA_DIR/radarr-qualityprofile.json")

echo ""
echo "✓ Radarr init done."
echo "  Verifier dans l'UI : Settings > Custom Formats / Quality Profiles"
