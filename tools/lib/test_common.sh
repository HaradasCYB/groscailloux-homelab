#!/usr/bin/env bash
# shellcheck source-path=SCRIPTDIR
# Tests des décisions de tools/lib/common.sh (2026-10-08, lot 4) : créneau horaire (y compris à cheval sur minuit) et
# nettoyage des textes envoyés sur Discord. Lancement : bash tools/lib/test_common.sh (code 0 = tout passe).
set -u
# shellcheck source=common.sh
. "$(dirname "$(readlink -f "$0")")/common.sh"
FAILS=0 N=0
eq() {  # libellé, obtenu, attendu
  N=$(( N + 1 ))
  if [ "$2" = "$3" ]; then echo "OK     $1"; else echo "ÉCHEC  $1 : obtenu « $2 », attendu « $3 »"; FAILS=$(( FAILS + 1 )); fi
}
at() { NOW_S=$(( 10#${1%%:*} * 3600 + 10#${1##*:} * 60 )); }
secs_of_day() { echo "$NOW_S"; }   # heure simulée

eq "créneau lu" "$(parse_window 04:30-05:20)" "270 320"
eq "créneau à cheval sur minuit lu" "$(parse_window 23:30-01:00)" "1410 60"
parse_window 25:00-26:00 > /dev/null; eq "heure invalide refusée" "$?" 1
parse_window 04:30-04:30 > /dev/null; eq "créneau vide refusé" "$?" 1
parse_window 4:30-5:20 > /dev/null; eq "format sans zéro refusé" "$?" 1

at 04:29; eq "04:29 avant 04:30-05:20 : hors créneau" "$(window_left 270 320)" 0
at 04:30; eq "04:30 : 50 min restantes" "$(window_left 270 320)" 3000
at 05:19; eq "05:19 : 1 min restante" "$(window_left 270 320)" 60
at 05:20; eq "05:20 : fin exclue" "$(window_left 270 320)" 0
at 23:45; eq "23:45 dans 23:30-01:00 : 75 min" "$(window_left 1410 60)" 4500
at 00:30; eq "00:30 dans 23:30-01:00 : 30 min" "$(window_left 1410 60)" 1800
at 01:00; eq "01:00 : fin exclue (minuit passé)" "$(window_left 1410 60)" 0
at 12:00; eq "12:00 hors de 23:30-01:00" "$(window_left 1410 60)" 0

eq "durée courte" "$(duree 42)" "42 s"
eq "durée longue" "$(duree 3725)" "62 min 05 s"
eq "nettoie : adresse, IP, domaine, clé" \
  "$(nettoie 'voir https://x.example.org/a?b=1 depuis 10.1.2.3:8096 sur exemple.org apikey=abc 0123456789abcdef0123456789abcdef')" \
  "voir <url> depuis <ip> sur <domaine> apikey=<…> <…>"
eq "nettoie : une seule ligne, coupée" "$(nettoie "$(printf 'a\nb\tc defghijklm')" 10)" "a b c defg"

echo "$(( N - FAILS ))/$N contrôle(s) réussi(s)"
[ "$FAILS" = 0 ]
