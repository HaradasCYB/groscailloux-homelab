#!/usr/bin/env python3
"""Nettoyage ciblé de la seedbox (analyse du 2026-09-20, catégories A, B et C validées par l'admin).

A/B — torrents **sans catégorie** (jamais Sonarr/Radarr) dont **aucun** fichier n'est relié (lien physique) à la
      médiathèque : supprimés avec leurs fichiers. Un torrent partiellement relié (Top Gun BONUS, DuckTales S03) est
      laissé. Règle C411 : un torrent fini depuis < 7 j avec ratio < 1 est **reporté** (liste `deferred.json`,
      repassage quotidien par timer) ; `--phase deferred` ne traite que ceux devenus éligibles.
C   — corbeilles Sonarr/Radarr : entrées listées (chemins exacts) supprimées ; le torrent d'une ancienne version
      qui n'est plus référencé que par la corbeille (Attack on Titan) est retiré avec ses fichiers.

    scripts/seedbox-cleanup.py --phase now --dry-run
    scripts/seedbox-cleanup.py --phase now
    scripts/seedbox-cleanup.py --phase deferred        # par timer, jusqu'à ce que deferred.json soit vide
"""
import argparse
import collections
import datetime as dt
import http.cookiejar
import json
import os
import shlex
import subprocess
import time
import tomllib
import urllib.parse
import urllib.request

BASE = "/opt/homelab"
WORK = f"{BASE}/backups/seedbox-cleanup-20260920"
SSH = ["ssh", "-o", "ConnectTimeout=20", "seedbox"]
HR_DAYS = 7
NONVIDEO = {".iso", ".exe", ".flac", ".mp3", ".pdf", ".epub", ".cue", ".zjphoi", ".60uvsv"}
RECYCLE = {  # catégorie C, telle qu'analysée (dossier → entrées)
    "Movies/.recycle": ["Street Smart (1987)", "Groove (2000)", "Airplane! (1980)", "The Strangers (2008)",
                        "The Descent - Part 2 (2009)", "John Wick - Chapter 3 - Parabellum (2019)", "La Haine (1995)"],
    "TV Shows/.recycle": ["The Sisters Grimm", "Star Wars Rebels", "Twin of Brothers", "Star Trek - Strange New Worlds",
                          "Better Call Saul", "The Blacklist", "Attack on Titan", "BLACK TORCH",
                          "Smoking Behind the Supermarket with You [tvdbid-465973]", "Erased [tvdbid-303071]",
                          "The Man in the High Castle"],
}

env = {}
for line in open(f"{BASE}/.env"):
    line = line.strip()
    if line and not line.startswith("#") and "=" in line:
        k, v = line.split("=", 1)
        env[k] = v.strip().strip('"')
SB = tomllib.load(open(f"{BASE}/homelab.toml", "rb"))["seedbox"]
HOME = os.path.dirname(SB["media_root"].rstrip("/"))  # /home/kakaouette
DRY = False
LOG = None


def log(msg):
    line = f"[{dt.datetime.now():%F %T}] {msg}"
    print(line, flush=True)
    if LOG:
        LOG.write(line + "\n")
        LOG.flush()


def remote(cmd, check=True):
    r = subprocess.run(SSH + [cmd], capture_output=True, text=True)
    if check and r.returncode != 0:
        raise RuntimeError(f"ssh: {r.stderr.strip()[:300]}")
    return r.stdout


class Qbit:
    def __init__(self):
        self.url = SB["qbit_url"].rstrip("/")
        self.op = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))
        r = self.op.open(urllib.request.Request(self.url + "/api/v2/auth/login", data=urllib.parse.urlencode(
            {"username": SB.get("qbit_user", "kakaouette"), "password": env["SEEDBOX_QBIT_PASSWORD"]}).encode()), timeout=60).read()
        if r != b"Ok.":
            raise RuntimeError("connexion qBittorrent seedbox refusée")

    def torrents(self):
        return json.load(self.op.open(self.url + "/api/v2/torrents/info", timeout=120))

    def delete(self, h):
        self.op.open(urllib.request.Request(self.url + "/api/v2/torrents/delete", data=urllib.parse.urlencode(
            {"hashes": h, "deleteFiles": "true"}).encode()), timeout=120).read()


def inventory():
    """(taille, inode, chemin relatif à ~) de tous les fichiers de downloads/ et media/."""
    out = remote("cd ~ && find downloads media -type f -printf '%s\\t%i\\t%p\\n'")
    files = []
    for line in out.splitlines():
        s, i, p = line.split("\t", 2)
        files.append((int(s), i, p))
    return files


def classify(files, torrents):
    byino = collections.defaultdict(list)
    for s, i, p in files:
        byino[i].append(p)

    def in_media(i):
        return any(p.startswith("media/") and "/.recycle/" not in p for p in byino[i])

    bypath = {t["content_path"].split("kakaouette/", 1)[1]: t for t in torrents if "kakaouette/" in t["content_path"]}
    g = collections.defaultdict(lambda: {"size": 0, "orph": 0, "exts": set(), "n": 0})
    for s, i, p in files:
        if not p.startswith("downloads/"):
            continue
        key = next((k for k in bypath if p == k or p.startswith(k + "/")), None)
        if not key:
            continue
        g[key]["size"] += s
        g[key]["n"] += 1
        g[key]["exts"].add(os.path.splitext(p)[1].lower())
        if not in_media(i):
            g[key]["orph"] += s
    out = []
    for key, v in g.items():
        t = bypath[key]
        if t["category"]:  # jamais un torrent géré par Sonarr/Radarr
            continue
        if v["orph"] == 0:
            continue
        if v["orph"] < v["size"]:
            if v["orph"] >= 0.1e9:  # sinon ce ne sont que des .nfo/.txt du pack, tout le reste est en médiathèque
                log(f"  laissé (partiellement relié à la médiathèque) : {t['name'][:70]} ({v['orph'] / 1e9:.1f}/{v['size'] / 1e9:.1f} Go)")
            continue
        out.append({"hash": t["hash"], "name": t["name"], "size": v["size"], "ratio": t["ratio"],
                    "completion_on": t["completion_on"], "nonvideo": bool(v["exts"] & NONVIDEO), "key": key})
    return out


def eligible(t, now):
    if t["completion_on"] <= 0:
        return False
    return t["ratio"] >= 1 or (now - t["completion_on"]) >= HR_DAYS * 86400


def free_date(t):
    return dt.datetime.fromtimestamp(t["completion_on"] + HR_DAYS * 86400).strftime("%d/%m %H:%M")


def phase_now():
    qb = Qbit()
    torrents = qb.torrents()
    files = inventory()
    cands = classify(files, torrents)
    now = time.time()
    do_now = [t for t in cands if eligible(t, now)]
    defer = [t for t in cands if not eligible(t, now)]
    for title, lst in (("A. non vidéo", [t for t in do_now if t["nonvideo"]]), ("B. vidéo jamais importée", [t for t in do_now if not t["nonvideo"]])):
        log(f"== {title} : {len(lst)} torrent(s), {sum(t['size'] for t in lst) / 1e9:.1f} Go")
        for t in sorted(lst, key=lambda x: -x["size"]):
            log(f"  {t['size'] / 1e9:6.1f} Go  ratio {t['ratio']:.2f}  {t['name'][:80]}")
    log(f"== reportés (H&R) : {len(defer)} torrent(s), {sum(t['size'] for t in defer) / 1e9:.1f} Go")
    for t in sorted(defer, key=lambda x: -x["size"]):
        log(f"  {t['size'] / 1e9:6.1f} Go  ratio {t['ratio']:.2f}  libre le {free_date(t)}  {t['name'][:70]}")
    # C : torrents dont tous les fichiers ne sont plus référencés que par une corbeille (ancienne version)
    byino = collections.defaultdict(list)
    for s, i, p in files:
        byino[i].append(p)
    bypath = {t["content_path"].split("kakaouette/", 1)[1]: t for t in torrents if "kakaouette/" in t["content_path"]}
    rec_only = []
    for key, t in bypath.items():
        fs = [(s, i, p) for s, i, p in files if p == key or p.startswith(key + "/")]
        if not fs:
            continue
        if all(any("/.recycle/" in q for q in byino[i]) and not any(q.startswith("media/") and "/.recycle/" not in q for q in byino[i]) for s, i, p in fs):
            rec_only.append({"hash": t["hash"], "name": t["name"], "size": sum(s for s, _, _ in fs), "ratio": t["ratio"], "completion_on": t["completion_on"], "category": t["category"]})
    log(f"== C. torrents d'anciennes versions (seule la corbeille les référence) : {len(rec_only)}")
    for t in rec_only:
        log(f"  {t['size'] / 1e9:6.1f} Go  ratio {t['ratio']:.2f}  cat={t['category'] or '-'}  {'éligible' if eligible(t, now) else 'H&R, libre le ' + free_date(t)}  {t['name'][:70]}")
    rec_now = [t for t in rec_only if eligible(t, now)]
    defer += [t for t in rec_only if not eligible(t, now)]
    # exécution
    if DRY:
        log("dry-run : rien supprimé")
        return
    freed = 0
    seen = set()
    for t in do_now + rec_now:
        if t["hash"] in seen:
            continue
        seen.add(t["hash"])
        qb.delete(t["hash"])
        freed += t["size"]
        log(f"  torrent + fichiers supprimés : {t['name'][:70]}")
    json.dump([{k: v for k, v in t.items() if k != "key"} for t in defer], open(f"{WORK}/deferred.json", "w"), indent=1, ensure_ascii=False)
    log(f"torrents : {len(do_now) + len(rec_now)} supprimés (~{freed / 1e9:.0f} Go), {len(defer)} reportés dans deferred.json")
    # C : corbeilles
    rec_freed = 0
    for folder, names in RECYCLE.items():
        for n in names:
            p = f"{HOME}/media/{folder}/{n}"
            size = remote(f"du -sb {shlex.quote(p)} 2>/dev/null | cut -f1", check=False).strip()
            if not size:
                log(f"  corbeille : absent, ignoré : {folder}/{n}")
                continue
            remote(f"rm -rf {shlex.quote(p)}")
            rec_freed += int(size)
            log(f"  corbeille vidée : {folder}/{n} ({int(size) / 1e9:.1f} Go)")
    log(f"corbeilles : {rec_freed / 1e9:.0f} Go retirés (part réellement libérée = ce qu'aucun torrent ne partageait)")
    log("usage ~ après : " + remote("du -sh ~ 2>/dev/null | cut -f1").strip())


def phase_deferred():
    path = f"{WORK}/deferred.json"
    if not os.path.exists(path):
        log("aucune liste reportée")
        return
    defer = json.load(open(path))
    if not defer:
        log("liste reportée vide")
        return
    qb = Qbit()
    live = {t["hash"]: t for t in qb.torrents()}
    now = time.time()
    keep = []
    for t in defer:
        cur = live.get(t["hash"])
        if not cur:
            log(f"  déjà parti : {t['name'][:70]}")
            continue
        t["ratio"] = cur["ratio"]
        if eligible(t, now):
            if not DRY:
                qb.delete(t["hash"])
            log(f"  {'(dry) ' if DRY else ''}torrent + fichiers supprimés : {t['name'][:70]} ({t['size'] / 1e9:.1f} Go, ratio {t['ratio']:.2f})")
        else:
            keep.append(t)
            log(f"  encore reporté (libre le {free_date(t)}, ratio {t['ratio']:.2f}) : {t['name'][:70]}")
    if not DRY:
        json.dump(keep, open(path, "w"), indent=1, ensure_ascii=False)
    log(f"reportés restants : {len(keep)}")


def main():
    global DRY, LOG
    ap = argparse.ArgumentParser()
    ap.add_argument("--phase", choices=["now", "deferred"], required=True)
    ap.add_argument("--dry-run", action="store_true")
    a = ap.parse_args()
    DRY = a.dry_run
    os.makedirs(WORK, exist_ok=True)
    LOG = open(f"{WORK}/cleanup.log", "a")
    log(f"=== phase {a.phase} dry-run={DRY}")
    (phase_now if a.phase == "now" else phase_deferred)()


if __name__ == "__main__":
    main()
