# Outils, bancs d'essai et travaux hors pic

À lire avant de lancer un banc d'interface, une instance d'essai, ou une opération qui coupe la lecture (redémarrage,
recréation de conteneur, montée de version). Mode d'emploi détaillé des outils : [tools/README.md](../../tools/README.md).

## 1. Principes

- **Un environnement de test, jamais la production.** Comptes ordinaires temporaires (préfixe `zz_`, toujours supprimés :
  `tools/bench/bench.sh` le garantit, sinon `homelabctl accounts delete`), réponses d'API simulées **dans le navigateur
  de test** (interception), instances à part (seconde instance de homelabd pour le tchat : [tchat.md](tchat.md#5-tests)).
- Supprimer puis recréer un compte de test plutôt que toucher aux appareils (piège `GET /Devices?userId=`, qui ignore le
  filtre). Après un banc, fermer l'appareil de test précis par `DELETE /Devices?id=<id vérifié>`.
- **Jamais un vrai groupe SyncPlay** : un banc ne clique que son propre groupe, par son nom exact.
- **Une vérification d'interface se fait à l'écran** (`getBoundingClientRect`), jamais par la seule présence.
- **Une instance d'essai ne reste jamais en marche après son banc**, et sa copie de base part avec elle
  ([sauvegardes.md](sauvegardes.md)).
- Pas de banc de 19:00 à 00:00, ni pendant une lecture de membre.

## 2. Outillage versionné (`tools/`, lot 4, 08/10)

Ces outils remplacent les copies de `backups/` (8 variantes d'« appliquer hors pic », 12 lanceurs de banc).

- **`tools/offpeak/offpeak.sh` : le SEUL outil « exécuter hors pic, seulement si personne ne regarde ».**
  - Toujours `--dry-run` d'abord (gardes réelles, rien d'écrit).
  - Programmation qui survit au redémarrage : `sudo tools/offpeak/offpeak.sh --schedule '<OnCalendar>' --name N …`
    (gabarit `homelab-offpeak@` avec `Persistent=true`, arguments dans `/etc/homelab-offpeak/N.args`, root 0644) ; suivi
    par `--list` et `--status N`, retrait par `--unschedule N`. Plus de minuteurs transitoires `systemd-run` (perdus au
    redémarrage).
  - **Tourne toujours en `deploy`** : lancé en root, il repasse en `deploy` ; avec `--as root`, seule la commande passe en
    root (`sudo -n`). Un dossier d'état inutilisable = erreur franche (code 2, bilan Discord). Si `state/offpeak/` existait
    en root : `sudo chown -R deploy:deploy /opt/homelab/state/offpeak`.
  - Un seul travail à la fois (verrou global) ; les autres réessaient au `--retry` suivant ; deux travaux programmés ne se
    bloquent plus l'un l'autre. **Jamais pendant un `lot3-*`.**
  - Gabarits dans `tools/offpeak/systemd/`, pas dans `systemd/` ; `homelabctl install` n'active jamais une unité `@`.
    `--state-dir` = essais seulement (il change aussi le verrou global).
  - Bilan sur le salon Discord admin (`--discord`).
- **`tools/bench/bench.sh` : le SEUL lanceur de banc d'interface.**
  - Comptes `zz_` toujours supprimés, même sur Ctrl-C ou TERM reçus pendant leur création (nom noté avant, id écrit par
    `hl.py` dès `POST /Users/New`, compte retiré par `hl.py` s'il est interrompu).
  - Refus de 19:00 à 00:00, pendant une lecture de membre, ou à moins de 20 min d'un `lot3-*` ou d'un
    `homelab-offpeak@*`.
  - Agent utilisateur `GcBanc/1` : `grep -v 'GcBanc/'` dans les journaux NPM (`hls_loop_watch` ne les ignore pas encore).
  - Contrat d'un scénario : `NO_INJECT=1` seulement avec `--no-inject` (les anciens scénarios testent
    `process.env.NO_INJECT ? …`, et la chaîne « 0 » vaut vrai) ; du dépôt, seuls `/repo/crates/homelabd/assets` et
    `/repo/branding` sont montés, en lecture seule (jamais `.env`, `state/` ni `backups/`) ; un ancien scénario de
    `backups/` a pour `/out` son propre dossier. Non repris : `CANDIDATE=0/1`, `PW1`/`PW2`, `/work`, `--user root`.
  - Les anciens scénarios (`modern_ui.js`, `airplay_flow.js`, `lg_audio.js`, `t6_subs.js`, `t8_quality.js`,
    `syncplay_menu.sh`, `chatshots.js`…) restent dans `backups/` et tournent tels quels par `bench.sh` ; seuls
    `header.js` et `candidats.js` sont versionnés.
  - Banc de l'en-tête à relancer après chaque montée de Jellyfin ou d'ElegantFin, hors pointe :
    `tools/bench/bench.sh tools/bench/scenarios/header.js desktop phone` (`--env WIDTH=800` pour ☰ sur bureau étroit).
- **Tests de non-régression** : `python3 tools/lib/test_hl.py` (24), `bash tools/lib/test_common.sh` (17),
  `tools/bench/bench.sh --offline tools/tests/compte-russe/compte-russe.js` (26, contre-épreuve `--env MUTATE=1`).
- Non fait (lot 4) : `tools/ops/` pour les outils d'application et de retour arrière (`apply_npm_hss.py`, `freeze.py`,
  codec-replace, `delete.py`…), qui restent dans `backups/` ; alerte sur des comptes `zz_` de plus de 2 h.

## 3. `scripts/` (inventaire au 08/10)

| Fichier | Lancé par | Rôle |
| --- | --- | --- |
| `homelab-alert.sh` | `systemd/homelab-alert@.service` (`OnFailure=`) | message Discord admin quand une unité root échoue |
| `homelabd-watchdog.sh` | `homelabd-watchdog.timer` (2 min) | relance homelabd quand `/health` ne répond plus |
| `jellyfin-transcodes-purge.sh` | `jellyfin-transcodes-purge.timer` (chaque minute) | purge du tmpfs de transcodage ([lecture-et-transcodage.md](lecture-et-transcodage.md#3-conteneur-jellyfin-et-tmpfs-de-transcodage)) |
| `seedbox-mount-watch.sh` | `seedbox-mount-watch.timer` (5 min) | ffprobe bloqués, fichier fantôme ([seedbox-et-rclone.md](seedbox-et-rclone.md#4-montage-rclone)) |
| `jellyfin-branding-apply.sh` | à la main | calque CSS ([jellyfin-interface.md](jellyfin-interface.md)) |
| `jellyfin-ui-rollback.sh` | à la main | retour arrière de l'interface |
| `jellyfin-js-apply.py` | à la main (`sudo`) | scripts JavaScript Injector (publics et privés) |
| `jellyseerr-rotate-key.py` | à la main (`sudo`) | rotation de la clé API Jellyseerr et de ses consommateurs |
| `move-to-seedbox.py` | à la main, hors pic | déménagement VPS → seedbox |
| `seedbox-cleanup.py` | à la main | ménage des torrents sans catégorie de la seedbox |
| `seedbox/gc-extract-sub.sh`, `seedbox/gc-ass2srt.py` | `subtitle_sync` par ssh (copiés dans `~/bin/` de la seedbox) | extraction des sous-titres sur la seedbox |
| `seedbox/homelab-apps-watch.sh` | crontab de la seedbox | relance des applis de la seedbox |

## 4. Fenêtre de homelabd

- Une tâche qui n'a rien à faire n'écrit plus de `run_done` au journal (passage « calme » en `debug`) : un script qui
  attend la fin d'un passage de `stack_health` par `grep 'run_done task="stack_health"'` (ancien `wait_homelabd_window` de
  `backups/jellyfin-plugins-20261005/common.sh`) **n'en trouverait plus jamais**. Lire `task_runs.<tâche>.last_start` et
  `last_end` dans `state/homelabd.json` (écrit au plus tard dans la minute) et `interval_secs` dans `homelab.toml` :
  prochain passage = `last_end` + intervalle (sémantique `OnUnitActiveSec`). C'est ce que font `offpeak.sh
  --homelabd-window` et `decision_fenetre` de `backups/lot3-20261008/common.sh`.

## 5. Exécutant du lot 3 (jusqu'au 12/10)

- Le lot 3 (mises à jour : NPM, Seerr, Arrs du VPS, outils d'administration, Homarr, gluetun, rclone, MySQL, puis
  redémarrage du VPS) garde **son propre exécutant**, `backups/lot3-20261008/run.sh J1|J2|J3|REBOOT`, lancé par les
  minuteurs `lot3-J1` (ven 09/10 08:05), `lot3-J2` (sam 10/10 08:05), `lot3-J3` (dim 11/10 08:05) et `lot3-REBOOT` (lun
  12/10 04:10). **Ne rien programmer qui chevauche ces créneaux** (09, 10 et 11/10 de 08:05 à 12:30, 12/10 de 04:10 à 07:00).
- Marqueurs dans `etat/` : `fait`, `reserve`, `echec` et `intervention` sont définitifs ; une intervention bloque tous les
  jours suivants jusqu'à ce qu'on retire `etat/<étape>`. Les refus de garde sont retentés toutes les 15 min. Journal
  `run.log`, sorties dans `journaux/<date>/`, bilan sur le salon Discord admin seulement.
- Essai à blanc : `DRY=1 [PLAN=1] [DRY_HEURE=HH:MM] [DRY_RES=etape=echec] ./run.sh <jour>`. Reprendre un jour non
  terminé : reprogrammer la même unité un autre matin (les étapes faites ne sont pas rejouées).
- **Après chaque jour réussi** : valider dans git `docker-compose.yml` et `diun/images.yml` (les scripts ne committent
  rien) et ajouter à `CHANGELOG.md` les lignes proposées par chaque préparation (sans pseudo, IP ni domaine).
- Les règles durables à reporter après exécution (NPM 2.16, Seerr 3.5, Arrs `AllowedHosts`/`TrustedNetworks`, Homarr
  1.77, gluetun v3.41.3, rclone 1.75 et MySQL 8.4, Docker 29.8, Glances, Portainer, Grafana) sont dans
  `backups/lot3-20261008/NOTES-DOC.txt` ; elles ne sont **pas** encore vraies en production et ne figurent donc pas dans
  ces runbooks. Après le redémarrage du 12/10 : mettre à jour `OLD_IMAGE` de Glances et l'ajouter à un jour.
- Hygiène : passer `prowlarr/`, `sonarr/`, `radarr/` et `gluetun/` de `backups/lot3-20261008/` en 700/600, puis supprimer
  les `run-*/` qui contiennent des secrets selon chaque README, puis `essai-a-blanc/` quand le lot est clos.
- Le minuteur transitoire `jellyfin-snapshot-purge` (10/10 12:00) n'est pas migré vers `offpeak.sh` (décision du
  propriétaire, ne pas le doubler).
- Les scripts `backups/playback-audit-*/apply-offpeak.sh` et autres « appliquer hors pic » de `backups/` sont remplacés par
  `offpeak.sh`.
