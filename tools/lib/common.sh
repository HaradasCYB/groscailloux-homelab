# shellcheck shell=bash
# shellcheck disable=SC2034  # HLPY, PATH_PROPRE… sont utilisés par les scripts qui sourcent ce fichier
# tools/lib/common.sh : fonctions bash communes à tools/offpeak et tools/bench (sourcé, jamais exécuté seul).
# Créé le 2026-10-08 (lot 4 de la revue Kaizen) à partir de backups/lot3-20261008/common.sh.
#
# Aucun secret n'est lu en bash : clé Jellyfin et webhook Discord ne sont lus que par tools/lib/hl.py, jamais passés
# en argument (rien dans /proc/<pid>/cmdline), jamais affichés ni journalisés.
# Coreutils du VPS = uutils : toujours « tail -n N » / « head -n N », jamais « tail -N ».

TOOLS_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# configuration, état et secrets : toujours ceux de la production (/opt/homelab), même quand l'outil est lancé
# depuis un autre arbre de travail (worktree) ; HOMELAB_DIR pour un environnement d'essai
HL=${HOMELAB_DIR:-/opt/homelab}
export HOMELAB_DIR=$HL
HLPY=$TOOLS_DIR/lib/hl.py
PATH_PROPRE=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

# une ligne sans URL, adresse IP, nom de domaine ni chaîne qui ressemble à une clé (Discord, journaux)
nettoie() {
  printf '%s' "$1" | tr '\n\t' '  ' | sed -E \
    -e 's#[a-zA-Z][a-zA-Z0-9+.-]*://[^ ]*#<url>#g' \
    -e 's/\b[0-9]{1,3}(\.[0-9]{1,3}){3}(:[0-9]+)?\b/<ip>/g' \
    -e 's/\b([a-zA-Z0-9-]+\.)+(org|net|com|fr|me|io|tw|dev|xyz|eu|info|cc|to|si|tv|app)\b/<domaine>/g' \
    -e 's/(api_?key|apikey|token|access_token)=[^ &]*/\1=<…>/Ig' \
    -e 's/\b[A-Za-z0-9_-]{32,}\b/<…>/g' \
    -e 's/\x1b\[[0-9;]*m//g' | cut -c1-"${2:-200}" | iconv -c -f UTF-8 -t UTF-8
}

# secondes depuis minuit (heure locale)
secs_of_day() { local h m s; read -r h m s < <(date '+%H %M %S'); echo $(( 10#$h * 3600 + 10#$m * 60 + 10#$s )); }
hm() { printf '%02d:%02d' $(( $1 / 60 )) $(( $1 % 60 )); }
duree() { local s=$1; if [ "$s" -ge 60 ]; then printf '%d min %02d s' $(( s / 60 )) $(( s % 60 )); else printf '%d s' "$s"; fi; }
date_fr() {
  local j
  j=$(LC_ALL=C date +%u | awk '{split("lundi mardi mercredi jeudi vendredi samedi dimanche", j, " "); print j[$1]}')
  printf '%s %s' "$j" "$(date +%d/%m)"
}

# « HH:MM-HH:MM » → « début fin » en minutes depuis minuit ; code 1 si invalide
parse_window() {
  local re='^([01][0-9]|2[0-3]):([0-5][0-9])-([01][0-9]|2[0-3]):([0-5][0-9])$'
  [[ $1 =~ $re ]] || return 1
  local a=$(( 10#${BASH_REMATCH[1]} * 60 + 10#${BASH_REMATCH[2]} )) b=$(( 10#${BASH_REMATCH[3]} * 60 + 10#${BASH_REMATCH[4]} ))
  [ "$a" != "$b" ] || return 1
  echo "$a $b"
}
# secondes restantes avant la fin du créneau « début fin » (minutes), 0 si hors créneau ; un créneau peut passer minuit
window_left() {
  local a=$1 b=$2 now in=0
  now=$(secs_of_day)
  if [ "$a" -lt "$b" ]; then
    [ "$now" -ge $(( a * 60 )) ] && [ "$now" -lt $(( b * 60 )) ] && in=1
  else
    { [ "$now" -ge $(( a * 60 )) ] || [ "$now" -lt $(( b * 60 )) ]; } && in=1
  fi
  if [ "$in" = 0 ]; then echo 0; return; fi
  echo $(( (b * 60 - now + 86400) % 86400 ))
}
