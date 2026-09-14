#!/usr/bin/env bash
# Applique le CSS « Groscailloux TV » (branding/jellyfin/groscailloux-tv.css) au branding Jellyfin.
#
#   scripts/jellyfin-branding-apply.sh [fichier.css] [--dry-run]
#
# Le CSS précédent est sauvegardé dans backups/jellyfin-branding-<date>.json. Aucun redémarrage :
# les clients le prennent au prochain chargement de page. Retour arrière : réappliquer l'ancien
# CustomCss (champ du JSON sauvegardé) ou scripts/jellyfin-ui-rollback.sh.
set -euo pipefail
BASE=/opt/homelab
CSS=$BASE/branding/jellyfin/groscailloux-tv.css
DRY=0
for a in "$@"; do
  case "$a" in
    --dry-run) DRY=1 ;;
    *) CSS=$a ;;
  esac
done
[[ -s "$CSS" ]] || { echo "CSS introuvable ou vide : $CSS" >&2; exit 1; }
grep -qE '@import[^;]*@(main|master|latest)[/"]' "$CSS" && { echo "refus : @import non épinglé (@main/@master/@latest) dans $CSS" >&2; exit 1; }
cd "$BASE"
set -a; . ./.env; set +a
DRY=$DRY python3 - "$CSS" <<'EOF'
import datetime, json, os, sys, urllib.request
J = "http://localhost:8096"; K = os.environ["JELLYFIN_API_KEY"]
def call(method, path, body=None):
    r = urllib.request.Request(J + path, method=method, data=json.dumps(body).encode() if body is not None else None)
    r.add_header("X-Emby-Token", K); r.add_header("Content-Type", "application/json")
    with urllib.request.urlopen(r) as x:
        d = x.read(); return json.loads(d) if d else None
css = open(sys.argv[1]).read()
cur = call("GET", "/System/Configuration/branding")
if cur.get("CustomCss", "") == css:
    print("déjà appliqué, rien à faire"); sys.exit(0)
print(f"CSS actuel : {len(cur.get('CustomCss') or '')} car. → nouveau : {len(css)} car.")
if os.environ.get("DRY") == "1":
    print("dry-run : rien d'appliqué"); sys.exit(0)
stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
bak = f"backups/jellyfin-branding-{stamp}.json"
with open(bak, "w") as f: json.dump(cur, f, ensure_ascii=False, indent=1)
os.chmod(bak, 0o600)
cur["CustomCss"] = css
call("POST", "/System/Configuration/branding", cur)
assert call("GET", "/System/Configuration/branding").get("CustomCss") == css
print(f"appliqué ; ancien branding sauvegardé dans {bak}")
EOF
