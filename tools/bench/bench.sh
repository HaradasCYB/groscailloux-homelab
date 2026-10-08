#!/usr/bin/env bash
# shellcheck source-path=SCRIPTDIR
# tools/bench/bench.sh : LE lanceur des bancs d'interface (navigateur sans écran, puppeteer) — 2026-10-08, lot 4.
#
# Remplace les lanceurs recopiés dans backups/ (runprod.sh, run.sh ×5, run12.sh, t6_run.sh, t8_run.sh, run_lite.sh,
# syncplay_menu.sh…) : un compte de banc créé puis TOUJOURS supprimé (aussi sur erreur ou Ctrl-C), avec ses appareils
# fermés par identifiant exact ; balayage des restes zz_* de plus de 2 h ; gardes horaires et de lecture ; conteneur
# bridé (1 cœur, basse priorité) ; agent utilisateur « … GcBanc/1 » pour trier les journaux NPM ; image épinglée.
#
# Usage : tools/bench/bench.sh [options] <scénario.js> [appareil …]
#   Appareils : desktop desktop-legacy phone phone-legacy iphone android tablet tv   (défaut : desktop)
#               ou « <mise en page>:<appareil> » comme runprod.sh (ex. « mobile-legacy:phone », « :desktop »)
# Options :
#   --offline          scénario autonome (page simulée) : aucun compte, aucun appel à Jellyfin
#   --accounts N       comptes temporaires (1 par défaut) : USER_NAME/PW/USER_ID, puis USER_NAME_k/PW_k/USER_ID_k
#   --prefix zz_xxx    préfixe des comptes (zz_bench) : zz_xxx, zz_xxx2…
#   --user-config K=V  réglage Jellyfin du compte (Configuration), répétable (ex. SubtitleMode=Smart)
#   --candidate DIR    scripts et CSS candidats injectés dans CE navigateur seulement (voir tools/README.md)
#   --no-inject        NO_INJECT=1 : ce que sert la production, rien d'injecté (sans l'option : NO_INJECT absent)
#   --item ID          ITEM transmis au scénario
#   --env K=V          variable transmise au scénario, répétable
#   --out DIR          sorties du scénario (/out) ; défaut : backups/bench/<date>/<scénario> pour un scénario de tools/,
#                      le DOSSIER DU SCÉNARIO pour un ancien scénario de backups/ (contrat de runprod.sh : il y lit
#                      ses entrées, ex. /out/gc-lang.candidate.js, /out/moverlay_lib.js)
#   --timeout S        durée maximale d'un passage (300 s)
#   --wait MIN         attendre jusqu'à MIN minutes que les gardes passent au vert (sinon refus immédiat)
#   --force            passer outre les refus (19:00–00:00, lecture d'un membre en cours, maintenance)
#   --sweep            seulement le balayage des comptes et appareils zz_* de plus de 2 h, puis sortie
#   --dry-run          gardes réelles et balayage à blanc ; aucun compte créé, aucun navigateur lancé
#
# Contrat du scénario (conteneur, node 20, puppeteer 22) : JF_URL (adresse publique, par NPM : /gc-chat/ et
# /gc-compte/ y sont servis), USER_NAME, PW, USER_ID, DEVICE (desktop|phone|iphone|android|tablet|tv), LAYOUT,
# BENCH_DEVICE (l'appareil demandé), ITEM, CANDIDATE_DIR=/cand, NO_INJECT=1 (seulement avec --no-inject), /out
# (sorties), /scen (dossier du scénario, lecture seule), /bench (tools/bench/lib : bench.js) et, en lecture seule,
# /repo/crates/homelabd/assets et /repo/branding SEULEMENT (jamais .env, state/ ni backups/). Code de sortie du
# scénario = résultat du passage. Contrats propres à certains anciens lanceurs NON repris (CANDIDATE=0/1 de
# lg-tv-20260929, PW1/PW2 de syncplay-20261003, /work de t6_run.sh) : les poser par --env ou adapter le scénario.
# Codes : 0 tous les passages réussis ; 1 un passage en échec ; 2 usage ; 3 refusé (garde) ; 4 nettoyage incomplet.
set -uo pipefail
export LANG=C.UTF-8 LC_ALL=C.UTF-8 PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
umask 077
SELF=$(readlink -f "$0")
BENCH_DIR=$(dirname "$SELF")
# shellcheck source=../lib/common.sh
. "$BENCH_DIR/../lib/common.sh"
REPO=$(cd "$BENCH_DIR/../.." && pwd)
# image épinglée (tag@sha256) : zenika/alpine-chrome with-puppeteer = node 20.15, puppeteer 22.9, utilisateur 1000
IMAGE=zenika/alpine-chrome:with-puppeteer@sha256:ee10e24217aa27443e6b58da628f3b09ea9b814459915b8b62fe15a555f9692a
# soirée = heures de pointe des membres : aucun banc (règle du propriétaire, revue lecture-bancs-en-pointe)
QUIET_FROM=19 QUIET_TO=24
SWEEP_AGE_MIN=120

usage() { sed -n '/^# Usage :/,/^# Codes :/p' "$SELF" | sed 's/^# \{0,1\}//' >&2; exit 2; }
fail() { echo "bench : $*" >&2; exit 2; }

OFFLINE=0 NACC=1 PREFIX=zz_bench CAND="" NOINJ=0 ITEM="" OUT="" TIMEOUT=300 WAIT=0 FORCE=0 SWEEP=0 DRY=0
UCONF=() XENV=() SCEN="" DEVS=()
need() { [ $# -ge 2 ] && [ -n "$2" ] || fail "$1 attend une valeur"; }
while [ $# -gt 0 ]; do
  case "$1" in
    --offline) OFFLINE=1 ;;
    --accounts) need "$@"; NACC=$2; shift ;;
    --prefix) need "$@"; PREFIX=$2; shift ;;
    --user-config) need "$@"; UCONF+=("$2"); shift ;;
    --candidate) need "$@"; CAND=$(readlink -f "$2"); shift ;;
    --no-inject) NOINJ=1 ;;
    --item) need "$@"; ITEM=$2; shift ;;
    --env) need "$@"; [[ $2 =~ ^[A-Z][A-Z0-9_]*=.*$ ]] || fail "--env attend CLÉ=VALEUR (clé en majuscules)"; XENV+=("$2"); shift ;;
    --out) need "$@"; OUT=$2; shift ;;
    --timeout) need "$@"; TIMEOUT=$2; shift ;;
    --wait) need "$@"; WAIT=$2; shift ;;
    --force) FORCE=1 ;;
    --sweep) SWEEP=1 ;;
    --dry-run) DRY=1 ;;
    -h|--help) usage ;;
    -*) fail "option inconnue : $1 (--help)" ;;
    *) if [ -z "$SCEN" ]; then SCEN=$1; else DEVS+=("$1"); fi ;;
  esac
  shift
done

SWEEP_ARGS=(sweep --max-age-min "$SWEEP_AGE_MIN")
[ "$DRY" = 1 ] && SWEEP_ARGS+=(--dry-run)
if [ "$SWEEP" = 1 ]; then "$HLPY" "${SWEEP_ARGS[@]}"; exit $?; fi
[ -n "$SCEN" ] || usage
[ -f "$SCEN" ] || fail "scénario introuvable : $SCEN"
SCEN=$(readlink -f "$SCEN")
[[ $NACC =~ ^[1-4]$ ]] || fail "--accounts : 1 à 4"
[[ $PREFIX =~ ^zz_[a-z0-9_]{1,20}$ ]] || fail "--prefix doit commencer par zz_ (a-z, 0-9, _)"
[[ $TIMEOUT =~ ^[0-9]+$ ]] && [[ $WAIT =~ ^[0-9]+$ ]] || fail "--timeout et --wait attendent un nombre"
[ -z "$CAND" ] || [ -d "$CAND" ] || fail "--candidate : dossier introuvable"
[ ${#DEVS[@]} -gt 0 ] || DEVS=(desktop)
for d in "${DEVS[@]}"; do
  case "$d" in desktop|desktop-legacy|phone|phone-legacy|iphone|android|tablet|tv|*:*) ;; *) fail "appareil inconnu : $d" ;; esac
done
SNAME=$(basename "$SCEN" .js)
SDIR=$(dirname "$SCEN")
# le dossier du scénario est monté dans le conteneur : jamais la racine du dépôt ni un dossier qui contient des secrets
if [ "$SDIR" = "$REPO" ] || [ "$SDIR" = "$HL" ] || [ "$SDIR" = / ] || [ -e "$SDIR/.env" ] || [ -e "$SDIR/.git" ]; then
  fail "scénario dans $SDIR : ce dossier serait monté dans le conteneur (dépôt, .env) ; le ranger dans un sous-dossier"
fi
# 2026-10-08 (revue) : un ancien scénario (hors tools/) lit ses entrées dans /out = son propre dossier (runprod.sh,
# t8_run.sh, syncplay_menu.sh…) ; un /out neuf le faisait planter, ou tourner SANS son correctif candidat (try/catch)
case "$SCEN" in
  */tools/bench/*|*/tools/tests/*) OUT=${OUT:-$HL/backups/bench/$(date +%Y%m%d)/$SNAME} ;;
  *) OUT=${OUT:-$SDIR} ;;
esac

# ------------------------------------------------------------------------------------------------ gardes
guards() {  # imprime le motif d'un refus ; 0 = voie libre
  local h n
  h=$(( 10#$(date +%H) ))
  if [ "$h" -ge "$QUIET_FROM" ] && [ "$h" -lt "$QUIET_TO" ]; then echo "heures de pointe ($QUIET_FROM:00–00:00) : aucun banc"; return 1; fi
  n=$("$HLPY" busy --timers --margin-min 20) || { echo "maintenance hors pic en cours ou imminente : $n"; return 1; }
  if [ "$OFFLINE" = 0 ]; then
    n=$("$HLPY" members-playing) || { echo "Jellyfin ne répond pas ($n)"; return 1; }
    [ "${n%% *}" = 0 ] || { echo "${n%% *} lecture(s) de membre en cours"; return 1; }
  else
    n=$("$HLPY" members-playing) && [ "${n%% *}" != 0 ] && { echo "${n%% *} lecture(s) de membre en cours (même hors ligne, le banc prend du processeur)"; return 1; }
  fi
  return 0
}
if [ "$FORCE" = 1 ]; then
  echo "gardes ignorées (--force)$(m=$(guards) || printf ' — elles auraient refusé : %s' "$m")"
else
  t_end=$(( $(date +%s) + WAIT * 60 ))
  while ! motif=$(guards); do
    if [ "$(date +%s)" -ge "$t_end" ]; then echo "REFUS : $motif (--force pour passer outre, --wait N pour attendre)"; exit 3; fi
    echo "attente : $motif"; sleep 60
  done
  echo "gardes : voie libre (hors soirée, aucune maintenance, aucune lecture de membre)"
fi

# restes d'anciens bancs (comptes et appareils zz_* de plus de 2 h), un par un, vérifiés
if [ "$OFFLINE" = 0 ] || [ "$DRY" = 1 ]; then
  "$HLPY" "${SWEEP_ARGS[@]}" | sed 's/^/  /'
fi

LAYOUT_OF() { case "$1" in desktop-legacy) echo desktop-legacy ;; phone-legacy) echo mobile-legacy ;; tv) echo tv ;; *) echo "" ;; esac; }
DEVICE_OF() { case "$1" in desktop-legacy) echo desktop ;; phone-legacy) echo phone ;; *) echo "$1" ;; esac; }
if [ "$DRY" = 1 ]; then
  echo "à blanc : $NACC compte(s) $([ "$OFFLINE" = 1 ] && echo '(aucun : --offline)' || echo "$PREFIX…") ; scénario $SCEN ; appareils ${DEVS[*]} ; sorties $OUT"
  exit 0
fi

# ------------------------------------------------------------------------------------------------ comptes et nettoyage
TMPD=$(mktemp -d "${XDG_RUNTIME_DIR:-/tmp}/gcbench.XXXXXX") || fail "mktemp"
CT=gcbench-$$
# comptes indexés par k (1…N) ; le NOM est noté AVANT la création, l'id dès que hl.py l'a écrit dans acc<k>.id :
# un signal reçu pendant la création (piège différé jusqu'à la fin de la commande) ne laisse plus de compte derrière
ACC_IDS=() ACC_NAMES=()
CLEANED=0
# shellcheck disable=SC2329  # appelée par les pièges ci-dessous
cleanup() {
  [ "$CLEANED" = 1 ] && return; CLEANED=1
  local k id rc=0
  docker kill "$CT" > /dev/null 2>&1 || true   # pages fermées AVANT la suppression des comptes (sinon rafales de 403 /socket)
  for k in "${!ACC_NAMES[@]}"; do
    id=${ACC_IDS[$k]:-}
    [ -n "$id" ] || id=$(head -n 1 "$TMPD/acc$k.id" 2>/dev/null)
    # pas d'id : création refusée ou jamais faite (hl.py retire lui-même un compte créé puis interrompu)
    [ -n "$id" ] || continue
    "$HLPY" account-delete --id "$id" --name "${ACC_NAMES[$k]}" | sed 's/^/nettoyage : /' || rc=4
  done
  rm -rf "$TMPD"
  return "$rc"
}
# shellcheck disable=SC2329  # appelée par le piège EXIT
on_exit() { local r=$?; cleanup || r=4; exit "$r"; }
trap 'cleanup; exit 130' INT TERM HUP
trap on_exit EXIT

ENVFILES=()
if [ "$OFFLINE" = 0 ]; then
  JF_PUBLIC=$("$HLPY" public JELLYFIN_PUBLIC_URL) || fail "adresse publique de Jellyfin introuvable ($JF_PUBLIC)"
  for k in $(seq 1 "$NACC"); do
    name=$PREFIX$([ "$k" = 1 ] || echo "$k")
    ACC_NAMES[k]=$name
    args=(account-create "$name" --env-out "$TMPD/acc$k.env" --id-out "$TMPD/acc$k.id" --index "$k")
    for c in "${UCONF[@]}"; do args+=(--config "$c"); done
    if ! id=$("$HLPY" "${args[@]}"); then echo "création de $name impossible : $id"; exit 1; fi
    ACC_IDS[k]=$id; ENVFILES+=(--env-file "$TMPD/acc$k.env")
    echo "compte de banc : $name (caché, politique d'un membre ordinaire, supprimé à la fin)"
  done
fi
# dossier de sorties créé ici : 0700 ; dossier existant (celui d'un ancien scénario) : droits inchangés
[ -d "$OUT" ] || { mkdir -p "$OUT" && chmod 700 "$OUT"; } || { echo "sorties : $OUT impossible à créer"; exit 1; }

# image : déjà là (identifiant), sinon tirée par son empreinte
IMG_ID=$(docker image inspect -f '{{.Id}}' "$IMAGE" 2>/dev/null) || { docker pull -q "$IMAGE" > /dev/null && IMG_ID=$(docker image inspect -f '{{.Id}}' "$IMAGE"); } \
  || { echo "image de banc introuvable : $IMAGE"; exit 1; }
# un seul conteneur de banc à la fois (les bancs d'autres sessions compris)
for i in $(seq 1 120); do
  [ -z "$(docker ps -q --filter "ancestor=$IMG_ID")" ] && break
  [ "$i" = 1 ] && echo "un autre banc tourne : attente (10 min au plus)"
  [ "$i" = 120 ] && { echo "REFUS : banc occupé depuis 10 min"; exit 3; }
  sleep 5
done

# ------------------------------------------------------------------------------------------------ passages
FAILS=0
for tok in "${DEVS[@]}"; do
  if [[ $tok == *:* ]]; then L=${tok%%:*}; D=${tok##*:}; else L=$(LAYOUT_OF "$tok"); D=$(DEVICE_OF "$tok"); fi
  echo "===== $SNAME · ${tok} (mise en page : ${L:-défaut})"
  run=(docker run --rm --name "$CT" --network host --cpus=1 --cpu-shares=256 --memory=2g --pids-limit=512
       --security-opt no-new-privileges "${ENVFILES[@]}"
       -e "DEVICE=$D" -e "LAYOUT=$L" -e "BENCH_DEVICE=$tok" -e "ITEM=$ITEM" -e "TZ=Europe/Paris"
       -e NODE_PATH=/usr/src/app/node_modules -e "NODE_OPTIONS=--require /bench/ua-hook.js"
       -v "$SDIR":/scen:ro -v "$BENCH_DIR/lib":/bench:ro -v "$OUT":/out
       # du dépôt, seulement ce que lisent les scénarios (2026-10-08, revue : tout /opt/homelab exposait .env, state/
       # et backups/ au code du scénario, qui tourne sous l'uid de deploy)
       -v "$REPO/crates/homelabd/assets":/repo/crates/homelabd/assets:ro -v "$REPO/branding":/repo/branding:ro)
  # NO_INJECT seulement avec --no-inject : les anciens scénarios testent « process.env.NO_INJECT ? … », où « 0 » vaut vrai
  [ "$NOINJ" = 1 ] && run+=(-e NO_INJECT=1)
  [ "$OFFLINE" = 0 ] && run+=(-e "JF_URL=$JF_PUBLIC")
  if [ -n "$CAND" ] && [ "$NOINJ" = 0 ]; then run+=(-v "$CAND":/cand:ro -e CANDIDATE_DIR=/cand); fi
  for e in "${XENV[@]}"; do run+=(-e "$e"); done
  run+=(--entrypoint node "$IMAGE" "/scen/$(basename "$SCEN")")
  # en tâche de fond + wait : un signal reçu par le lanceur déclenche le nettoyage TOUT DE SUITE (bash ne lance un
  # piège qu'après la fin d'une commande au premier plan, ici jusqu'à --timeout)
  ( timeout -k 15 "$TIMEOUT" "${run[@]}" 2>&1 | grep --line-buffered -vE '^\s+at '; exit "${PIPESTATUS[0]}" ) &
  wait $!
  rc=$?
  docker kill "$CT" > /dev/null 2>&1 || true
  case "$rc" in
    0) echo "----- $tok : réussi" ;;
    124|137) echo "----- $tok : ÉCHEC (délai de $TIMEOUT s dépassé)"; FAILS=$(( FAILS + 1 )) ;;
    *) echo "----- $tok : ÉCHEC (code $rc)"; FAILS=$(( FAILS + 1 )) ;;
  esac
done
echo "sorties : $OUT"
[ "$FAILS" = 0 ] || exit 1
exit 0
