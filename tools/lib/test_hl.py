#!/usr/bin/env python3
"""Tests des décisions de tools/lib/hl.py (2026-10-08, lot 4) : fenêtre homelabd, maintenance en cours, politique des
comptes de banc, dates Jellyfin. Aucun appel réseau, aucun fichier de production lu.

Lancement : python3 tools/lib/test_hl.py   (ou python3 -m unittest discover -s tools/lib)
"""
import json
import os
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
                return services if '--type=service' in args else timers
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


class Dates(unittest.TestCase):
    def test_dates_jellyfin(self):
        self.assertEqual(hl.parse_date('2026-10-07T21:55:39.3762918Z'), 1791410139.376291)
        self.assertIsNone(hl.parse_date('0001-01-01T00:00:00.0000000Z'))
        self.assertIsNone(hl.parse_date(''))
        self.assertIsNone(hl.parse_date('pas une date'))


if __name__ == '__main__':
    unittest.main(verbosity=1)
