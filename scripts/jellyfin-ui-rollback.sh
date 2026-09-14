#!/usr/bin/env bash
# Retour arrière de la refonte d'interface Jellyfin (« Groscailloux TV »).
#
#   scripts/jellyfin-ui-rollback.sh <dossier de sauvegarde> [--with-db] [--dry-run]
#
# Restaure, depuis backups/jellyfin-ui-<date>/ :
#   - config/*.xml (dont le CSS de branding) et plugins/ (DLL + configurations) ;
#   - les préférences d'affichage de chaque compte (API DisplayPreferences, après redémarrage).
# --with-db remet aussi jellyfin.db : à éviter (efface l'historique de visionnage postérieur à la
# sauvegarde) ; seulement si Jellyfin ne démarre plus.
# Jellyfin est arrêté pendant la copie : lancer quand personne ne regarde.
set -euo pipefail

BASE=/opt/homelab
SRC=${1:?usage: $0 <backups/jellyfin-ui-...> [--with-db] [--dry-run]}
shift || true
WITH_DB=0
DRY=0
for a in "$@"; do
  case "$a" in
    --with-db) WITH_DB=1 ;;
    --dry-run) DRY=1 ;;
    *) echo "option inconnue : $a" >&2; exit 2 ;;
  esac
done
[[ "$SRC" = /* ]] || SRC="$BASE/$SRC"
for f in config plugins users-display-prefs.json; do
  [[ -e "$SRC/$f" ]] || { echo "sauvegarde incomplète : $SRC/$f absent" >&2; exit 1; }
done
run() { if ((DRY)); then echo "[dry-run] $*"; else "$@"; fi; }

cd "$BASE"
set -a; . ./.env; set +a
J=http://localhost:8096

playing=$(curl -fsS -H "X-Emby-Token: $JELLYFIN_API_KEY" "$J/Sessions?activeWithinSeconds=120" |
  python3 -c 'import sys,json; print(sum(1 for s in json.load(sys.stdin) if s.get("NowPlayingItem")))')
if ((playing > 0)) && ((!DRY)); then
  echo "$playing lecture(s) en cours : relancer plus tard (Jellyfin va redémarrer)" >&2
  exit 1
fi

run docker compose stop jellyfin
# copie de ce qui va être écrasé, pour pouvoir annuler l'annulation
STAMP=$(date +%Y%m%d-%H%M%S)
run mkdir -p "backups/jellyfin-ui-before-rollback-$STAMP"
run cp -a jellyfin/config/config jellyfin/config/plugins "backups/jellyfin-ui-before-rollback-$STAMP/"
run rsync -a --delete "$SRC/config/" jellyfin/config/config/
run rsync -a --delete "$SRC/plugins/" jellyfin/config/plugins/
if ((WITH_DB)); then
  run cp -a jellyfin/config/data/jellyfin.db "backups/jellyfin-ui-before-rollback-$STAMP/"
  run rm -f jellyfin/config/data/jellyfin.db-wal jellyfin/config/data/jellyfin.db-shm
  run cp "$SRC/jellyfin.db" jellyfin/config/data/jellyfin.db
fi
run docker compose up -d jellyfin

if ((!DRY)); then
  until [[ "$(docker inspect -f '{{.State.Health.Status}}' jellyfin 2>/dev/null)" == healthy ]]; do sleep 2; done
fi
# préférences d'affichage (accueil, thème par compte), par nom de compte
run python3 - "$SRC/users-display-prefs.json" <<'EOF'
import json, os, sys, urllib.request
J = "http://localhost:8096"; K = os.environ["JELLYFIN_API_KEY"]
def call(method, path, body=None):
    r = urllib.request.Request(J + path, method=method,
                               data=json.dumps(body).encode() if body is not None else None)
    r.add_header("X-Emby-Token", K); r.add_header("Content-Type", "application/json")
    with urllib.request.urlopen(r) as x:
        d = x.read(); return json.loads(d) if d else None
saved = json.load(open(sys.argv[1]))
ids = {u["Name"]: u["Id"] for u in call("GET", "/Users")}
for name, d in saved.items():
    uid = ids.get(name)
    if not uid:
        print(f"  {name} : compte absent, ignoré"); continue
    for client, prefs in (d.get("prefs") or {}).items():
        if prefs:
            call("POST", f"/DisplayPreferences/usersettings?userId={uid}&client={client}", prefs)
    if d.get("Configuration"):
        call("POST", f"/Users/{uid}/Configuration", d["Configuration"])
    print(f"  {name} : préférences restaurées")
EOF
echo "retour arrière terminé depuis $SRC"
