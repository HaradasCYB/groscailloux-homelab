# tools/ — outillage versionné

Créé le 2026-10-08 (lot 4 de la revue Kaizen). Avant, ces outils étaient recopiés et adaptés dans `backups/<chantier>/`
(hors git, un seul exemplaire sur le disque du VPS) : 8 variantes de « appliquer hors pic » et 12 lanceurs de banc.
Les **sorties** (journaux, captures) ne vont jamais dans le dépôt : `state/offpeak/` et `backups/bench/`.

| Dossier | Rôle |
|---|---|
| `offpeak/offpeak.sh` | exécuter une commande hors pic, seulement si personne ne regarde ; programmation qui survit au redémarrage |
| `bench/bench.sh` | lancer un banc d'interface (navigateur sans écran) avec un compte de banc toujours supprimé |
| `tests/` | bancs de non-régression lancés par `bench.sh` (`compte-russe/` : voie russe de Mon compte ; `compte-vo/` : filet du mode VO) ; `compte-telechargements/` : appariement de l'onglet Téléchargements, sous Node |
| `lib/` | `hl.py` (appels Jellyfin, homelabd, Discord ; seul à lire les secrets de `.env`), `hlconf.py` (`.env` et `homelab.toml` lus comme par homelabd : `${NOM}` remplacés ; aussi pour `scripts/*.py`), `common.sh`, leurs tests |

Configuration et secrets : toujours ceux de `/opt/homelab` (`.env`, `homelab.toml`, `state/`), même lancé depuis un
autre arbre de travail. Aucun secret en argument ni à l'écran ; aucune adresse en dur (`[urls]` du TOML, `.env`).

## Hors pic : `offpeak/offpeak.sh`

```bash
# 1. toujours d'abord à blanc : gardes réelles, rien lancé, rien écrit, bilan affiché
tools/offpeak/offpeak.sh --name jf-restart --window 04:30-05:20 --members \
  --homelabd-window stack_health:150,playback_canary:240 --healthy jellyfin --discord --dry-run \
  -- docker compose restart jellyfin

# 2. programmer (unités installées : survit au redémarrage ; tous les jours jusqu'à réussite grâce à --once)
sudo tools/offpeak/offpeak.sh --schedule '*-*-* 04:30' --name jf-restart --window 04:30-05:20 --members \
  --homelabd-window stack_health:150,playback_canary:240 --healthy jellyfin --once --discord \
  -- docker compose restart jellyfin

tools/offpeak/offpeak.sh --list                 # travaux programmés, prochain passage, dernier résultat
tools/offpeak/offpeak.sh --status jf-restart    # dernier résultat + 20 lignes de journal
sudo tools/offpeak/offpeak.sh --unschedule jf-restart
```

Gardes (toutes réelles, aussi à blanc) :
- `--if-idle` : aucune lecture Jellyfin ;
- `--members` : en plus, aucune tâche planifiée Jellyfin en cours ;
- `--seedbox` : aucun fichier ouvert sous le montage ;
- `--homelabd-window tâche:secondes` : prochain passage de la tâche à au moins tant de secondes (lu dans
  `state/homelabd.json` + `interval_secs` du TOML) ;
- toujours : aucun service `lot3-*` actif, et un seul travail hors pic à la fois (verrou global : un travail qui ne
  l'obtient pas réessaie au `--retry` suivant ; deux travaux en attente ne se bloquent jamais l'un l'autre).

Refus : nouvel essai toutes les `--retry` minutes (10) jusqu'à la fin du `--window`. Après la commande : `--healthy
<service>` doit redevenir sain ; si la commande a recréé **gluetun**, qBittorrent est recréé et le port transféré
reposé (piège du 19/09). Codes : 0 fait, 1 échec, 2 usage, 3 rien fait. Journal et sorties : `state/offpeak/<nom>/`.

Gabarits systemd : `offpeak/systemd/homelab-offpeak@.{service,timer}`, installés par `--schedule` (pas par
`homelabctl install`, qui n'active d'ailleurs jamais un gabarit `@`). Un minuteur sans `OnCalendar` (posé dans
`homelab-offpeak@<nom>.timer.d/when.conf`) ne démarre pas : rien n'agit seul. La commande et les options sont dans
`/etc/homelab-offpeak/<nom>.args` (root, 0644) ; `--as root` pour une commande qui l'exige (redémarrage de l'hôte).

**`offpeak.sh` tourne toujours en `deploy`** : lancé en root (`sudo offpeak.sh …`), il repasse en `deploy` et seule la
commande passe en root, par `sudo -n` (sans `--as`, `sudo offpeak.sh` garde la commande en root). Le dossier d'état,
les verrous et les journaux appartiennent donc toujours à `deploy` ; un dossier inutilisable (créé par root à la main)
est une erreur franche (code 2, bilan Discord), jamais « un autre travail tourne ».

## Bancs : `bench/bench.sh`

```bash
tools/bench/bench.sh tools/bench/scenarios/header.js desktop phone          # banc de fumée de l'en-tête
tools/bench/bench.sh tools/bench/scenarios/candidats.js desktop             # l'injection de candidats marche-t-elle ?
tools/bench/bench.sh --accounts 2 --prefix zz_sp --item <id> --env ITEM2=<id> --timeout 1500 \
  tools/bench/scenarios/syncplay.js desktop                                  # SyncPlay : reprise après un saut (~12 min)
tools/bench/bench.sh --candidate /chemin/candidats tools/bench/scenarios/header.js desktop-legacy tv
tools/bench/bench.sh --offline tools/tests/compte-russe/compte-russe.js     # sans compte ni Jellyfin
tools/bench/bench.sh backups/jellyfin12-test-20261003/modern_ui.js :desktop :iphone   # ancien scénario (voir plus bas)
tools/bench/bench.sh --sweep [--dry-run]     # comptes et appareils zz_* de plus de 2 h, vérifiés un par un
```

- **Appareils** : `desktop desktop-legacy phone phone-legacy iphone android tablet tv`, ou `mise-en-page:appareil`
  comme l'ancien `runprod.sh`.
- **Compte de banc** `zz_bench` (`--prefix`, `--accounts 2`, `--user-config SubtitleMode=Smart`) :
  - créé caché, avec la politique d'un membre ordinaire (jamais toutes les bibliothèques) ;
  - **toujours supprimé**, aussi sur erreur, Ctrl-C ou TERM, même reçus pendant sa création (nom noté avant, id écrit
    par `hl.py` dès la création, compte retiré par `hl.py` lui-même s'il est interrompu) ; le navigateur est fermé avant ;
  - ses appareils sont fermés par identifiant exact, après relecture de leur dernier utilisateur.
- **Gardes** (`--force` pour passer outre, `--wait N` pour attendre) :
  - aucun banc de 19:00 à 00:00 ;
  - ni pendant une lecture de membre ;
  - ni pendant, ni dans les 20 min autour d'un passage `lot3-*` / `homelab-offpeak@*` ;
  - un seul conteneur de banc à la fois.
- **Conteneur** : image épinglée `tag@sha256`, 1 cœur, `cpu-shares` 256, 2 Go.
- **Agent utilisateur** suffixé `GcBanc/1` sur toutes les pages, anciens scénarios compris : `grep -v 'GcBanc/'` dans
  les journaux NPM.
- **Candidats** (`--candidate DIR`, dans CE navigateur seulement) :
  - `chat.js` et `compte.js` remplacent `/gc-chat/app.js` et `/gc-compte/app.js` ;
  - `gc-*.js` sont ajoutés au `private.js` servi (`gc-tv.js`, `gc-lang.js` et `gc-socket.js` au `public.js`) ;
  - `groscailloux-tv.css` remplace le CSS ;
  - `deployed-<x>.txt` = texte exact d'une version déployée à retirer.
  
  `--no-inject` : ce que sert la production (`NO_INJECT=1` ; sans l'option, `NO_INJECT` n'est pas posé).
- **Contrat d'un scénario** :
  - variables `JF_URL`, `USER_NAME`, `PW`, `USER_ID` (`_2`… pour les comptes suivants), `DEVICE`, `LAYOUT`,
    `BENCH_DEVICE`, `ITEM`, `NO_INJECT=1` avec `--no-inject` seulement ;
  - dossiers `/out` (sorties), `/scen` (dossier du scénario), `/bench/bench.js` (appareils, connexion, mesure à
    l'écran, résultats) ; du dépôt, **seulement** `/repo/crates/homelabd/assets` et `/repo/branding` en lecture seule
    (jamais `.env`, `state/` ni `backups/` : le code du scénario tourne sous l'uid de `deploy`) ;
  - code de sortie 0 = réussi.
  - Toute vérification d'interface se fait **à l'écran** (`getBoundingClientRect`), jamais par la seule présence.
- **Anciens scénarios** (`backups/…`, hors de `tools/`) : `/out` est par défaut **leur propre dossier**, comme avec
  `runprod.sh` (ils y lisent leurs entrées : `/out/gc-lang.candidate.js`, `/out/moverlay_lib.js`…) ; `--out` pour en
  changer. Ne sont **pas** repris : `CANDIDATE=0/1` de `lg-tv-20260929`, `PW1`/`PW2` de `syncplay-20261003`, `/work` de
  `t6_run.sh`, `--user root` (le conteneur tourne sous l'uid 1000) : poser par `--env` ou adapter le scénario.

## Tests : `tests/compte-russe/`

Le vrai `crates/homelabd/assets/compte/app.js` dans une page simulée (aucun appel réseau réel) :
- les réglages avancés d'une demande ne partent que pour un dossier russe choisi par un compte autorisé ;
- refus par défaut si l'API répond en erreur ou pas encore ;
- bloc « Version » caché aux autres et sur télé.

Contre-épreuve : `--env MUTATE=1` doit échouer. La décision côté serveur (`route_set`) a ses tests unitaires dans
`subs_api.rs`.

## Tests : `tests/compte-vo/`

`tools/bench/bench.sh --offline tools/tests/compte-vo/compte-vo.js desktop tv` : le vrai `app.js` de Mon compte pendant une
lecture simulée (session, fiche, pistes, `/api/original` servis par interception), pour le filet du mode VO face à la
préférence native « Langue d'origine » (`homelab_core::vo_native`) :
- piste reçue déjà en VO : aucune commande ni autre requête ;
- partie en VF : langue d'origine lue sur la fiche Jellyfin (ou sa série), TMDB seulement pour une fiche vide, anglais si
  inconnue ; jamais un titre d'origine française, jamais l'audiodescription, jamais un doublage qui n'est pas la VO ;
- une seule tentative par titre, rien en mode « fr ».

Contre-épreuve : `--env MUTATE=1` (piste reçue et fiche ignorées) doit échouer. Côté serveur, les décisions (mode,
comptes gardés, migration, garde) ont leurs tests dans `vo_native.rs`.

## Tests : `tests/compte-telechargements/`

`node tools/tests/compte-telechargements/match.test.js` : les fonctions pures du vrai `app.js` de Mon compte (entre les
marqueurs `gc-dl-match:start` et `gc-dl-match:end`) sous Node, sans navigateur ni réseau, pour l'onglet Téléchargements :
- une carte d'épisode trouve son avertissement expliqué, et seulement le sien (source, titre sans accents, `SxxEyy`) ;
- jamais d'appariement ambigu (deux candidats, deux cartes identiques, série sans épisode, autre source) ;
- film par le titre (l'année départage), pack de saison seulement si tous ses épisodes portent la même note.

Contre-épreuve : `MUTATE=1` (unicité des candidats non exigée) doit échouer. Côté serveur, les familles et le nettoyage du
message d'origine ont leurs tests dans `queue_explain.rs`.

## Tests des outils

```bash
python3 tools/lib/test_hl.py      # fenêtre homelabd, maintenance en cours (deux travaux en attente ne se bloquent pas),
                                  # politique et création interrompue des comptes de banc, dates, variables ${NOM} du TOML
bash tools/lib/test_common.sh     # créneau horaire (à cheval sur minuit compris), nettoyage des textes Discord
sh scripts/test_seedbox-rclone-conf.sh   # config effective du montage seedbox : hôte et compte DANS le fichier,
                                         # refus qui nomment la variable (jamais sa valeur)
```

Aucun appel réseau ni fichier de production. À relancer après toute modification de `lib/`.
