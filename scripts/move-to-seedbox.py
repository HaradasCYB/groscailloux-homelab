#!/usr/bin/env python3
"""Déplace des titres du VPS vers la seedbox sans rien casser (2026-09-20).

Par titre : copie rsync (ssh, clé admin de la seedbox) vers la racine Sonarr/Radarr de la seedbox, vérification
(mêmes fichiers, mêmes tailles), reconnaissance par l'Arr de la seedbox (fiche existante rescannée ou créée sans
recherche), puis seulement côté VPS : fiche supprimée avec ses fichiers, torrents liés retirés s'ils ont fini de
partager depuis > 7 j (sinon gardés, l'espace sera libéré plus tard), Jellyfin prévenu (dossier VPS supprimé, dossier
seedbox ajouté, cache rclone rafraîchi) — même bibliothèque, autre dossier, l'historique de visionnage suit les
identifiants. Un titre en cours de lecture est copié mais sa bascule est reportée (relancer plus tard).

    scripts/move-to-seedbox.py --list                 # numérote les titres du VPS (taille décroissante) → titles.json
    scripts/move-to-seedbox.py --titles 1-30 --dry-run
    scripts/move-to-seedbox.py --titles 1-30 --worker 0/2   # deux instances en parallèle : 0/2 et 1/2
    scripts/move-to-seedbox.py --titles 1-30 --bwlimit 15000  # HORS PIC (08:30-12:30) : un flux plafonné, sinon les
                                                               # lectures seedbox calent (mesuré le 2026-09-20 : 8-10 Mo/s
                                                               # en lecture avec deux rsync à 20 Mo/s, un membre bloqué)

Journal et état : backups/move-to-seedbox-<date>/. Rejouable : chaque étape est idempotente.
"""
import argparse
import datetime as dt
import json
import os
import re
import shlex
import subprocess
import sys
import time
import tomllib
import urllib.error
import urllib.parse
import urllib.request

BASE = "/opt/homelab"
MEDIA = f"{BASE}/library/media"
DOWNLOADS = f"{BASE}/library/downloads"
WORK = f"{BASE}/backups/move-to-seedbox-20260920"
SSH = ["ssh", "-o", "ConnectTimeout=20", "-c", "aes128-gcm@openssh.com", "seedbox"]
# bibliothèque VPS → (racine Arr VPS, racine seedbox (hôte), sous-dossier Jellyfin seedbox, arr, profil seedbox, anime)
LIBS = {
    "movies": ("/movies", "Movies", "radarr", 7, False),
    "anime-films": ("/anime-films", "Anime Movies", "radarr", 8, True),
    "tvshows": ("/tv", "TV Shows", "sonarr", 7, False),
    "anime": ("/anime", "Anime", "sonarr", 8, True),
}
SEED_KEEP_DAYS = 7
VIDEO = re.compile(r"\.(mkv|mp4|avi|m4v|ts|webm|mov)$", re.I)

env = {}
for line in open(f"{BASE}/.env"):
    line = line.strip()
    if line and not line.startswith("#") and "=" in line:
        k, v = line.split("=", 1)
        env[k] = v.strip().strip('"')
toml = tomllib.load(open(f"{BASE}/homelab.toml", "rb"))
SB = toml["seedbox"]
SB_MEDIA = SB["media_root"].rstrip("/")  # /home/kakaouette/media
JF_SEED = SB["jellyfin_root"].rstrip("/")  # /seedbox/media

ARR = {
    "sonarr": ("http://127.0.0.1:8989/api/v3", env["SONARR_API_KEY"]),
    "radarr": ("http://127.0.0.1:7878/api/v3", env["RADARR_API_KEY"]),
    "sonarr-seedbox": (SB["sonarr_url"].rstrip("/") + "/api/v3", env["SEEDBOX_SONARR_API_KEY"]),
    "radarr-seedbox": (SB["radarr_url"].rstrip("/") + "/api/v3", env["SEEDBOX_RADARR_API_KEY"]),
}
JF = ("http://127.0.0.1:8096", env["JELLYFIN_API_KEY"])

DRY = False
LOG = None
BWLIMIT = 0  # Ko/s pour rsync (0 = illimité) ; hors pic seulement : deux flux à 20 Mo/s font caler les lectures seedbox


def log(msg):
    line = f"[{dt.datetime.now():%F %T}] {msg}"
    print(line, flush=True)
    if LOG:
        LOG.write(line + "\n")
        LOG.flush()


def api(name, method, path, body=None, timeout=300):
    base, key = ARR[name]
    data = json.dumps(body).encode() if body is not None else None
    for attempt in range(6):
        req = urllib.request.Request(base + path, data=data, method=method, headers={"X-Api-Key": key, "Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(req, timeout=timeout) as r:
                raw = r.read()
                return json.loads(raw) if raw else None
        except urllib.error.HTTPError as e:
            body = e.read()[:300]
            if e.code == 500 and b"database is locked" in body and attempt < 5:
                time.sleep(5 + 5 * attempt)
                continue
            raise RuntimeError(f"{name} {method} {path} → {e.code} {body!r}") from e


def jf(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(JF[0] + path, data=data, method=method, headers={"X-Emby-Token": JF[1], "Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=120) as r:
        raw = r.read()
        return json.loads(raw) if raw else None


def sh(cmd, check=True, **kw):
    r = subprocess.run(cmd, capture_output=True, text=True, **kw)
    if check and r.returncode != 0:
        raise RuntimeError(f"{' '.join(cmd[:3])}… → {r.returncode}: {r.stderr.strip()[:400]}")
    return r


def remote(cmd):
    return sh(SSH + [cmd])


# ------------------------------------------------------------------------------------------- inventaire
def du(path):
    return int(sh(["sudo", "du", "-s", "--block-size=1", path]).stdout.split("\t")[0] or 0)


def list_titles():
    out = []
    for lib in LIBS:
        base = f"{MEDIA}/{lib}"
        for n in sorted(os.listdir(base)):
            if n.startswith("."):
                continue
            out.append({"lib": lib, "folder": n, "size": du(f"{base}/{n}")})
    out.sort(key=lambda t: -t["size"])
    for i, t in enumerate(out, 1):
        t["n"] = i
    return out


def vps_record(t):
    arr_root, _, arr, _, _ = LIBS[t["lib"]]
    path = f"{arr_root}/{t['folder']}"
    if arr == "sonarr":
        for s in api("sonarr", "GET", "/series"):
            if s["path"] == path:
                return s
    else:
        for m in api("radarr", "GET", "/movie"):
            if m["path"] == path:
                return m
    return None


def video_files(root):
    r = sh(["find", root, "-type", "f", "-printf", "%s\t%i\t%p\n"])
    files = []
    for line in r.stdout.splitlines():
        size, inode, p = line.split("\t", 2)
        files.append((int(size), int(inode), p))
    return files


# ------------------------------------------------------------------------------------------- copie
def rsync(src, dest):
    q = shlex.quote(dest)
    if DRY:
        log(f"  dry-run : rsync {src} → seedbox:{dest}")
        return 0.0
    remote(f"mkdir -p {q}")
    # sans sudo : les médias appartiennent à deploy, et la clé ssh de la seedbox est la sienne
    cmd = ["nice", "-n", "10", "ionice", "-c2", "-n7", "rsync", "-a", "--partial", "--no-owner", "--no-group", "--chmod=Du=rwx,Dgo=rx,Fu=rw,Fgo=r"]
    if BWLIMIT:
        cmd.append(f"--bwlimit={BWLIMIT}")
    cmd += ["-e", " ".join(SSH[:-1]), src.rstrip("/") + "/", f"seedbox:{dest}/"]
    t0 = time.time()
    sh(cmd)
    return time.time() - t0


def verify(src, dest):
    """Mêmes fichiers (noms, tailles) des deux côtés ; renvoie (nombre de vidéos, octets)."""
    local = {p[len(src) + 1:]: s for s, _, p in video_files(src)}
    r = remote(f"cd {shlex.quote(dest)} && find . -type f -printf '%s\\t%p\\n'")
    rem = {}
    for line in r.stdout.splitlines():
        s, p = line.split("\t", 1)
        rem[p[2:]] = int(s)
    missing = [p for p in local if rem.get(p) != local[p]]
    if missing:
        raise RuntimeError(f"copie incomplète : {len(missing)} fichier(s) absents ou de taille différente, ex. {missing[:2]}")
    return sum(1 for p in local if VIDEO.search(p)), sum(local.values())


# ------------------------------------------------------------------------------------------- Arr seedbox
def wait_command(name, cid, timeout=900):
    t0 = time.time()
    while time.time() - t0 < timeout:
        c = api(name, "GET", f"/command/{cid}")
        if c.get("status") in ("completed", "failed", "aborted"):
            return c.get("status")
        time.sleep(5)
    return "timeout"


def seedbox_series(rec, t, dest_default):
    """Fiche seedbox de la série (par tvdbId), créée sans recherche si absente. Renvoie (fiche, chemin distant)."""
    tvdb = rec["tvdbId"]
    for s in api("sonarr-seedbox", "GET", "/series"):
        if s["tvdbId"] == tvdb:
            return s, s["path"]
    _, root, _, profile, anime = LIBS[t["lib"]]
    lk = api("sonarr-seedbox", "GET", f"/series/lookup?term=tvdb:{tvdb}")
    if not lk:
        raise RuntimeError(f"tvdb {tvdb} introuvable côté seedbox")
    body = lk[0]
    body.update({
        "rootFolderPath": f"{SB_MEDIA}/{root}",
        "path": dest_default,
        "qualityProfileId": profile,
        "monitored": False,
        "seasonFolder": True,
        "seriesType": "anime" if anime else rec.get("seriesType", "standard"),
        "tags": [],
        "addOptions": {"monitor": "none", "searchForMissingEpisodes": False, "searchForCutoffUnmetEpisodes": False},
    })
    if DRY:
        log(f"  dry-run : créerait la série seedbox tvdb {tvdb} → {dest_default}")
        return body, dest_default
    s = api("sonarr-seedbox", "POST", "/series", body)
    return s, s["path"]


def seedbox_movie(rec, t, dest_default):
    tmdb = rec["tmdbId"]
    for m in api("radarr-seedbox", "GET", "/movie"):
        if m["tmdbId"] == tmdb:
            return m, m["path"]
    _, root, _, profile, _ = LIBS[t["lib"]]
    body = api("radarr-seedbox", "GET", f"/movie/lookup/tmdb?tmdbId={tmdb}")
    body.update({
        "rootFolderPath": f"{SB_MEDIA}/{root}",
        "path": dest_default,
        "qualityProfileId": profile,
        "monitored": False,
        "minimumAvailability": "released",
        "tags": [],
        "addOptions": {"searchForMovie": False},
    })
    if DRY:
        log(f"  dry-run : créerait le film seedbox tmdb {tmdb} → {dest_default}")
        return body, dest_default
    m = api("radarr-seedbox", "POST", "/movie", body)
    return m, m["path"]


def monitor_existing_series(series):
    """Série et saisons/épisodes pourvus surveillés (ce que ferait monitor = existing à l'ajout)."""
    eps = api("sonarr-seedbox", "GET", f"/episode?seriesId={series['id']}")
    with_file = [e["id"] for e in eps if e.get("hasFile") and not e.get("monitored")]
    if with_file:
        api("sonarr-seedbox", "PUT", "/episode/monitor", {"episodeIds": with_file, "monitored": True})
    seasons_with_files = {e["seasonNumber"] for e in eps if e.get("hasFile")}
    changed = not series.get("monitored")
    for sn in series["seasons"]:
        if sn["seasonNumber"] in seasons_with_files and not sn.get("monitored"):
            sn["monitored"] = True
            changed = True
    if changed:
        series["monitored"] = True
        api("sonarr-seedbox", "PUT", f"/series/{series['id']}", series)
    log(f"  Sonarr seedbox : {len(with_file)} épisode(s) et {len(seasons_with_files)} saison(s) mis en surveillance")


# ------------------------------------------------------------------------------------------- VPS : torrents liés
def linked_torrents(inodes):
    """Torrents qBittorrent VPS dont des fichiers partagent un inode avec le titre."""
    if not inodes:
        return []
    r = sh(["sudo", "find", DOWNLOADS, "-type", "f", "-printf", "%i\t%p\n"])
    tops = set()
    for line in r.stdout.splitlines():
        ino, p = line.split("\t", 1)
        if int(ino) in inodes:
            rel = p[len(DOWNLOADS) + 1:]
            tops.add(rel.split("/")[0])
    if not tops:
        return []
    with urllib.request.urlopen("http://127.0.0.1:8080/api/v2/torrents/info", timeout=60) as r:
        ts = json.load(r)
    out = []
    for tor in ts:
        cp = tor.get("content_path", "")
        top = cp[len(tor["save_path"]):].strip("/").split("/")[0] if cp.startswith(tor["save_path"]) else tor["name"]
        if top in tops or tor["name"] in tops:
            out.append(tor)
    return out


def qbit_delete(hash_):
    data = urllib.parse.urlencode({"hashes": hash_, "deleteFiles": "true"}).encode()
    urllib.request.urlopen(urllib.request.Request("http://127.0.0.1:8080/api/v2/torrents/delete", data=data), timeout=60).read()


def rclone_refresh(rel):
    """Le dossier parent d'abord (sinon le montage ne voit jamais le nouveau dossier), puis le dossier lui-même."""
    for d, rec in ((os.path.dirname(rel), "false"), (rel, "true")):
        try:
            data = json.dumps({"dir": d, "recursive": rec}).encode()
            urllib.request.urlopen(urllib.request.Request("http://127.0.0.1:5572/vfs/refresh", data=data, headers={"Content-Type": "application/json"}), timeout=300).read()
        except Exception as e:  # noqa: BLE001
            log(f"  rclone vfs/refresh {d} : {e}")
    mounted = f"{SB['mount_point']}/{rel}"
    if not os.path.isdir(mounted):
        raise RuntimeError(f"le montage rclone ne voit pas {mounted}")


JELLYSEERR_DB = f"{BASE}/jellyseerr/config/db/db.sqlite3"


def jellyseerr_repoint(kind, tmdb, tvdb, sb_id, slug):
    """Jellyseerr garde (serviceId, externalServiceId) de la fiche Arr : après un déménagement, la fiche VPS n'existe
    plus et la fiche seedbox est inconnue (monitor_sync et l'avancement des demandes perdent le titre). On repointe
    la ligne `media` vers la seedbox (serviceId 1). Petite écriture SQLite en WAL, Jellyseerr en marche."""
    import sqlite3
    if DRY or not os.path.exists(JELLYSEERR_DB):
        return
    try:
        con = sqlite3.connect(JELLYSEERR_DB, timeout=10)
        if kind == "sonarr":
            n = con.execute("UPDATE media SET serviceId=1, externalServiceId=?, externalServiceSlug=? WHERE mediaType='tv' AND tvdbId=? AND serviceId=0", (sb_id, slug, tvdb)).rowcount
        else:
            n = con.execute("UPDATE media SET serviceId=1, externalServiceId=?, externalServiceSlug=? WHERE mediaType='movie' AND tmdbId=? AND serviceId=0", (sb_id, slug, tmdb)).rowcount
        con.commit()
        if n:
            log(f"  Jellyseerr : fiche média repointée vers la seedbox ({n})")
    except Exception as e:  # noqa: BLE001
        log(f"  Jellyseerr : repointage impossible ({e})")


def playing_under(prefix):
    for s in jf("GET", "/Sessions"):
        np = s.get("NowPlayingItem") or {}
        if (np.get("Path") or "").startswith(prefix):
            return s.get("UserName", "?")
    return None


# ------------------------------------------------------------------------------------------- un titre
def process(t, state):
    key = f"{t['lib']}/{t['folder']}"
    st = state.setdefault(key, {"n": t["n"], "status": "todo"})
    if st["status"] == "done":
        log(f"#{t['n']} {t['folder']} : déjà fait")
        return
    arr_root, root, arr, _, _ = LIBS[t["lib"]]
    src = f"{MEDIA}/{t['lib']}/{t['folder']}"
    jf_src = f"/media/{t['lib']}/{t['folder']}"
    rec = vps_record(t)
    dest_default = f"{SB_MEDIA}/{root}/{t['folder']}"
    log(f"#{t['n']} {t['folder']} ({t['size'] / 1e9:.1f} Go, {arr}, fiche VPS {'oui' if rec else 'NON'})")

    # 1. cible seedbox (fiche existante = son dossier)
    if rec:
        if arr == "sonarr":
            sb_rec, dest = seedbox_series(rec, t, dest_default)
        else:
            sb_rec, dest = seedbox_movie(rec, t, dest_default)
    else:
        sb_rec, dest = None, dest_default
    jf_dest = f"{JF_SEED}/{root}/{os.path.basename(dest)}"
    log(f"  cible : {dest}")

    # 2. copie + vérification
    inodes = {ino for _, ino, _ in video_files(src)}
    if st["status"] in ("todo", "copied"):
        secs = rsync(src, dest)
        if DRY:
            log(f"  dry-run : rsync fait ({secs:.0f} s, aucune écriture)")
        else:
            n, total = verify(src, dest)
            log(f"  copié et vérifié : {n} vidéo(s), {total / 1e9:.1f} Go en {secs / 60:.0f} min")
            st["status"] = "copied"
            save(state)
            rclone_refresh(f"{root}/{os.path.basename(dest)}")

    # 3. reconnaissance par l'Arr de la seedbox
    if rec and not DRY:
        if arr == "sonarr":
            before = (sb_rec.get("statistics") or {}).get("episodeFileCount", 0) if "id" in sb_rec else 0
            cid = api("sonarr-seedbox", "POST", "/command", {"name": "RescanSeries", "seriesId": sb_rec["id"]})["id"]
            status = wait_command("sonarr-seedbox", cid)
            fresh = api("sonarr-seedbox", "GET", f"/series/{sb_rec['id']}")
            after = (fresh.get("statistics") or {}).get("episodeFileCount", 0)
            copied = sum(1 for _, _, p in video_files(src) if VIDEO.search(p))
            log(f"  Sonarr seedbox : rescan {status}, fichiers {before} → {after} (copiés : {copied})")
            if after < before + copied:
                raise RuntimeError(f"Sonarr seedbox ne voit que {after - before} fichier(s) sur {copied} : bascule refusée")
            monitor_existing_series(fresh)
        else:
            cid = api("radarr-seedbox", "POST", "/command", {"name": "RescanMovie", "movieId": sb_rec["id"]})["id"]
            status = wait_command("radarr-seedbox", cid)
            fresh = api("radarr-seedbox", "GET", f"/movie/{sb_rec['id']}")
            log(f"  Radarr seedbox : rescan {status}, hasFile={fresh.get('hasFile')}")
            if not fresh.get("hasFile"):
                raise RuntimeError("Radarr seedbox ne voit pas le fichier : bascule refusée")
            if not fresh.get("monitored"):
                fresh["monitored"] = True
                api("radarr-seedbox", "PUT", f"/movie/{fresh['id']}", fresh)
    elif not rec:
        log("  pas de fiche Arr sur le VPS : le dossier sera copié tel quel (Jellyfin le lira via la seedbox)")

    # 4. bascule : jamais pendant une lecture
    who = playing_under(jf_src)
    if who:
        log(f"  {who} regarde ce titre : bascule reportée (copie conservée), relancer plus tard")
        st["status"] = "copied"
        save(state)
        return
    if DRY:
        tors = linked_torrents(inodes)
        log(f"  dry-run : supprimerait la fiche VPS + fichiers, torrents liés : {[x['name'][:50] for x in tors]}")
        return
    tors = linked_torrents(inodes)
    now = time.time()
    for tor in tors:
        age_d = (now - tor.get("completion_on", now)) / 86400 if tor.get("completion_on", 0) > 0 else 0
        if age_d >= SEED_KEEP_DAYS:
            qbit_delete(tor["hash"])
            log(f"  torrent retiré avec fichiers : {tor['name'][:60]} (fini depuis {age_d:.0f} j, ratio {tor['ratio']:.1f})")
        else:
            log(f"  torrent GARDÉ en partage : {tor['name'][:60]} (fini depuis {age_d:.0f} j < {SEED_KEEP_DAYS}) — ses fichiers restent dans downloads")
    if rec:
        if arr == "sonarr":
            api("sonarr", "DELETE", f"/series/{rec['id']}?deleteFiles=true&addImportListExclusion=false")
        else:
            api("radarr", "DELETE", f"/movie/{rec['id']}?deleteFiles=true&addImportExclusion=false")
        log("  fiche VPS supprimée avec ses fichiers")
        if sb_rec and "id" in sb_rec:
            jellyseerr_repoint(arr, rec.get("tmdbId"), rec.get("tvdbId"), sb_rec["id"], sb_rec.get("titleSlug", ""))
    if os.path.isdir(src) and src.startswith(MEDIA + "/"):
        sh(["rm", "-rf", src])
        log("  dossier VPS restant supprimé")
    # 5. Jellyfin : cache rclone rafraîchi, puis dossier VPS retiré / dossier seedbox ajouté
    rclone_refresh(f"{root}/{os.path.basename(dest)}")
    jf("POST", "/Library/Media/Updated", {"Updates": [{"Path": jf_src, "UpdateType": "Deleted"}, {"Path": jf_dest, "UpdateType": "Created"}]})
    log(f"  Jellyfin prévenu : {jf_src} → {jf_dest}")
    st["status"] = "done"
    st["dest"] = dest
    st["done_at"] = dt.datetime.now().isoformat(timespec="seconds")
    save(state)


STATE_FILE = None


def save(state):
    if DRY:
        return
    tmp = STATE_FILE + ".tmp"
    json.dump(state, open(tmp, "w"), indent=1, ensure_ascii=False)
    os.replace(tmp, STATE_FILE)


def load_state():
    return json.load(open(STATE_FILE)) if os.path.exists(STATE_FILE) else {}


def parse_range(s):
    out = set()
    for part in s.split(","):
        if "-" in part:
            a, b = part.split("-")
            out.update(range(int(a), int(b) + 1))
        else:
            out.add(int(part))
    return out


def main():
    global DRY, LOG, STATE_FILE, BWLIMIT
    ap = argparse.ArgumentParser()
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--titles", default="")
    ap.add_argument("--worker", default="0/1")
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--bwlimit", type=int, default=0, help="Ko/s pour rsync (ex. 15000)")
    a = ap.parse_args()
    DRY = a.dry_run
    BWLIMIT = a.bwlimit
    os.makedirs(WORK, exist_ok=True)
    if a.list:
        titles = list_titles()
        json.dump(titles, open(f"{WORK}/titles.json", "w"), indent=1, ensure_ascii=False)
        for t in titles:
            print(f"{t['n']:2}  {t['size'] / 1e9:6.1f} Go  {t['lib']:11} {t['folder']}")
        return
    titles = json.load(open(f"{WORK}/titles.json"))
    wanted = parse_range(a.titles) if a.titles else set()
    wi, wn = (int(x) for x in a.worker.split("/"))
    sel = [t for t in titles if t["n"] in wanted and (t["n"] % wn) == wi]
    LOG = open(f"{WORK}/worker-{wi}{'-dry' if DRY else ''}.log", "a")
    STATE_FILE = f"{WORK}/state-{wi}.json"
    log(f"=== worker {a.worker} : {len(sel)} titre(s), {sum(t['size'] for t in sel) / 1e9:.0f} Go, dry-run={DRY}")
    for t in sel:
        try:
            process(t, load_state())
        except Exception as e:  # noqa: BLE001
            log(f"  ERREUR #{t['n']} {t['folder']} : {e} — titre laissé en l'état (rien supprimé côté VPS)")
    log(f"=== worker {a.worker} : terminé")


if __name__ == "__main__":
    main()
