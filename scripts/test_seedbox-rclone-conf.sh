#!/bin/sh
# Tests de scripts/seedbox-rclone-conf.sh (2026-10-09, lot 4) : config rclone effective du montage seedbox.
# Lancement : sh scripts/test_seedbox-rclone-conf.sh (code 0 = tout passe). Valeurs d'exemple seulement ; aucun
# contact réseau, aucune lecture du vrai .env (HOMELAB_ENV_FILE pointe sur un fichier d'essai).
set -u
GEN="$(dirname "$(readlink -f "$0")")/seedbox-rclone-conf.sh"
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
FAILS=0 N=0
eq() {  # libellé, obtenu, attendu
  N=$((N + 1))
  if [ "$2" = "$3" ]; then echo "OK     $1"; else echo "ÉCHEC  $1 : obtenu « $2 », attendu « $3 »"; FAILS=$((FAILS + 1)); fi
}
# Lance le générateur sans SEEDBOX_* hérités du shell : seules les affectations passées en argument comptent.
gen() {  # [VAR=valeur…] modèle sortie
  env -u SEEDBOX_SFTP_HOST -u SEEDBOX_USER HOMELAB_ENV_FILE="$T/env" "$@" sh "$GEN" "$TPL" "$T/out.conf" 2> "$T/err"
}

TPL="$T/modele.conf"
cat > "$TPL" <<'EOF'
# commentaire : host = jamais lu
[seedbox]
type = sftp
host = seedbox.invalid
user = seedbox
port = 22
concurrency = 16
EOF
printf 'AUTRE=1\nSEEDBOX_SFTP_HOST="sftp.seedbox.example"\nSEEDBOX_USER=compte-exemple\n' > "$T/env"

gen; eq "valeurs lues dans .env : réussite" "$?" 0
eq "host et user remplacés juste après [seedbox], le reste gardé" "$(cat "$T/out.conf")" "# commentaire : host = jamais lu
[seedbox]
host = sftp.seedbox.example
user = compte-exemple
type = sftp
port = 22
concurrency = 16"
eq "fichier en 600" "$(stat -c %a "$T/out.conf")" 600
eq "aucun fichier temporaire laissé" "$(find "$T" -maxdepth 1 -name '.rclone.conf.*' | wc -l)" 0

gen SEEDBOX_USER=autre-compte
eq "l'environnement l'emporte sur .env" "$(sed -n 4p "$T/out.conf")" "user = autre-compte"

printf 'SEEDBOX_SFTP_HOST=sftp.seedbox.example\n' > "$T/env"
rm -f "$T/out.conf"
gen; eq "SEEDBOX_USER absent : refus" "$?" 1
eq "le refus nomme la variable" "$(grep -c 'SEEDBOX_USER absent' "$T/err")" 1
eq "rien n'est écrit" "$([ -e "$T/out.conf" ] && echo écrit || echo absent)" absent

printf 'SEEDBOX_SFTP_HOST=\nSEEDBOX_USER=compte-exemple\n' > "$T/env"
gen; eq "SEEDBOX_SFTP_HOST vide : refus" "$?" 1
eq "le refus nomme SEEDBOX_SFTP_HOST" "$(grep -c 'SEEDBOX_SFTP_HOST absent' "$T/err")" 1

printf 'SEEDBOX_SFTP_HOST=sftp.seedbox.example\nSEEDBOX_USER=compte-exemple\n' > "$T/env"
gen SEEDBOX_SFTP_HOST='x.example
port = 2222'
eq "saut de ligne dans la valeur : refus" "$?" 1
eq "le refus ne montre pas la valeur" "$(grep -c 'x.example\|2222' "$T/err")" 0
gen SEEDBOX_USER='a b'; eq "espace dans le compte : refus" "$?" 1

cat > "$TPL" <<'EOF'
[seedbox]
type = sftp
chunk_size = 255k
EOF
gen; eq "modèle sans host ni user : réussite" "$?" 0
eq "host et user ajoutés" "$(head -n 3 "$T/out.conf" | tail -n 2 | tr '\n' '|')" "host = sftp.seedbox.example|user = compte-exemple|"

printf '[autre]\ntype = local\n' > "$TPL"
gen; eq "modèle sans section [seedbox] : refus" "$?" 1
printf '[seedbox]\ntype = sftp\n[seedbox]\ntype = sftp\n' > "$TPL"
gen; eq "section [seedbox] en double : refus" "$?" 1
printf '[autre]\nhost = garde\n[seedbox]\nhost = x\n' > "$TPL"
gen; eq "host d'une autre section gardé" "$(sed -n 2p "$T/out.conf")" "host = garde"

echo "$((N - FAILS))/$N contrôle(s) réussi(s)"
[ "$FAILS" = 0 ]
