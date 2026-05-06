#!/bin/bash
# 03-jellyseerr-init.sh — configure Jellyseerr post-deploy :
#   - Active l'agent email avec les creds .env
#   - Definit les permissions par defaut (Request basique)
#   - Active localLogin (pour cas d'edge) + mediaServerLogin
#
# Prerequis :
#   - Jellyseerr UP (port 5055), assistant initial complet (Jellyfin lie)
#   - JELLYSEERR_API_KEY dans /opt/homelab/.env
#   - SMTP_* dans .env (Gmail App Password)
#
# Idempotent.

set -euo pipefail

# shellcheck disable=SC1091
[ -f /opt/homelab/.env ] && set -a && . /opt/homelab/.env && set +a

JELLYSEERR_URL="${JELLYSEERR_URL:-http://localhost:5055}"
JELLYSEERR_API_KEY="${JELLYSEERR_API_KEY:?missing JELLYSEERR_API_KEY}"

if ! curl -fsS -H "X-Api-Key: $JELLYSEERR_API_KEY" "$JELLYSEERR_URL/api/v1/status" >/dev/null; then
  echo "Erreur: Jellyseerr injoignable sur $JELLYSEERR_URL ou cle API invalide" >&2
  exit 1
fi

echo "==> 1. SMTP / agent email"

EMAIL_PAYLOAD=$(jq -n \
  --arg from "${SMTP_FROM:?missing SMTP_FROM}" \
  --arg host "${SMTP_HOST:-smtp.gmail.com}" \
  --argjson port "${SMTP_PORT:-465}" \
  --arg user "${SMTP_USER:?missing SMTP_USER}" \
  --arg pass "${SMTP_PASS:?missing SMTP_PASS}" \
  --arg name "${SMTP_FROM_NAME:-Homelab}" '
{
  enabled: true,
  embedPoster: true,
  options: {
    userEmailRequired: false,
    emailFrom: $from,
    smtpHost: $host,
    smtpPort: $port,
    secure: ($port == 465),
    ignoreTls: false,
    requireTls: ($port == 587),
    allowSelfSigned: false,
    senderName: $name,
    authUser: $user,
    authPass: $pass
  }
}')

if curl -fsS -X POST -H "X-Api-Key: $JELLYSEERR_API_KEY" -H "Content-Type: application/json" \
     -d "$EMAIL_PAYLOAD" "$JELLYSEERR_URL/api/v1/settings/notifications/email" >/dev/null; then
  echo "    SMTP configure (host=$SMTP_HOST:$SMTP_PORT)"
else
  echo "    error configuring SMTP" >&2
fi

echo ""
echo "==> 2. Test SMTP"
TEST_RESP=$(curl -sS -X POST -H "X-Api-Key: $JELLYSEERR_API_KEY" -H "Content-Type: application/json" \
  -d "$EMAIL_PAYLOAD" "$JELLYSEERR_URL/api/v1/settings/notifications/email/test" || echo "ERROR")
echo "    response: $TEST_RESP"

echo ""
echo "==> 3. Defaut permissions / login mode (settings.main)"

MAIN_PAYLOAD=$(jq -n '
{
  defaultPermissions: 32,
  localLogin: true,
  mediaServerLogin: true,
  newPlexLogin: true,
  partialRequestsEnabled: true,
  enableSpecialEpisodes: true
}')

if curl -fsS -X POST -H "X-Api-Key: $JELLYSEERR_API_KEY" -H "Content-Type: application/json" \
     -d "$MAIN_PAYLOAD" "$JELLYSEERR_URL/api/v1/settings/main" >/dev/null; then
  echo "    main settings updated (defaultPermissions=32, localLogin=true)"
else
  echo "    error updating main settings"
fi

echo ""
echo "✓ Jellyseerr init done."
echo "  Verifier dans l'UI : Settings > Notifications > Email + Settings > Users"
