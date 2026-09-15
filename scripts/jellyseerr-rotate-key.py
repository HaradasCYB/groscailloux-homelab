#!/usr/bin/env python3
# Usage : sudo scripts/jellyseerr-rotate-key.py   (Homarr redémarré ; sauvegardes dans backups/)
"""Rotation de la clé API Jellyseerr. Aucune clé n'est affichée.
Consommateurs : .env (homelabd), Jellyfin Enhanced + Home Screen Sections (JellyseerrApiKey),
Homarr (integrationSecret chiffré, intégration « Jellyseerr »)."""
import datetime, json, os, re, shutil, sqlite3, subprocess, sys, time, urllib.error, urllib.request
from cryptography.hazmat.primitives import padding
from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes

BASE = "/opt/homelab"
STAMP = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
ENVF = f"{BASE}/.env"
ENV = dict(l.split("=", 1) for l in open(ENVF).read().splitlines() if "=" in l and not l.startswith("#"))
val = lambda k: ENV[k].strip().strip('"')
OLD, JFK = val("JELLYSEERR_API_KEY"), val("JELLYFIN_API_KEY")
HKEY = bytes.fromhex(val("SECRET_ENCRYPTION_KEY"))
JS, JF = "http://localhost:5055/api/v1", "http://localhost:8096"
PLUGINS = {"b8298e012697407ab44daa8dc795e850": "HomeScreenSections", "f69e946a4b3c4e9a8f0a8d7c1b2c4d9b": "JellyfinEnhanced"}

def http(method, url, headers, body=None):
    req = urllib.request.Request(url, method=method, data=json.dumps(body).encode() if body is not None else None)
    for k, v in headers.items(): req.add_header(k, v)
    if body is not None: req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            d = r.read(); return r.status, (json.loads(d) if d else None)
    except urllib.error.HTTPError as e:
        return e.code, None

def enc(plain):
    iv = os.urandom(16); p = padding.PKCS7(128).padder(); data = p.update(plain.encode()) + p.finalize()
    e = Cipher(algorithms.AES(HKEY), modes.CBC(iv)).encryptor(); return (e.update(data) + e.finalize()).hex() + "." + iv.hex()

def dec(v):
    c, iv = v.split("."); d = Cipher(algorithms.AES(HKEY), modes.CBC(bytes.fromhex(iv))).decryptor()
    u = padding.PKCS7(128).unpadder(); return (u.update(d.update(bytes.fromhex(c)) + d.finalize()) + u.finalize()).decode()

def save(path, data):
    with open(path, "w") as f: f.write(data)
    os.chmod(path, 0o600)

# 0. sauvegardes
shutil.copy2(ENVF, f"{BASE}/backups/env-{STAMP}-jskey.bak"); os.chmod(f"{BASE}/backups/env-{STAMP}-jskey.bak", 0o600)
confs = {}
for pid, name in PLUGINS.items():
    st, c = http("GET", f"{JF}/Plugins/{pid}/Configuration", {"X-Emby-Token": JFK})
    assert st == 200 and c.get("JellyseerrApiKey") == OLD, f"{name} : config inattendue ({st})"
    confs[pid] = c
    save(f"{BASE}/backups/jellyfin-plugin-{name}-{STAMP}.json", json.dumps(c))
assert http("GET", f"{JS}/settings/main", {"X-Api-Key": OLD})[0] == 200, "ancienne clé déjà refusée ?"
print("sauvegardes faites")

# 1. nouvelle clé
st, s = http("POST", f"{JS}/settings/main/regenerate", {"X-Api-Key": OLD}, {})
NEW = (s or {}).get("apiKey")
assert st == 200 and NEW and NEW != OLD, f"régénération : HTTP {st}"
t0 = time.time()
print("nouvelle clé générée par Jellyseerr")

# 2. .env (même format de ligne)
txt = open(ENVF).read()
lines = [l for l in txt.splitlines() if l.startswith("JELLYSEERR_API_KEY=")]
assert len(lines) == 1
q = '"' if lines[0].split("=", 1)[1].startswith('"') else ""
save(ENVF, txt.replace(lines[0], f"JELLYSEERR_API_KEY={q}{NEW}{q}"))

# 3. plugins Jellyfin
for pid, name in PLUGINS.items():
    c = confs[pid]; c["JellyseerrApiKey"] = NEW
    st, _ = http("POST", f"{JF}/Plugins/{pid}/Configuration", {"X-Emby-Token": JFK}, c)
    st2, c2 = http("GET", f"{JF}/Plugins/{pid}/Configuration", {"X-Emby-Token": JFK})
    print(f"{name} : POST {st}, clé à jour : {c2.get('JellyseerrApiKey') == NEW}")

# 4. homelabd
subprocess.run(["systemctl", "restart", "homelabd"], check=True)
print(f"homelabd redémarré ({time.time() - t0:.0f} s après la régénération)")

# 5. Homarr (arrêté pendant l'écriture, base sauvegardée)
subprocess.run(["docker", "compose", "stop", "homarr"], cwd=BASE, check=True, capture_output=True)
shutil.copy2(f"{BASE}/homarr/db/db.sqlite", f"{BASE}/backups/homarr-db-{STAMP}-jskey.sqlite")
os.chmod(f"{BASE}/backups/homarr-db-{STAMP}-jskey.sqlite", 0o600)
db = sqlite3.connect(f"{BASE}/homarr/db/db.sqlite"); n = 0
for rowid, v in db.execute("select s.rowid, s.value from integrationSecret s join integration i on i.id = s.integration_id where i.kind = 'jellyseerr' and s.kind = 'apiKey'").fetchall():
    if dec(v) == OLD:
        db.execute("update integrationSecret set value = ?, updated_at = ? where rowid = ?", (enc(NEW), int(time.time()), rowid)); n += 1
db.commit()
ok = all(dec(v) == NEW for (v,) in db.execute("select s.value from integrationSecret s join integration i on i.id = s.integration_id where i.kind = 'jellyseerr' and s.kind = 'apiKey'"))
db.close()
subprocess.run(["docker", "compose", "start", "homarr"], cwd=BASE, check=True, capture_output=True)
print(f"Homarr : {n} secret(s) mis à jour, vérifié : {ok}")

# 6. contrôles
print("ancienne clé :", http("GET", f"{JS}/settings/main", {"X-Api-Key": OLD})[0], "(attendu 401/403)")
print("nouvelle clé :", http("GET", f"{JS}/status", {"X-Api-Key": NEW})[0], "/", http("GET", f"{JS}/settings/main", {"X-Api-Key": NEW})[0])
print(f"fin, {time.time() - t0:.0f} s après la régénération")
