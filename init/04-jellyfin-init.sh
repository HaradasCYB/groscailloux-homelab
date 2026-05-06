#!/bin/bash
# 04-jellyfin-init.sh — verifie que les libraries Jellyfin Films + Series sont creees
# et expose leur GUID pour mise a jour de .env (JELLYFIN_LIB_FILMS / JELLYFIN_LIB_SERIES).
#
# Ne CREE PAS les libraries automatiquement (Jellyfin l'API library setup est complexe
# et il vaut mieux le faire via UI Dashboard une fois). Affiche juste les ids.
#
# Prerequis :
#   - Jellyfin UP (port 8096), assistant initial complet
#   - JELLYFIN_API_KEY dans /opt/homelab/.env
#   - Libraries "Films" (path /media/movies) et "Series" (path /media/tvshows) creees via UI

set -euo pipefail

# shellcheck disable=SC1091
[ -f /opt/homelab/.env ] && set -a && . /opt/homelab/.env && set +a

JELLYFIN_URL="${JELLYFIN_URL:-http://localhost:8096}"
JELLYFIN_API_KEY="${JELLYFIN_API_KEY:?missing JELLYFIN_API_KEY}"

if ! curl -fsS -H "X-Emby-Token: $JELLYFIN_API_KEY" "$JELLYFIN_URL/System/Info" >/dev/null; then
  echo "Erreur: Jellyfin injoignable sur $JELLYFIN_URL ou cle API invalide" >&2
  exit 1
fi

echo "==> Libraries Jellyfin actuelles"

LIBS=$(curl -fsS -H "X-Emby-Token: $JELLYFIN_API_KEY" \
  "$JELLYFIN_URL/Library/VirtualFolders")

echo "$LIBS" | jq -r '.[] | "  - \(.Name)\n      ItemId: \(.ItemId)\n      CollectionType: \(.CollectionType // "mixed")\n      Locations: \(.Locations | join(", "))"'

echo ""
echo "==> Suggestion .env"
echo ""

FILMS_ID=$(echo "$LIBS" | jq -r '.[] | select(.Name=="Films" or .CollectionType=="movies") | .ItemId' | head -1)
SERIES_ID=$(echo "$LIBS" | jq -r '.[] | select(.Name=="Series" or .Name=="Séries" or .CollectionType=="tvshows") | .ItemId' | head -1)

if [ -n "$FILMS_ID" ]; then
  echo "    JELLYFIN_LIB_FILMS=$FILMS_ID"
else
  echo "    Films library non trouvee — la creer via UI Dashboard > Libraries puis relancer"
fi

if [ -n "$SERIES_ID" ]; then
  echo "    JELLYFIN_LIB_SERIES=$SERIES_ID"
else
  echo "    Series library non trouvee — la creer via UI Dashboard > Libraries puis relancer"
fi

echo ""
echo "✓ Jellyfin init done. Mettre a jour /opt/homelab/.env avec les valeurs ci-dessus."
