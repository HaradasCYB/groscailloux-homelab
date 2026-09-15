#!/usr/bin/env python3
"""Synchronise les scripts « Groscailloux » du plugin JavaScript Injector depuis branding/jellyfin/.

    scripts/jellyfin-js-apply.py [--dry-run]

Scripts gérés (nom dans le plugin → fichier) ; tous « Requires authentication ». Les autres scripts
(personnels ou enregistrés par d'autres plugins, ex. Jellysleep) ne sont pas touchés. La configuration
précédente est sauvegardée dans backups/jellyfin-plugin-JavaScriptInjector-<date>.json. Effet au
prochain chargement de page des clients.
"""
import datetime
import json
import os
import sys
import urllib.request

BASE = "/opt/homelab"
PLUGIN = "f5a34f7b2e8a4e6aa7223a216a81b374"  # JavaScript Injector
SCRIPTS = {
    "Groscailloux Tchat": "branding/jellyfin/gc-chat-loader.js",
    "Groscailloux Lire sur": "branding/jellyfin/gc-cast-filter.js",
}

env = dict(l.split("=", 1) for l in open(f"{BASE}/.env").read().splitlines() if "=" in l and not l.startswith("#"))
KEY = env["JELLYFIN_API_KEY"].strip().strip('"')
URL = f"http://localhost:8096/Plugins/{PLUGIN}/Configuration"


def call(method, body=None):
    req = urllib.request.Request(URL, method=method, data=json.dumps(body).encode() if body is not None else None)
    req.add_header("X-Emby-Token", KEY)
    req.add_header("Content-Type", "application/json")
    with urllib.request.urlopen(req) as r:
        data = r.read()
        return json.loads(data.decode("utf-8-sig")) if data else None


conf = call("GET")
current = {e.get("Name"): e for e in conf.get("CustomJavaScripts", [])}
wanted = {name: open(f"{BASE}/{path}").read() for name, path in SCRIPTS.items()}
changes = [n for n, s in wanted.items() if n not in current or current[n].get("Script") != s
           or not current[n].get("Enabled") or not current[n].get("RequiresAuthentication")]
if not changes:
    print("déjà à jour")
    sys.exit(0)
print("à mettre à jour :", ", ".join(changes))
if "--dry-run" in sys.argv:
    sys.exit(0)
bak = f"{BASE}/backups/jellyfin-plugin-JavaScriptInjector-{datetime.datetime.now():%Y%m%d-%H%M%S}.json"
with open(bak, "w") as f:
    json.dump(conf, f)
os.chmod(bak, 0o600)
others = [e for e in conf.get("CustomJavaScripts", []) if e.get("Name") not in wanted]
conf["CustomJavaScripts"] = others + [
    {"Name": n, "Script": s, "Enabled": True, "RequiresAuthentication": True} for n, s in wanted.items()
]
call("POST", conf)
after = {e.get("Name"): e.get("Script") for e in call("GET").get("CustomJavaScripts", [])}
assert all(after.get(n) == s for n, s in wanted.items()), "vérification échouée"
print(f"appliqué ; ancienne configuration : {bak.replace(BASE + '/', '')}")
