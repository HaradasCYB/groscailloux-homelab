#!/bin/bash
# homelab-onboard-user.sh — onboarding unifie Jellyfin + Jellyseerr
# Usage:
#   /opt/homelab/scripts/homelab-onboard-user.sh <username> <email> [password]
# Si password est omis, un mot de passe aleatoire de 16 caracteres est genere.
#
# Cree le compte Jellyfin (source de verite du mdp) puis l'importe dans Jellyseerr
# en tant que user "Jellyfin-imported". Aucun mdp local cote Jellyseerr.
# Envoie un mail unifie avec les creds.

set -euo pipefail
[ -f /opt/homelab/.env ] && { set -a; . /opt/homelab/.env; set +a; }

JELLYFIN_URL="${JELLYFIN_URL:-http://localhost:8096}"
JELLYFIN_PUBLIC_URL="${JELLYFIN_PUBLIC_URL:-https://jellyfin.groscaillouxmovie.duckdns.org}"
JELLYFIN_API_KEY="${JELLYFIN_API_KEY:-${JELLYFIN_API_KEY}}"

JELLYSEERR_URL="${JELLYSEERR_URL:-http://localhost:5055}"
JELLYSEERR_PUBLIC_URL="${JELLYSEERR_PUBLIC_URL:-https://jellyseerr.groscaillouxmovie.duckdns.org}"
JELLYSEERR_API_KEY="${JELLYSEERR_API_KEY:-${JELLYSEERR_API_KEY}}"

SMTP_HOST="smtp.gmail.com"
SMTP_PORT="465"
SMTP_USER="${SMTP_USER}"
SMTP_PASS="${SMTP_PASS}"
SMTP_FROM="${SMTP_USER}"
SMTP_FROM_NAME="Jellyseerr Groscailloux"

JELLYFIN_USERTYPE=3

usage() {
  echo "Usage: $0 <username> <email> [password]" >&2
  exit 2
}

[ $# -ge 2 ] && [ $# -le 3 ] || usage
USERNAME="$1"
EMAIL="$2"
PASSWORD="${3:-}"

if [[ ! "$EMAIL" =~ ^[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}$ ]]; then
  echo "Erreur: email invalide '$EMAIL'" >&2
  exit 1
fi

if [ -z "$PASSWORD" ]; then
  PASSWORD=$(openssl rand -base64 16 | tr -d '=+/' | cut -c1-16)
fi

for cmd in curl jq openssl; do
  command -v "$cmd" >/dev/null || { echo "Erreur: $cmd requis" >&2; exit 1; }
done

JF_HDR="X-Emby-Token: $JELLYFIN_API_KEY"
JS_HDR="X-Api-Key: $JELLYSEERR_API_KEY"

echo "==> Pre-checks"
curl -fsS -H "$JF_HDR" "$JELLYFIN_URL/System/Info" >/dev/null \
  || { echo "Erreur: Jellyfin injoignable ou cle invalide" >&2; exit 1; }
curl -fsS -H "$JS_HDR" "$JELLYSEERR_URL/api/v1/status" >/dev/null \
  || { echo "Erreur: Jellyseerr injoignable ou cle invalide" >&2; exit 1; }

EXISTING_JF=$(curl -fsS -H "$JF_HDR" "$JELLYFIN_URL/Users" \
  | jq -r --arg n "$USERNAME" '.[] | select((.Name | ascii_downcase) == ($n | ascii_downcase)) | .Id')
[ -n "$EXISTING_JF" ] && { echo "Erreur: user Jellyfin '$USERNAME' existe deja (Id=$EXISTING_JF)" >&2; exit 1; }

EXISTING_JS_EMAIL=$(curl -fsS -H "$JS_HDR" "$JELLYSEERR_URL/api/v1/user?take=1000" \
  | jq -r --arg e "$EMAIL" '.results[] | select((.email // "" | ascii_downcase) == ($e | ascii_downcase)) | .id')
[ -n "$EXISTING_JS_EMAIL" ] && { echo "Erreur: email '$EMAIL' deja utilise cote Jellyseerr (id=$EXISTING_JS_EMAIL)" >&2; exit 1; }

echo "    OK Jellyfin/Jellyseerr UP, username et email libres"

echo "==> Creation Jellyfin"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
"$SCRIPT_DIR/jellyfin-create-user.sh" "$USERNAME" "$PASSWORD" >/dev/null \
  || { echo "Erreur: jellyfin-create-user.sh a echoue" >&2; exit 1; }

JELLYFIN_USER_ID=$(curl -fsS -H "$JF_HDR" "$JELLYFIN_URL/Users" \
  | jq -r --arg n "$USERNAME" '.[] | select(.Name == $n) | .Id')
[ -n "$JELLYFIN_USER_ID" ] || { echo "Erreur: user Jellyfin cree mais Id introuvable" >&2; exit 1; }
echo "    Jellyfin user Id: $JELLYFIN_USER_ID"

echo "==> Import dans Jellyseerr"
IMPORT_BODY=$(jq -n --arg id "$JELLYFIN_USER_ID" '{jellyfinUserIds:[$id]}')
IMPORT_RESP=$(curl -fsS -X POST -H "$JS_HDR" -H "Content-Type: application/json" \
  -d "$IMPORT_BODY" "$JELLYSEERR_URL/api/v1/user/import-from-jellyfin")
JELLYSEERR_USER_ID=$(echo "$IMPORT_RESP" | jq -r --arg id "$JELLYFIN_USER_ID" \
  '. | (if type == "array" then .[] else . end) | select(.jellyfinUserId == $id) | .id' | head -n1)

if [ -z "$JELLYSEERR_USER_ID" ] || [ "$JELLYSEERR_USER_ID" = "null" ]; then
  JELLYSEERR_USER_ID=$(curl -fsS -H "$JS_HDR" "$JELLYSEERR_URL/api/v1/user?take=1000" \
    | jq -r --arg id "$JELLYFIN_USER_ID" '.results[] | select(.jellyfinUserId == $id) | .id' | head -n1)
fi
[ -n "$JELLYSEERR_USER_ID" ] && [ "$JELLYSEERR_USER_ID" != "null" ] \
  || { echo "Erreur: user Jellyseerr non trouve apres import" >&2; exit 1; }
echo "    Jellyseerr user id: $JELLYSEERR_USER_ID"

echo "==> Set email cote Jellyseerr"
curl -fsS -X POST -H "$JS_HDR" -H "Content-Type: application/json" \
  -d "$(jq -n --arg e "$EMAIL" --arg u "$USERNAME" '{email:$e, username:$u}')" \
  "$JELLYSEERR_URL/api/v1/user/$JELLYSEERR_USER_ID/settings/main" >/dev/null \
  || echo "    Avertissement: set email a echoue (a corriger manuellement)"

echo "==> Envoi du mail unifie"
MAIL_FILE=$(mktemp)
trap 'rm -f "$MAIL_FILE"' EXIT
BOUNDARY="$(openssl rand -hex 16)"

cat > "$MAIL_FILE" <<EOF
From: $SMTP_FROM_NAME <$SMTP_FROM>
To: $USERNAME <$EMAIL>
Subject: Bienvenue sur Groscailloux — tes identifiants
MIME-Version: 1.0
Content-Type: text/plain; charset=UTF-8
Content-Transfer-Encoding: 8bit

Salut $USERNAME,

Si tu trouves ce mail dans tes spams / courrier indesirable / promotions,
merci de le marquer comme "Pas indesirable" et d'ajouter $SMTP_FROM
a tes contacts — sinon les futurs mails pourraient ne pas arriver.

Tu as peut-etre recu un premier mail "Reset password" automatique de
Jellyseerr juste avant celui-ci : IGNORE-LE. Ce sont les identifiants
ci-dessous qu'il faut utiliser.

Voici tes acces :

🎬 Streaming (regarder films/series) :
   $JELLYFIN_PUBLIC_URL

🎯 Requetes (demander de nouveaux contenus) :
   $JELLYSEERR_PUBLIC_URL

Identifiants (les memes sur les deux services) :
   Username : $USERNAME
   Password : $PASSWORD

⚠️  Important — ton compte parent est Jellyfin.
   En cas de changement de mot de passe, la procedure se fait UNIQUEMENT
   sur Jellyfin (Profil → Mot de passe). Le changement sera automatiquement
   actif sur Jellyseerr egalement.

Sur Jellyseerr, connecte-toi via l'onglet "Use your Jellyfin account".

—
Jellyseerr Groscailloux
EOF

if curl -fsS --url "smtps://$SMTP_HOST:$SMTP_PORT" \
  --ssl-reqd \
  --user "$SMTP_USER:$SMTP_PASS" \
  --mail-from "$SMTP_FROM" \
  --mail-rcpt "$EMAIL" \
  --upload-file "$MAIL_FILE" >/dev/null; then
  echo "    Mail envoye a $EMAIL"
else
  echo "    Avertissement: envoi mail a echoue — transmets les creds manuellement"
fi

cat <<EOF

✓ Onboarding termine pour $USERNAME

  Username       : $USERNAME
  Email          : $EMAIL
  Password       : $PASSWORD
  Jellyfin Id    : $JELLYFIN_USER_ID
  Jellyseerr id  : $JELLYSEERR_USER_ID

  Streaming  : $JELLYFIN_PUBLIC_URL
  Requetes   : $JELLYSEERR_PUBLIC_URL

EOF
