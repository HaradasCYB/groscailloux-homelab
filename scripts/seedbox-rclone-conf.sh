#!/bin/sh
# Config rclone effective du montage seedbox (2026-10-09, lot 4), lancée par homelab-seedbox-mount.service
# (ExecStartPre) à chaque démarrage du montage :
#
#   seedbox-rclone-conf.sh <modèle> <config effective>
#   (unité : /opt/homelab/rclone/rclone.conf -> /run/homelab-seedbox-mount/rclone.conf)
#
# L'hôte SFTP et le compte de la seedbox sont hors du dépôt public (SEEDBOX_SFTP_HOST, SEEDBOX_USER dans .env). Ils
# sont écrits ici DANS la section [seedbox] d'une copie du modèle, jamais passés à rclone en options : une option de
# backend en ligne de commande (--sftp-host…) ou par variable (RCLONE_SFTP_*, RCLONE_CONFIG_SEEDBOX_*) ajoute au nom
# interne du remote un suffixe tiré de sa valeur, et ce nom est celui du dossier du cache VFS (120 Go). Une valeur
# écrite dans le fichier ne change pas ce nom (seedbox{9oylk} avec --sftp-connections 32, vérifié en 1.72.1).
#
# Valeurs : environnement du processus d'abord, puis .env (HOMELAB_ENV_FILE, défaut /opt/homelab/.env ; dernière ligne
# `NOM=`, guillemets retirés), comme homelabd (dotenvy) et tools/lib/hlconf.py. Variable absente, vide ou avec un
# caractère inattendu : refus qui nomme la variable, jamais sa valeur. Les lignes host/user du modèle (absentes du
# modèle versionné, présentes dans un rclone.conf d'avant le 09/10) sont remplacées. Écriture atomique, fichier en 600.
# Tests : sh scripts/test_seedbox-rclone-conf.sh
set -eu
umask 077

die() { echo "seedbox-rclone-conf: $*" >&2; exit 1; }

[ "$#" -eq 2 ] || { echo "usage: $0 <modèle rclone.conf> <config effective>" >&2; exit 2; }
TEMPLATE=$1
OUT=$2
ENV_FILE=${HOMELAB_ENV_FILE:-/opt/homelab/.env}

# Valeur de la variable $1 (jamais affichée).
env_value() {
  v=$(printenv "$1" 2>/dev/null || true)
  if [ -z "$v" ] && [ -r "$ENV_FILE" ]; then
    v=$(sed -n "s/^[[:space:]]*$1[[:space:]]*=//p" "$ENV_FILE" | tail -n 1 | tr -d '\r' \
      | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' -e 's/^["'\'']*//' -e 's/["'\'']*$//')
  fi
  printf '%s' "$v"
}

HOST=$(env_value SEEDBOX_SFTP_HOST)
USER_NAME=$(env_value SEEDBOX_USER)
[ -n "$HOST" ] || die "SEEDBOX_SFTP_HOST absent ou vide (environnement et $ENV_FILE) : montage refusé, rclone partirait sans hôte"
[ -n "$USER_NAME" ] || die "SEEDBOX_USER absent ou vide (environnement et $ENV_FILE) : montage refusé"
case $HOST in
  *[!A-Za-z0-9.:_-]*) die "SEEDBOX_SFTP_HOST : caractère inattendu (nom d'hôte ou adresse attendus, sans compte ni port)" ;;
esac
case $USER_NAME in
  *[!A-Za-z0-9._-]*) die "SEEDBOX_USER : caractère inattendu (nom de compte attendu)" ;;
esac
[ -r "$TEMPLATE" ] || die "modèle illisible : $TEMPLATE"

dir=$(dirname "$OUT")
[ -d "$dir" ] || die "dossier absent : $dir"
tmp=$(mktemp "$dir/.rclone.conf.XXXXXX")
trap 'rm -f "$tmp"' EXIT
# host et user juste après l'en-tête [seedbox] ; toute ligne host/user de cette section est retirée (une seule section
# [seedbox] attendue, sinon refus).
if ! awk -v host="$HOST" -v user="$USER_NAME" '
  /^[ \t]*\[/ {
    sec = ($0 ~ /^[ \t]*\[seedbox\][ \t]*$/)
    print
    if (sec) { print "host = " host; print "user = " user; n++ }
    next
  }
  sec && /^[ \t]*(host|user)[ \t]*=/ { next }
  { print }
  END { exit (n == 1 ? 0 : 3) }
' "$TEMPLATE" > "$tmp"; then
  die "section [seedbox] absente ou en double dans $TEMPLATE"
fi
chmod 600 "$tmp"
mv -f "$tmp" "$OUT"
trap - EXIT
