#!/usr/bin/env bash
# shellcheck source-path=SCRIPTDIR
# tools/offpeak/offpeak.sh : exécuter une commande HORS PIC, seulement si personne ne regarde (2026-10-08, lot 4).
#
# Remplace les 8 copies recopiées dans backups/ du 18/09 au 05/10 (apply-offpeak.sh, restart-if-idle.sh ×2,
# recreate-if-idle.sh, reboot-if-idle.sh, jellyfin-*-offpeak.sh, run-morning.sh) ; logique reprise de l'exécutant du
# lot 3 (backups/lot3-20261008/run.sh + common.sh) : gardes, fenêtre homelabd, verrou, marqueurs, journal, bilan.
# La programmation passe par des unités systemd INSTALLÉES (gabarit homelab-offpeak@, Persistent=true) : elle survit à
# un redémarrage, contrairement aux minuteurs transitoires de systemd-run (/run), perdus au reboot.
#
# Usage :
#   offpeak.sh [options] -- <commande> [arguments…]          exécuter maintenant (créneau et gardes respectés)
#   sudo offpeak.sh --schedule '<OnCalendar>' --name <nom> [--as root] [options] -- <commande…>   programmer
#   sudo offpeak.sh --unschedule <nom>                       retirer une programmation
#   offpeak.sh --list | --status <nom>                       travaux programmés, dernier résultat, journal
#
# offpeak.sh tourne TOUJOURS en deploy (dossier d'état, verrous et journaux communs à tous les travaux) : lancé en root
# (sudo offpeak.sh …, ou une unité), il repasse en deploy et seule la commande tourne en root, par sudo -n.
#
# Options :
#   --name NOM            nom du travail (a-z, 0-9, -) : verrou, marqueurs, journal (défaut : adhoc)
#   --label TEXTE         libellé lisible dans le journal et le bilan
#   --as root|deploy      utilisateur de la COMMANDE : deploy par défaut, root par sudo -n (redémarrage de l'hôte…) ;
#                         sans --as, « sudo offpeak.sh » garde la commande en root
#   --window HH:MM-HH:MM  créneau autorisé (peut passer minuit) ; hors créneau : rien n'est fait (code 3)
#   --retry MIN           nouvel essai toutes les MIN minutes si une garde refuse, jusqu'à la fin du créneau (10 ; 0 = un essai)
#   --max-wait MIN        sans --window : durée maximale d'attente des gardes (0 = un seul essai)
#   --if-idle             aucune lecture en cours dans Jellyfin (sessions avec NowPlayingItem)
#   --members             --if-idle + aucune tâche planifiée Jellyfin en cours (ce qui coupe la lecture des membres)
#   --idle-if-down        Jellyfin muet = personne ne regarde (cas du redémarrage de l'hôte)
#   --seedbox             aucun fichier ouvert sous le montage seedbox (tout ce qui touche rclone)
#   --homelabd-window T:S[,T:S…]  aucun passage en cours de la tâche homelabd T, et le prochain à au moins S secondes
#                         (ex. stack_health:150 : il relancerait un conteneur arrêté) ; T:S:I pour forcer l'intervalle
#   --healthy SVC         après la commande, le service compose SVC doit redevenir sain sous 10 min (répétable)
#   --once                après une réussite, les passages suivants ne font plus rien (marqueur « fait »)
#   --discord             bilan sur le salon Discord ADMIN : réussite, échec, créneau passé sans voie libre
#   --timeout SECONDES    durée maximale de la commande (3600) ; dépassée = échec
#   --cd DOSSIER          dossier de travail de la commande (/opt/homelab)
#   --no-gluetun-guard    ne pas réparer qBittorrent si la commande a recréé gluetun (réparé par défaut, voir plus bas)
#   --state-dir DOSSIER   marqueurs, journaux et verrou global (/opt/homelab/state/offpeak) : essais seulement, un
#                         autre dossier = un autre verrou global
#   --dry-run             gardes réelles, commande NON lancée, rien d'écrit, bilan affiché au lieu d'être envoyé ;
#                         avec --schedule : montre la programmation (sans sudo) et n'installe rien
#
# Codes de sortie : 0 fait (ou déjà fait avec --once, ou à blanc avec voie libre) ; 1 commande en échec ; 2 usage ;
# 3 rien fait (hors créneau, gardes jamais vertes dans le créneau, autre passage en cours).
#
# La commande tourne avec un environnement vide (PATH, HOME, LANG, TMPDIR privé), entrée fermée, sortie complète dans
# <state-dir>/<nom>/runs/<date>.log. Un seul travail hors pic à la fois (verrou global : les autres attendent et
# réessaient), et jamais pendant un service lot3-* actif.
# Garde gluetun (piège de CLAUDE.md, 19/09 : qBittorrent coupé 11 h) : si la commande a recréé gluetun et que
# qBittorrent est resté sur l'ancien espace réseau, il est recréé (--no-deps) et le port transféré reposé.
set -uo pipefail
export LANG=C.UTF-8 LC_ALL=C.UTF-8 PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
umask 077
ARGV=("$@")
RUN_USER=deploy   # offpeak.sh lui-même (les Arrs et gluetun refusent root ; dossier d'état commun)
SELF=$(readlink -f "$0")
# shellcheck source=../lib/common.sh
. "$(dirname "$SELF")/../lib/common.sh"
JOBS_ETC=${OFFPEAK_ETC:-/etc/homelab-offpeak}   # OFFPEAK_ETC : essais seulement (le fichier doit rester à root)
UNIT_SRC=$(dirname "$SELF")/systemd

usage() { sed -n '/^# Usage :/,/^# Codes de sortie/p' "$SELF" | sed 's/^# \{0,1\}//' >&2; exit 2; }
fail() { echo "offpeak : $*" >&2; exit 2; }

# ------------------------------------------------------------------------------------------------ arguments
NAME=adhoc LABEL="" WINDOW="" RETRY=10 MAXWAIT=0 IDLE=0 JFTASKS=0 IDLEDOWN=0 SEEDBOX=0 ONCE=0 DISCORD=0
TIMEOUT=3600 CD=$HL GLUETUN=1 STATE_DIR=$HL/state/offpeak DRY=0
HWIN=() HEALTHY=() CMD=() SCHED=() AS="" MODE=run
ORIG=()   # arguments recopiés tels quels dans /etc/homelab-offpeak/<nom>.args par --schedule
need() { [ $# -ge 2 ] && [ -n "$2" ] || fail "$1 attend une valeur"; }
while [ $# -gt 0 ]; do
  case "$1" in
    --schedule)   need "$@"; SCHED+=("$2"); MODE=schedule; shift 2; continue ;;
    --as)         need "$@"; AS=$2; shift 2; continue ;;
    --unschedule) need "$@"; NAME=$2; MODE=unschedule; shift 2; continue ;;
    --list)       MODE=list; shift; continue ;;
    --status)     need "$@"; NAME=$2; MODE=status; shift 2; continue ;;
    --job)        need "$@"; NAME=$2; MODE=job; shift 2; continue ;;
    -h|--help)    usage ;;
  esac
  ORIG+=("$1")
  case "$1" in
    --name)       need "$@"; NAME=$2; ORIG+=("$2"); shift ;;
    --label)      need "$@"; LABEL=$2; ORIG+=("$2"); shift ;;
    --window)     need "$@"; WINDOW=$2; ORIG+=("$2"); shift ;;
    --retry)      need "$@"; RETRY=$2; ORIG+=("$2"); shift ;;
    --max-wait)   need "$@"; MAXWAIT=$2; ORIG+=("$2"); shift ;;
    --if-idle)    IDLE=1 ;;
    --members)    IDLE=1; JFTASKS=1 ;;
    --idle-if-down) IDLEDOWN=1 ;;
    --seedbox)    SEEDBOX=1 ;;
    --homelabd-window) need "$@"; IFS=, read -r -a _w <<< "$2"; HWIN+=("${_w[@]}"); ORIG+=("$2"); shift ;;
    --healthy)    need "$@"; HEALTHY+=("$2"); ORIG+=("$2"); shift ;;
    --once)       ONCE=1 ;;
    --discord)    DISCORD=1 ;;
    --timeout)    need "$@"; TIMEOUT=$2; ORIG+=("$2"); shift ;;
    --cd)         need "$@"; CD=$2; ORIG+=("$2"); shift ;;
    --no-gluetun-guard) GLUETUN=0 ;;
    --state-dir)  need "$@"; STATE_DIR=$2; ORIG+=("$2"); shift ;;
    --dry-run)    DRY=1 ;;
    --)           shift; CMD=("$@"); ORIG+=("$@"); break ;;
    *)            fail "option inconnue : $1 (--help)" ;;
  esac
  shift
done

[[ $NAME =~ ^[a-z0-9][a-z0-9-]{0,40}$ ]] || fail "nom invalide : « $NAME » (a-z, 0-9, -)"
case "$AS" in ""|root|deploy) ;; *) fail "--as root|deploy" ;; esac
for n in "$RETRY" "$MAXWAIT" "$TIMEOUT"; do [[ $n =~ ^[0-9]+$ ]] || fail "nombre attendu : « $n »"; done
WIN_A="" WIN_B=""
if [ -n "$WINDOW" ]; then read -r WIN_A WIN_B < <(parse_window "$WINDOW") || fail "créneau invalide : « $WINDOW » (HH:MM-HH:MM)"; fi
for c in "${HWIN[@]}"; do [[ $c =~ ^[a-z_]+:[0-9]+(:[0-9]+)?$ ]] || fail "condition homelabd invalide : « $c » (tâche:secondes)"; done
for s in "${HEALTHY[@]}"; do [[ $s =~ ^[a-z0-9_-]+$ ]] || fail "service invalide : « $s »"; done
LABEL=${LABEL:-$NAME}
JOBDIR=$STATE_DIR/$NAME
UNIT=homelab-offpeak@$NAME

# ------------------------------------------------------------------------------------------------ programmation
if [ "$MODE" = job ]; then
  f=$JOBS_ETC/$NAME.args
  [ -f "$f" ] || fail "travail inconnu : $f absent"
  # le fichier pilote une commande (peut-être en root) : il doit appartenir à root et n'être modifiable que par lui
  [ "$(stat -c '%u' "$f")" = 0 ] && [ $(( 0$(stat -c '%a' "$f") & 022 )) = 0 ] || fail "$f doit appartenir à root, sans droit d'écriture pour les autres"
  mapfile -d '' -t ARGS < "$f"
  # l'utilisateur de la commande est celui du fichier (--as), jamais celui qui lance --job : un « sudo offpeak.sh --job »
  # ne fait pas passer en root la commande d'un travail programmé en deploy
  has_as=0
  for x in "${ARGS[@]}"; do [ "$x" = -- ] && break; [ "$x" = --as ] && has_as=1; done
  [ "$has_as" = 1 ] || ARGS=(--as deploy "${ARGS[@]}")
  exec "$SELF" "${ARGS[@]}"
fi

if [ "$MODE" = schedule ]; then
  [ "$NAME" != adhoc ] || fail "--schedule exige --name"
  [ ${#CMD[@]} -gt 0 ] || fail "commande absente (après --)"
  # l'unité tourne toujours en deploy (offpeak.sh) ; --as root va dans le .args : seule la commande passe en root
  [ "$AS" = root ] && ORIG=(--as root "${ORIG[@]}")
  for at in "${SCHED[@]}"; do systemd-analyze calendar "$at" > /dev/null 2>&1 || fail "calendrier invalide : « $at »"; done
  if [ "$DRY" = 1 ]; then
    # à blanc : ce qui serait installé ; le travail lui-même tournera SANS --dry-run (retiré des arguments)
    kept=() cmd_part=0
    for x in "${ORIG[@]}"; do
      if [ "$cmd_part" = 0 ] && [ "$x" = --dry-run ]; then continue; fi
      [ "$x" = -- ] && cmd_part=1
      kept+=("$x")
    done
    ORIG=("${kept[@]}")
    echo "à blanc : installerait $UNIT.timer (offpeak.sh en $RUN_USER, commande en ${AS:-deploy}), passages : $(printf '%s ; ' "${SCHED[@]}" | sed 's/ ; $//')"
    for at in "${SCHED[@]}"; do systemd-analyze calendar "$at" | sed -n 's/^ *Next elapse: /  prochain passage : /p'; done
    echo "  $JOBS_ETC/$NAME.args : $(printf '%q ' "${ORIG[@]}")"
    exit 0
  fi
  [ "$(id -u)" = 0 ] || fail "--schedule écrit dans /etc/systemd : à lancer avec sudo"
  for u in homelab-offpeak@.service homelab-offpeak@.timer; do
    if ! cmp -s "$UNIT_SRC/$u" "/etc/systemd/system/$u"; then install -m 0644 -o root -g root "$UNIT_SRC/$u" "/etc/systemd/system/$u"; echo "installé /etc/systemd/system/$u"; fi
  done
  install -d -m 0755 -o root -g root "$JOBS_ETC" "/etc/systemd/system/$UNIT.timer.d"
  tmp=$(mktemp "$JOBS_ETC/.$NAME.XXXXXX")
  printf '%s\0' "${ORIG[@]}" > "$tmp" && chmod 0644 "$tmp" && mv -f "$tmp" "$JOBS_ETC/$NAME.args"
  { echo "# écrit par tools/offpeak/offpeak.sh --schedule le $(date '+%F %T')"; echo "[Timer]"; echo "OnCalendar="
    for at in "${SCHED[@]}"; do echo "OnCalendar=$at"; done; } > "/etc/systemd/system/$UNIT.timer.d/when.conf"
  # avant le 2026-10-08, --as root posait User=root dans un drop-in : offpeak.sh tournait en root et rendait le dossier
  # d'état inutilisable pour les travaux en deploy. Un tel drop-in restant est retiré.
  rm -f "/etc/systemd/system/$UNIT.service.d/user.conf"
  rmdir "/etc/systemd/system/$UNIT.service.d" 2>/dev/null || true
  systemctl daemon-reload
  systemctl enable --now "$UNIT.timer" 2>&1 | grep -v '^Created symlink' || true
  echo "programmé : $UNIT.timer ($(printf '%s ; ' "${SCHED[@]}" | sed 's/ ; $//')), commande en ${AS:-deploy}"
  systemctl list-timers --all --no-pager "$UNIT.timer" | head -n 2
  echo "suivi : journalctl -u $UNIT ; $SELF --status $NAME ; retrait : sudo $SELF --unschedule $NAME"
  exit 0
fi

if [ "$MODE" = unschedule ]; then
  [ "$(id -u)" = 0 ] || fail "--unschedule écrit dans /etc/systemd : à lancer avec sudo"
  systemctl disable --now "$UNIT.timer" 2>&1 | grep -v '^Removed' || true
  rm -rf "/etc/systemd/system/$UNIT.timer.d" "/etc/systemd/system/$UNIT.service.d"
  rm -f "$JOBS_ETC/$NAME.args"
  systemctl daemon-reload
  echo "retiré : $UNIT (journal et marqueurs gardés dans $JOBDIR)"
  exit 0
fi

if [ "$MODE" = list ]; then
  shopt -s nullglob
  found=0
  for f in "$JOBS_ETC"/*.args; do
    n=$(basename "$f" .args); found=1
    nxt=$(systemctl show -p NextElapseUSecRealtime --value "homelab-offpeak@$n.timer" 2>/dev/null)
    st=$(sed -n 's/^\(statut\|date\|heure\)=//p' "$STATE_DIR/$n/last" 2>/dev/null | tr '\n' ' ')
    printf '%-24s prochain : %-32s dernier : %s\n' "$n" "${nxt:-—}" "${st:-—}"
  done
  [ "$found" = 1 ] || echo "aucun travail programmé ($JOBS_ETC vide)"
  exit 0
fi

if [ "$MODE" = status ]; then
  [ -r "$JOBDIR/last" ] && cat "$JOBDIR/last" || echo "aucun résultat dans $JOBDIR"
  [ -f "$JOBS_ETC/$NAME.args" ] && { printf 'programmation : '; tr '\0' ' ' < "$JOBS_ETC/$NAME.args"; echo; }
  [ -r "$JOBDIR/journal.log" ] && { echo "--- journal (20 dernières lignes)"; tail -n 20 "$JOBDIR/journal.log"; }
  exit 0
fi

# ------------------------------------------------------------------------------------------------ exécution
[ ${#CMD[@]} -gt 0 ] || fail "commande absente (après --)"
# 2026-10-08 (revue) : offpeak.sh repasse TOUJOURS en deploy, comme l'exécutant du lot 3. Lancé en root, il créait
# state/offpeak (ou .global.lock) en root 0700/0600 : tout travail deploy suivant échouait (mkdir) ou se croyait
# bloqué par « un autre travail » chaque jour. La commande seule passe en root (--as root, par sudo -n).
if [ "$(id -u)" = 0 ]; then
  id -u "$RUN_USER" > /dev/null 2>&1 || fail "utilisateur $RUN_USER absent"
  if [ -z "$AS" ]; then exec runuser -u "$RUN_USER" -- "$SELF" --as root "${ARGV[@]}"; fi
  exec runuser -u "$RUN_USER" -- "$SELF" "${ARGV[@]}"
fi
[ "$(id -un)" = "$RUN_USER" ] || fail "à lancer en tant que $RUN_USER (ou root, qui repasse en $RUN_USER)"
AS=${AS:-deploy}
[ -d "$CD" ] || fail "dossier de travail absent : $CD"

log() {
  local l
  l=$(printf '[%s] [%s] %s' "$(date '+%F %T')" "$NAME" "$*")
  printf '%s\n' "$l"
  [ "$DRY" = 1 ] || printf '%s\n' "$l" 2>/dev/null >> "$JOBDIR/journal.log" || true
}
ecrire() { local f=$1; shift; printf '%s\n' "$*" > "$f.tmp.$$" && mv -f "$f.tmp.$$" "$f"; }
TXT=""
bilan() {  # texte de la ligne de résultat
  [ "$DISCORD" = 1 ] || return 0
  local f
  TXT="**Hors pic · $(nettoie "$LABEL" 80)** — $(date_fr)
$1"
  [ -n "${RUNLOG:-}" ] && TXT+="
Sortie : ${RUNLOG#"$HL"/}"
  if [ "$DRY" = 1 ]; then log "à blanc : message Discord NON envoyé :"; printf '%s\n' "$TXT" | sed 's/^/    | /'; return 0; fi
  # texte par l'entrée standard (aucun fichier : le bilan part même si le dossier d'état est inutilisable)
  local r
  r=$(printf '%s\n' "$TXT" | "$HLPY" discord - 2>&1) || { log "  bilan Discord NON envoyé ($(nettoie "$r" 120))"; return 0; }
  log "  $r"
}
# erreur avant toute garde (dossier d'état, verrous, sudo) : bilan si demandé, puis code 2 (unité en échec, visible)
abort() {
  log "ERREUR : $*"
  bilan "❌ NON lancé : $(nettoie "$*" 240)"
  exit 2
}

if [ "$DRY" = 0 ]; then
  mkdir -p "$JOBDIR/runs" "$JOBDIR/tmp" 2>/dev/null || abort "impossible de créer $JOBDIR (droits ?)"
  chmod 700 "$STATE_DIR" "$JOBDIR" "$JOBDIR/runs" "$JOBDIR/tmp" 2>/dev/null
  for d in "$STATE_DIR" "$JOBDIR" "$JOBDIR/runs" "$JOBDIR/tmp"; do [ -w "$d" ] || abort "$d non inscriptible par $(id -un) (créé par root ? chown $RUN_USER)"; done
  # code de retour vérifié : un verrou illisible n'est pas « un autre travail tourne »
  { exec 8>>"$JOBDIR/lock"; } 2>/dev/null || abort "verrou $JOBDIR/lock inutilisable (droits ?)"
  { exec 9>>"$STATE_DIR/.global.lock"; } 2>/dev/null || abort "verrou global $STATE_DIR/.global.lock inutilisable (droits ?)"
  flock -n 8 || { echo "un passage de « $NAME » tourne déjà : rien à faire"; exit 3; }
  if [ "$ONCE" = 1 ] && [ -f "$JOBDIR/done" ]; then echo "« $NAME » déjà fait ($(tr '\n' ' ' < "$JOBDIR/done")) : rien à faire"; exit 0; fi
fi
# commande en root : sudo -n doit marcher (sinon refus franc, pas une erreur au milieu du créneau)
if [ "$AS" = root ]; then
  if sudo -n true 2>/dev/null; then SUDO_OK=1; else SUDO_OK=0; fi
  [ "$SUDO_OK" = 1 ] || [ "$DRY" = 1 ] || abort "--as root : sudo -n refusé pour $(id -un)"
fi
STOP=0
trap 'STOP=1' TERM INT HUP

# fin des essais : fin du créneau, sinon --max-wait
T0=$(date +%s)
if [ -n "$WINDOW" ]; then
  left=$(window_left "$WIN_A" "$WIN_B")
  if [ "$left" = 0 ]; then echo "hors créneau ($(hm "$WIN_A")–$(hm "$WIN_B")) : rien à faire"; exit 3; fi
  DEADLINE=$(( T0 + left ))
else
  DEADLINE=$(( T0 + MAXWAIT * 60 ))
fi

# --busy : services lot3-* seulement ; les autres travaux hors pic se suivent par le verrou global (2026-10-08 : deux
# homelab-offpeak@ en attente se refusaient l'un l'autre jusqu'à la fin de leur créneau)
GARGS=(--busy)
[ "$IDLE" = 1 ] && GARGS+=(--idle)
[ "$JFTASKS" = 1 ] && GARGS+=(--jf-tasks)
[ "$IDLEDOWN" = 1 ] && GARGS+=(--idle-if-down)
[ "$SEEDBOX" = 1 ] && GARGS+=(--seedbox)
for c in "${HWIN[@]}"; do GARGS+=(--homelabd "$c"); done

log "$([ "$DRY" = 1 ] && echo 'À BLANC : ')« $LABEL » : $(printf '%q ' "${CMD[@]}" | cut -c1-200)$([ -n "$WINDOW" ] && echo " (créneau $WINDOW)")"
REFUS="" attempt=0
while :; do
  [ "$STOP" = 1 ] && { log "arrêt demandé avant le lancement : rien n'est fait"; exit 3; }
  attempt=$(( attempt + 1 ))
  # un seul travail hors pic à la fois (verrou global tenu jusqu'à la fin de la commande)
  if [ "$DRY" = 0 ]; then
    if ! flock -n 9; then out="refus un autre travail hors pic tourne"; rc=1; else out=""; fi
  else out=""; fi
  if [ -z "$out" ]; then
    # fenêtre homelabd : attente courte (12 min au plus), comme le lot 3
    wend=$(( $(date +%s) + 720 )); first=1
    while :; do
      out=$("$HLPY" guard "${GARGS[@]}"); rc=$?
      [ "$rc" = 4 ] || break
      s=$(awk '{print $2}' <<< "$out")
      why=${out#attendre "$s" }; vertes=${why#* || }; why=${why%% || *}
      if [ "$DRY" = 1 ]; then
        log "  à blanc : attendrait ~$s s ($why) puis relancerait toutes les gardes"
        out="ok $vertes ; fenêtre homelabd attendue (à blanc)"; rc=0; break
      fi
      if [ "$(date +%s)" -ge "$wend" ] || [ $(( $(date +%s) + s )) -ge "$DEADLINE" ]; then out="refus pas de fenêtre homelabd sûre ($why)"; rc=1; break; fi
      [ "$first" = 1 ] && log "  attente d'une fenêtre homelabd sûre ($why ; déjà vertes : $vertes)"
      first=0; sleep "$s"
    done
  fi
  [ "$STOP" = 1 ] && { log "arrêt demandé avant le lancement : rien n'est fait"; exit 3; }
  if [ "$rc" = 0 ]; then log "  gardes : ${out#ok }"; break; fi
  REFUS=${out#refus }
  [ "$DRY" = 0 ] && flock -u 9
  now=$(date +%s)
  if [ "$RETRY" = 0 ] || [ $(( now + RETRY * 60 )) -ge "$DEADLINE" ]; then
    log "  REFUS ($REFUS) : rien n'est lancé ; $([ -n "$WINDOW" ] && echo "plus d'essai possible dans le créneau" || echo 'abandon')"
    [ "$DRY" = 0 ] && ecrire "$JOBDIR/last" "statut=non-lance
date=$(date +%Y%m%d)
heure=$(date +%H:%M)
note=$(nettoie "$REFUS" 240)"
    if [ "$attempt" -gt 1 ] || [ -n "$WINDOW" ]; then
      bilan "⏸️ NON lancé : $(nettoie "$REFUS" 240)$([ -n "$WINDOW" ] && echo " (créneau $WINDOW, $attempt essai(s)) — rien n'a changé")"
    fi
    exit 3
  fi
  log "  REFUS ($REFUS) : nouvel essai dans $RETRY min"
  sleep $(( RETRY * 60 )) &
  wait $! || true
done

# ------------------------------------------------------------------------------------------------ lancement
gluetun_id() { docker inspect -f '{{.Id}}' gluetun 2>/dev/null || true; }
G_BEFORE=""
[ "$GLUETUN" = 1 ] && G_BEFORE=$(gluetun_id)
if [ "$DRY" = 1 ]; then
  log "  à blanc : lancerait $(printf '%q ' "${CMD[@]}" | cut -c1-200)dans $CD en $AS$([ "$AS" = root ] && echo " par sudo -n ($([ "$SUDO_OK" = 1 ] && echo possible || echo 'REFUSÉ : le vrai passage échouerait'))") (délai $(duree "$TIMEOUT"))"
  [ ${#HEALTHY[@]} -gt 0 ] && log "  à blanc : vérifierait ensuite : ${HEALTHY[*]} sain(s)"
  bilan "✅ (à blanc) serait lancé à $(date +%H:%M) : gardes vertes"
  exit 0
fi
RUNID=$(date +%Y%m%d-%H%M%S)
RUNLOG=$JOBDIR/runs/$RUNID.log
RUNTMP=$JOBDIR/tmp/$RUNID   # dossier temporaire propre au passage, retiré ensuite
mkdir -p "$RUNTMP"
if [ "$AS" = root ]; then RUNNER=(sudo -n --); U=root; else RUNNER=(); U=$(id -un); fi
HOME_U=$(getent passwd "$U" | cut -d: -f6)
log "  lancement en $U (sortie : ${RUNLOG#"$HL"/})"
t0=$(date +%s); H0=$(date +%H:%M)
( cd "$CD" && exec "${RUNNER[@]}" env -i PATH="$PATH_PROPRE" HOME="${HOME_U:-/tmp}" USER="$U" LOGNAME="$U" SHELL=/bin/bash \
    LANG=C.UTF-8 TMPDIR="$RUNTMP" OFFPEAK_NAME="$NAME" timeout -k 120 "$TIMEOUT" "${CMD[@]}" ) > "$RUNLOG" 2>&1 8>&- 9>&- < /dev/null
rc=$?
if [ "$AS" = root ]; then sudo -n rm -rf --one-file-system -- "$RUNTMP" 2>/dev/null; else rm -rf --one-file-system -- "$RUNTMP" 2>/dev/null; fi
dt=$(( $(date +%s) - t0 ))
case "$rc" in
  0)   statut=fait; note="code 0" ;;
  124|137) statut=echec; note="délai de $(duree "$TIMEOUT") dépassé (code $rc)" ;;
  *)   statut=echec; note="code $rc" ;;
esac
log "  fin de la commande : $note en $(duree "$dt")"

# garde gluetun : qBittorrent sur l'espace réseau d'un gluetun recréé par la commande
if [ -n "$G_BEFORE" ]; then
  G_AFTER=$(gluetun_id)
  if [ -n "$G_AFTER" ] && [ "$G_AFTER" != "$G_BEFORE" ]; then
    if [ "$(docker inspect -f '{{.HostConfig.NetworkMode}}' qbittorrent 2>/dev/null)" = "container:$G_AFTER" ]; then
      log "  gluetun recréé : qBittorrent déjà sur le nouvel espace réseau"
    else
      log "  gluetun recréé et qBittorrent resté sur l'ancien espace réseau : recréation de qBittorrent (--no-deps)"
      ( cd "$HL" && docker compose config --quiet && docker compose up -d --force-recreate --no-deps qbittorrent ) >> "$RUNLOG" 2>&1
      # adresse de l'API qBittorrent vue de l'hôte : [urls] de homelab.toml
      ok=0; qb=$("$HLPY" url qbittorrent 2>/dev/null) || qb=""
      for _ in $(seq 1 40); do
        if [ "$(docker inspect -f '{{.HostConfig.NetworkMode}}' qbittorrent 2>/dev/null)" = "container:$G_AFTER" ] \
           && { [ -z "$qb" ] || curl -fsS -m 10 -o /dev/null "$qb/api/v2/app/version"; }; then ok=1; break; fi
        sleep 3
      done
      [ -n "$qb" ] || log "  [urls] qbittorrent absent du TOML : seul l'espace réseau de qBittorrent est vérifié"
      if [ "$ok" = 1 ]; then
        # port transféré reposé par le hook de gluetun, comme la sonde stack_health de homelabd
        # shellcheck disable=SC2016  # $(cat …) est évalué dans le conteneur gluetun
        if docker exec gluetun sh -c '/gluetun/scripts/qbit-update-port.sh "$(cat /tmp/gluetun/forwarded_port)"' >> "$RUNLOG" 2>&1; then
          log "  qBittorrent recréé, port transféré reposé"
        else
          log "  qBittorrent recréé ; port transféré NON reposé (voir la sortie)"
        fi
      else
        statut=echec; note="$note ; qBittorrent injoignable après la recréation de gluetun"
        log "  ÉCHEC : qBittorrent ne répond pas après sa recréation"
      fi
    fi
  fi
fi

# services qui doivent redevenir sains
if [ "$statut" = fait ]; then
  for s in "${HEALTHY[@]}"; do
    okh=0
    for _ in $(seq 1 60); do
      st=$(cd "$HL" && docker compose ps --format '{{.State}} {{.Health}}' "$s" 2>/dev/null | head -n 1)
      case "$st" in "running healthy"|"running ") okh=1; break ;; esac
      sleep 10
    done
    if [ "$okh" = 1 ]; then log "  $s : sain"; else statut=echec; note="$note ; $s pas sain après 10 min (${st:-absent})"; log "  ÉCHEC : $s pas sain (${st:-absent})"; fi
  done
fi

ecrire "$JOBDIR/last" "statut=$statut
date=$(date +%Y%m%d)
heure=$(date +%H:%M)
code=$rc
duree=$dt
note=$(nettoie "$note" 240)
sortie=${RUNLOG#"$HL"/}"
if [ "$statut" = fait ]; then
  [ "$ONCE" = 1 ] && ecrire "$JOBDIR/done" "$(date '+%F %T')"
  log "OK : « $LABEL » fait en $(duree "$dt")"
  bilan "✅ fait à $H0 ($(duree "$dt"), $note)"
  exit 0
fi
log "ÉCHEC : « $LABEL » ($note)"
bilan "❌ ÉCHEC à $H0 ($(duree "$dt"), $(nettoie "$note" 200)) — voir la sortie"
exit 1
