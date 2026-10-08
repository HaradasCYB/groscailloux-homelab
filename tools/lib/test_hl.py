#!/usr/bin/env python3
"""Tests des décisions de tools/lib/hl.py (2026-10-08, lot 4) : fenêtre homelabd, maintenance en cours, politique des
comptes de banc, création interrompue d'un compte de banc, dates Jellyfin. Aucun appel réseau, aucun fichier de
production lu.

Lancement : python3 tools/lib/test_hl.py   (ou python3 -m unittest discover -s tools/lib)
"""
import argparse
import contextlib
import fnmatch
import io
import json
import os
import signal
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True  # pas de __pycache__ dans le dépôt
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import hl  # noqa: E402

NOW = 1_800_000_000


class FenetreHomelabd(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.state = os.path.join(self.dir.name, 'homelabd.json')
        self.toml = os.path.join(self.dir.name, 'homelab.toml')
        with open(self.toml, 'w') as f:
            f.write('[tasks.stack_health]\ninterval_secs = 300\n[tasks.playback_canary]\ninterval_secs = 900\n')
        self.old = (hl.STATE_FILE, hl.TOML_FILE)
        hl.STATE_FILE, hl.TOML_FILE = self.state, self.toml

    def tearDown(self):
        hl.STATE_FILE, hl.TOML_FILE = self.old
        self.dir.cleanup()

    def runs(self, **tasks):
        with open(self.state, 'w') as f:
            json.dump({'task_runs': {k: {'last_start': s, 'last_end': e} for k, (s, e) in tasks.items()}}, f)

    def test_le_prochain_passage_assez_loin_est_une_fenetre_sure(self):
        self.runs(stack_health=(NOW - 100, NOW - 99))  # prochain dans 201 s
        wait, why, info = hl.homelabd_window(['stack_health:150'], NOW)
        self.assertEqual((wait, why), (0, []))
        self.assertEqual(info, ['stack_health dans 201 s'])

    def test_un_passage_imminent_fait_attendre_au_plus_une_minute(self):
        self.runs(stack_health=(NOW - 250, NOW - 249))  # prochain dans 51 s
        wait, why, _ = hl.homelabd_window(['stack_health:150'], NOW)
        self.assertEqual(wait, 60)
        self.assertEqual(why, ['stack_health dans 51 s'])
        self.runs(stack_health=(NOW - 290, NOW - 289))  # prochain dans 11 s : attendre qu'il soit passé
        self.assertEqual(hl.homelabd_window(['stack_health:150'], NOW)[0], 26)

    def test_un_passage_en_cours_fait_attendre(self):
        self.runs(stack_health=(NOW - 5, NOW - 400))  # fin plus ancienne que le début : en cours
        wait, why, _ = hl.homelabd_window(['stack_health:150'], NOW)
        self.assertEqual(wait, 15)
        self.assertIn('en cours', why[0])
        self.runs(stack_health=(NOW - 200, None))  # en cours depuis 200 s (sous RUN_TIMEOUT)
        self.assertEqual(hl.homelabd_window(['stack_health:150'], NOW)[0], 30)

    def test_un_passage_interrompu_compte_depuis_son_debut(self):
        # début il y a 700 s sans fin : plus vieux que RUN_TIMEOUT (daemon relancé) → prochain = début + 300 = passé
        self.runs(stack_health=(NOW - 700, None))
        wait, why, _ = hl.homelabd_window(['stack_health:150'], NOW)
        self.assertEqual(wait, 20)
        self.assertEqual(why, ['stack_health attendu (état relu dans la minute)'])

    def test_un_passage_en_retard_de_peu_attend_la_relecture_de_l_etat(self):
        # depuis le lot 2, l'état sur disque peut retarder d'une minute : en retard de 30 s = probablement fait
        self.runs(stack_health=(NOW - 330, NOW - 330))
        wait, why, _ = hl.homelabd_window(['stack_health:150'], NOW)
        self.assertEqual(wait, 15)
        self.assertIn('attendu', why[0])

    def test_la_pire_des_conditions_l_emporte(self):
        self.runs(stack_health=(NOW - 100, NOW - 99), playback_canary=(NOW - 800, NOW - 799))  # canari dans 101 s
        wait, why, info = hl.homelabd_window(['stack_health:150', 'playback_canary:240'], NOW)
        self.assertEqual(wait, 60)
        self.assertEqual(why, ['playback_canary dans 101 s'])
        self.assertEqual(info, ['stack_health dans 201 s'])

    def test_tache_inconnue_ou_jamais_passee_fait_attendre(self):
        self.runs(stack_health=(NOW - 100, NOW - 99))
        wait, why, _ = hl.homelabd_window(['inconnue:60'], NOW)
        self.assertEqual(wait, 30)
        self.assertIn('inconnu', why[0])

    def test_intervalle_force(self):
        self.runs(maintenance=(NOW - 100, NOW - 99))
        self.assertEqual(hl.homelabd_window(['maintenance:60:3600'], NOW)[0], 0)

    def test_condition_impossible_ou_mal_ecrite_est_une_erreur(self):
        self.runs(stack_health=(NOW - 100, NOW - 99))
        with self.assertRaises(ValueError):
            hl.homelabd_window(['stack_health:400'], NOW)  # passe toutes les 300 s : jamais vert
        with self.assertRaises(ValueError):
            hl.homelabd_window(['stack_health'], NOW)

    def test_etat_illisible_fait_attendre_sans_planter(self):
        with open(self.state, 'w') as f:
            f.write('{tronqué')
        wait, why, _ = hl.homelabd_window(['stack_health:150'], NOW)
        self.assertEqual(wait, 30)
        self.assertIn('illisible', why[0])


class Maintenance(unittest.TestCase):
    def setUp(self):
        self.old = hl.systemctl

    def tearDown(self):
        hl.systemctl = self.old

    def fake(self, services='', timers='', props=None):
        props = props or {}

        def systemctl(*args):
            if args[0] == 'list-units':
                # comme systemctl : seules les unités qui répondent à un des motifs passés
                pats = [x for x in args[1:] if not x.startswith('-')]
                lines = (services if '--type=service' in args else timers).splitlines()
                return ''.join(ln + '\n' for ln in lines if ln.split() and any(fnmatch.fnmatch(ln.split()[0], p) for p in pats))
            if args[0] == 'show':
                return props.get(args[-1], '')
            return ''
        hl.systemctl = systemctl

    def test_un_service_lot3_actif_bloque_mais_pas_soi_meme(self):
        self.fake(services='lot3-J1.service loaded active running Lot 3\n'
                           'homelab-offpeak@moi.service loaded activating start Hors pic\n'
                           'lot3-J2.service loaded inactive dead Lot 3\n')
        self.assertEqual(hl.maintenance_busy(self_unit='homelab-offpeak@moi.service', now=NOW), ['lot3-J1.service actif'])

    def test_un_minuteur_proche_bloque_seulement_avec_timers(self):
        self.fake(timers='lot3-J1.timer loaded active waiting Lot 3\nlot3-J2.timer loaded active waiting Lot 3\n',
                  props={'lot3-J1.timer': 'LastTriggerUSec=\nNextElapseUSecRealtime=@%d\n' % (NOW + 600),
                         'lot3-J2.timer': 'LastTriggerUSec=@%d\nNextElapseUSecRealtime=@%d\n' % (NOW - 7200, NOW + 86400)})
        self.assertEqual(hl.maintenance_busy(now=NOW), [])
        self.assertEqual(hl.maintenance_busy(timers=True, margin_min=20, now=NOW), ['lot3-J1.timer attendu dans 10 min'])
        self.fake(timers='lot3-J1.timer loaded active waiting Lot 3\n',
                  props={'lot3-J1.timer': 'LastTriggerUSec=@%d\nNextElapseUSecRealtime=\n' % (NOW - 300)})
        self.assertEqual(hl.maintenance_busy(timers=True, margin_min=20, now=NOW), ['lot3-J1.timer passé il y a 5 min'])

    def guard(self):
        a = argparse.Namespace(busy=True, idle=False, jf_tasks=False, idle_if_down=False, seedbox=False, homelabd=[])
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            rc = hl.cmd_guard(a)
        return rc, out.getvalue().strip()

    def test_deux_travaux_hors_pic_en_attente_ne_se_bloquent_pas(self):
        # 2026-10-08 (revue) : deux homelab-offpeak@ « activating » (même OnCalendar, rattrapage au démarrage) se
        # refusaient l'un l'autre jusqu'à la fin du créneau ; le verrou global d'offpeak.sh suffit à les sérialiser
        self.fake(services='homelab-offpeak@a.service loaded activating start Hors pic : a\n'
                           'homelab-offpeak@b.service loaded activating start Hors pic : b\n')
        for me in ('homelab-offpeak@a.service', 'homelab-offpeak@b.service'):
            self.assertEqual(hl.maintenance_busy(self_unit=me, offpeak=False, now=NOW), [], me)
        self.assertEqual(self.guard(), (0, 'ok aucune maintenance lot3 en cours'))
        # le lanceur de banc, lui, les compte toujours (aucun banc pendant un travail hors pic)
        self.assertEqual(hl.maintenance_busy(now=NOW), ['homelab-offpeak@a.service actif', 'homelab-offpeak@b.service actif'])

    def test_la_garde_d_offpeak_refuse_toujours_pendant_le_lot3(self):
        self.fake(services='lot3-J2.service loaded activating start Lot 3\n'
                           'homelab-offpeak@a.service loaded activating start Hors pic : a\n')
        rc, out = self.guard()
        self.assertEqual(rc, 1)
        self.assertEqual(out, 'refus maintenance en cours : lot3-J2.service actif')


class CompteDeBanc(unittest.TestCase):
    def test_politique_d_un_membre_ordinaire_jamais_toutes_les_bibliotheques(self):
        env = {'JELLYFIN_LIB_FILMS': 'aaa', 'JELLYFIN_LIB_SERIES': 'bbb', 'JELLYFIN_LIB_EXTRA': 'ccc, ddd,'}
        p = hl.bench_policy({'IsAdministrator': True, 'EnableAllFolders': True, 'AuthenticationProviderId': 'x'}, env)
        self.assertFalse(p['IsAdministrator'])
        self.assertFalse(p['EnableAllFolders'])
        self.assertTrue(p['IsHidden'])
        self.assertEqual(p['EnabledFolders'], ['aaa', 'bbb', 'ccc', 'ddd'])
        self.assertFalse(p['EnableContentDeletion'])
        self.assertFalse(p['EnableContentDownloading'])
        self.assertFalse(p['EnableLiveTvAccess'])
        self.assertEqual(p['LoginAttemptsBeforeLockout'], 5)
        self.assertEqual(p['AuthenticationProviderId'], 'x')  # le reste de la politique d'origine est gardé

    def test_noms_de_banc(self):
        for ok in ('zz_bench', 'zz_bench2', 'zz_outil_int'):
            self.assertTrue(hl.BENCH_NAME_RE.match(ok), ok)
        for ko in ('bench', 'zz_', 'zz_A', 'Zz_bench', 'zz_bench;rm', 'zz_' + 'a' * 31):
            self.assertFalse(hl.BENCH_NAME_RE.match(ko), ko)


class FakeJf:
    """Jellyfin simulé pour cmd_account_create : `on` = {(méthode, chemin): action} appelée avant la réponse."""

    def __init__(self, on=None, created_on_interrupt=True):
        self.users, self.calls, self.on = [], [], on or {}
        self.created_on_interrupt = created_on_interrupt

    def get(self, path):
        return self.call('GET', path)[1]

    def call(self, method, path, body=None, timeout=20):
        self.calls.append((method, path))
        act = self.on.get((method, path)) or self.on.get((method, path.split('/')[-1]))
        if method == 'POST' and path == '/Users/New':
            if act and not self.created_on_interrupt:
                act()
            self.users.append({'Name': body['Name'], 'Id': 'id-' + body['Name'], 'Policy': {}})
            if act:
                act()  # interrompu APRÈS la création côté serveur, avant d'avoir reçu la réponse
            return 200, dict(self.users[-1])
        if act:
            act()
        if method == 'GET' and path == '/Users':
            return 200, list(self.users)
        if method == 'DELETE' and path.startswith('/Users/'):
            self.users = [u for u in self.users if u['Id'] != path.split('/')[-1]]
            return 204, None
        return 204, None


class CreationInterrompue(unittest.TestCase):
    """2026-10-08 (revue) : un compte de banc créé puis interrompu (Ctrl-C, TERM, HUP) doit être retiré tout de suite,
    et son id doit être écrit avant tout réglage pour que le lanceur le retrouve."""

    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.sigs = {s: signal.getsignal(s) for s in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP)}
        self.old = (hl.Jf, hl.env)
        hl.env = lambda: {}

    def tearDown(self):
        hl.Jf, hl.env = self.old
        for s, h in self.sigs.items():
            signal.signal(s, h)
        self.dir.cleanup()

    def run_create(self, jf):
        hl.Jf = lambda: jf
        a = argparse.Namespace(name='zz_essai', env_out=os.path.join(self.dir.name, 'acc.env'),
                               id_out=os.path.join(self.dir.name, 'acc.id'), index=1, config=None)
        out = io.StringIO()
        code = 0
        with contextlib.redirect_stdout(out):
            try:
                hl.cmd_account_create(a)
            except SystemExit as e:
                code = e.code
        return code, out.getvalue().strip()

    def deleted(self, jf):
        return [p for m, p in jf.calls if m == 'DELETE']

    def test_creation_normale(self):
        jf = FakeJf()
        code, out = self.run_create(jf)
        self.assertEqual((code, out), (0, 'id-zz_essai'))
        self.assertEqual(self.deleted(jf), [])
        with open(os.path.join(self.dir.name, 'acc.id')) as f:
            self.assertEqual(f.read().strip(), 'id-zz_essai')
        self.assertEqual(oct(os.stat(os.path.join(self.dir.name, 'acc.env')).st_mode & 0o777), '0o600')

    def test_ctrl_c_pendant_le_reglage_retire_le_compte(self):
        def ctrl_c():
            raise KeyboardInterrupt
        jf = FakeJf(on={('POST', 'Policy'): ctrl_c})
        code, out = self.run_create(jf)
        self.assertEqual(code, 130)
        self.assertEqual(self.deleted(jf), ['/Users/id-zz_essai'])
        self.assertEqual(jf.users, [])
        self.assertIn('compte retiré', out)
        self.assertTrue(os.path.exists(os.path.join(self.dir.name, 'acc.id')))  # le lanceur le retrouve aussi

    def test_term_pendant_le_reglage_retire_le_compte(self):
        jf = FakeJf(on={('POST', 'Policy'): lambda: os.kill(os.getpid(), signal.SIGTERM)})
        code, _ = self.run_create(jf)
        self.assertEqual(code, 130)
        self.assertEqual(jf.users, [])

    def test_int_retire_le_compte_meme_si_le_shell_l_ignorait(self):
        # lancé par « … & » dans un script, Python hérite de SIGINT ignoré : le réglage explicite le rétablit
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        jf = FakeJf(on={('POST', 'Policy'): lambda: os.kill(os.getpid(), signal.SIGINT)})
        code, _ = self.run_create(jf)
        self.assertEqual(code, 130)
        self.assertEqual(jf.users, [])

    def test_interrompu_pendant_la_creation_retrouve_le_compte_par_son_nom(self):
        def ctrl_c():
            raise KeyboardInterrupt
        jf = FakeJf(on={('POST', '/Users/New'): ctrl_c})
        code, out = self.run_create(jf)
        self.assertEqual(code, 130)
        self.assertEqual(self.deleted(jf), ['/Users/id-zz_essai'])
        self.assertEqual(jf.users, [])
        self.assertIn('compte retiré', out)

    def test_interrompu_avant_la_creation_ne_supprime_rien(self):
        def ctrl_c():
            raise KeyboardInterrupt
        jf = FakeJf(on={('POST', '/Users/New'): ctrl_c}, created_on_interrupt=False)
        jf.users = [{'Name': 'quelqu_un', 'Id': 'id-autre', 'Policy': {}}]
        code, out = self.run_create(jf)
        self.assertEqual(code, 130)
        self.assertEqual(self.deleted(jf), [])
        self.assertIn('aucun compte créé', out)

    def test_un_compte_du_meme_nom_deja_la_est_refuse_sans_rien_toucher(self):
        jf = FakeJf()
        jf.users = [{'Name': 'zz_essai', 'Id': 'id-banc-en-cours', 'Policy': {}}]
        code, _ = self.run_create(jf)
        self.assertEqual(code, 1)
        self.assertEqual(self.deleted(jf), [])
        self.assertFalse(os.path.exists(os.path.join(self.dir.name, 'acc.id')))  # rien à nettoyer pour le lanceur


class VariablesDuToml(unittest.TestCase):
    """hlconf : `${NOM}` dans homelab.toml, mêmes règles que homelabd (2026-10-08, seedbox hors du dépôt public)."""

    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.toml = os.path.join(self.dir.name, 'homelab.toml')
        self.env = os.path.join(self.dir.name, '.env')
        with open(self.toml, 'w') as f:
            f.write('[urls]\njellyfin = "http://localhost:8096"\n'
                    '[tasks.anime_library]\ninterval_secs = 300\nseedbox_series_root = "${SEEDBOX_HOME}/media/Anime"\n'
                    '[tasks.stack_health]\npost_exec = ["sh", "-c", "x \\"$(cat /tmp/p)\\""]\n'
                    '[seedbox]\nradarr_url = "${SEEDBOX_PUBLIC_URL}/radarr"\nqbit_user = "${SEEDBOX_USER}"\n'
                    'literal = "$${PAS_UNE_VARIABLE}"\n')
        self.saved = {k: os.environ.pop(k) for k in ('SEEDBOX_HOME', 'SEEDBOX_PUBLIC_URL', 'SEEDBOX_USER')
                      if k in os.environ}
        self.old = (hl.TOML_FILE, hl.ENV_FILE)
        hl.TOML_FILE, hl.ENV_FILE = self.toml, self.env

    def tearDown(self):
        hl.TOML_FILE, hl.ENV_FILE = self.old
        for k in ('SEEDBOX_HOME', 'SEEDBOX_PUBLIC_URL', 'SEEDBOX_USER'):
            os.environ.pop(k, None)
        os.environ.update(self.saved)
        self.dir.cleanup()

    def write_env(self, text):
        with open(self.env, 'w') as f:
            f.write(text)

    def test_les_variables_de_env_sont_remplacees(self):
        self.write_env('# commentaire\nSEEDBOX_HOME=/home/seedbox/\nSEEDBOX_PUBLIC_URL="https://seedbox.example"\n'
                       'SEEDBOX_USER=seedbox\n')
        cfg = hl.toml()
        self.assertEqual(cfg['tasks']['anime_library']['seedbox_series_root'], '/home/seedbox/media/Anime')
        self.assertEqual(cfg['seedbox']['radarr_url'], 'https://seedbox.example/radarr')
        self.assertEqual(cfg['seedbox']['qbit_user'], 'seedbox')
        self.assertEqual(cfg['seedbox']['literal'], '${PAS_UNE_VARIABLE}')
        self.assertEqual(cfg['tasks']['stack_health']['post_exec'][2], 'x "$(cat /tmp/p)"')
        self.assertEqual(cfg['tasks']['anime_library']['interval_secs'], 300)

    def test_l_environnement_passe_avant_env(self):
        self.write_env('SEEDBOX_HOME=/home/a\nSEEDBOX_PUBLIC_URL=https://a.example\nSEEDBOX_USER=a\n')
        os.environ['SEEDBOX_HOME'] = '/home/b'
        self.assertEqual(hl.toml()['tasks']['anime_library']['seedbox_series_root'], '/home/b/media/Anime')

    def test_une_variable_absente_est_nommee_sans_aucune_valeur(self):
        self.write_env('SEEDBOX_HOME=/home/compte-secret\nSEEDBOX_USER=\n')
        with self.assertRaises(hl.hlconf.ConfError) as ctx:
            hl.toml()
        msg = str(ctx.exception)
        self.assertIn('SEEDBOX_PUBLIC_URL (seedbox.radarr_url)', msg)
        self.assertIn('SEEDBOX_USER (seedbox.qbit_user)', msg)  # vide = absente
        self.assertNotIn('compte-secret', msg)
        self.assertNotIn('SEEDBOX_HOME', msg)
        # les outils affichent ce message (sans valeur), pas seulement le type de l'erreur
        self.assertIn('SEEDBOX_PUBLIC_URL', hl.reason(ctx.exception))

    def test_env_illisible_compte_comme_vide_et_n_est_lu_qu_au_besoin(self):
        with open(self.toml, 'w') as f:
            f.write('[urls]\njellyfin = "http://localhost:8096"\n')
        self.assertEqual(hl.toml()['urls']['jellyfin'], 'http://localhost:8096')  # pas de .env, pas besoin
        with open(self.toml, 'a') as f:
            f.write('[seedbox]\nmedia_root = "${SEEDBOX_HOME}/media"\n')
        with self.assertRaises(hl.hlconf.ConfError):
            hl.toml()

    def test_syntaxe_mal_formee(self):
        look = hl.hlconf.lookup_from({'A': 'x'})
        for bad in ('${', '${A', '${}', '${1A}', '${A-B}'):
            with self.assertRaises(ValueError, msg=bad):
                hl.hlconf.expand(bad, look)
        with self.assertRaises(hl.hlconf.ConfError) as ctx:
            hl.hlconf.expand_tree({'seedbox': {'media_root': '${A'}}, look)
        self.assertIn('seedbox.media_root', str(ctx.exception))
        self.assertEqual(hl.hlconf.expand('${A}/${A}$1', look), 'x/x$1')


class Dates(unittest.TestCase):
    def test_dates_jellyfin(self):
        self.assertEqual(hl.parse_date('2026-10-07T21:55:39.3762918Z'), 1791410139.376291)
        self.assertIsNone(hl.parse_date('0001-01-01T00:00:00.0000000Z'))
        self.assertIsNone(hl.parse_date(''))
        self.assertIsNone(hl.parse_date('pas une date'))


if __name__ == '__main__':
    unittest.main(verbosity=1)
