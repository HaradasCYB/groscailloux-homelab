#!/usr/bin/env bash
# Alerte Discord admin quand une unité systemd échoue (2026-10-07, revue Kaizen). Lancé par
# systemd/homelab-alert@.service, que `OnFailure=homelab-alert@%n.service` déclenche pour homelab-backup,
# homelabd-watchdog, seedbox-mount-watch et jellyfin-transcodes-purge.
#
# Pourquoi : ces unités tournent en root, hors de homelabd, donc sans aucun chemin d'alerte. Une sauvegarde qui plante
# ou un minuteur de surveillance dont le script casse ne prévenaient personne (les chiens de garde eux-mêmes n'étaient
# surveillés par rien). Même mécanisme que homelabd-watchdog.sh : webhook DISCORD_WEBHOOK_ADMIN lu dans .env
# (repli sur DISCORD_WEBHOOK_MEMBERS), embed posté par curl. L'URL du webhook n'est jamais affichée ni journalisée,
# et masquée si elle apparaissait dans les lignes de journal reprises.
#
# Message : « <unité> en échec », résultat de l'unité (code de sortie) et ses 20 dernières lignes de journal.
# Un minuteur à la minute (jellyfin-transcodes-purge) dont le script plante à chaque passage ferait un message par
# minute : au plus UN message par unité et par COOLDOWN secondes (3600 par défaut), le suivant indique combien
# d'échecs ont été tus entre-temps.
#
#   homelab-alert.sh <unité>             poste l'alerte (ou la tait, si une est partie il y a moins d'une heure)
#   homelab-alert.sh --dry-run <unité>   affiche le message qui serait posté, sans rien envoyer ni noter
#
# Variables (essais) : HOMELAB_ENV_FILE (défaut /opt/homelab/.env), HOMELAB_ALERT_STATE (défaut /run/homelab-alert),
# COOLDOWN (secondes).
set -u
DRY=0
if [ "${1:-}" = "--dry-run" ]; then DRY=1; shift; fi
UNIT="${1:-}"
ENV_FILE="${HOMELAB_ENV_FILE:-/opt/homelab/.env}"
STATE="${HOMELAB_ALERT_STATE:-/run/homelab-alert}"
COOLDOWN="${COOLDOWN:-3600}"

# Journal : <4> = avertissement, <5> = notice (préfixes syslog lus par systemd quand la sortie va au journal)
say() { logger -t homelab-alert -p "user.${2:-notice}" -- "$1"; [ "$DRY" = 1 ] && echo "[$(date '+%F %T')] $1"; return 0; }

case "$UNIT" in
  ''|*[!A-Za-z0-9@:_.\\-]*) echo "usage: $0 [--dry-run] <unité systemd>" >&2; exit 2 ;;
esac

webhook() {
  local w
  w=$(grep -E '^DISCORD_WEBHOOK_ADMIN=' "$ENV_FILE" 2>/dev/null | cut -d= -f2- | tr -d '"')
  [ -z "$w" ] && w=$(grep -E '^DISCORD_WEBHOOK_MEMBERS=' "$ENV_FILE" 2>/dev/null | cut -d= -f2- | tr -d '"')
  echo "$w"
}

# Cooldown par unité (sur /run : remis à zéro au redémarrage de la machine, ce qui est voulu)
mkdir -p "$STATE" 2>/dev/null
SAFE=$(printf '%s' "$UNIT" | tr -c 'A-Za-z0-9_.-' '_')
LAST_FILE="$STATE/$SAFE.last"
MUTED_FILE="$STATE/$SAFE.muted"
now=$(date +%s)
if [ "$DRY" = 0 ] && [ -f "$LAST_FILE" ]; then
  last=$(cat "$LAST_FILE" 2>/dev/null || echo 0)
  case "$last" in ''|*[!0-9]*) last=0 ;; esac
  if [ $((now - last)) -lt "$COOLDOWN" ]; then
    muted=$(( $(cat "$MUTED_FILE" 2>/dev/null || echo 0) + 1 ))
    echo "$muted" > "$MUTED_FILE"
    say "$UNIT en échec de nouveau : alerte déjà envoyée il y a $(( (now - last) / 60 )) min, $muted échec(s) tu(s) depuis" notice
    exit 0
  fi
fi

W=$(webhook)
if [ -z "$W" ]; then
  say "$UNIT en échec, mais aucun webhook Discord admin n'est configuré dans $ENV_FILE : rien posté" err
  exit 0
fi

result=$(systemctl show -p Result -p ExecMainStatus "$UNIT" 2>/dev/null | tr '\n' ' ' | sed 's/ *$//')
# 20 dernières lignes du journal de l'unité, sans l'URL d'un webhook si elle y traînait, sans fermer le bloc de code
lines=$(journalctl -u "$UNIT" -n 20 --no-pager -o short-iso --no-hostname 2>/dev/null \
  | sed -E 's#https?://[A-Za-z0-9.-]*discord(app)?\.com/api/webhooks/[^[:space:]]*#[webhook masqué]#g; s#```#'"'''"'#g')
[ -n "$lines" ] || lines="(aucune ligne de journal)"
muted=0
[ -f "$MUTED_FILE" ] && muted=$(cat "$MUTED_FILE" 2>/dev/null || echo 0)
case "$muted" in ''|*[!0-9]*) muted=0 ;; esac
extra=""
[ "$muted" -gt 0 ] && extra=$'\n'"$muted échec(s) de plus ont été tus depuis le dernier message."

title="$UNIT en échec"
desc="Résultat : ${result:-inconnu}${extra}"$'\n\n```\n'"$lines"$'\n```\n'"Voir \`journalctl -u $UNIT\` et \`systemctl status $UNIT\`."

payload=$(python3 -c '
import json, sys
title, desc = sys.argv[1], sys.argv[2]
if len(desc) > 4000:
    # on garde la fin du bloc (les lignes les plus récentes) et on referme le bloc de code
    desc = desc[:200] + "\n…\n" + desc[-3600:]
    if desc.count("```") % 2: desc += "\n```"
print(json.dumps({"username": "Groscailloux", "embeds": [{"title": title[:256], "description": desc, "color": 15680580}]}))
' "$title" "$desc") || { say "$UNIT en échec : message non construit (python3)" err; exit 0; }

if [ "$DRY" = 1 ]; then
  echo "$payload"
  exit 0
fi

code=$(curl -s -m 15 -o /dev/null -w '%{http_code}' -H 'Content-Type: application/json' -d "$payload" "$W" 2>/dev/null || true)
case "$code" in
  2??)
    echo "$now" > "$LAST_FILE"; rm -f "$MUTED_FILE"
    say "alerte envoyée sur Discord : $UNIT en échec" notice ;;
  *)
    say "alerte NON postée pour $UNIT : Discord a répondu '${code:-aucune réponse}'" warning ;;
esac
exit 0
