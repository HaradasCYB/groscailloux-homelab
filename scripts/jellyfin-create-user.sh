#!/bin/bash
# jellyfin-create-user.sh — crée un user Jellyfin non-admin via API
# Usage:
#   /opt/homelab/scripts/jellyfin-create-user.sh <username> [password]
# Si password est omis, un mot de passe aléatoire de 16 caractères est généré.

set -euo pipefail
[ -f /opt/homelab/.env ] && { set -a; . /opt/homelab/.env; set +a; }

JELLYFIN_URL="${JELLYFIN_URL:-http://localhost:8096}"
JELLYFIN_PUBLIC_URL="${JELLYFIN_PUBLIC_URL:-https://jellyfin.groscaillouxmovie.duckdns.org}"
API_KEY="${JELLYFIN_API_KEY:-${JELLYFIN_API_KEY}}"

LIB_FILMS="${JELLYFIN_LIB_FILMS}"
LIB_SERIES="${JELLYFIN_LIB_SERIES}"

usage() {
  echo "Usage: $0 <username> [password]" >&2
  exit 2
}

[ $# -ge 1 ] && [ $# -le 2 ] || usage
USERNAME="$1"
PASSWORD="${2:-}"

if [ -z "$PASSWORD" ]; then
  PASSWORD=$(openssl rand -base64 16 | tr -d '=+/' | cut -c1-16)
fi

command -v jq >/dev/null || { echo "Erreur: jq requis"; exit 1; }
command -v curl >/dev/null || { echo "Erreur: curl requis"; exit 1; }

AUTH_HEADER="X-Emby-Token: $API_KEY"

# 1. Pré-check : Jellyfin répond + clé API valide
SYS_INFO=$(curl -fsS -H "$AUTH_HEADER" "$JELLYFIN_URL/System/Info" 2>/dev/null) || {
  echo "Erreur: Jellyfin ne répond pas sur $JELLYFIN_URL ou clé API invalide" >&2
  exit 1
}
SERVER_NAME=$(echo "$SYS_INFO" | jq -r '.ServerName // "?"')
echo "→ Jellyfin OK ($SERVER_NAME)"

# 2. Pré-check : user n'existe pas déjà (case-insensitive)
EXISTS=$(curl -fsS -H "$AUTH_HEADER" "$JELLYFIN_URL/Users" \
  | jq -r --arg n "$USERNAME" '.[] | select((.Name | ascii_downcase) == ($n | ascii_downcase)) | .Id')
if [ -n "$EXISTS" ]; then
  echo "Erreur: l'utilisateur '$USERNAME' existe déjà (Id=$EXISTS). Suppression manuelle requise avant recréation." >&2
  exit 1
fi

# 3. Création
CREATE_RESP=$(curl -fsS -X POST \
  -H "$AUTH_HEADER" \
  -H "Content-Type: application/json" \
  -d "$(jq -n --arg u "$USERNAME" --arg p "$PASSWORD" '{Name:$u, Password:$p}')" \
  "$JELLYFIN_URL/Users/New") || {
  echo "Erreur: échec POST /Users/New" >&2
  exit 1
}
USER_ID=$(echo "$CREATE_RESP" | jq -r '.Id')
[ -n "$USER_ID" ] && [ "$USER_ID" != "null" ] || {
  echo "Erreur: pas d'Id retourné par Jellyfin" >&2
  exit 1
}
echo "→ User créé (Id=$USER_ID)"

# 4. Application de la policy non-admin
POLICY=$(jq -n --arg f1 "$LIB_FILMS" --arg f2 "$LIB_SERIES" '{
  IsAdministrator: false,
  IsHidden: false,
  IsDisabled: false,
  EnableUserPreferenceAccess: true,
  EnableRemoteAccess: true,
  EnableMediaPlayback: true,
  EnableAudioPlaybackTranscoding: true,
  EnableVideoPlaybackTranscoding: true,
  EnablePlaybackRemuxing: true,
  EnableLiveTvAccess: false,
  EnableLiveTvManagement: false,
  EnableContentDeletion: false,
  EnableContentDownloading: false,
  EnableSyncTranscoding: true,
  EnableSubtitleManagement: false,
  EnableAllDevices: true,
  EnableAllChannels: false,
  EnableAllFolders: false,
  EnabledFolders: [$f1, $f2],
  EnabledChannels: [],
  EnabledDevices: [],
  BlockedTags: [],
  BlockedChannels: [],
  BlockedMediaFolders: [],
  AccessSchedules: [],
  LoginAttemptsBeforeLockout: 5,
  MaxActiveSessions: 0,
  AuthenticationProviderId: "Jellyfin.Server.Implementations.Users.DefaultAuthenticationProvider",
  PasswordResetProviderId: "Jellyfin.Server.Implementations.Users.DefaultPasswordResetProvider",
  SyncPlayAccess: "CreateAndJoinGroups"
}')

if curl -fsS -X POST \
  -H "$AUTH_HEADER" \
  -H "Content-Type: application/json" \
  -d "$POLICY" \
  "$JELLYFIN_URL/Users/$USER_ID/Policy" >/dev/null; then
  echo "→ Policy appliquée (Films + Séries, non-admin)"
else
  echo "Avertissement: policy non appliquée — à configurer manuellement dans le Dashboard Jellyfin" >&2
fi

# 5. Sortie creds (à transmettre à l'utilisateur)
cat <<EOF

✓ Compte Jellyfin créé
  URL:      $JELLYFIN_PUBLIC_URL
  Username: $USERNAME
  Password: $PASSWORD

→ Pour Jellyseerr, l'utilisateur clique l'onglet "Use your Jellyfin account"
  sur https://jellyseerr.groscaillouxmovie.duckdns.org/login
  et se connecte avec ces mêmes identifiants. Jellyseerr l'importera automatiquement.
EOF
