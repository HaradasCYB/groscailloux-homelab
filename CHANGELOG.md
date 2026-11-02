# Journal des versions de Groscailloux

Groscailloux est un serveur de streaming privé pour une petite communauté : Jellyfin pour regarder, Jellyseerr
pour demander, et une chaîne d'acquisition automatique derrière. Ce journal retrace les grandes étapes, de la
première pile Docker à la version 1.0.0 qui clôt la bêta.

Les dates antérieures au dépôt git (avril-août 2026) sont **reconstituées** à partir des configurations et des
notes d'exploitation ; à partir du 10/09/2026, chaque ligne renvoie aux commits. Depuis le 21/09/2026, les versions
publiées le même jour sont regroupées sous la dernière d'entre elles (les tags git intermédiaires restent).

Le 08/10/2026, ce journal a été remis en ordre : l'ancienne section « 1.19.1 » (treize jours de nouveautés, du 23/09
au 05/10) est découpée en 1.19.1, 1.20.0 et 1.21.0, et l'ancienne « 1.20.0 » (revue Kaizen) est devenue 1.22.0.

## En bref

| Version | Date | Thème |
| --- | --- | --- |
| **[1.31.0](#1310--02112026--pages-web-en-noir-et-bleu-groscailloux-tv)** | 02/11/2026 | **Pages web en noir et bleu Groscailloux TV** |
| [1.30.5](#1305--31102026--inscription--lessai-annoncé-tel-quil-est) | 31/10/2026 | Inscription : l'essai annoncé tel qu'il est |
| [1.30.4](#1304--30102026--série-sortie-le-jour-même-reprise-toutes-les-2-h) | 30/10/2026 | Série sortie le jour même reprise toutes les 2 h |
| [1.30.3](#1303--30102026--films-français-cherchés-malgré-une-date-tmdb-tardive) | 30/10/2026 | Films français cherchés malgré une date TMDB tardive |
| [1.30.2](#1302--29102026--une-saison-non-demandée-nest-plus-téléchargée) | 29/10/2026 | Une saison non demandée n'est plus téléchargée |
| [1.30.1](#1301--29102026--saisons-netflix-en--parties--rangées-dans-la-bonne-saison) | 29/10/2026 | Saisons Netflix en « Parties » rangées dans la bonne saison |
| [1.30.0](#1300--27102026---continuer-à-regarder--à-la-carte-et-filtre-de-durée) | 27/10/2026 | « Continuer à regarder » à la carte et filtre de durée |
| [1.29.0](#1290--21102026--liste-rouge-et-imports-partiels-signalés) | 21/10/2026 | Liste rouge et imports partiels signalés |
| [1.28.0](#1280--15102026--mails-aux-couleurs-de-groscailloux-tv) | 15/10/2026 | Mails aux couleurs de Groscailloux TV |
| [1.27.0](#1270--14102026--nouveau-domaine-principal) | 14/10/2026 | Nouveau domaine principal |
| [1.26.0](#1260--10102026--syncplay-après-un-saut-films-guettés-avant-leur-sortie-et-lot-3-jour-2) | 10/10/2026 | SyncPlay après un saut, films guettés avant leur sortie et lot 3, jour 2 |
| [1.25.0](#1250--09102026--téléchargements-expliqués-et-lot-3-jour-1) | 09/10/2026 | Téléchargements expliqués et lot 3, jour 1 |
| [1.24.0](docs/changelog/1.0-1.24.md#1240--09102026--seedbox-hors-du-dépôt-mode-vo-natif-préparé-et-releases-publiques) | 09/10/2026 | Seedbox hors du dépôt, mode VO natif préparé et releases publiques |
| [1.23.0](docs/changelog/1.0-1.24.md#1230--08102026--abonnés-à-la-main--aide-et-annonces--catalogue-et-langue-dorigine) | 08/10/2026 | Abonnés à la main, « Aide et annonces », catalogue, langue d'origine et outils |
| [1.22.0](docs/changelog/1.0-1.24.md#1220--07--08102026--revue-kaizen--alertes-sobriété-et-portes-dadministration) | 07 → 08/10/2026 | Revue Kaizen : alertes, sobriété et portes d'administration |
| [1.21.0](docs/changelog/1.0-1.24.md#1210--02--05102026--résilience-jellyfin-121-et-nouvelle-interface) | 02 → 05/10/2026 | Résilience, Jellyfin 12.1 et nouvelle interface |
| [1.20.0](docs/changelog/1.0-1.24.md#1200--2509--01102026--codec-x265-voie-russe-chromecast-et-site-de-secours) | 25/09 → 01/10/2026 | Codec x265, voie russe, Chromecast et site de secours |
| [1.19.1](docs/changelog/1.0-1.24.md#1191--23--25092026--audit--fiabilité-et-sécurité) | 23 → 25/09/2026 | Audit : fiabilité et sécurité |
| [1.19.0](docs/changelog/1.0-1.24.md#1190--21--22092026--suivi-des-demandes-langue-et-canari) | 21 → 22/09/2026 | Suivi des demandes, langue et canari |
| [1.18.0](docs/changelog/1.0-1.24.md#1180--20092026--abonnement-automatique--mon-compte--discord-et-bienvenue) | 20/09/2026 | Abonnement automatique, « Mon compte », Discord et bienvenue |
| [1.15.1](docs/changelog/1.0-1.24.md#1151--19092026--télé-français-partout-airplay-et--lire-sur-) | 19/09/2026 | Télé, français partout, AirPlay et « Lire sur » |
| [1.14.0](docs/changelog/1.0-1.24.md#1140--18092026--animés-complets-tout-par-la-seedbox-lecture-qui-tient-la-nuit) | 18/09/2026 | Animés complets, tout par la seedbox, lecture qui tient la nuit |
| [1.8.0](docs/changelog/1.0-1.24.md#180--17092026--recherche-par-identifiant-un-seul-indexeur-menus-anime) | 17/09/2026 | Recherche par identifiant, un seul indexeur, menus Anime |
| [1.1.1](docs/changelog/1.0-1.24.md#111--16092026--la-lecture-saide-elle-même-adaptée-aux-téléviseurs) | 16/09/2026 | La lecture s'aide elle-même, adaptée aux téléviseurs |
| [1.0.0](docs/changelog/1.0-1.24.md#100--15092026--fin-de-la-bêta--une-seule-plateforme-vps--seedbox) | 12 → 15/09/2026 | Fin de la bêta : une seule plateforme VPS + seedbox |
| [0.9.0](docs/changelog/1.0-1.24.md#090--10--11092026--la-refonte--homelabd) | 10 → 11/09/2026 | La refonte : l'automatisation réécrite en Rust (homelabd) |
| [0.3.0](docs/changelog/1.0-1.24.md#030--août-2026-reconstitué--jellyfin-enrichi) | août 2026 | Jellyfin enrichi par les plugins |
| [0.2.0](docs/changelog/1.0-1.24.md#020--fin-mai-2026-reconstitué--stabilisation) | fin mai 2026 | Stabilisation |
| [0.1.0](docs/changelog/1.0-1.24.md#010--30042026--06052026--les-fondations) | 30/04 → 06/05/2026 | Les fondations : demander un film depuis Jellyfin, tout arrive seul |

Les sections de la 1.24.0 et des versions antérieures (0.1.0 à 1.24.0) sont dans
[docs/changelog/1.0-1.24.md](docs/changelog/1.0-1.24.md) ; le tableau ci-dessus y renvoie.

---

## 1.31.0 — 02/11/2026 — Pages web en noir et bleu Groscailloux TV

Nouveauté : toutes les pages servies par homelabd (hors Jellyfin) prennent la DA de l'interface et des mails, noir,
bleu et bouton principal blanc, avec un seul gabarit commun au lieu de six styles recopiés (commits 950b857, 5299ca2,
3a86274, 29c3284, 04afa04, 9c3a432, 795f62c, 4645b4f, b794767, d17792f, 919e26d, bb31b33).

### Pour les membres

- Inscription, bienvenue, premiers pas, lien expiré, abonnement Premium (page, activation, merci) et guide : fond noir,
  logo Groscailloux TV en en-tête, police du système (plus de polices Google), bouton principal blanc ; textes et
  fonctionnement inchangés. Le guide a un sommaire lisible sur téléphone. Contrastes conformes WCAG AA.
- Le tchat et « Mon compte » prennent le même bleu.

### Pour l'administrateur

- Module `crates/homelabd/src/ui.rs` (jetons, CSS de base, `page`, `page_admin`, `page_bare`, page d'erreur) et logo
  servi à `/assets/logo.png` ; référence de la DA : [docs/DESIGN.md](docs/DESIGN.md).
- Onboarding admin, connexion, `/recherche`, `/accounts`, liste rouge et `status.html` (sans en-tête dans Homarr) sur
  le même gabarit ; onboarding admin entièrement en français ; colonnes de la liste rouge alignées d'une section à
  l'autre ; tableaux qui défilent signalés sur téléphone ; boutons de `/recherche` à 44 px ; réponses brutes
  (« Session expirée », « Jellyfin injoignable ») en page d'erreur stylée ; `/recherche`, `/connexion` et
  l'onboarding sans référent. `assets/don.html` (mort) supprimé.
- Test ignoré `web::render_tests::render_pages_to_dir` : rend toutes les pages avec des données factices dans
  `GC_RENDER_DIR`. Aucune clé de `homelab.toml`.

## 1.30.5 — 31/10/2026 — Inscription : l'essai annoncé tel qu'il est

Correction : à l'inscription publique, l'essai gratuit est décidé avant les mails et la page, qui disent enfin ce qui
s'est passé (commit d7094f9).

### Pour les membres

- Après l'inscription, le mail de bienvenue annonce « Ton essai de N jours a commencé » et la page « Ton essai de
  N jours démarre dès que ton mot de passe est choisi » (N = `[subscriptions] trial_days`, 7 aujourd'hui), au lieu de
  promettre une activation par l'administrateur pour un compte déjà actif. Le formulaire `/inscription` annonce aussi
  l'essai. Si l'essai est impossible (plafond de comptes atteint), le message reste « sera activé par
  l'administrateur ».

### Pour l'administrateur

- `onboard::run` démarre l'essai (`subscription_ops::start_trial`, sous `onboard_lock`) avant le mail de bienvenue
  et le mail admin. Essai démarré : « Nouveau membre en essai : X » (fin d'essai, parrain, sans parrain ou code
  inconnu ignoré, rien à faire). Essai impossible : « Nouveau compte à activer : X » avec la raison (plafond premium,
  `trial_days = 0`, abonnements désactivés, erreur) et le parrain ; plafond atteint : fiche « suspendue » au lieu
  d'un essai qui courait sur un compte fermé. Lien de bienvenue non parti : dit dans le mail admin (avertissement).
  Inscription arrêtée après la création du compte Jellyfin : alerte « Inscription ratée à moitié ». La réponse
  neutre (adresse déjà inscrite) tient compte du plafond, pour ne rien révéler. Onboarding admin, `homelabctl
  onboard` et `user_poller` inchangés. Nouvel exemple `homelabctl mail-test --kind welcome_trial` (18 types) :
  installer homelabd et homelabctl ensemble. Aucune clé de `homelab.toml`.

## 1.30.4 — 30/10/2026 — Série sortie le jour même reprise toutes les 2 h

Correction : une saison cherchée trop tôt après sa sortie, avant que C411 ne la publie, n'attend plus le lendemain ;
elle est reprise toutes les 2 h pendant 48 h (commit 0111e5d).

### Pour les membres

- Une série demandée le jour de sa sortie arrive dans les heures qui suivent sa publication au lieu d'un jour plus
  tard. L'onglet Demandes affiche la prochaine tentative réelle (« dans ~2 h »), et « version française attendue »
  quand seule une version sous-titrée toute récente existe.

### Pour l'administrateur

- `series_search` : fenêtre de sortie. Une saison sans release dont le dernier épisode manquant est diffusé depuis au
  plus `release_window_hours` (48) est reprise toutes les `release_retry_hours` (2) au lieu de `retry_after_hours`
  (24), par identifiant seul (le texte libre reste fait au premier passage puis toutes les 24 h). Cas du 30/10 :
  *Dead-End Job* (« Un job mortel »), 8 épisodes diffusés à 04:00 UTC, cherchés à 08:15 sans résultat, publiés en
  MULTi VFF sur C411 le jour même et remis au lendemain ; le RSS de Sonarr ne les rattrape pas (titre français inconnu
  de Sonarr et de TMDB). Probablement le même cas pour *Doctor X* (à mesurer). Coût au pire : 28 requêtes par saison
  introuvable sur 48 h (24 passages par identifiant, dont 2 avec le texte libre).
- `series_search` : VOSTFR/VO de moins de `vo_wait_hours` (3) h retenue hors animés, épisode par épisode (résultat
  `vo_wait`, repris après 2 h), comme pour les films.
- `series_search` : une saison dont un torrent étiqueté `homelab:series=…:season=…` se télécharge dans qBittorrent
  n'est plus reprise (doublon ou réétiquetage évités, garde-fou borné à 24 h).
- Onglet Demandes : « prochaine tentative » calculée par la règle de la tâche (`record_wait_secs`) ; corrigé au
  passage : épisodes pris (5 min au lieu de 7 j), secours public (`fallback_retry_hours` au lieu de 12 h en dur),
  film pris (`[tasks.movie_search] retry_after_hours` au lieu de 7 j).
- **Trois clés ajoutées** à `[tasks.series_search]` (`release_window_hours`, `release_retry_hours`, `vo_wait_hours`) :
  installer homelabd **et** homelabctl ensemble (`deny_unknown_fields`). État : `latest_air` et `full_at` ajoutés aux
  enregistrements de saison (un ancien binaire les ignore). Détail :
  [recherche-et-indexeurs.md](docs/automation/recherche-et-indexeurs.md#series_search--10-min-remplace-unknown_series_grab) et
  [arrs-et-indexeurs.md](docs/runbooks/arrs-et-indexeurs.md).

## 1.30.3 — 30/10/2026 — Films français cherchés malgré une date TMDB tardive

Correction : un film français sorti en salle depuis plus de 110 jours est cherché même quand TMDB ne connaît qu'une
date de DVD ou une date numérique encore à venir (bfaeb6b).

### Pour les membres

- Un film français sorti au cinéma depuis environ 4 mois n'attend plus sa date de DVD : il est cherché tous les
  3 jours, et l'onglet Demandes l'explique (« En salle depuis le … : la VOD française est sans doute sortie ») au lieu
  de « Pas encore sorti en numérique ».

### Pour l'administrateur

- `movie_search` (`vod_overdue`) : un film en langue originale française, non disponible pour Radarr, en salle depuis
  au moins `min_days_after_cinema` (110) jours et dont la date TMDB est encore à venir est cherché. Cas du 30/10 :
  *De Gaulle : Résistance*, salle le 03/06, aucune date numérique, DVD le 04/11 (Radarr : disponible le 02/11), déjà
  sur C411 et jamais cherché, alors que la seconde partie, datée en numérique, a été prise en 5 min. Films étrangers
  inchangés. Résumé : « N français en salle depuis plus de 110 jours ». Aucune clé ajoutée à `homelab.toml`.
- `movie_search` ne prend jamais une qualité d'avant-sortie (CAM, TS, TC, DVDSCR, WORKPRINT, REGIONAL) ni une qualité
  inconnue, quel que soit le profil (`PRE_RELEASE_QUALITIES`).
- Onglet Demandes : un film français sans date en attente de sa VOD affiche « VOD vers le … » même avant 90 jours de
  salle (il affichait « Pas encore sorti en numérique »). Détail :
  [recherche-et-indexeurs.md](docs/automation/recherche-et-indexeurs.md#movie_search--5-min) et [arrs-et-indexeurs.md](docs/runbooks/arrs-et-indexeurs.md).

## 1.30.2 — 29/10/2026 — Une saison non demandée n'est plus téléchargée

Correction : une saison que personne n'a demandée, suivie dans Sonarr seulement parce qu'elle contient des fichiers,
n'est plus cherchée ni téléchargée par homelabd (936d65a).

### Pour les membres

- Demander une seule saison d'une série ne fait plus télécharger les autres : seules les saisons demandées dans
  Jellyseerr sont cherchées automatiquement.

### Pour l'administrateur

- `series_search` ne cherche plus que les saisons **demandées** (même notion que `monitor_sync` : statuts refusé et en
  échec exclus, demande routée par `media.serviceId`, saison supprimée par `deletion_cleanup` écartée tant qu'elle
  n'est pas redemandée). Cascade Lupin du 28/10 : une saison 3 demandée seule, des fichiers mal rangés en saison 2,
  `monitor_sync` la rend suivie (`|| has_files`), `series_search` la prend, et de même pour la saison 1. Les séries
  inconnues de Jellyseerr (ajoutées à la main) restent cherchées comme avant ; les demandes d'un autre titre (tvdbId
  différent, id Sonarr périmé côté Jellyseerr) sont ignorées, et une fiche qui n'a que de telles demandes est cherchée
  comme avant ; le suivi et les mises à niveau par le RSS de Sonarr ne changent pas. Jellyseerr est
  lu après les fiches Sonarr, en entier : illisible ou lu en partie, le passage échoue sans aucune requête à
  l'indexeur. Résumé : `not_requested=<n>` ; détail par saison en `debug`. Aucune clé ajoutée à `homelab.toml`.
  Détail : [recherche-et-indexeurs.md](docs/automation/recherche-et-indexeurs.md#series_search--10-min-remplace-unknown_series_grab) et
  [arrs-et-indexeurs.md](docs/runbooks/arrs-et-indexeurs.md).

## 1.30.1 — 29/10/2026 — Saisons Netflix en « Parties » rangées dans la bonne saison

Correction : une saison demandée d'une série que Sonarr numérote en « Parties » (numérotation scène) n'est plus rangée
dans une autre saison à l'import (7bd45dd).

### Pour les membres

- Une saison demandée de *Lupin* (et des séries Netflix découpées de même) arrive dans la bonne saison, au lieu d'être
  rangée à moitié dans la saison précédente.

### Pour l'administrateur

- `torrent_import` ne range plus un pack pris pour la saison N dans la saison de la numérotation scène de Sonarr (Lupin,
  28/10 : `S03` rangé en 2x01…2x07, `S02` en 1x06…1x10). Quand Sonarr bute (« was unexpected considering the … folder
  name »), le nom et l'étiquette font foi pour tout le torrent ; au moindre doute, rien n'est rangé hors de la saison N.
  Détail : [arrs-et-indexeurs.md § 5](docs/runbooks/arrs-et-indexeurs.md#5-séries--saisons-packs-et-numérotations).

## 1.30.0 — 27/10/2026 — « Continuer à regarder » à la carte et filtre de durée

Deux demandes des membres, dans l'interface web (navigateur, applis iPhone et Android, télé web) : retirer un titre de
« Continuer à regarder » sans perdre sa progression, et trier les films par durée (3a0a6b9).

### Pour les membres

- **« Retirer de Continuer à regarder »** dans le menu « … » d'une vignette de « Continuer à regarder » ou « À suivre »
  (sur téléphone : appui long, puis ⋮). Une série part entière ; la progression, l'historique et « vu » ne bougent pas,
  et le titre revient tout seul dès qu'on le relance. Message avec « Annuler ». Retrait gardé par compte, sur tous les
  appareils. Liste des titres retirés et bouton « Remettre » dans **Mon compte**.
- **Filtre « Durée »** dans le menu Filtres des bibliothèques de films (Films, Films d'animation) : « Moins de 2 h » ou
  « Moins de 1 h 30 », avec les autres filtres, le tri et les pages ; « Réinitialiser les filtres » le retire.

### Pour l'administrateur

- Scripts injectés `gc-continue.js` et `gc-duree.js` (`sudo scripts/jellyfin-js-apply.py`), section de Mon compte dans
  `crates/homelabd/assets/compte/app.js` ; retraits dans les `DisplayPreferences` du compte (`gc-continue`/`groscailloux`).
  Détail et limites (applis natives non couvertes) : [jellyfin-interface.md § 11 et § 12](docs/runbooks/jellyfin-interface.md#11--continuer-à-regarder--à-la-carte-2710).
- Le retrait de Jellyfin Enhanced (par épisode, sans gestion chez nous) est caché dans les menus.
- Bancs `tools/bench/scenarios/continuer.js` et `duree.js`.

## 1.29.0 — 21/10/2026 — Liste rouge et imports partiels signalés

L'admin a une page pour libérer de l'espace sans script : la **liste rouge** des films et séries que personne n'a lancés
depuis 60 jours, avec « Exempter » et « Nettoyer » (3f10a8b, 69bec12, cea9866, 1768505, 7744508). Côté téléchargements,
un pack d'animé numéroté en absolu n'est plus importé à moitié en silence (c7c3401, 37e5c2e, c74f8c7).

### Pour les membres

- **Saisons d'animé complètes.** Un pack dont les épisodes portent leur numéro absolu sous la forme « S01E1156 » est
  maintenant importé en entier (le 18/10, 18 épisodes sur 25 d'une saison étaient restés de côté).
- **Carte de demande juste après l'import** : elle n'affiche plus « en attente de qBittorrent » une fois les épisodes
  arrivés.

### Pour l'administrateur

- **Liste rouge (`/accounts/liste-rouge`)**, nouvelle tâche `red_list` (24 h, 34 tâches) : rouge = aucune activité
  (arrivée, dernière diffusion ou dernier épisode, lecture d'un compte ou de Playback Reporting) depuis 60 j ; un titre
  présent des deux côtés n'est rouge que si toutes ses copies le sont ; sans identifiant ou absent de Jellyfin, jamais
  rouge. Sections « Bientôt rouges », « Exemptés », « Non évalués ». Page sans JavaScript, formulaires en POST, liens
  depuis `/accounts` et la section catalogue de `/status.html` ; tuile prévue sur le tableau privé de Homarr
  ([homarr.md](docs/runbooks/homarr.md)).
- **« Nettoyer »** (50 titres par clic, les plus gros d'abord, ou un titre seul) : confirmation de la liste exacte,
  un seul travail à la fois, nouveau calcul complet, revérification de chaque titre (exemption, lecture en cours, fiche
  inchangée), fiche sauvegardée, `DELETE` jamais rejoué, corbeille de l'Arr vidée sous gardes, torrents, montage
  seedbox, média Seerr, alerte récapitulative. **Simulation tant que `[tasks.red_list] clean_dry_run = true`, « Nettoyer
  ce titre » compris.** Travail coupé par un redémarrage marqué « interrompu » ; étapes en échec rejouées par la tâche
  ([AUTOMATION.md](docs/AUTOMATION.md), [seedbox-et-rclone.md § 7](docs/runbooks/seedbox-et-rclone.md#7-supprimer-faire-le-ménage-remplacer)).
- **`torrent_import` : numéro absolu écrit SxxEnnn** dans un pack d'animé de saison, rapproché par son numéro absolu
  seulement si chaque numéro désigne un seul épisode de la saison visée (sinon refusé, comme avant) ; deux fichiers ne
  visent jamais le même épisode (c7c3401, c74f8c7).
- **Imports partiels signalés** : nouvelle issue « partiel » quand des vidéos utiles restent de côté (échantillons,
  bonus et épisodes déjà présents exclus), liste gardée dans l'état, une alerte admin par torrent, réexamen borné toutes
  les 6 h ; toujours `ManualImport` en copie, par chemin exact, sans remplacement (37e5c2e, c74f8c7).
- **Liste rouge en service (22/10)** : `clean_dry_run = false` après revue de deux simulations ; « Nettoyer » supprime
  désormais pour de vrai.
- **Liste rouge plus sûre (22/10)** : bouton « Simuler d'abord » en suppression réelle ; simulation juste pour les
  films du VPS liés à leur torrent (ils étaient comptés « partagés ») ; Jellyseerr libéré seulement une fois les dossiers
  partis ; torrents retenus par la corbeille du VPS revus à sa reprise ; titre écarté si ses copies ont changé depuis la
  confirmation ; reprise d'une corbeille seedbox seulement si l'Arr déclare la même corbeille.

## 1.28.0 — 15/10/2026 — Mails aux couleurs de Groscailloux TV

Tous les mails qu'un membre peut recevoir changent d'habit : sombres, au logo de Groscailloux TV, un seul gros bouton,
en vrai français. Nouveau : le mail « ta demande est disponible » avec l'affiche du film ou de la série, qui remplace
celui de Jellyseerr depuis le 15/10 (Jellyseerr n'envoie plus aucun mail). L'admin peut recevoir un exemple de chaque
mail dans sa propre boîte avant de valider (8550908).

### Pour les membres

- **Des mails à l'image du service.** Bienvenue, compte activé, abonnement, messages et annonces d'« Aide et
  annonces » : même présentation sombre, logo Groscailloux TV, un seul bouton bien visible, accents et espaces à la
  française, et une version texte complète pour les messageries qui n'affichent pas les images. Les objets perdent le
  préfixe « Groscailloux : » (le nom de l'expéditeur suffit) et vont droit au but : « Ton essai se termine demain »,
  « Ton accès est en pause », « Ton essai est terminé » après un essai (au lieu de « ton abonnement est arrivé à
  échéance »).
- **« Ton compte est actif » mène directement à Groscailloux TV** quand le lien de bienvenue a servi dans les 7 derniers
  jours (mot de passe choisi), sans nouveau lien inutile ; sinon, le bouton propose de choisir le mot de passe, ou de se
  connecter avec son mot de passe habituel.
- **« Ta demande est disponible », avec l'affiche, depuis le 15/10.** Quand un film ou une saison demandés arrivent,
  un mail en français avec l'image du titre, l'affiche, l'année, la durée, la note, la langue (VF · VO, VOSTFR) et un
  bouton « Regarder » qui ouvre la fiche dans Jellyfin. Une saison arrivée est annoncée sans attendre les autres
  saisons de la même demande ; plusieurs titres arrivés ensemble donnent un seul mail. Il remplace celui de Jellyseerr
  (moitié en anglais, logo cassé dans Gmail, bouton vers Jellyseerr) : Jellyseerr n'envoie plus aucun mail.
- **Liens cliquables dans les annonces et les messages de l'admin.** Une adresse écrite dans une annonce ou un message
  d'« Aide et annonces » devient un lien dans le mail, et une longue adresse passe à la ligne au lieu d'élargir le mail
  sur un téléphone.
- **Guide : « Nouvelle adresse ».** En tête du guide, un encart résume le changement : rien à changer, tout se fait
  dans Jellyfin (regarder, et demander un film ou une série dans l'onglet *Découvrir*, comme le dit aussi l'accroche du
  guide), et nos mails arrivent d'une adresse à ajouter aux contacts. La partie « Tu utilisais déjà l'ancienne adresse ? » explique comment passer une
  appli sur la nouvelle adresse, se reconnecter et retrouver son mot de passe.
- **Guide : plus de mail « Reset password » de Jellyseerr à ignorer.** Jellyseerr n'envoie plus aucun mail, la
  consigne de « Ton mail de bienvenue » disparaît.
- **Le quota de demandes se voit avant, pendant et après.** Dans la recherche, une ligne sous « Découvrir sur Seerr »
  dit combien de films et de saisons il reste à demander sur 7 jours glissants, et quand une place se libère ; après une
  demande, un court message redonne le compte ; une demande refusée pour quota ouvre de nouveau la fenêtre « Limite de
  demandes atteinte », avec la raison en français (depuis Jellyfin Enhanced 12.10, elle ne s'ouvrait plus : le refus
  arrivait en anglais, sans explication). Une série que TMDB ne relie à aucune fiche TVDB se demande de nouveau avec la
  fiche TVDB choisie à la main (le choix se perdait en route) (7d2ebdd).
- **Un lien vers Groscailloux TV s'affiche en aperçu** (17/10). Collé dans Discord, WhatsApp, iMessage, Signal ou
  Telegram, le lien montre le nom, une phrase de présentation et une grande image au logo, au lieu d'un lien nu. Rien
  ne change en ouvrant le lien : seuls les robots d'aperçu de ces messageries reçoivent la carte
  ([npm.md § 2](docs/runbooks/npm.md#aperçu-des-liens-open-graph-1710)) (e65502a).

### Pour l'administrateur

- **Mon compte enveloppe toujours les demandes de Jellyfin Enhanced, quota compris** : tous les arguments passent
  (JE 12.10 a ajouté en 6e le TVDB saisi à la main) ; le 403 générique de JE 12.10 est réécrit en refus « Quota
  exceeded » français seulement quand le quota l'explique (côté épuisé, ou plus de saisons demandées qu'il n'en reste ;
  série entière : côté épuisé seulement), tout autre 403 reste celui de JE ; quota relu sans le cache de 30 min de JE,
  gardé 60 s ; le message d'après demande ne capte aucun toucher, se cache sous une fenêtre ou un dialogue et se pose
  au-dessus de celui de JE. Rien pour un compte sans limite. Banc `tools/bench/scenarios/quota.js` (proxy Seerr simulé
  dans le navigateur, aucune vraie demande ; bureau, téléphone et télé ; contre-épreuve) (7d2ebdd)
  ([jellyfin-interface.md § 10](docs/runbooks/jellyfin-interface.md#10-vue-densemble-des-membres)).
- **Gabarit commun `mail_layout`** : logo, fond, affiche et puces, bouton, cartes d'infos, encadrés, carte de message,
  pied ; tout texte échappé, typographie française dans le HTML seulement, HTML sous 40 Ko (Gmail coupe vers 102 Ko),
  mode sombre déclaré, aucun `token=`. Chaque mail aux membres est une fonction pure (`welcome`, `subscriptions`,
  `chat`, `available_mail`). Images **jointes au mail** (`multipart/related`, `cid:`) : logo et bandeau embarqués,
  affiches et fonds lus dans Jellyfin (`format=Jpg`) ou chez TMDB en secours, PNG ou JPEG vérifiés à leurs premiers
  octets, 400 Ko et 10 s au plus chacune. Un objet long est plié en plusieurs mots RFC 2047. Mails d'admin inchangés
  (texte). Prix des mails d'abonnement tiré de `[subscriptions] price_text` (il était écrit en dur).
- **Nouvelle tâche `media_mail`** (5 min, 33 tâches) : mails « disponible » à partir de toutes les demandes Jellyseerr
  (film disponible, saison terminée **et** disponible : Jellyseerr termine aussi une saison supprimée ; saison 0 et 4K
  ignorées), premier passage = référence sans mail, jamais deux fois
  ni l'arriéré (plus de 48 h : jamais envoyé), titres d'un même membre regroupés (10 min sans nouvelle arrivée, 1 h au
  plus), titre relu dans Jellyfin **comme le membre**, choix de notifications de Jellyseerr respecté, envoi noté avant
  `curl` (au plus un mail). Installée d'abord en observation (`send_to_members = false` : journal, état `logged`, copie
  facultative à `MEDIA_MAIL_PREVIEW_TO`) ; `MEDIA_MAIL_OPTOUT` (adresse, pseudo ou id) pour un membre qui répond « stop ».
  Mail abandonné (échecs répétés, attente de plus de 48 h) : alerte admin gardée dans l'état jusqu'à ce qu'elle parte,
  une par passage ; écart de l'état au-delà duquel rien n'est envoyé, ni ce qui attendait avant lui, réglable
  (`max_gap_mins`, 120 ; refusé au démarrage s'il ne dépasse pas `interval_secs` + 10 min).
- **Envoi des mails « disponible » allumé le 15/10, agent e-mail de Seerr coupé** (5d62957) : bloc `[tasks.media_mail]`
  décommenté avec `send_to_members = true` (même défaut dans le code ; le test du bloc commenté devient
  `media_mail_block_is_live_and_complete` : bloc actif, complet, sans copie commentée), homelabd et homelabctl installés
  ensemble. **Seerr n'envoie plus aucun mail** : Seerr 3.5 n'a pas de types de notification pour l'e-mail, d'où la
  coupure de tout l'agent ; son « Mot de passe oublié ? » disparaît (sans usage pour des comptes Jellyfin) et « Add
  User » demande un mot de passe quelconque, repris par `user_poller`. Premier passage avec envoi : il note `logged`
  ce qui attendait encore **et** ce qui est apparu depuis le dernier passage d'observation, quand Seerr l'a annoncé
  (le binaire installé à 11:31 aurait envoyé aussi une arrivée de ces 5 dernières minutes, en double avec Seerr) ;
  seul ce qui arrive ensuite part. Seerr n'annonce une série qu'une fois **toutes** les saisons demandées arrivées
  (`notifyAvailableSeries`) : une saison arrivée d'une demande dont d'autres saisons manquent reste à homelabd (sinon
  personne ne l'annonçait). Date de la bascule notée dans l'état (`media_mail_last_pass.send_since`) : une clé
  disponible avant elle mais absente de sa lecture paginée (demande supprimée pendant la lecture) suit la même règle
  au passage suivant, au lieu de partir en double. `send_to_members = true` ne s'écrit que dans la minute de la
  bascule, jamais à l'avance (homelabd ne lit `homelab.toml` qu'au démarrage : un redémarrage du binaire d'avant
  enverrait pendant que Seerr envoie encore). `homelabctl run media_mail --dry-run` prend la même décision que le
  passage réel. Retour arrière (`send_to_members = false`, restart, agent de Seerr réactivé) :
  [comptes-et-abonnements.md § 2](docs/runbooks/comptes-et-abonnements.md#2-onboarding-et-mails),
  [comptes-abonnements-mails.md](docs/automation/comptes-abonnements-mails.md#media_mail--5-min-2026-10-15).
- **`homelabctl mail-test` montre chaque mail** : `--all` envoie un exemple de chacun des 17 mails aux membres (compte
  fictif « Exemple », objet « [Exemple] … », 2 s d'écart, vrais titres arrivés pour « disponible »), `--kind <type>` un
  seul ; `--dump <dossier>` les écrit en `.html` (images à côté), `.eml` et `.txt` sans homelabd ni envoi. Sans option,
  toujours le mail de bienvenue d'exemple. Aucun compte ni abonnement touché.
- **En-têtes plus sûrs pour Gmail** : `Reply-To` posé seulement s'il est sur le domaine d'envoi (une adresse de
  réponse sur une messagerie gratuite à côté d'un expéditeur sur le domaine du service est un signal d'hameçonnage :
  les mails de bienvenue n'en portent plus tant que `GUIDE_CONTACT_EMAIL` n'est pas sur ce domaine, une réponse arrive
  à l'adresse d'envoi) ; nom du destinataire mis entre guillemets ou encodé selon la norme (une virgule dans un nom
  Jellyseerr cassait l'en-tête `To`) ; adresse refusée si ce n'est pas une adresse simple ; en-têtes longs pliés.
- **Aperçu d'une annonce par mail** : `homelabctl mail-test <adresse> --announce <fichier> [--author <modérateur>]`
  envoie à cette adresse seulement le mail exact que `chat announce <fichier> --mail` enverrait aux membres (même
  auteur, même texte nettoyé, même fonction, même enveloppe), objet « [Aperçu] … ». Rien dans le tchat, sur Discord ni
  aux membres : route à part `POST /admin/mail-test/announce`, sans accès au tchat (un homelabd plus ancien répond 404
  au lieu d'envoyer le mail de bienvenue d'exemple). `--dump <dossier>` le rend sans homelabd.
  [tchat.md](docs/runbooks/tchat.md).
- **Jellyseerr envoie aussi par Proton** (15/10), depuis la même adresse `contact@` que homelabd : le 14/10, seuls les
  mails de homelabd avaient quitté Gmail. Le jeton SMTP est donc aussi enregistré dans Jellyseerr (Paramètres →
  Notifications → E-mail) : une rotation met à jour les deux ([SECRETS.md](docs/SECRETS.md)).
- **Guide** : marqueur `{{MAIL_FROM}}` (expéditeur `SMTP_FROM`, échappé ; formule neutre sans SMTP), encart
  `#nouvelle-adresse` et partie `#changer-adresse`, sans adresse en dur (dépôt public).
- **Échec d'envoi mieux expliqué** : quand le serveur SMTP refuse avant le message (authentification, destinataire),
  un mail à images (plus de 64 Kio) ne remonte plus une « Broken pipe » trompeuse mais le vrai motif de `curl` ; l'alerte
  d'un mail de bienvenue en échec distingue « n'est pas parti » de « est peut-être parti » (réponse du serveur perdue :
  demander au membre avant de transmettre le lien).
- **`.env` mal formé : l'erreur ne cite plus la ligne.** L'erreur de `dotenvy` contenait la ligne fautive entière (donc
  un secret), qui finissait dans le journal de homelabd ou à l'écran de homelabctl ; désormais « ligne N de .env mal
  formée (contenu masqué) ».

---

## 1.27.0 — 14/10/2026 — Nouveau domaine principal

Le serveur a un vrai nom de domaine, plus court, chez Cloudflare. L'ancienne adresse DuckDNS reste servie à
l'identique : rien à changer dans les applis déjà réglées, et les liens des anciens mails marchent toujours.

### Pour les membres

- **Nouvelle adresse principale** : le domaine nu ouvre Jellyfin (le plus simple à taper sur une télé) ; les demandes,
  « Mon compte », l'abonnement et le guide sont sur le même domaine. Les mails, le guide et Discord donnent désormais
  ces adresses. **L'ancienne adresse marche toujours**, pour tout.
- **Les mails arrivent de l'adresse du domaine** (`contact@`) et non plus d'une adresse Gmail : ceux du service le
  14/10, ceux de Jellyseerr (« ta demande est disponible ») le 15/10 ([1.28.0](#1280--15102026--mails-aux-couleurs-de-groscailloux-tv)).

### Pour l'administrateur

- **DNS chez Cloudflare en « DNS only »** (`A @` et `A *` vers le VPS, posés par l'API avec `CLOUDFLARE_API_TOKEN`,
  jeton limité à la zone) : jamais le proxy de Cloudflare, qui exclut la vidéo.
- **NPM : chaque hôte porte les deux noms** et un certificat Let's Encrypt HTTP-01 qui couvre les deux (`npm-2` à
  `npm-16`), émis sans couper aucun hôte ; le certificat joker DuckDNS (défi DNS, renouvellement à surveiller le 27/10)
  n'est plus utilisé. Hôte 7 (Jackett du VPS, retiré) laissé sur l'ancien nom. [npm.md § 6](docs/runbooks/npm.md#6-deux-domaines-1410).
- **homelabd** : `*_PUBLIC_URL` sur le nouveau domaine ; nouvelle variable facultative `ONBOARD_FALLBACK_URL` (l'ancien
  onboarder) dont l'hôte sert aussi les pages d'administration (`admin_auth::host_allowed_any`).
- **Seerr** (`applicationUrl`, adresse externe de Jellyfin), **Home Screen Sections** (lien vers Seerr) et **Homarr**
  (25 liens de tuiles) passés au nouveau domaine (f5f3344). `PublishedServerUrl` de Jellyfin : passage hors pic
  `jf-published-url` (15/10, 04:30). Le webhook PayPal reste sur l'ancien nom, toujours servi.
- **Homarr : sessions Jellyfin plus lisibles sans connexion.** Le widget « Nouveautés » du tableau public était relié à
  l'intégration Jellyfin, ce qui ouvrait aux anonymes la liste des sessions en cours (trouvé par le point complet du
  14/10) ; lien retiré, deux contrôles anonymes dans [homarr.md](docs/runbooks/homarr.md).
- **Mails de homelabd envoyés par Proton** depuis l'adresse `contact@` du domaine (SPF, DKIM et DMARC publiés chez
  Cloudflare) au lieu de Gmail ; port 465 en TLS direct, donc aucun changement de code ni de diun. Jellyseerr est resté
  sur Gmail jusqu'au 15/10 (1.28.0). [SECRETS.md](docs/SECRETS.md).

---

## 1.26.0 — 10/10/2026 — SyncPlay après un saut, films guettés avant leur sortie et lot 3, jour 2

Séance SyncPlay à trois dans la nuit du 09 au 10/10 : après chaque saut, tout le groupe attendait 12 s un membre sur
Jellyfin Desktop 2.0.0-dev, et un groupe créé pendant la lecture est resté bloqué 30 s au démarrage. Le contournement
SyncPlay est corrigé et ses délais choisis au banc. Un film attendu est désormais guetté avant sa date de sortie
numérique. Deuxième jour des mises à jour du lot 3, arrêté sur un contrôle.

### Pour les membres

- **SyncPlay dans Jellyfin Desktop : la lecture repart quand tout le monde a vraiment l'image.** Avec l'appli 2.x, le
  groupe repartait trop tôt après un saut puis se remettait « en chargement » 3 s plus tard, ou attendait 12 s à chaque
  saut. Si une appli ne répond pas, le groupe repart quand même au bout de 10 s (appli 2.x) ou 5 s (appli 1.x), au lieu
  de 12 s.
- **Regarder ensemble : créer ou rejoindre le groupe d'abord, puis lancer l'épisode depuis le groupe.** Un groupe créé
  pendant une lecture reste bloqué 30 s (bogue de Jellyfin 12.1) ; pour débloquer, relancer l'épisode depuis le groupe.
- **Un film demandé avant sa sortie arrive dès qu'il est en ligne.** Il est recherché toutes les 2 h à partir de
  2 jours avant sa date de sortie numérique (et jusqu'à 2 jours après), au lieu d'attendre le jour même : la version
  française arrive souvent la veille. L'onglet Demandes l'annonce : « recherché toutes les 2 h dès le … ».
- **Une VOSTFR toute fraîche ne passe plus devant la VF.** Publiée depuis moins de 3 h, elle attend qu'une version
  française sorte ; s'il n'y en a toujours pas, elle est prise ensuite.

### Pour l'administrateur

- **`gc-syncplay.js` VERSION 2** : sur le lecteur web, l'arrivée après un saut se lit sur l'élément `<video>` (saut
  fini, `readyState` ≥ 3, à la cible), plus sur sa position, que Chrome annonce dès le début du saut ; mpv garde la
  position. Attente maximale 10 s (lecteur web) et 5 s (mpv) : au banc, couper plus tôt a fait perdre son saut à un
  lecteur encore en chargement (10 min de décalage). Chaque attente maximale atteinte part au journal client du serveur
  (`upload_*.log`, « gc-syncplay v2 »). Nouveau banc `tools/bench/scenarios/syncplay.js` (deux comptes temporaires ;
  sauts normaux, seedbox à froid, premier octet lent, appli muette, démarrage) ; cause du démarrage bloqué : liste de
  lecture vide envoyée par le serveur 12.1 (runbook jellyfin-interface § 8).
- **`movie_search` : fenêtre de sortie** (`[tasks.movie_search] release_window_days = 2`, `release_retry_hours = 2`).
  La tâche pose `availabilityDelay = -2` dans le Radarr de la seedbox : Radarr tient le film pour sorti 2 jours avant
  sa date, ce qui ouvre aussi son RSS (15 min). Dans la fenêtre, « aucune release » est retenté toutes les 2 h au lieu
  de 72 h. *Spider-Man: Brand New Day* était sur C411 dès le 05/10 à 15:43 et n'a été pris que le 06/10 à 00:05 ; sur
  14 films récents, aucune release n'est arrivée plus de 23 h avant la date (3 jours le premier soir, 2 ensuite).
- **`movie_search` : VOSTFR fraîche retenue** (`vo_wait_hours = 3`, `vo_wait_bypass_score = 2500`). Le RSS de Radarr
  prend la première release acceptable et le profil ne remplace jamais un fichier (*Insidious* : VOSTFR 48 min avant
  la VFF). homelabd retient une release sans audio français de moins de 3 h (résultat `vo_wait`) ; la tâche pose le
  profil de délai par défaut de Radarr (torrents retenus 180 min sauf score ≥ 2500, protocole préféré torrent).
  Au passage, la valeur par défaut de `[tasks.original_language] series` passe à `true` comme `homelab.toml`.
- **Lot 3, jour 2** (10/10, 08:35–08:44, aucune lecture en cours) : Prowlarr 2.6.5 appliqué. Sonarr 4.0.20 remis en
  4.0.17 par son retour arrière (le contrôle d'après a refusé un simple changement d'ordre de champs du client de
  téléchargement), Radarr 6.4.4 sauté, gluetun v3.41.3 remis en place d'origine (avertissement passager de Sonarr juste
  après la recréation de qBittorrent ; la restauration notée « incomplète » ne l'était pas : rsync refuse un `chgrp`,
  données vérifiées identiques), Homarr pas lancé. Téléchargements et port transféré vérifiés sains.

## 1.25.0 — 09/10/2026 — Téléchargements expliqués et lot 3, jour 1

Une version corrigée (REPACK) de Carrie S01E02 restait affichée « Avertissement » dans l'onglet Téléchargements, sans
dire pourquoi. Chaque avertissement est maintenant expliqué, et la cause de ce blocage-là est corrigée dans les Arrs.
Premier jour des mises à jour du lot 3, le matin.

### Pour les membres

- **Téléchargements : chaque avertissement est expliqué.** Une carte bloquée ne montre plus seulement
  « Avertissement » : un libellé court (« Déjà disponible », « Titre attendu », « Sans source », « Non reconnu »,
  « Pack incomplet », « Échec »…) et une phrase disent ce qui se passe, et si quelque chose se fera tout seul. Une
  action automatique n'est annoncée que si la tâche qui la fait tourne vraiment. Rien ne s'affiche quand l'appariement
  entre la carte et la file n'est pas sûr.

### Pour l'administrateur

- **L'admin voit en plus le message d'origine de Sonarr ou Radarr**, nettoyé (ni chemin, ni adresse, ni clé), sous
  l'explication. Route `GET /compte/api/downloads` (session Jellyfin ; message d'origine réservé aux administrateurs ;
  cache partagé de 30 s, échec gardé 60 s) ; règles dans `homelab_core::queue_explain`. Aucune nouvelle clé de
  configuration.
- **Les versions corrigées ne restent plus bloquées** : le format « WEB-DL » des 4 Arrs donnait +400 au fichier déjà
  rangé, dont le nom renommé contient « WEBDL-1080p », mais pas à une REPACK nommée « WEB » (« Not a Custom Format
  upgrade »). Il ne compte plus ce nom renommé. Aucun téléchargement déclenché : tous les profils concernés ont les mises
  à niveau coupées. Carrie S01E02 a été remplacé par sa REPACK.
- **Lot 3, jour 1** (09/10, 08:35–08:44, aucune lecture en cours) :
  - NPM 2.16.0 (OpenResty 1.31.1.1) : CVE-2026-42945, -8711, -9256 et la RCE de l'interface corrigées ; le jeton
    DuckDNS n'est plus gardé sur le disque ;
  - Seerr 3.5.0 : CVE-2026-73291 corrigée, fin du piège des bibliothèques désactivées, notifications en français ;
  - outils d'administration : Grafana 13.0.10, Portainer 2.39.8, Glances 4.5.7 (tag sans `v`), File Browser v2.63.23
    (dernière version publiée, projet archivé) ; scripts d'application et de retour arrière avec sauvegarde des données
    et contrôles fonctionnels.

### Outillage

- `tools/tests/compte-telechargements/match.test.js` (appariement d'une carte de Téléchargements avec la file, Node,
  sans navigateur ni réseau).
- Commits : [`2f999d3`][2f999d3] (lot 3, jour 1), [`82cf246`][82cf246] [`e90dc54`][e90dc54] [`b133708`][b133708]
  (Téléchargements expliqués) ; fusion [`00954b4`][00954b4].

---

## Numérotation pour la suite

- **1.x.y** (y > 0) : corrections ;
- **1.x.0** : nouveautés ;
- **2.0.0** : changement majeur d'architecture.

Chaque version ajoute sa section en haut de ce fichier, avec ses commits. La version du programme
(`workspace.package.version` de `Cargo.toml`, affichée par `homelabctl --version`) est celle de la section du haut ;
un tag annoté `vX.Y.Z` marque le dernier commit de chaque version.

<!-- liens des commits -->

[2f999d3]: https://github.com/HaradasCYB/groscailloux-homelab/commit/2f999d3
[82cf246]: https://github.com/HaradasCYB/groscailloux-homelab/commit/82cf246
[e90dc54]: https://github.com/HaradasCYB/groscailloux-homelab/commit/e90dc54
[b133708]: https://github.com/HaradasCYB/groscailloux-homelab/commit/b133708
[00954b4]: https://github.com/HaradasCYB/groscailloux-homelab/commit/00954b4
