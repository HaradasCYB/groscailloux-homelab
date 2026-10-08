#!/usr/bin/env python3
"""hl.py : appels communs aux outils de tools/ (offpeak, bench). Créé le 2026-10-08 (lot 4 de la revue Kaizen).

Sous-commandes :
  guard [--idle] [--jf-tasks] [--idle-if-down] [--seedbox] [--homelabd T:S[:I] …] [--busy]
        une ligne « ok <détail> » (code 0), « refus <motif> » (1) ou « attendre <s> <motif> || <gardes vertes> » (4) ;
        --busy ne compte que les lot3-* (les travaux hors pic se suivent par le verrou global d'offpeak.sh)
  busy [--timers] [--margin-min N] [--self-unit U]      maintenance en cours (lot3-*, homelab-offpeak@*) : 1 si oui
  discord <fichier|-> [--tries N]                        texte (- : entrée standard) → salon Discord ADMIN
  members-playing                                        « <lectures de membres> <lectures de bancs> » (3 si muet)
  url <service> | public <CLÉ>                           adresse locale ([urls] du TOML) | clé PUBLIQUE de .env
  account-create <zz_nom> --env-out <fichier> [--id-out <fichier>] [--index k] [--config K=V …]
  account-delete --id <id> --name <zz_nom>
  sweep [--prefix zz_] [--max-age-min 120] [--max 20] [--dry-run]

Règles (CLAUDE.md, « Secrets ») : la clé Jellyfin et le webhook Discord sont lus dans .env PAR CE PROGRAMME, jamais
passés en argument ni affichés ; une erreur réseau n'imprime que son type ou son code HTTP (le texte pourrait contenir
une adresse). Le mot de passe d'un compte de banc ne va que dans le fichier --env-out (0600), jamais sur la sortie.
Toute suppression est vérifiée une à une (nom, préfixe zz_, non-admin) : jamais de boucle DELETE sur une liste brute
(le 2026-09-15, `GET /Devices?userId=` ignorait son filtre et une boucle a déconnecté tout le monde).
"""
import argparse
import json
import os
import re
import secrets
import signal
import subprocess
import sys
import time
import tomllib
import urllib.error
import urllib.parse
import urllib.request
from datetime import datetime

HL = os.environ.get('HOMELAB_DIR', '/opt/homelab')
ENV_FILE = os.environ.get('HOMELAB_ENV_FILE', os.path.join(HL, '.env'))
TOML_FILE = os.environ.get('HOMELAB_TOML', os.path.join(HL, 'homelab.toml'))
STATE_FILE = os.environ.get('HOMELAB_STATE_FILE', os.path.join(HL, 'state', 'homelabd.json'))
BENCH_PREFIX = 'zz_'
BENCH_NAME_RE = re.compile(r'^zz_[a-z0-9_]{2,30}$')
# fichiers ouverts qui comptent pour --seedbox : le montage vu de l'hôte et vu du conteneur Jellyfin
SEEDBOX_PATHS = ('/mnt/seedbox/', '/seedbox/')
MAINT_PATTERNS = ('lot3-*', 'homelab-offpeak@*')
# durée maximale d'un passage de homelabd (scheduler::RUN_TIMEOUT) et retard maximal de l'état sur disque (LAZY_MAX_AGE)
RUN_TIMEOUT = 600
STATE_LAG = 90


def die(msg, code=2):
    print(msg)
    sys.exit(code)


# ------------------------------------------------------------------------------------------------ configuration
def env():
    out = {}
    try:
        with open(ENV_FILE, encoding='utf-8') as f:
            for line in f:
                line = line.strip()
                if line and not line.startswith('#') and '=' in line:
                    k, v = line.split('=', 1)
                    out[k.strip()] = v.strip().strip('"').strip("'")
    except OSError as e:
        die('refus .env illisible (%s)' % type(e).__name__, 1)
    return out


def toml():
    with open(TOML_FILE, 'rb') as f:
        return tomllib.load(f)


def jf_base():
    """Adresse locale de Jellyfin, lue dans [urls] de homelab.toml (aucune adresse en dur dans les outils)."""
    try:
        return toml()['urls']['jellyfin'].rstrip('/')
    except Exception as e:  # noqa: BLE001
        raise Unreachable('[urls] jellyfin illisible dans homelab.toml (%s)' % type(e).__name__) from None


class Unreachable(Exception):
    pass


class Jf:
    """Appels à l'API Jellyfin locale avec la clé de .env (en-tête, jamais dans l'URL)."""

    def __init__(self):
        self.base = jf_base()
        self.key = env().get('JELLYFIN_API_KEY')
        if not self.key:
            raise Unreachable('clé absente de .env')

    def call(self, method, path, body=None, timeout=20):
        data = json.dumps(body).encode() if body is not None else None
        req = urllib.request.Request(self.base + path, data=data, method=method, headers={
            'Authorization': 'MediaBrowser Token="%s"' % self.key, 'Content-Type': 'application/json',
            'User-Agent': 'homelab-tools'})
        try:
            with urllib.request.urlopen(req, timeout=timeout) as r:
                raw = r.read()
                return r.status, (json.loads(raw) if raw[:1] in (b'{', b'[') else None)
        except urllib.error.HTTPError as e:
            return e.code, None
        except Exception as e:  # noqa: BLE001
            raise Unreachable(type(e).__name__) from None

    def get(self, path):
        st, body = self.call('GET', path)
        if st != 200:
            raise Unreachable('HTTP %s' % st)
        return body


def norm_id(s):
    return (s or '').replace('-', '').lower()


def parse_date(s):
    """Date ISO de Jellyfin (« 2026-10-07T21:55:39.3762918Z ») → epoch, ou None."""
    if not s or s.startswith('0001-'):
        return None
    s = re.sub(r'(\.\d{6})\d+', r'\1', s.replace('Z', '+00:00'))
    try:
        return datetime.fromisoformat(s).timestamp()
    except ValueError:
        return None


# ------------------------------------------------------------------------------------------------ gardes
def jf_state(jf):
    """(lectures de membres, lectures de bancs, tâches planifiées en cours)."""
    sessions = jf.get('/Sessions')
    tasks = jf.get('/ScheduledTasks')
    members = bench = 0
    for s in sessions:
        if s.get('NowPlayingItem'):
            if (s.get('UserName') or '').lower().startswith(BENCH_PREFIX):
                bench += 1
            else:
                members += 1
    running = [t.get('Name', '?') for t in tasks if t.get('State') == 'Running']
    return members, bench, running


def seedbox_open():
    """Processus qui ont un fichier ouvert sous le montage seedbox : « 2×ffprobe 1×jellyfin » (root requis)."""
    if os.geteuid() != 0:
        try:
            r = subprocess.run(['sudo', '-n', sys.executable, os.path.abspath(__file__), 'seedbox-open'],
                               capture_output=True, text=True, timeout=60)
        except (OSError, subprocess.TimeoutExpired) as e:
            return None, type(e).__name__
        if r.returncode != 0:
            return None, 'sudo refusé'
        return r.stdout.strip(), None
    counts = {}
    for pid in os.listdir('/proc'):
        if not pid.isdigit():
            continue
        try:
            fds = os.listdir('/proc/%s/fd' % pid)
        except OSError:
            continue
        hit = False
        for fd in fds:
            try:
                target = os.readlink('/proc/%s/fd/%s' % (pid, fd))
            except OSError:
                continue
            if target.startswith(SEEDBOX_PATHS):
                hit = True
                break
        if hit:
            try:
                with open('/proc/%s/comm' % pid) as f:
                    comm = f.read().strip()
            except OSError:
                comm = '?'
            counts[comm] = counts.get(comm, 0) + 1
    return ' '.join('%d×%s' % (n, c) for c, n in sorted(counts.items())), None


def homelabd_window(conds, now=None):
    """Conditions « tâche:secondes[:intervalle] » : aucun passage en cours, et le prochain à au moins tant de
    secondes. Rend (attente en s, motifs, infos) ; attente 0 = fenêtre sûre. Reprise de lot3 common.sh
    (decision_fenetre), avec le retard de l'état sur disque (écriture paresseuse, 60 s au plus depuis le lot 2)."""
    now = time.time() if now is None else now
    try:
        with open(STATE_FILE) as f:
            runs = json.load(f).get('task_runs', {})
        cfg = toml().get('tasks', {})
    except Exception as e:  # noqa: BLE001
        return 30, ['état de homelabd illisible (%s)' % type(e).__name__], []
    wait, why, info = 0, [], []
    for c in conds:
        parts = c.split(':')
        if len(parts) not in (2, 3) or not all(p.isdigit() for p in parts[1:]):
            raise ValueError('condition invalide : %s (attendu tâche:secondes[:intervalle])' % c)
        task, gap = parts[0], int(parts[1])
        iv = int(parts[2]) if len(parts) == 3 else (cfg.get(task) or {}).get('interval_secs')
        r = runs.get(task)
        if iv and gap >= iv:
            # 2026-10-08 : « stack_health:400 » pour un passage toutes les 300 s ne serait jamais vert
            raise ValueError('condition impossible : %s (la tâche passe toutes les %d s)' % (c, iv))
        if not iv or not r:
            wait = max(wait, 30)
            why.append('%s inconnu (ni intervalle ni passage)' % task)
            continue
        start, end = r.get('last_start') or 0, r.get('last_end')
        if end is None or end < start:
            age = now - start
            if age <= STATE_LAG:
                wait = max(wait, 15)
                why.append('%s en cours (ou état pas encore relu)' % task)
                continue
            if age <= RUN_TIMEOUT:
                wait = max(wait, 30)
                why.append('%s en cours depuis %d s' % (task, age))
                continue
            end = start  # passage interrompu (daemon relancé) : on compte à partir du début
        nxt = end + iv - now
        if nxt < gap:
            # en retard de moins de 90 s : passage probablement fait, état pas encore relu
            wait = max(wait, max(nxt, 0) + 15 if nxt > -STATE_LAG else 20)
            why.append('%s dans %d s' % (task, nxt) if nxt >= 0 else '%s attendu (état relu dans la minute)' % task)
        else:
            info.append('%s dans %d s' % (task, nxt))
    return (min(int(wait), 60) if why else 0), why, info


def systemctl(*args):
    try:
        r = subprocess.run(['systemctl', *args], capture_output=True, text=True, timeout=30)
        return r.stdout
    except (OSError, subprocess.TimeoutExpired):
        return ''


def maintenance_busy(self_unit=None, timers=False, margin_min=20, now=None, offpeak=True):
    """Motifs d'une maintenance en cours : service lot3-* ou homelab-offpeak@* actif (autre que soi) ; avec
    timers, aussi un de leurs minuteurs passé il y a moins de margin_min ou attendu dans moins de margin_min (le
    créneau d'un lot qui repasse toutes les 15 min).
    offpeak=False (2026-10-08, garde d'offpeak.sh) : les homelab-offpeak@* ne comptent pas. Une unité Type=oneshot reste
    « activating » pendant toute son attente (--retry, fenêtre homelabd) : deux travaux programmés se refusaient l'un
    l'autre jusqu'à la fin de leur créneau (même OnCalendar, rattrapage Persistent=true au démarrage, travail qui attend
    --if-idle). Le verrou global d'offpeak.sh suffit à n'en lancer qu'un à la fois."""
    now = time.time() if now is None else now
    patterns = MAINT_PATTERNS if offpeak else tuple(x for x in MAINT_PATTERNS if not x.startswith('homelab-offpeak@'))
    out = []
    for line in systemctl('list-units', '--all', '--plain', '--no-legend', '--type=service', *patterns).splitlines():
        f = line.split()
        if len(f) >= 3 and f[0] != self_unit and f[2] in ('active', 'activating', 'reloading', 'deactivating'):
            out.append('%s actif' % f[0])
    if timers:
        names = [line.split()[0] for line in
                 systemctl('list-units', '--all', '--plain', '--no-legend', '--type=timer', *patterns).splitlines()
                 if line.split()]
        for t in names:
            props = dict(x.split('=', 1) for x in systemctl(
                'show', '--timestamp=unix', '-p', 'LastTriggerUSec', '-p', 'NextElapseUSecRealtime', t).splitlines() if '=' in x)
            last = props.get('LastTriggerUSec', '').lstrip('@')
            nxt = props.get('NextElapseUSecRealtime', '').lstrip('@')
            if last.isdigit() and 0 <= now - int(last) < margin_min * 60:
                out.append('%s passé il y a %d min' % (t, (now - int(last)) // 60))
            elif nxt.isdigit() and 0 <= int(nxt) - now < margin_min * 60:
                out.append('%s attendu dans %d min' % (t, (int(nxt) - now) // 60))
    return out


def cmd_guard(a):
    if a.busy:
        b = maintenance_busy(offpeak=False)
        if b:
            print('refus maintenance en cours : ' + ', '.join(b))
            return 1
    infos = ['aucune maintenance lot3 en cours'] if a.busy else []
    if a.idle or a.jf_tasks:
        try:
            members, bench, running = jf_state(Jf())
        except Unreachable as e:
            if a.idle_if_down and not a.jf_tasks:
                infos.append('Jellyfin muet (%s) : personne ne peut regarder' % e)
                members, bench, running = 0, 0, []
            else:
                print('refus Jellyfin ne répond pas (%s) : impossible de vérifier les lectures' % e)
                return 1
        if a.idle and members + bench:
            print('refus %d lecture(s) en cours dans Jellyfin%s' % (members + bench, ' (dont %d de banc)' % bench if bench else ''))
            return 1
        if a.jf_tasks and running:
            print('refus tâche(s) planifiée(s) Jellyfin en cours : ' + ', '.join(running))
            return 1
        infos.append('aucune lecture' if a.idle else 'lectures non vérifiées')
        if a.jf_tasks:
            infos.append('aucune tâche Jellyfin')
    if a.seedbox:
        out, err = seedbox_open()
        if err:
            print('refus fichiers ouverts du montage seedbox illisibles (%s)' % err)
            return 1
        if out:
            print('refus fichier(s) de la seedbox ouvert(s) : %s (lecture ou analyse en cours)' % out)
            return 1
        infos.append('aucun fichier seedbox ouvert')
    # fenêtre homelabd en dernier : « attendre » n'est rendu que si toutes les autres gardes sont vertes (l'appelant
    # attend quelques secondes puis relance TOUTES les gardes)
    if a.homelabd:
        if subprocess.run(['systemctl', 'is-active', '--quiet', 'homelabd.service']).returncode != 0:
            infos.append('homelabd arrêté (aucune de ses tâches ne passera)')
        else:
            try:
                w, why, info = homelabd_window(a.homelabd)
            except ValueError as e:
                print('refus %s' % e)
                return 1
            if why:
                # après « || » : les gardes déjà vertes, pour que le journal (et le à blanc) les montre aussi
                print('attendre %d %s || %s' % (w, ', '.join(why), '; '.join(infos) or 'aucune autre garde'))
                return 4
            infos.append('homelabd : ' + ', '.join(info))
    print('ok ' + ('; '.join(infos) or 'aucune garde demandée'))
    return 0


def cmd_busy(a):
    b = maintenance_busy(a.self_unit, a.timers, a.margin_min)
    if b:
        print(', '.join(b))
        return 1
    return 0


def cmd_members_playing(_a):
    try:
        members, bench, _ = jf_state(Jf())
    except Unreachable as e:
        print('muet %s' % e)
        return 3
    print(members, bench)
    return 0


PUBLIC_KEYS = ('JELLYFIN_PUBLIC_URL', 'ONBOARD_PUBLIC_URL', 'JELLYSEERR_PUBLIC_URL')


def cmd_public(a):
    """Valeur d'une clé PUBLIQUE de .env (adresse publique de Jellyfin…) ; toute autre clé est refusée."""
    if a.key not in PUBLIC_KEYS:
        die('refus %s n\'est pas une clé publique (%s)' % (a.key, ', '.join(PUBLIC_KEYS)), 1)
    v = env().get(a.key)
    if not v:
        die('refus %s absente de .env' % a.key, 1)
    print(v.rstrip('/'))
    return 0


def cmd_url(a):
    """Adresse locale d'un service ([urls] de homelab.toml) : jamais écrite en dur dans les outils."""
    try:
        print(toml()['urls'][a.service].rstrip('/'))
    except Exception:  # noqa: BLE001
        print('service inconnu dans [urls] : %s' % a.service)
        return 1
    return 0


def cmd_seedbox_open(_a):
    out, err = seedbox_open()
    if err:
        return 1
    print(out)
    return 0


# ------------------------------------------------------------------------------------------------ Discord
def cmd_discord(a):
    url = env().get('DISCORD_WEBHOOK_ADMIN')
    if not url:
        print('DISCORD_WEBHOOK_ADMIN absent : message non envoyé')
        return 1
    if a.file == '-':  # 2026-10-08 : texte par l'entrée standard, aucun fichier à écrire (dossier d'état illisible…)
        text = sys.stdin.read()[:1900]
    else:
        with open(a.file, encoding='utf-8', errors='replace') as f:
            text = f.read()[:1900]
    body = json.dumps({'content': text, 'allowed_mentions': {'parse': []}}).encode()
    for attempt in range(a.tries):
        try:
            req = urllib.request.Request(url, data=body, headers={'Content-Type': 'application/json',
                                                                  'User-Agent': 'homelab-tools'})
            urllib.request.urlopen(req, timeout=15).close()
            print('message Discord envoyé (salon admin)')
            return 0
        except Exception as e:  # noqa: BLE001 — le texte de l'erreur n'est pas affiché : il pourrait contenir l'adresse
            print('Discord : essai %d en échec (%s)' % (attempt + 1, type(e).__name__))
            if attempt + 1 < a.tries:
                time.sleep(15)
    return 1


# ------------------------------------------------------------------------------------------------ comptes de banc
def bench_policy(base, e):
    """Même politique qu'un membre ordinaire (`clients::jellyfin::non_admin_policy`, à garder en phase) : les
    bibliothèques explicites des nouveaux comptes (jamais les russes), ni TV en direct ni téléchargement, verrouillage
    après 5 échecs. Avant le 2026-10-08, les bancs posaient `EnableAllFolders = true` (revue : mbr-bancs-comptes-pollution)."""
    libs = [x for x in [e.get('JELLYFIN_LIB_FILMS', ''), e.get('JELLYFIN_LIB_SERIES', '')]
            + e.get('JELLYFIN_LIB_EXTRA', '').split(',') if x.strip()]
    p = dict(base)
    p.update({
        'IsAdministrator': False, 'IsHidden': True, 'IsDisabled': False,
        'EnableUserPreferenceAccess': True, 'EnableRemoteAccess': True, 'EnableMediaPlayback': True,
        'EnableAudioPlaybackTranscoding': True, 'EnableVideoPlaybackTranscoding': True,
        'EnablePlaybackRemuxing': True, 'EnableLiveTvAccess': False, 'EnableLiveTvManagement': False,
        'EnableContentDeletion': False, 'EnableContentDownloading': False, 'EnableSyncTranscoding': True,
        'EnableSubtitleManagement': False, 'EnableAllDevices': True, 'EnableAllChannels': False,
        'EnableAllFolders': False, 'EnabledFolders': [x.strip() for x in libs], 'EnabledChannels': [],
        'BlockedTags': [], 'BlockedMediaFolders': [], 'LoginAttemptsBeforeLockout': 5,
        'MaxActiveSessions': 0, 'SyncPlayAccess': 'CreateAndJoinGroups',
    })
    return p


def parse_kv(items):
    out = {}
    for kv in items or []:
        if '=' not in kv:
            die('refus --config attend CLÉ=VALEUR (%s)' % kv)
        k, v = kv.split('=', 1)
        try:
            out[k] = json.loads(v)
        except json.JSONDecodeError:
            out[k] = v
    return out


def _interrupted(signum, _frame):
    raise KeyboardInterrupt('signal %d' % signum)


def write_private(path, text):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, 'w') as f:
        f.write(text)


def cmd_account_create(a):
    if not BENCH_NAME_RE.match(a.name):
        die('refus nom de compte de banc invalide (zz_ puis 2 à 30 caractères a-z, 0-9, _)')
    # 2026-10-08 : TERM et HUP deviennent une interruption comme Ctrl-C (KeyboardInterrupt) ; sans ça, Python meurt sans
    # rien nettoyer et un compte créé par POST /Users/New restait jusqu'au balayage suivant (plus de 2 h). INT aussi,
    # explicitement : lancé en tâche de fond par un shell, Python hérite d'un SIGINT ignoré et finissait la création.
    for sig in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
        signal.signal(sig, _interrupted)
    jf = Jf()
    users = jf.get('/Users')
    if any(u['Name'].lower() == a.name for u in users):
        die('refus le compte %s existe déjà (banc en cours ? sinon : tools/bench/bench.sh --sweep)' % a.name, 1)
    pw = secrets.token_urlsafe(18)
    uid = None
    try:
        try:
            st, u = jf.call('POST', '/Users/New', {'Name': a.name, 'Password': pw})
        except BaseException:
            # interrompu pendant la création : le serveur a pu créer le compte sans qu'on ait son id. Le nom était
            # libre juste avant : un compte de ce nom maintenant est le nôtre.
            uid = find_user_id(jf, a.name)
            raise
        if st != 200 or not u or 'Id' not in u:
            die('refus création du compte en échec (HTTP %s)' % st, 1)
        uid = u['Id']
        if a.id_out:  # tout de suite : le lanceur le retrouve même s'il est interrompu avant d'avoir lu notre sortie
            write_private(a.id_out, uid + '\n')
        st, _ = jf.call('POST', '/Users/%s/Policy' % uid, bench_policy(u.get('Policy') or {}, env()))
        if st not in (200, 204):
            raise RuntimeError('politique HTTP %s' % st)
        conf = parse_kv(a.config)
        if conf:
            cur = jf.get('/Users/%s' % uid).get('Configuration') or {}
            cur.update(conf)
            st, _ = jf.call('POST', '/Users/%s/Configuration' % uid, cur)
            if st not in (200, 204):
                raise RuntimeError('configuration HTTP %s' % st)
        # compte n° k : USER_NAME_k, PW_k, USER_ID_k ; le premier aussi sans suffixe (contrat des anciens scénarios)
        lines = ['USER_NAME_%d=%s' % (a.index, a.name), 'PW_%d=%s' % (a.index, pw), 'USER_ID_%d=%s' % (a.index, uid)]
        if a.index == 1:
            lines += ['USER_NAME=%s' % a.name, 'PW=%s' % pw, 'USER_ID=%s' % uid]
        write_private(a.env_out, '\n'.join(lines) + '\n')
    except SystemExit:
        raise
    except BaseException as e:  # noqa: BLE001 — compte à moitié réglé ou interrompu (Ctrl-C, TERM) : retiré tout de suite
        if uid:
            for sig in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):  # le retrait lui-même n'est plus interrompu
                signal.signal(sig, signal.SIG_IGN)
            try:
                jf.call('DELETE', '/Users/%s' % uid)
            except Unreachable:
                pass  # le lanceur réessaie avec l'id écrit dans --id-out, sinon le balayage s'en charge
        what = 'interrompu' if isinstance(e, KeyboardInterrupt) else 'réglage du compte en échec (%s)' % e
        die('refus %s : %s' % (what, 'compte retiré' if uid else 'aucun compte créé'), 130 if isinstance(e, KeyboardInterrupt) else 1)
    print(uid)
    return 0


def find_user_id(jf, name):
    """Id du compte de banc de ce nom exact, ou None (aucune exception : appelé pendant une interruption)."""
    try:
        st, users = jf.call('GET', '/Users')
    except Exception:  # noqa: BLE001
        return None
    if st != 200 or not users:
        return None
    ids = [u['Id'] for u in users if (u.get('Name') or '').lower() == name.lower()]
    return ids[0] if len(ids) == 1 else None


def delete_device(jf, dev_id, name, uid):
    """Ferme UN appareil après avoir relu son dernier utilisateur ; 'fermé', 'déjà parti' ou 'refusé'."""
    q = urllib.parse.quote(dev_id, safe='')
    st, info = jf.call('GET', '/Devices/Info?id=%s' % q)
    if st == 404:
        return 'déjà parti'
    if st != 200 or not info:
        return 'refusé (lecture HTTP %s)' % st
    last_name = (info.get('LastUserName') or '').lower()
    if not last_name.startswith(BENCH_PREFIX) or (last_name != name.lower() and norm_id(info.get('LastUserId')) != norm_id(uid)):
        return 'refusé (dernier utilisateur inattendu)'
    st, _ = jf.call('DELETE', '/Devices?id=%s' % q)
    if st not in (200, 204):
        return 'refusé (HTTP %s)' % st
    st, _ = jf.call('GET', '/Devices/Info?id=%s' % q)
    return 'fermé' if st == 404 else 'fermé ? (encore lisible)'


def delete_account(jf, uid, name, dry=False):
    """Supprime UN compte de banc après vérification (préfixe zz_, nom attendu, non-admin), puis ses appareils."""
    st, u = jf.call('GET', '/Users/%s' % uid)
    if st == 404:
        u = None
    elif st != 200 or not u:
        return 'refusé (lecture HTTP %s)' % st
    if u is not None:
        if u.get('Name', '').lower() != name.lower() or not name.lower().startswith(BENCH_PREFIX):
            return 'refusé (nom inattendu)'
        if (u.get('Policy') or {}).get('IsAdministrator'):
            return 'refusé (administrateur)'
    devices = [d['Id'] for d in (jf.get('/Devices').get('Items') or [])
               if norm_id(d.get('LastUserId')) == norm_id(uid) and (d.get('LastUserName') or '').lower() == name.lower()]
    if len(devices) > 10:
        return 'refusé (%d appareils : vérifier à la main)' % len(devices)
    if dry:
        return 'à supprimer (%s, %d appareil(s))' % ('compte' if u else 'compte déjà parti', len(devices))
    how = 'déjà parti'
    if u is not None:
        try:
            r = subprocess.run(['homelabctl', 'accounts', 'delete', uid, '--yes'], capture_output=True, text=True,
                               timeout=180, cwd=HL)
            how = 'homelabctl' if r.returncode == 0 else 'homelabctl en échec (code %d)' % r.returncode
        except (OSError, subprocess.TimeoutExpired) as e:
            how = 'homelabctl indisponible (%s)' % type(e).__name__
        st, _ = jf.call('GET', '/Users/%s' % uid)
        if st == 200:  # repli : le daemon ne répond pas ; le compte de banc n'est pas lié à Jellyseerr
            st, _ = jf.call('DELETE', '/Users/%s' % uid)
            how += ' + API Jellyfin (HTTP %s)' % st
            st, _ = jf.call('GET', '/Users/%s' % uid)
            if st == 200:
                return 'ÉCHEC : compte toujours présent (%s)' % how
    res = [delete_device(jf, d, name, uid) for d in devices]
    return 'supprimé (%s), appareils : %s' % (how, ', '.join(res) if res else 'aucun')


def cmd_account_delete(a):
    if not a.name.lower().startswith(BENCH_PREFIX):
        die('refus seuls les comptes zz_ sont supprimés par cet outil', 1)
    r = delete_account(Jf(), a.id, a.name)
    print('%s : %s' % (a.name, r))
    return 0 if r.startswith(('supprimé', 'déjà')) else 1


def cmd_sweep(a):
    jf = Jf()
    now = time.time()
    max_age = a.max_age_min * 60
    users = jf.get('/Users')
    since = datetime.fromtimestamp(now - 35 * 86400).strftime('%Y-%m-%dT%H:%M:%SZ')
    log = jf.get('/System/ActivityLog/Entries?limit=20000&minDate=%s' % since).get('Items') or []
    created = {}
    for e in log:
        if e.get('Type') == 'UserCreated' and e.get('UserId'):
            t = parse_date(e.get('Date'))
            if t:
                k = norm_id(e['UserId'])
                created[k] = max(created.get(k, 0), t)
    todo, young = [], []
    for u in users:
        name = u.get('Name', '')
        if not name.lower().startswith(a.prefix):
            continue
        if (u.get('Policy') or {}).get('IsAdministrator'):
            print('ignoré : %s est administrateur' % name)
            continue
        c = created.get(norm_id(u['Id']))
        last = max([t for t in (parse_date(u.get('LastActivityDate')), parse_date(u.get('LastLoginDate'))) if t] or [0])
        if (c and now - c < max_age) or (last and now - last < max_age):
            young.append(name)
        else:
            todo.append((u['Id'], name))
    ids = {norm_id(u['Id']) for u in users}
    orphans = []
    for d in jf.get('/Devices').get('Items') or []:
        n = (d.get('LastUserName') or '').lower()
        t = parse_date(d.get('DateLastActivity'))
        if n.startswith(a.prefix) and norm_id(d.get('LastUserId')) not in ids and (not t or now - t >= max_age):
            orphans.append((d['Id'], n, d.get('LastUserId')))
    print('balayage %s* de plus de %d min : %d compte(s), %d appareil(s) orphelin(s)%s' % (
        a.prefix, a.max_age_min, len(todo), len(orphans), ' ; récents laissés : ' + ', '.join(young) if young else ''))
    if len(todo) > a.max or len(orphans) > a.max:
        print('refus plus de %d éléments : vérifier à la main, rien n\'est supprimé' % a.max)
        return 1
    bad = 0
    for uid, name in todo:
        r = delete_account(jf, uid, name, a.dry_run)
        bad += not r.startswith(('supprimé', 'à supprimer', 'déjà'))
        print('  compte %s : %s' % (name, r))
    for dev, name, uid in orphans:
        r = 'à fermer' if a.dry_run else delete_device(jf, dev, name, uid)
        bad += not r.startswith(('fermé', 'à fermer', 'déjà'))
        print('  appareil orphelin de %s : %s' % (name, r))
    return 1 if bad else 0


# ------------------------------------------------------------------------------------------------ entrée
def main():
    ap = argparse.ArgumentParser(prog='hl.py', description=__doc__.split('\n')[0])
    sub = ap.add_subparsers(dest='cmd', required=True)
    g = sub.add_parser('guard')
    g.add_argument('--idle', action='store_true')
    g.add_argument('--jf-tasks', action='store_true')
    g.add_argument('--idle-if-down', action='store_true')
    g.add_argument('--seedbox', action='store_true')
    g.add_argument('--homelabd', action='append', default=[])
    g.add_argument('--busy', action='store_true')
    b = sub.add_parser('busy')
    b.add_argument('--timers', action='store_true')
    b.add_argument('--margin-min', type=int, default=20)
    b.add_argument('--self-unit')
    d = sub.add_parser('discord')
    d.add_argument('file')
    d.add_argument('--tries', type=int, default=4)
    sub.add_parser('members-playing')
    sub.add_parser('seedbox-open')
    u = sub.add_parser('url')
    u.add_argument('service')
    pk = sub.add_parser('public')
    pk.add_argument('key')
    c = sub.add_parser('account-create')
    c.add_argument('name')
    c.add_argument('--env-out', required=True)
    c.add_argument('--id-out')
    c.add_argument('--index', type=int, default=1)
    c.add_argument('--config', action='append')
    x = sub.add_parser('account-delete')
    x.add_argument('--id', required=True)
    x.add_argument('--name', required=True)
    s = sub.add_parser('sweep')
    s.add_argument('--prefix', default=BENCH_PREFIX)
    s.add_argument('--max-age-min', type=int, default=120)
    s.add_argument('--max', type=int, default=20)
    s.add_argument('--dry-run', action='store_true')
    a = ap.parse_args()
    if a.cmd == 'sweep' and not a.prefix.lower().startswith(BENCH_PREFIX):
        die('refus le préfixe doit commencer par zz_')
    handlers = {'guard': cmd_guard, 'busy': cmd_busy, 'discord': cmd_discord, 'members-playing': cmd_members_playing,
                'seedbox-open': cmd_seedbox_open, 'url': cmd_url, 'public': cmd_public,
                'account-create': cmd_account_create,
                'account-delete': cmd_account_delete, 'sweep': cmd_sweep}
    try:
        return handlers[a.cmd](a)
    except Unreachable as e:
        print('refus Jellyfin ne répond pas (%s)' % e)
        return 3
    except KeyboardInterrupt:
        print('refus interrompu')
        return 130


if __name__ == '__main__':
    sys.exit(main())
