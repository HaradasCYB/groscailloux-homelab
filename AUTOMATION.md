# Automatisation : homelabd et homelabctl

`homelabd` (systemd `homelabd.service`, user `deploy`) exécute chaque tâche dans sa propre
boucle tokio, à l'intervalle de `homelab.toml`, avec un jitter initial de 0–30 s et un
timeout de 10 min par passage. Un mutex sérialise toute mutation de qBittorrent, un autre les
onboardings. `HOMELABD_DRY_RUN=1` (ou `--dry-run`) : aucune écriture, les tâches loggent ce
qu'elles feraient. Les logs vont dans le journal : `journalctl -u homelabd -f`.

`homelabctl run <tâche> [--dry-run]` exécute la même implémentation une fois ;
`homelabctl status` lit `state/homelabd.json` (dernier passage, erreurs, items suivis).

Configuration : `homelab.toml` (toute clé inconnue est refusée ; `paths.downloads` doit
exister sinon le daemon refuse de démarrer — plus jamais un watcher mort en silence).
Secrets : `.env` via `EnvironmentFile`.

## Tâches planifiées

### playback_limit — 20 s
Lectures simultanées par compte (`accounts.max_playbacks_per_user`, 2 ; comptes `accounts.protected`
exemptés ; 0 = illimité). `GET /Sessions` : pour chaque compte au-delà du maximum, les lectures les plus
récentes (première apparition la plus tardive, à égalité la moins avancée) qui durent depuis `grace_secs`
(30 s, le temps de passer d'un appareil à l'autre) reçoivent un message à l'écran (« Ce compte regarde déjà
sur 2 écrans… ») puis un ordre d'arrêt (`Sessions/{id}/Playing/Stop`) ; 3 tentatives au plus par lecture,
`max_actions_per_run` arrêts par passage. Remplace `MaxActiveSessions`, laissé à 0 (voir CLAUDE.md).

### stack_health — 5 min (première passe ~30 s après le démarrage de homelabd)
Auto-réparation du stack. `docker compose config --services` donne les services attendus (moins
`ignore`), `docker compose ps -a --format json` leur état.
- absent / `exited` / `created` → `docker compose up -d <svc>` (respecte `depends_on`) ;
- `unhealthy` depuis ≥ `unhealthy_grace_secs` (2 min) → `docker compose restart <svc>` ;
- sonde applicative (`[[tasks.stack_health.probes]]`) en échec → restart, seulement si les
  services de `requires_healthy` sont healthy (sinon « waiting ») ;
- jamais deux restarts du même service en moins de `restart_cooldown_secs` (10 min).
Sonde par défaut : Guacamole, `POST /guacamole/api/tokens` avec de faux identifiants → attendu
`403` + `INVALID_CREDENTIALS` ; un `500` signifie que l'extension MySQL est morte (guacdb pas
prêt au démarrage : l'image ne l'attend jamais). Cette sonde compte comme un échec de login
côté Guacamole : garder l'intervalle ≥ 5 min (extension `ban` : 5 échecs / 300 s). Une réponse
`429` (throttling) est considérée non concluante : ni échec ni redémarrage.
Pour arrêter un service volontairement : l'ajouter à `ignore` avant, sinon il sera relancé.
Résumé : `expected=21 running=21 healthy=16 started=[] restarted=[] waiting=[] failed=[]`.

### id_match_import — 5 min
Radarr/Sonarr bloquent l'import quand le nom de la release ne correspond pas au titre de la fiche,
même si l'indexer a fourni l'id au grab : message « Found matching movie/series via grab history,
but release was matched to … by ID ». Fréquent avec les releases C411 titrées en français.
Pour chaque item de queue (tous les Arrs, VPS et seedbox) `status=completed`,
`trackedDownloadState=importBlocked` avec ce message : `GET /api/v3/manualimport?downloadId=…&movieId|seriesId=…`,
puis `ManualImport` (importMode auto → hardlink) des seuls fichiers **sans rejet** ; séries :
uniquement les fichiers dont l'épisode est identifié, les autres restent en manuel (avertissement
dans les logs). Au plus 10 téléchargements par passage.

### torrent_import — 10 min
Torrents ajoutés **à la main** dans qBittorrent (VPS, et seedbox si `[seedbox] qbit_url` est défini),
pour qu'ils arrivent dans Jellyfin sans passer par Jellyseerr. Pour chaque torrent terminé pas encore
jugé (`state.torrent_import`, clé `vps:<hash>` / `seedbox:<hash>`), dans l'ordre :
1. `GET /api/v3/history?downloadId=<HASH en majuscules>` sur le Radarr et le Sonarr du côté : un
   enregistrement ⇒ l'Arr gère ce téléchargement (`arr_managed`) ;
2. `torrents/files` : aucune vidéo hors `sample` ⇒ `no_video` ; VPS : toutes les vidéos ont déjà
   plus d'un lien (importées) ⇒ `already_linked` ;
3. série si le nom ou un fichier porte un marqueur d'épisode/saison (`S01E02`, `S01`, `Saison`…),
   sinon film ; `parse` du nom (puis du premier fichier vidéo) → `lookup` → choix par
   `matching` : titre identique après normalisation (titre original et alternatifs Radarr compris,
   « Fusion (2003) » → *The Core*), année, saison ; à défaut premier résultat s'il commence par le
   même mot (journalisé « approximate match ») ; rien ⇒ `no_match` ;
4. la fiche a des fichiers dans les Arrs de **l'autre machine** ⇒ `dup_other_side` (pas de doublon Jellyfin) ;
5. fiche absente ⇒ ajout **non surveillé**, sans recherche (racine et profil du côté : `auto_import`
   pour le VPS, `[seedbox] radarr_root|sonarr_root|quality_profile_id`) ; série : attente de ses
   épisodes (`series_ready_secs`) ;
6. fiche **avec fichiers** : `GET /api/v3/manualimport?folder=<content_path>&movieId|seriesId=…`
   (**sans** `downloadId` : un téléchargement non suivi renverrait une liste vide), fichiers sans rejet
   (`select_files`) ; fiche **vide** (son dossier n'existe pas : avec l'id, Radarr et Sonarr répondent
   500) : `GET manualimport?folder=…` sans id, rejets d'identification ignorés (`Unknown Movie/Series`,
   `matched … by ID`), film = la fiche choisie, épisodes = `parse` du nom de fichier → saison/numéros
   (ou numéros absolus) → ids de `GET /api/v3/episode?seriesId=` (`map_episodes`) ; puis
   `ManualImport` en **`importMode: copy`** (= hardlink). Jamais `auto` : pour un téléchargement non
   suivi, `auto` = déplacement, le torrent perd ses fichiers.
Déjà importé ⇒ rejet de l'Arr ⇒ `nothing_importable`. Erreur (Arr injoignable…) ⇒ `retry`, `error`
après `max_attempts`. Au plus `max_per_run` torrents coûteux par passage, les plus récents d'abord.
Aucune modification des torrents. Jellyfin : LibraryMonitor (VPS) ou `seedbox_refresh` (seedbox).
Le watcher `auto_import` ignore désormais les vidéos qui appartiennent à un torrent qBittorrent.
Résumé : `files=3 arr_managed=67 already_linked=8 imported=2 no_match=1 pending=0`.

### unknown_series_grab — 3 h
Sonarr rejette « Unknown Series » les releases au titre traduit (« New York Police Judiciaire » pour
*Law & Order*) : jamais prises en automatique, même sur C411. Pour chaque Sonarr (VPS, seedbox) :
`GET wanted/missing` → saisons avec épisodes diffusés manquants, hors file d'attente, pas cherchées
récemment (`state.unknown_series` : sans candidat → 72 h, prise → 7 j) et **absentes de l'autre
machine** ; ordre global : **séries ajoutées le plus récemment d'abord** (une demande passe devant
l'arriéré), puis diffusion la plus récente ; **8 recherches par passage**, une seule saison d'anime
(recherche épisode par épisode, limite d'API C411), arrêt après 8 min (le planificateur coupe à 10) → `GET release?seriesId&seasonNumber` → releases de `indexer` (C411)
dont le **seul** rejet est « Unknown Series » → garde-fous : titre parsé **identique** (normalisé)
au titre FR ou original (Jellyseerr `tv/{tmdbId}?language=fr`), au titre Sonarr ou à un titre
alternatif (une série voisine est refusée) ; bonne saison ; épisodes tous manquants ; qualité
autorisée par le profil et ≤ 1080p ; au moins 1 seeder ; marqueur FR (VFF > MULTi > FRENCH >
VOSTFR). Pack si la moitié de la saison manque, sinon épisodes ; tri langue, résolution, H.264,
seeders → `POST /api/v3/release` en **grab forcé** (`shouldOverride`, `seriesId`, `episodeIds`).
L'import est débloqué ensuite par `id_match_import`. C411 ne renvoie pas d'id TVDB : le titre est le
seul garde-fou possible. Résumé : `grabbed=1 none=2 pending=20`.

### seedbox_refresh — 5 min (si `[seedbox] enabled`)
Lit l'historique `downloadFolderImported` (eventType 3) des Radarr/Sonarr de la seedbox depuis le
dernier id traité (`state.seedbox_history` ; la première passe initialise le curseur sans rejouer).
Pour chaque nouvel import : chemin `media_root/…` → dossier relatif, `POST <rclone_rc>/vfs/refresh`
sur ce dossier et ses parents, puis `POST /Library/Media/Updated` à Jellyfin avec le chemin
`jellyfin_root/…` (Jellyfin ne scanne que ces dossiers). Montage absent → avertissement, le
curseur n'avance pas.

### Arrs multi-instances
Avec la seedbox activée, `stuck_handler` traite aussi les queues des Arrs seedbox,
`tba_bypass` scanne aussi `seedbox.sonarr_downloads` avec le Sonarr seedbox, et `monitor_sync`
**route chaque demande Jellyseerr selon `media.serviceId`** (`jellyseerr_vps_sonarr_id` → Sonarr
VPS, `jellyseerr_sonarr_id` → Sonarr seedbox) : les ids de séries diffèrent entre instances.

### tracker_ratio — 30 min
Share limits par tracker via `POST /api/v2/torrents/setShareLimits`.
`unlimited` (c411) → ratio -1 / temps -1 ; `secondary` (yggleak, u2p, ygg.gratis) et
`public` (liste de trackers publics) → 2.0 / 14 j ; défaut → 1.0 / 7 j.
Ne touche un torrent que si sa limite actuelle diffère (ratio comparé à 2 décimales).

### stuck_handler — 5 min
`GET /api/v3/queue?pageSize=200` sur Sonarr et Radarr. Un item dont `errorMessage` matche
`stalled|metadata|no connections` est suivi (`state.stuck`). Passé `stall_secs` (8 h) :
`DELETE /api/v3/queue/{id}?removeFromClient=true&blocklist=true` → l'Arr relance une
recherche. Max 5 par passage. Les items disparus de la queue sont oubliés.

### disk_pressure — 15 min
`statvfs` sur `paths.base`, arrondi comme `df`. < 95 % : rien. 95–98 % : supprime avec
fichiers (`POST /api/v2/torrents/delete`, `deleteFiles=true`) jusqu'à 5 torrents en état
`stoppedUP`, les plus anciens d'abord — les hardlinks de `library/media` survivent.
≥ 98 % : log d'erreur, aucune action automatique.

### trending — 6 h

Rangée « Tendances cette semaine » de l'accueil Jellyfin. Classement lu dans Playback Reporting
(`POST user_usage_stats/submit_custom_query`, SQL en lecture) : épisodes regroupés par série, spectateurs
distincts puis heures ; un spectateur compte à partir de `min_minutes` sur le titre. Semaine trop calme :
complétée par `fallback_days`. Tient à jour la collection `collection_name` (création, puis ajouts et
retraits ciblés). Collection et non playlist : une playlist qui reçoit une série y déplie ses épisodes.

### deletion_cleanup — 5 min

Suite d'une suppression faite dans Jellyfin (bouton « Supprimer » ; médias montés en écriture pour ça).
Un fichier connu d'un Arr est tenu pour supprimé seulement si **trois signaux** concordent : absent du
disque (`NotFound`, pas une erreur d'E/S), absent de Jellyfin (`GET /Items?Fields=Path`), et déjà absent
au passage précédent (`confirm_after_secs`). Garde-fous : imports de moins de `min_file_age_mins` ignorés ;
côté seedbox, montage vérifié et cache rclone rafraîchi (`vfs/refresh`) avant de conclure ; plus de
`abort_if_missing_titles_over` titres manquants d'un coup ⇒ rien n'est fait (disque ou montage) ;
`max_titles_per_run` titres par passage ; une machine injoignable est sautée, l'autre continue.

- **Film** : `DELETE movie/{id}?deleteFiles=true` ; média Jellyseerr supprimé (le titre redevient
  demandable) si aucune autre machine n'a le film.
- **Série entière** : idem côté Sonarr. **Saison entière** : épisodes non surveillés, saison non
  surveillée, notée dans `deletions.seasons` : `monitor_sync` ne la re-surveille plus, sauf demande
  Jellyseerr créée après la suppression. **Épisodes isolés** : non surveillés.
- **Torrents** : sources = historique du titre (`history/movie`, `history/series` filtré sur les épisodes
  supprimés, jamais un évènement d'un autre titre) + nom de release (`originalFilePath`, `sceneName`).
  Gardé s'il sert encore (lien physique d'un de ses fichiers sur le VPS, ou autre import du même
  `downloadId` qui a encore son fichier). C411 ou tracker inconnu : retiré avec ses fichiers seulement à
  ratio `c411_min_ratio` ou `c411_min_seed_days` de seed (en attente dans `deletions.pending_torrents`,
  revu à chaque passage) ; autres trackers : tout de suite.

Testé le 2026-09-14 de bout en bout sur un film jetable du VPS (import Radarr, scan, `DELETE /Items`,
fiche Radarr supprimée, aucun torrent touché).

### monitor_sync — note (2026-09-13)
Une saison demandée n'est suivie que si elle n'a pas déjà des fichiers **sur l'autre machine**
(rapprochement par tvdbId) : sans ça, une nouvelle demande routée vers la seedbox y faisait suivre
toutes les saisons historiques, y compris celles présentes sur le VPS (doublons).

### tba_bypass — 5 min
`GET /api/v3/manualimport?folder=/downloads&filterExistingFiles=true` (Sonarr, timeout
5 min). Candidat = exactement un rejet, commençant par « Episode has a TBA title », série et
épisode identifiés, `episodeFileId == 0`. Alors `POST /api/v3/command` `ManualImport`
(importMode auto). Contourne le refus d'import des épisodes anime fraîchement diffusés dont
TVDB n'a pas encore le titre.

### monitor_sync — 10 min
Jellyseerr ajoute les séries sans `addOptions.monitor` ⇒ Sonarr monitore tout. La tâche lit
`GET /api/v1/request?filter=all` (paginé), construit `sonarrSeriesId → saisons demandées`
(statuts DECLINED/FAILED exclus), puis pour chaque série Sonarr concernée :
`monitored = demandée OR a des fichiers`, S00 jamais modifiée, `PUT /api/v3/series/{id}`.
Les séries inconnues de Jellyseerr ne sont pas touchées.

### user_poller — 60 s
`GET /api/v1/user?take=200` : users `userType == 2` (locaux), non admin, email valide, créés
depuis < 10 min, email pas encore traité (`state.onboarded`). Si un user Jellyfin du même nom
existe → marqué et ignoré. Sinon `DELETE /api/v1/user/{id}` puis onboarding unifié.

### cleanup — 24 h
Fichiers de `jellyfin/cache/transcodes` > 1 j ; dossiers vides de `library/downloads`
(profondeur ≤ 3) > 2 j ; fichiers des `.recycle` Sonarr/Radarr > 14 j ; logs Jellyfin > 7 j.

## Watcher auto_import (continu)

inotify non récursif sur `paths.downloads` (create, close_write, moved_to), chaque nom traité
au plus une fois par 2 min **et un seul traitement à la fois par nom** (verrou « en cours »).
Vidéo (`mkv/mp4/avi`, hors `.!qB`/`.part`) : attente 5 s puis **attente d'une taille stable**
(3 relevés identiques à 5 s d'écart, 6 h max) — un envoi Filebrowser/scp écrit par morceaux, un
scan trop tôt importerait (voire déplacerait) un fichier incomplet. Fichier d'un torrent qBittorrent
⇒ laissé à `torrent_import`. Classification : `S01E02`, `1x02`, `S01`, `Season`, `Saison`,
`Complete`, `Intégrale` ⇒ série ; sinon `GET /api/v3/parse` Sonarr : série suivie + épisodes
identifiés (ex. numérotation absolue « Bleach - 48 ») ⇒ série ; sinon film.
Série : `GET /api/v3/parse` → `series/lookup` → `GET /series?tvdbId` → `POST /series` si
absent (profil `QUALITY_PROFILE_ID`, monitor all, pas de recherche) → `DownloadedEpisodesScan`.
Film : idem côté Radarr (`movie/lookup`, `tmdbId`, `DownloadedMoviesScan`).
Archive `zip/rar` : attente de taille stable (3 relevés à 5 s, max 30 min), extraction
(`unzip` / `unrar` / `7z`), suppression de l'archive, scan du dossier extrait d'après sa
première vidéo.

## Onboarding

`homelabctl onboard <user> <email> [--password]`, `POST /onboard` (UI sur `:8766`, en-tête
`X-Onboard-Token` = `HOMELABD_ONBOARD_TOKEN`, la page lit `?token=` ; 1 requête / 30 s / IP)
ou le poller. Séquence sous mutex : pré-contrôles Jellyfin + Jellyseerr → nom Jellyfin libre
(insensible à la casse) → email Jellyseerr libre → `POST /Users/New` → policy non-admin
limitée aux bibliothèques `JELLYFIN_LIB_FILMS`/`SERIES` → `POST /api/v1/user/import-from-jellyfin`
→ `POST /api/v1/user/{id}/settings/main` (email) → mail de bienvenue via `curl smtps://`.
La policy donne accès à `JELLYFIN_LIB_FILMS`, `JELLYFIN_LIB_SERIES` et aux ids éventuels de
`JELLYFIN_LIB_EXTRA` (aujourd'hui la bibliothèque « Collections » : sagas et rangées de l'accueil).
Le mot de passe (16 caractères alphanumériques) n'apparaît jamais dans les logs.
La policy limite aussi les lectures simultanées (`accounts.max_streams_per_user`). Si
`accounts.new_accounts_premium = false` (réglage actuel), le compte est ensuite **suspendu** (après
l'import Jellyseerr) et le mail de bienvenue prévient que l'accès sera activé par l'administrateur.
Avant la suspension, `accounts.jellyseerr_auto_approve` ajoute la validation automatique des demandes
(bit 128, `accounts::request_permissions`) aux droits Jellyseerr de l'import ; la suspension les
sauvegarde et l'activation les rend (en ajoutant le bit aux comptes suspendus avant ce réglage).
Le mail de bienvenue renvoie vers le guide (`ONBOARD_PUBLIC_URL` + `/guide`, pas de lien si absent) et
annonce la validation automatique.

## Guide des nouveaux membres

`GET /guide` (public, sans jeton, sur l'onboarder) : `crates/homelabd/assets/guide.html`, captures
intégrées, affiches floutées. Le fichier ne contient ni adresse ni contact (dépôt public) : `guide.rs`
remplace `{{JELLYFIN_URL}}`, `{{JELLYSEERR_URL}}` (et `_HOST`) et `{{CONTACT}}` depuis `.env`
(`JELLYFIN_PUBLIC_URL`, `JELLYSEERR_PUBLIC_URL`, `GUIDE_CONTACT_EMAIL`, `GUIDE_CONTACT_DISCORD`). Pour le
modifier : sources et captures dans `backups/guide-draft-20260915/` (`guide.final.tmpl.html`,
`build_final.py <asset> <aperçu>`, script de captures `shots.js` avec un compte ordinaire temporaire,
aucune demande envoyée), puis rebuild de homelabd. Validé par l'utilisateur le 2026-09-15.

## Tchat des membres

Bulle en haut à droite de l'interface web de Jellyfin (navigateur, Jellyfin Desktop, applis Android/iPhone,
LG webOS). Salons `annonces` (modérateurs seulement), `entraide`, `discussion`, et un fil privé
`prive:<id Jellyfin>` par membre, lisible par lui et par les modérateurs (`[chat] moderators`).

- **Chemin** : script chargé par JavaScript Injector (`branding/jellyfin/gc-chat-loader.js`) →
  `/gc-chat/app.js` → API `/gc-chat/api/*`, publiées par NPM (hôte Jellyfin) vers homelabd `/chat/*`.
- **API** : `GET /me` (salons, non-lus, dernière annonce non lue), `GET|POST /messages`,
  `DELETE /messages/{id}`, `POST /read`, `GET /private` (modérateurs). 401 sans session valide, 403 hors
  droits ou hors `beta_users`, 429 au-delà du débit (1 message / 3 s, 30 / 10 min).
- **Règles** (`homelab_core::chat`, testées) : texte nettoyé, 2 000 caractères ; suppression de son message
  pendant 15 min, de tout message pour un modérateur (message conservé, marqué supprimé).
- **Mails** : toutes les minutes, s'il y a de nouveaux messages d'entraide ou privés de membres et que le
  dernier récapitulatif a plus de 15 min, un mail à `CHAT_ADMIN_EMAIL`. Annonce avec « envoyer aussi par
  mail » : un mail par compte actif ayant une adresse valide dans Jellyseerr (2 s d'écart), sauf l'auteur.
- **Messages privés de l'admin** : onglet Privé → « Nouveau message privé », un ou plusieurs membres actifs
  (`GET /members`, `POST /direct`, modérateurs seulement) ; chacun reçoit le message **séparément** dans son
  fil privé (personne ne voit les autres destinataires), avec mail facultatif (`direct_mail`, adresse
  Jellyseerr). Côté membre : badge et bannière « Message de l'admin » sur l'accueil (prioritaire sur
  l'annonce), qui disparaît une fois le message lu ou quand il répond.
- **Client** : rafraîchi toutes les 5 s panneau ouvert, 60 s fermé, rien onglet caché ; bulle masquée pendant
  la lecture ; bannière de la dernière annonce non lue sur l'accueil ; aucun HTML de message interprété.
- **Couper** : `[chat] enabled = false` (+ restart homelabd) et désactiver le script dans JavaScript Injector.

## Page de don

`GET /don` (public, sous-domaine `don.`) : page statique `crates/homelabd/assets/don.html` avec le
bouton PayPal (`DONATION_PAYPAL_CLIENT_ID`, `DONATION_PAYPAL_PLAN_ID` dans `.env` ; absents = 404).
Don facultatif, sans aucun lien avec le service : la page le dit, rien n'y renvoie et elle ne renvoie à
rien. Le conteneur du bouton ne doit pas avoir `id="paypal"` (l'élément masquerait `window.paypal` et
le SDK plante). Avatar : `npm/data/don/avatar.jpg` (hors git), servi par NPM en `/avatar.jpg`. Le cadre
PayPal reste en `color-scheme: light` (sinon fond blanc opaque en thème sombre).

## Comptes premium

Premium = compte Jellyfin actif ; non-premium = `Policy.IsDisabled = true` (fonction native de
Jellyfin : connexion refusée, jetons existants rejetés, rien n'est supprimé). La politique Jellyfin est
la seule source de vérité ; les comptes admin ne sont jamais listés ni modifiés.

- **Page** `onboarder.<domaine>/accounts?token=<HOMELABD_ONBOARD_TOKEN>`, derrière la connexion NPM
  « admin-outils » : un interrupteur par compte, compteur « N premium / max ». Formulaires POST sans
  JavaScript, jeton en champ caché (anti-CSRF). Lien « Comptes » dans le tableau Homarr Opérations.
- **CLI** : `homelabctl accounts list | on <compte> | off <compte> | limits` (`--dry-run` respecté).
- **Suspension** : politique relue puis seuls `IsDisabled` et `MaxActiveSessions` changent (bibliothèques
  intactes) ; lectures en cours arrêtées ; permissions Jellyseerr sauvegardées dans `state.accounts` puis
  mises à 0 (une session Jellyseerr ouverte survit à la suspension Jellyfin).
- **Activation** : refusée au-delà de `accounts.max_premium` ; permissions Jellyseerr restaurées (à
  défaut de sauvegarde, celles par défaut de Jellyseerr).
- **Comptes protégés** (`accounts.protected` : Haradas, LeGrosCailloux) : affichés avec un badge,
  sans interrupteur ni suppression, hors plafond. Les autres admins sont gérés normalement.
- **Suppression** : bouton « Supprimer » → page de confirmation (`GET /accounts/delete`) → `POST
  /accounts/delete` : compte Jellyfin puis compte Jellyseerr (ses demandes partent avec). CLI :
  `homelabctl accounts delete <compte> --yes`.
- **Plafonds** (`[accounts]`) : 25 comptes premium, 2 lectures simultanées par compte. Dimensionnés
  pour 6 vCPU sans GPU (1 à 2 transcodages 1080p) et le lien seedbox (~190 Mbit/s, une douzaine de
  flux) ; pic mesuré le 2026-09-14 : 4 lectures simultanées pour 13 comptes, 92 % de lecture directe.
  Pas de limite de débit par utilisateur : elle forcerait des transcodages.

## Autres commandes

- `homelabctl vpn status|on|off` : voir DEPLOY.md (profils compose).
- `sudo homelabctl backup` : voir DEPLOY.md ; aussi `homelab-backup.timer`.
- `homelabctl check` : ping Sonarr, Radarr, qBittorrent, Jellyfin, Jellyseerr ; présence SMTP,
  token, dossier surveillé.
- `sudo homelabctl install` : copie `systemd/*` dans `/etc/systemd/system`, enable.

## Hook gluetun (reste en shell)

`hooks/qbit-update-port.sh` tourne **dans** le conteneur gluetun (busybox, pas de curl), appelé
par `VPN_PORT_FORWARDING_UP_COMMAND` avec le port forwardé : attend que qBittorrent réponde
sur 127.0.0.1:8080 puis `setPreferences {listen_port}`. Log sur stdout de gluetun.

## Tests

`cargo test` (unitaires : classification, politiques de tracker, décisions stuck/disk, filtre
TBA, règle monitor_sync, candidats du poller, validateurs d'onboarding, état, config).
`cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`. CI : `.github/workflows/rust.yml`.
