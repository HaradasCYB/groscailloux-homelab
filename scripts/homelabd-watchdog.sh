#!/usr/bin/env bash
# Chien de garde de homelabd (2026-10-02, audit de résilience) — lancé toutes les 2 min par
# systemd/homelabd-watchdog.timer, en root, indépendamment de homelabd.
#
# systemd relance déjà homelabd s'il s'ARRÊTE (Restart=always). Ce script couvre l'autre cas : homelabd vivant mais
# figé (plus de recherches, d'imports ni de pages membres, sans alerte). `/health` qui ne répond pas 3 fois de suite
# (≈ 6 min) → `systemctl restart homelabd` + message Discord admin ; un second message quand il répond de nouveau.
#
#   homelabd-watchdog.sh            un passage
#   homelabd-watchdog.sh --check    état seulement, ne relance rien
#
# Journal (2026-10-07) : l'unité est en LogLevelMax=notice. Le script n'écrit rien tant que /health répond ; ses
# lignes `logger` portent une priorité explicite (warning : échec et relance, notice : retour à la normale).
set -u
MODE="${1:-}"
URL="${HOMELABD_HEALTH_URL:-http://127.0.0.1:8766/health}"
STATE=/run/homelabd-watchdog
mkdir -p "$STATE"
FAILS="$STATE/fails"
ALERTED="$STATE/alerted"
LIMIT=3

webhook() {
  local w
  w=$(grep -E '^DISCORD_WEBHOOK_ADMIN=' /opt/homelab/.env 2>/dev/null | cut -d= -f2- | tr -d '"')
  [ -z "$w" ] && w=$(grep -E '^DISCORD_WEBHOOK_MEMBERS=' /opt/homelab/.env 2>/dev/null | cut -d= -f2- | tr -d '"')
  echo "$w"
}

discord() { # $1 = titre, $2 = texte, $3 = couleur
  local w; w=$(webhook)
  [ -z "$w" ] && return 0
  local payload
  payload=$(python3 -c 'import json,sys; print(json.dumps({"embeds":[{"title":sys.argv[1],"description":sys.argv[2],"color":int(sys.argv[3])}]}))' "$1" "$2" "$3")
  curl -s -m 15 -H 'Content-Type: application/json' -d "$payload" "$w" >/dev/null 2>&1 || true
}

if curl -fsS -m 10 "$URL" >/dev/null 2>&1; then
  if [ -f "$ALERTED" ]; then
    logger -t homelabd-watchdog -p user.notice "homelabd répond de nouveau"
    [ "$MODE" = "--check" ] || discord "homelabd répond de nouveau" "Le chien de garde l'avait relancé ; /health répond." 3066993
    rm -f "$ALERTED"
  fi
  rm -f "$FAILS"
  [ "$MODE" = "--check" ] && echo "homelabd répond"
  exit 0
fi

n=$(( $(cat "$FAILS" 2>/dev/null || echo 0) + 1 ))
echo "$n" > "$FAILS"
state=$(systemctl is-active homelabd 2>/dev/null)
logger -t homelabd-watchdog -p user.warning "homelabd ne répond pas sur /health (échec $n/$LIMIT, état systemd : $state)"
if [ "$MODE" = "--check" ]; then
  echo "homelabd ne répond pas (échec $n/$LIMIT, état systemd : $state)"
  exit 0
fi
[ "$n" -lt "$LIMIT" ] && exit 0

logger -t homelabd-watchdog -p user.warning "relance de homelabd"
systemctl restart homelabd
rm -f "$FAILS"
touch "$ALERTED"
discord "homelabd relancé par le chien de garde" \
  "/health ne répondait plus depuis environ $((LIMIT * 2)) min (état systemd : $state). homelabd a été redémarré. Voir \`journalctl -u homelabd\` et \`journalctl -t homelabd-watchdog\`." \
  15105570
exit 0
