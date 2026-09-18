# CLAUDE.md

Guide pour Claude Code dans ce dépôt. Lire aussi ARCHITECTURE.md et AUTOMATION.md.

## Ce qu'est ce dépôt

`/opt/homelab` est **à la fois** le dépôt git (branche `main`, remote `HaradasCYB/groscailloux-homelab`)
et le répertoire de production : compose, config et code Rust sont versionnés ; l'état des
services (`<service>/`), `library/`, `backups/`, `state/`, `logs/` et `.env` sont ignorés par
git. « Déployer » = `docker compose up -d` pour les conteneurs, `cargo build` + `install` +
`systemctl restart homelabd` pour l'automatisation (`sudo ./setup.sh` fait tout).

## Commandes

```bash
docker compose config --quiet             # TOUJOURS avant un up -d
docker compose up -d [svc]                # recrée seulement ce qui a changé
docker compose ps ; docker compose logs -f <svc>

cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test
cargo build --release --target x86_64-unknown-linux-musl -j4      # laisser 2 vCPU à Jellyfin
sudo install target/x86_64-unknown-linux-musl/release/homelab{d,ctl} /usr/local/bin/ && sudo systemctl restart homelabd

homelabctl check | list | status | run <task> --dry-run | onboard | vpn | backup
journalctl -u homelabd -f
```

## Règles

- **Secrets** : uniquement dans `.env`. Ne jamais mettre une valeur en dur dans compose, TOML,
  code, scripts ou docs ; ne jamais coller `.env`, `backups/` ou une config de service dans un
  outil externe. Les scripts `scripts/*.sh` restants sourcent `.env`.
- **Images** : pinnées `tag@sha256`. Pour mettre à jour : nouveau tag + digest (`docker pull`
  puis `docker image inspect --format '{{index .RepoDigests 0}}'`), `up -d <svc>`, mettre
  `diun/images.yml` en cohérence. Pas de `:latest` nu.
- **Pas de `chown -R /opt/homelab`** : npm/, homarr/ (root), grafana/ (472), guacamole/mysql (999).
- **qBittorrent.conf** : arrêter le conteneur avant d'éditer, sinon il écrase le fichier.
- **Jamais de purge globale** de queue ou de torrents : toute suppression est ciblée et
  plafonnée (`max_actions_per_run`), c'est un invariant des tâches `stuck_handler`/`disk_pressure`.
- **Une saison entière en une requête** : sans pack, `series_search` prend **une release par épisode manquant**
  dans le même lot de résultats (`choose_episodes`, plafond `max_grabs_per_season` 20), puis reprend 15 min
  après. Avant, c'était un épisode toutes les 2 h : 22 h pour une saison de 11 épisodes (BLACK TORCH, le
  2026-09-17).
- **Deux clés C411, deux compteurs** (`homelab_core::indexer`) : indexers Prowlarr « C411 » et « C411 (2) »,
  `[indexers] c411_max_per_hour` = 40 **par clé** (80/h au total), `manual_reserve` 10 pour `/recherche`. Une
  clé qui répond 429 est mise de côté `cooldown_after_429_mins` (15) et la requête repart **aussitôt sur
  l'autre** : tant qu'une clé répond, rien ne s'arrête. La clé RSS des Arrs est la deuxième (`C411_RSS_API_KEY`).
- **Réglages des 4 Arrs (audit du 2026-09-18)** : `episodeTitleRequired = never` (les animés tout juste sortis
  ont un titre « TBA » : c'était la cause du rejet que `tba_bypass` contournait — tâche désormais dans
  `tasks.disabled`, gardée comme filet) ; `importExtraFiles = true` avec `srt,ass,ssa,sub,idx` (les
  sous-titres externes des VOSTFR étaient **jetés**) ; `animeEpisodeFormat` avec `{absolute:000}` ;
  `rssSyncInterval = 15` (le tracker sert le RSS avec un cache 5 min et ETag) ; corbeille configurée aussi sur
  le VPS (`/data/media/*/.recycle`, purge 14 j par `cleanup`). Toutes les fiches du VPS sont passées sur le
  profil **FR-friendly H.264** (elles étaient sur « HD - 720p/1080p », sans aucun format personnalisé).
  Sauvegarde : `backups/arr-settings-20260918-072920/`.
- **Type « anime » obligatoire** : une série rangée dans Anime est mise en `seriesType = anime` par
  `anime_library` (au déplacement et en rattrapage), sinon Sonarr ne comprend pas la numérotation absolue
  (« Bleach - 367 ») et les imports tombent à côté. 23 séries corrigées le 2026-09-18, sans perte de fichier.
- **Tout ce qui est neuf passe par la SEEDBOX** (2026-09-18) : `[downloads] auto_sides = ["seedbox"]` — `series_search`
  et `movie_search` ignorent les Arrs du VPS — **et** `enableRss = false` sur l'indexer C411 de Sonarr et Radarr du
  VPS (sauvegarde `backups/vps-rss-off-20260918-093803/`). Le VPS garde ses fiches, ses fichiers, `torrent_import` et
  `deletion_cleanup` ; il ne prend simplement plus aucune release. Remettre `"vps"` dans `auto_sides` **ne suffit
  pas** : il faut aussi rallumer le RSS côté Arr. Jellyseerr envoie déjà tout sur la seedbox (`isDefault` sur les
  serveurs id 1).
- **Les numéros de profil diffèrent d'une machine à l'autre** : VPS `6` = FR-friendly H.264, `7` = Anime - JAP/VOSTFR ;
  seedbox `7` = FR-friendly H.264, `8` = Anime - JAP/VOSTFR. Le 2026-09-18, les 24 séries et 46 films du VPS avaient
  été basculés sur le `7` du VPS en croyant viser FR-friendly : ils se sont retrouvés sur le profil japonais (VOSTFR
  +3000, JAP Audio +2500, **FRENCH −500**, `minFormatScore` 0 au lieu de −9999), donc prêts à préférer la VOSTFR et à
  refuser une release française. Remis sur le `6` le jour même, sans perte (352 épisodes, 46 films ; `upgradeAllowed`
  est à `false`, rien n'est retéléchargé). **Toujours vérifier le NOM du profil, jamais son numéro.**
- **Une recherche par identifiant peut ne couvrir qu'une partie d'une saison** : Bleach S17 le 2026-09-18, C411 par
  `{TmdbId}{Season}` renvoie 41 releases couvrant E01–26 et E41–48, **jamais E27–40**. `series_search::uncovered`
  compare les épisodes manquants aux candidats ; s'il en reste, le repli en texte libre est lancé **en plus** de
  l'identifiant (`how = "tmdb+texte"`), et ce qui reste introuvable est écrit dans l'état (`uncovered`) puis affiché
  sur `/status.html` (« Saisons sans release »). Sans ça la recherche repartait tous les jours pour rien, en silence.
- **Les cours d'un animé sont publiés sous leur propre titre** : `BLEACH.Thousand-Year.Blood.War.S01/S02/S03` (packs
  H264 MULTi VFF) couvrent Bleach S17. Sonarr n'en rattache correctement que **S01** (→ saison 17) ; **S02 → saison 2
  et S03 → saison 3 de Bleach**. Les prendre automatiquement écraserait deux vraies saisons : le garde-fou d'égalité
  de saison de `series_candidate` les refuse, et il ne faut pas le retirer. Ces releases sont montrées sur
  `/recherche` marquées « autre saison », à l'admin de trancher.
- **Aucune recherche depuis Sonarr/Radarr** : C411 y est en **RSS seulement** (`enableAutomaticSearch` et
  `enableInteractiveSearch` à `false` sur les 4 Arrs, depuis le 2026-09-17). Un bouton « Search » sur une saison
  d'animé interrogeait C411 épisode par épisode : 30 requêtes d'un coup, « API Request Limit reached, disabled
  for 01:00:00 », et comme C411 est seul, Sonarr passait en « All indexers are unavailable ». Toutes les
  recherches se font dans **`/recherche`** (budget horaire). L'avertissement « No indexers available with
  Automatic Search enabled » est normal et voulu. **Deux clés C411** : `C411_RSS_API_KEY` pour le RSS des Arrs,
  la clé historique pour les recherches (Prowlarr) — une rafale de recherche ne peut plus couper le RSS.
- **Œuvres dérivées** : une release dont le titre contient `mini`, `specials`, `OVA`, `recap`, `abridged`,
  `junior`… absent des titres de la fiche est écartée du choix automatique (`series_search::derivative`) ; le
  repli en texte libre exige en plus que l'Arr rattache la release à **cette** fiche. Le 2026-09-17, le pack
  *Smoking Behind the Supermarket with You (Mini Episodes)* (12 min/épisode) avait été pris et importé à la
  place de la série officielle (24 min) : fichiers et torrent retirés, vraie saison 1 récupérée.
- **Indexers** : **C411 est le seul indexer**, dans les 4 Arrs comme dans Prowlarr (2026-09-17 : les 34
  indexers publics passant par Jackett ont été retirés, Jackett et FlareSolverr arrêtés et sortis du compose ;
  ils ne servaient qu'en interactif, faisaient durer une recherche plusieurs minutes et remplissaient le
  journal d'erreurs — sauvegarde `backups/indexers-20260917-204443/`). Un seul compteur horaire pour tout ce
  qui l'interroge : `[indexers] c411_max_per_hour` (20), dont `manual_reserve` (6) gardées pour `/recherche`
  (les tâches s'arrêtent à 14/h) ; Prowlarr coupe à 30/h, C411 vers 50/h. **Profils** FR-friendly : 1080p max (jamais 2160p), `minFormatScore=-9999`, FR d'abord,
  VO/VOSTFR en dernier recours. **Jellyfin** : pas de GPU, préférer x264 à HEVC.
- **C411 annonce sur deux domaines** : `c411.org` **et** `tk.c411.tw`. Toute règle par tracker doit viser les deux
  (`tracker_ratio.unlimited`, décision torrent de `deletion_cleanup`). Jusqu'au 2026-09-14, 24 torrents
  `c411.tw` héritaient de la limite globale de qBittorrent (ratio 1 / 7 j puis **arrêt**) : 53 torrents C411
  étaient arrêtés, relancés ce jour-là.
- **Recherche des séries = homelabd, par identifiant TMDB** (tâche `series_search`), plus par Sonarr :
  Jellyseerr est en **`preventSearch`** sur les deux Sonarr. C411 renvoie les releases d'une série par
  `{TmdbId}{Season}` quel que soit leur nom (japonais, anglais, français) et **ignore l'identifiant IMDb**
  (100 releases sans rapport). Sonarr garde le RSS, l'import et le suivi. Ne pas remettre la recherche
  à la demande dans Jellyseerr : un animé = 3 à 4 requêtes C411 par épisode → **429** → pause de l'indexeur
  qui s'allonge jusqu'à 24 h (le 2026-09-17, niveau 9). `animeCategories=[5070]` et
  `animeStandardFormatSearch=true` restent dans les deux Sonarr pour le RSS. Films : Radarr cherche déjà par
  identifiant ; `movie_search` rattrape ce qui manque après 24 h. Filet : C411 limité à 25 requêtes/heure
  dans Prowlarr ; homelabd en envoie au plus 12/h pour les séries et 1/h pour les films : la clé est
  partagée avec les 4 Arrs et le 429 est tombé vers 50 requêtes/heure le 2026-09-17. Éviter les recherches interactives en rafale sur un animé.
- **Indexeur en pause** : `indexer_unblock` lève la pause (table `IndexerStatus`, application arrêtée ~20 s,
  base sauvegardée) une heure après le dernier échec ; au-delà de 3 fois en 24 h, mail seulement.
- **Import d'un téléchargement que l'Arr n'a pas demandé** : `ManualImport` en `importMode: copy`
  (hardlink), jamais `auto` (= déplacement, le torrent perd ses fichiers) ; `GET manualimport` sans
  `downloadId` (liste vide sinon). C'est ce que fait `torrent_import`.
- **Arrêter un service volontairement** : l'ajouter à `tasks.stack_health.ignore` dans
  `homelab.toml` (+ restart homelabd) ou désactiver la tâche, sinon `stack_health` le relance
  dans les 5 min. `guacamole` est en `restart: "no"` exprès (course au boot avec guacdb).
- **Lecture Jellyfin** : aucune tâche lourde (trickplay, analyse de segments, scan complet,
  extraction) entre 13 h et 05 h ; trickplay jamais pendant les scans (il lit tout le fichier, par le
  lien seedbox pour ses titres). `cpu_shares` : jellyfin 2048, fond 512 — le garder sur tout nouveau service de fond.
- **Aucune option qui lit la vidéo à l'ajout d'un titre** (Intro Skipper `AutoDetectIntros`, source
  d'images « Screen Grabber », `SaveLocalMetadata`/NFO) : sur les dossiers seedbox, chaque lecture passe
  par le lien (~24 Mo/s partagés) et fait buffer les spectateurs. Pas de `--vfs-read-ahead` sur rclone.
- **Médias en écriture pour Jellyfin** (`/media` et `/seedbox` sans `:ro`, rclone sans `--read-only`) : seulement
  pour que le bouton « Supprimer » marche. Aucune option qui écrit dans les dossiers médias
  (`SaveLocalMetadata`, `SaveSubtitlesWithMedia`, trickplay avec le média : tous à `false` ; `MetadataSavers`
  vide sur Films et Séries, sinon Jellyfin écrit des `.nfo`). Une suppression dans Jellyfin est suivie par
  `deletion_cleanup` (fiche Arr, Jellyseerr, torrent).
- **Clé rclone de la seedbox** (`authorized_keys` côté seedbox, commentaire `homelab-sftp-rd`) : `sftp-server -P
  write,mkdir,rename,…` = lecture + suppression, **aucune écriture** (un `open` en création peut laisser un fichier
  vide). Sauvegarde `~/.ssh/authorized_keys.bak-20260915`. Une écriture sur `/mnt/seedbox` réussit en local puis
  reste coincée dans le cache (`cache/rclone/vfsMeta`, rclone réessaie toutes les 5 min, « permission denied ») :
  arrêter le montage, retirer les entrées (vfsMeta + vfs), relancer.
- **Comptes** : premium = compte Jellyfin actif, non-premium = `IsDisabled` ; passer par
  `homelab_core::accounts` (page `/accounts`, `homelabctl accounts`) qui garde les permissions Jellyseerr.
  `accounts.protected` (Haradas, LeGrosCailloux) : jamais suspendus ni supprimés par la page ; les autres
  admins sont gérés comme tout le monde. Plafonds dans `[accounts]` : 25 premium, **2 lectures
  simultanées** par compte (tâche `playback_limit`, comptes protégés exemptés) ; pas de `RemoteClientBitrateLimit`
  (forcerait des transcodages). **`MaxActiveSessions` reste à 0** (`max_devices_per_user`) : il ne joue qu'à la
  connexion (403 « maximum number of sessions », affiché comme une erreur d'identifiants ou sans message) et
  l'appli **iOS crée une nouvelle session à chaque ouverture** : le 2026-09-15, 5 sessions fantômes d'un iPhone
  bloquaient la TV et la voiture d'un membre, Quick Connect compris.
- **Demandes Jellyseerr** : validation automatique pour tous (bit 128, `accounts.jellyseerr_auto_approve`, posé à
  la création et à l'activation, et `defaultPermissions = 160` dans Jellyseerr). Garde-fou : quota par défaut
  Jellyseerr 10 films + 10 saisons / 7 j (`defaultQuotas`, admins et gestionnaires de demandes exemptés).
  Sauvegarde d'avant : `backups/jellyseerr-settings-main-20260915-094557.json`. Une réponse de `settings/main`
  contient la clé API : ne jamais l'afficher (filtrer les champs).
- **Tchat des membres** (`homelab_core::chat`, API `crates/homelabd/src/chat_api.rs`, client
  `crates/homelabd/assets/chat/app.js`) : servi sous `/gc-chat/` **sur l'adresse de Jellyfin** (NPM hôte 1,
  `location ^~ /gc-chat/` → `172.18.0.1:8766/chat/` ; le `^~` est obligatoire, sinon la règle de cache
  des `.js` de NPM envoie `app.js` à Jellyfin). Chargé par le plugin **JavaScript Injector** (script
  « Groscailloux Tchat » = `branding/jellyfin/gc-chat-loader.js`, « Requires authentication ») : une mise à
  jour du tchat = rebuild de homelabd, pas le plugin. Identité = jeton de session Jellyfin vérifié par
  `/Users/Me`, cache mémoire 5 min (une suspension prend effet sous 5 min), jamais écrit ni journalisé.
  Modérateurs et phase de test (`beta_users`) dans `[chat]` ; base `state/chat.db` (sauvegardée par
  `homelabctl backup`). Mails : récapitulatif à `CHAT_ADMIN_EMAIL` (repli `GUIDE_CONTACT_EMAIL`), annonces
  aux comptes actifs ayant une adresse **valide** dans Jellyseerr (celle de Haradas y vaut `haradas`).
  Pas de tchat dans les applis natives (Android TV, Swiftfin).
  **Téléviseurs** (webOS, Tizen, Android TV — détectés par l'agent, l'absence de pointeur ou ≤ 2 cœurs) :
  boucle à 3 s au lieu de 1 s, sondages espacés (10 s ouvert / 180 s fermé), ni ombre ni animation, et les
  bandeaux (annonce, message privé, aide à la qualité) se ferment à la touche **Retour** (keyCode 461 webOS,
  10009 Tizen) **et** s'effacent seuls au bout de 12 s : sans pointeur, la croix est inatteignable. Mesuré le
  2026-09-16 sur un téléviseur simulé (processeur bridé 6x) : nos scripts coûtent ~2 points de processeur sur
  l'accueil et rien de mesurable en lecture — la lenteur vient du client webOS lui-même. **Jamais de `window.confirm/alert/prompt`** dans
  les scripts injectés : la WebView de l'appli iPhone et Jellyfin Desktop les ignorent (réponse « non » sans rien
  afficher) — le bouton Supprimer du tchat était inerte pour cette raison ; confirmer dans la page.
- **Saccades en cours de lecture** : Jellyfin choisit **une seule qualité par session** (pas d'ABR). En « Auto »,
  un 1080p part tel quel (~5 Mbit/s) : si le débit du membre baisse, la lecture cale et jellyfin-web relance le
  flux toutes les ~30 s (journal : « non-keyframe breaks »), ce qui aggrave le retard. Le 2026-09-15, un membre a
  eu ce cas (4,7 Mbit/s demandés, 2 à 4 Mbit/s disponibles ; serveur à 80 % de CPU libre, fichier déjà à 93 % dans
  le cache rclone) ; à 1,5 Mbit/s la même lecture a tenu 55 min sans une relance. Pour diagnostiquer : taille et
  cadence des segments dans `npm/data/logs/proxy-host-1_access.log` (horodatage **UTC**), journaux ffmpeg
  (`jellyfin/config/log/FFmpeg.*`), croissance du cache dans `journalctl -u homelab-seedbox-mount`, et
  `docker_container_net` dans InfluxDB. Aide en place : `branding/jellyfin/gc-quality-helper.js` (JavaScript
  Injector, « Groscailloux Qualité ») surveille la progression de l'image — les événements `waiting` ne suffisent
  pas, hls.js les absorbe — et propose au 3ᵉ blocage de passer au palier sous 2 Mbit/s **par le menu du lecteur**
  (roue crantée → Qualité) : la commande `SetMaxStreamingBitrate` n'est pas gérée par le client web
  (« does not recognize ») et écrire `maxbitrate-Video-*` ne change pas la lecture en cours.
- **« Lire sur » (diffuser vers un autre appareil)** : filtré par `branding/jellyfin/gc-cast-filter.js`
  (JavaScript Injector, déployé avec `scripts/jellyfin-js-apply.py`) : seulement ses propres appareils,
  et seulement ceux qui ont la même adresse publique que l'appareil courant (même box = même Wi-Fi ; pas en
  données mobiles). Adresse inconnue ou privée ⇒ rien d'autre proposé. Jellyfin garde la liste des cibles en
  mémoire pendant la vie de la page : le filtre doit être chargé avant la première ouverture du menu.
  Google Cast ne marche que dans Chrome et l'appli Android (« non pris en charge » dans Safari et Jellyfin
  Desktop, c'est normal) ; AirPlay passe par le bouton du lecteur dans Safari/iPhone.
- **Adresses des clients** : `KnownProxies = 172.18.0.0/16` (réseau Docker entier) dans la configuration
  réseau de Jellyfin, lu **au démarrage seulement**. Avant le 2026-09-15 il valait l'ancienne IP de NPM
  (`.15`, NPM est passé en `.16`) : Jellyfin voyait tout le monde comme NPM, donc comme réseau local.
  Aucune limite de débit « distant » n'est réglée, ce qui compte si ça change.
- **NPM hôte 1 (Jellyfin)** : sa configuration avancée contient les réglages SyncPlay (tampons coupés,
  délais 3600 s ; avant le 2026-09-15 ils n'étaient que dans le fichier conf, pas en base) et la route du
  tchat. Toujours éditer base **et** fichier ensemble (sauvegarde `backups/npm-*-chat`).
- **Identifications surveillées** : la tâche `identity_check` (30 min) compare l'identifiant de chaque fiche
  Jellyfin à celui de Sonarr/Radarr, qui fait foi, et corrige seule (`RemoteSearch` + `Apply` + métadonnées,
  3 titres au plus par passage, jamais pendant une lecture). Les nouveaux dossiers portent l'identifiant
  (`seriesFolderFormat = {Series Title} [tvdbid-{TvdbId}]`, `movieFolderFormat` avec `[tmdbid-…]`) ; les
  anciens n'ont pas été renommés. Le 2026-09-17, 6 titres étaient mal identifiés (dont *The Walking Dead* vu
  comme *Dead City* et *Tomb Raider* comme *Lara Croft*).
- **Titre mal identifié par Jellyfin** : un film au titre court ou ambigu peut être rattaché au mauvais TMDB
  (le 2026-09-15, *Midnight* 2021 → *Before Midnight* 2013) ; Jellyseerr, qui compare les ids TMDB, laisse alors la
  demande « en cours ». Corriger par `POST /Items/RemoteSearch/Movie` (ProviderIds Tmdb) puis
  `/Items/RemoteSearch/Apply/{id}`, et lancer le job Jellyseerr `jellyfin-full-scan` (le scan « recently added »
  ne revoit pas un titre ajouté la veille). **Séries aussi** : le 2026-09-16, le dossier « Attack on Titan » a été
  rattaché au spin-off *Junior High School* (TVDB 299882) alors que Sonarr avait la bonne fiche (267440) —
  `POST /Items/RemoteSearch/Series` (ProviderIds Tvdb), `Apply` (peut dépasser 3 min, vérifier ensuite plutôt que
  relancer) puis `Refresh` récursif en `FullRefresh` + `ReplaceAllMetadata` pour rattacher tous les épisodes.
- **Animés** : bibliothèques Jellyfin « Anime » (séries) et « Films d'animation » (films), dossiers `/anime` et
  `/anime-films` (VPS), `Anime` et `Anime Movies` (seedbox), rangés par `anime_library` d'après **TMDB** (genre
  Animation + origine japonaise), jamais d'après le type « anime » de Sonarr. Forcer : tag `anime` ou `pas-anime`
  dans l'Arr. Tout déplacement en masse hors de cette tâche : `deletion_cleanup` dans `tasks.disabled` pendant ce
  temps. Ids Jellyfin : Anime `0c41907140d802bb58430fed7e2cd79e`, Films d'animation `bebdce85c5b682ddbce0412f41cff060`
  (dans `JELLYFIN_LIB_EXTRA` et les `EnabledFolders` des comptes ; ordre du menu `OrderedViews` : Films, Séries, Anime, Films
  d'animation, Collections, posé à la création du compte) ; Jellyseerr : les 4 bibliothèques activées,
  `activeAnimeDirectory` + `animeTags` des deux Sonarr sur le dossier et le tag anime. **Une bibliothèque Jellyfin
  créée par l'API reste vide**, et **un déplacement sur la seedbox n'apparaît pas** dans la nouvelle bibliothèque,
  tant que l'analyse complète de la médiathèque n'est pas passée (ni `Items/{id}/Refresh`, ni
  `Library/Media/Updated`, ni un redémarrage ; vu le 2026-09-17, analyse de 65 à 315 s) : ranger en masse
  **avant 13 h**, ou attendre l'analyse de 05 h. Après une réidentification, vérifier les `ProviderIds` contre
  l'Arr : le 2026-09-17, *L'Attaque des Titans* est repartie sur son spin-off et *Slime* sur *Slime Diaries*.
- **Langue** : `lang_rank` classe VF 4 > MULTi 3 > FRENCH 2 > VOSTFR 1 > **VO 0** ; une release sans français
  n'est prise qu'en dernier recours (`[indexers] allow_no_french`), quand aucune française n'est acceptable.
  Plafonds de taille du choix automatique : `max_gb_per_episode` (6) et `max_gb_per_movie` (25) — sinon un pack
  de 134 Go à une seule source peut gagner contre un 27,8 Go bien partagé. Un refus « blocked till … » compte
  comme une **erreur** (nouvelle tentative dans l'heure), plus comme « aucun candidat » (24 h).
- **Suppression d'une demande dans Jellyseerr** : `deletion_cleanup` supprime la fiche Arr **et ses fichiers**,
  retire les torrents devenus inutiles, puis la fiche média. **Seuls les médias « en attente » (2) ou « en
  cours » (3) sans demande** sont concernés : un scan Jellyfin crée une fiche média pour tout ce qui est déjà
  dans la bibliothèque (237 médias pour 117 demandes), toutes « disponible » ou « partiel » — les toucher
  effacerait la médiathèque.
- **Recherche manuelle** : passer par la page **`/recherche`** de homelabd (hôte d'onboarding, liste « admin-outils »
  + jeton), jamais par la recherche de Sonarr/Radarr sur un **animé** : celle-ci interroge chaque indexeur avec
  chaque titre connu, épisode par épisode (le 2026-09-17 : plusieurs minutes, « timed out » du proxy seedbox à
  300 s, et 429 de C411 pendant une heure). La page cherche par identifiant TMDB chez C411 (1 requête, plafond
  `[manual_search] max_queries_per_hour`) et chez Nyaa pour les animés (liens **magnet** : ajoutés au qBittorrent
  du côté concerné avec l'étiquette `homelab:`). Toutes les releases sont montrées et marquées (VOSTFR, hors
  profil, autre saison…), le choix reste à l'admin.
- **Dossiers de saison** : `enableSeasonFolders` était à `false` sur les deux Sonarr de Jellyseerr — toute fiche créée
  par une demande rangeait ses épisodes à plat (28 séries sur 77 le 2026-09-18, dont Bleach et ses 366 épisodes).
  Remis à `true` (sauvegarde `backups/jellyseerr-sonarr-20260918/`) ; les tâches homelabd créaient déjà avec
  `seasonFolder: true`. Les fiches déjà à plat le restent tant qu'on ne les renomme pas.
- **Vue d'ensemble des membres** : onglets Demandes et Calendrier de Jellyfin Enhanced ouverts à tous
  (`DownloadsFilterByUserRequests` et `CalendarFilterByLibraryAccess` à `false`, `SonarrInstances`/`RadarrInstances`
  = VPS **et** seedbox) et droit Jellyseerr « voir les demandes » (bit 16384) sur les comptes actifs, dans
  `defaultPermissions` et à l'activation (`[accounts] jellyseerr_view_requests`). Chaque membre voit donc les
  demandes des autres, avec leur pseudo. Le lien compte Jellyfin ↔ Jellyseerr est en cache 30 min dans le plugin.
  Sauvegardes : `backups/jellyfin-ui-20260917-191027-overview/`.
- **Historique Arr** : `GET history?movieId=` / `?seriesId=` n'existe pas, le filtre est ignoré et tout
  l'historique revient. Utiliser `history/movie?movieId=` et `history/series?seriesId=` (seul `downloadId`
  filtre vraiment `GET history`).
- **Profils compose** : `COMPOSE_PROFILES=vpn|novpn` dans `.env`, changé uniquement par
  `homelabctl vpn`. `gluetun`+`qbittorrent` et `qbittorrent-direct` ne coexistent jamais.
- **Journal des versions** : tout changement visible pour les membres ou l'admin ajoute une ligne dans
  `UPDATE.md` (section de la version en cours, en haut, avec ses commits ; 1.0.x corrections, 1.x.0 nouveautés).
  Jamais de pseudo de membre, d'IP, de domaine ni d'e-mail dans le dépôt (public) : écrire « un membre ».
- **Nouvelle tâche** : un module dans `crates/homelab-core/src/tasks/`, `impl Task`, ajout dans
  `registry()`, section `[tasks.<nom>]` dans `config.rs` + `homelab.toml`, dry-run respecté,
  tests unitaires de la décision, paragraphe dans AUTOMATION.md.
- **Changement de comportement** = changement de `homelab.toml` (seuils, intervalles) avant
  changement de code. Les valeurs par défaut du code doivent rester égales à celles du TOML.
- **Reboot** : `homelab-stack.service` relance compose ; vérifier `docker compose ps` et
  `systemctl status homelabd` après.
- `scripts/` ne contient plus que des outils ponctuels (les anciens scripts bash planifiés ont été
  retirés) : `jellyfin-branding-apply.sh`, `jellyfin-ui-rollback.sh` (voir « Interface Jellyfin »),
  `jellyseerr-rotate-key.py` (voir « Pièges connus ») et `jellyfin-js-apply.py` (scripts JavaScript Injector).

## Interface Jellyfin (« Groscailloux TV », 2026-09-14)

- **Thème** : ElegantFin **épinglé** + calque maison, source `branding/jellyfin/groscailloux-tv.css`, appliqué par
  `scripts/jellyfin-branding-apply.sh` (sauvegarde l'ancien, refuse tout `@import` en `@main/@master/@latest`).
  Ne pas éditer le CSS dans l'interface. Monter ElegantFin = changer le tag, repasser le banc d'essai
  (captures bureau + téléphone), puis appliquer. Jellyfin 10.11 lit ce CSS dans `Branding/Configuration`
  (champ `CustomCss`), plus `Branding/Css`.
- **Écran de connexion** : aucun compte listé (`IsHidden = true` pour tous, et dans `non_admin_policy` pour les
  nouveaux, depuis le 2026-09-15 : la liste publique exposait les noms). Titre : `--loginPageText` dans le calque
  (« Connecte-toi » ; plus long, il passe sur deux lignes et chevauche le cadre).
- **Logo** : `branding/jellyfin/logo/` (source `logo.html`, rendu par Chromium), déposé via
  `POST /JellyfinEnhanced/UploadBrandingImage` (noms : `banner-light.png`, `banner-dark.png`,
  `icon-transparent.png`, `favicon.ico`, `apple-touch-icon.png`).
- **Retour arrière** : `scripts/jellyfin-ui-rollback.sh backups/jellyfin-ui-<date>` (config, plugins,
  préférences d'affichage ; `--with-db` seulement si Jellyfin ne démarre plus).
- Plugins IAmParadox27 (Home Screen Sections, Plugin Pages, Collection Sections) : un même numéro de
  version existe pour plusieurs ABI ; installer par le catalogue du serveur (`/Packages`), qui prend la
  compilation compatible, et vérifier `targetAbi` dans `meta.json`.
- La bibliothèque « Collections » est donnée à tous les comptes (`JELLYFIN_LIB_EXTRA`) : sans elle, un
  compte ordinaire ne voit ni les sagas ni les rangées de collections.
- Jellyfin Enhanced : `ThemeSelectorEnabled = false` (ses couleurs Jellyfish entreraient en conflit avec
  ElegantFin). Onglets Découvrir / Demandes / Calendrier = pages natives de Jellyfin Enhanced, renommées en
  français dans le calque CSS (`#je-native-tab-btn-*`). Les anciens onglets Movies / TV Shows / Requests /
  Letterboxd venaient de SeerrFin (retiré).
- Après un redémarrage de Jellyfin, une appli restée ouverte (Jellyfin Desktop) garde l'ancienne page : nouveau
  CSS mais anciens scripts (pas de rangées, ancien logo, SyncPlay qui envoie des titres Seerr sans id →
  `Guid can't be empty`). Faire recharger (Ctrl+R) ; un redémarrage coupe aussi les groupes SyncPlay.
- **Accueil** (configs de plugins hors git, sauvegardées dans `backups/jellyfin-ui-*`) : Home Screen Sections
  (16 rangées, ordre Netflix, chargement 4 par 4 : au-delà l'accueil ralentit à froid ; « Séries à venir »
  désactivée car badge et dates en anglais incrustés par le plugin), Collection Sections (Tendances,
  Anime, Les mieux notés, Films français), Auto Collections (collections françaises, orphelines supprimées).
  Un compte absent de Jellyseerr ne voit pas les rangées « Découvrir ».
- Tester l'interface : navigateur jetable + compte ordinaire temporaire ; remplacer le CSS dans CE navigateur
  en interceptant `Branding/Configuration` (et contourner le service worker), jamais en production.

## Seedbox

- Accès admin : `ssh seedbox` (clé `~/.ssh/seedbox_ed25519`) ; apps via `app-<x> …`, en conteneurs
  Docker sur la seedbox (Radarr 16127, Sonarr 16126, Jackett 16129, FlareSolverr 16111 sur
  `172.17.0.1`), qBittorrent natif `127.0.0.1:16141`, autobrr natif `127.0.0.1:16123`. API
  publiques : `https://kakaouette.tofino.usbx.me/<app>`.
- Montage : rclone dans **`/mnt/seedbox/media`**, Jellyfin lie le **parent** `/mnt/seedbox`
  (rslave). Lier le point de montage FUSE lui-même casse la reprise après coupure.
- Nouvelles demandes Jellyseerr → Arrs seedbox (id 1). Ne rien importer côté seedbox qui existe
  déjà sur le VPS (doublons dans Jellyfin) : `torrent_import` le refuse (`dup_other_side`).
- Jellyfin : « Films » et « Séries » ont chacune deux dossiers (`/media/…` et `/seedbox/media/…`) ;
  plus de bibliothèques « (Seedbox) ». qBittorrent seedbox : `[seedbox] qbit_url` + `SEEDBOX_QBIT_PASSWORD`.
- Jellyseerr : ne jamais appeler `settings/jellyfin/library?sync=true` sans renvoyer `?enable=`
  avec la liste complète des bibliothèques.

## Pièges connus

- **Tests** : un environnement de test, jamais la prod. Pour le tchat : une **seconde instance de homelabd**
  (binaire de la branche, `HOMELABD_DRY_RUN=1`, toutes les tâches dans `tasks.disabled`, `auto_import`
  coupé, port `127.0.0.1:18766`, `state_file` et `chat.db_file` à part, modérateurs = comptes de test), et
  le navigateur de test qui redirige `/gc-chat/*` vers elle ; voir `backups/chat-tests-20260915/`. Comptes ordinaires temporaires (supprimés avec
  `homelabctl accounts delete`), et pour les captures, réponses d'API simulées **dans le navigateur de test**
  (interception, voir `backups/chat-tests-20260915/chatshots.js`). Une session ouverte par l'API compte dans
  la limite de 2 appareils : supprimer puis recréer le compte de test plutôt que toucher aux appareils.
- **`GET /Devices?userId=` ignore le filtre** et renvoie **tous** les appareils : le 2026-09-15, une boucle
  `DELETE /Devices` dessus a déconnecté tous les membres de toutes leurs applis. Aucune suppression en boucle
  sans vérifier le nombre et le propriétaire (`LastUserId`) de chaque élément.
- **JavaScript Injector** active aussi les scripts d'autres plugins qui s'y enregistrent (Jellysleep : minuteur
  de mise en veille dans le lecteur, visible depuis le 2026-09-15).
- **Rotation de la clé API Jellyseerr** (faite le 2026-09-15, clé exposée dans une conversation) : `POST
  /api/v1/settings/main/regenerate`, puis tous les consommateurs dans la foulée : `.env` (`JELLYSEERR_API_KEY`,
  restart homelabd), `JellyseerrApiKey` de Jellyfin Enhanced **et** de Home Screen Sections (API des plugins),
  Homarr (intégration « Jellyseerr », secret chiffré : Homarr arrêté, base sauvegardée). Tout est fait par
  `sudo scripts/jellyseerr-rotate-key.py` (~12 s de coupure, aucune clé affichée). Nouveau consommateur de la clé
  = l'ajouter au script.

- `.env` est lu par bash (`.` ), compose et dotenvy : pas d'expression shell, guillemets seulement
  autour des valeurs avec espaces.
- Le hook `hooks/qbit-update-port.sh` s'exécute dans l'image gluetun (busybox) : POSIX sh,
  `wget` uniquement.
- `GET /api/v3/manualimport` de Sonarr dure ~25 s sur un gros `/downloads` (timeout 5 min).
  Avec `folder=` un fichier **situé dans le dossier d'une série**, Sonarr renvoie tous les fichiers de
  la saison : toujours choisir le candidat par **chemin exact**, jamais le premier (le 12/09, S17E41 a
  été rattaché à E48 par erreur, corrigé en réimportant chaque fichier vers son épisode).
- Prowlarr n'a aucune application configurée : les indexers vivent dans Sonarr/Radarr et les
  publics passent par **Jackett** (+ FlareSolverr pour Cloudflare). Seule exception, depuis le 2026-09-16 :
  **C411 est aussi déclaré dans Prowlarr** (même clé, 25 requêtes/heure), uniquement pour les recherches de
  `series_search` et `movie_search` (par identifiant TMDB, texte libre en secours). La clé n'est pas lisible par l'API des Arrs (champ masqué) : elle
  vient de leur base. Ne pas y brancher d'application, sinon Prowlarr réécrirait les indexers des Arrs. Avant de retirer un service,
  vérifier qui l'appelle : `grep -r <nom>:<port>` dans les configs et les champs `baseUrl` des
  indexers Arr (`GET /api/v3/indexer`) — le retrait de Jackett/FlareSolverr le 2026-09-10 a coupé
  les indexers publics pendant deux jours.
- **Indexer bloqué par un Arr** : après des échecs (délais dépassés, 429), Sonarr met l'indexer en pause
  jusqu'à **24 h** (« Indexer C411 is blocked till … due to failures ») ; ni `testall` ni un réenregistrement ne
  lèvent le blocage (pas d'API : `indexerstatus` → 404), et aucune recherche ni `release/push` ne passe.
  L'état est dans la table `IndexerStatus` ; `indexer_unblock` s'en charge. Pendant la pause, `series_search`
  passe par qBittorrent (étiquette `homelab:`).
- **`torrent_import` ne remplace jamais un fichier** : épisode (ou film) déjà présent ⇒ fichier écarté, et la
  correspondance d'épisodes de l'Arr prime sur l'analyse du nom. Le 2026-09-17, « The.Final.Season.E01 » (sans
  saison) a été lu S01E01 et la saison 1 d'une série écrasée ; réparé en réimportant les fichiers d'origine
  (toujours présents dans le dossier du torrent, hardlink) avec la correspondance de Sonarr, et vérifié saison
  par saison. Vérifier l'historique (`episodeFileDeleted`, raison `Upgrade`) après tout import manuel.
- **homelabd est cloisonné** (`ProtectSystem=strict`) : tout nouveau dossier écrit par une tâche va dans
  `ReadWritePaths` de `systemd/homelabd.service` (sinon « Read-only file system », vu le 2026-09-17).
- Jellyfin 10.11 : une bibliothèque supprimée (API ou UI) reste dans les vues des utilisateurs,
  même après un scan global, jusqu'au redémarrage de Jellyfin (`docker compose restart jellyfin`).
  `DELETE /Items/<id>` **efface le disque** (c'est le bouton « Supprimer » de Jellyfin) : jamais pour
  « nettoyer » une vue ou une bibliothèque.
- **qBittorrent** : ne jamais remettre `172.18.0.0/16` dans `bypass_auth_subnet_whitelist` (NPM y
  est : qBit serait public sans mot de passe) ; seulement `127.0.0.0/8` et `172.18.0.1/32`.
  Garder `web_ui_reverse_proxy_enabled` (proxies de confiance `172.18.0.0/16`) : sans ça, qBit voit
  toutes les connexions venir de NPM et un ban (5 échecs, 1 h) bloque l'accès web pour tout le monde
  (arrivé le 2026-09-14). Lever un ban : redémarrer qbittorrent (bans en mémoire).
- **NPM** : pas d'identifiants admin NPM ici ; la liste d'accès « admin-outils » (id 2) a été écrite en
  imitant NPM (base + `npm/data/access/2` + bloc dans `location /` de chaque site), sauvegarde
  `backups/npm-20260912-212419/`. Tout nouvel outil d'admin exposé : même liste. Ne jamais afficher
  les colonnes `password` des tables NPM ou Homarr.
- **Homarr** : modifier la base Homarr **arrêté** et après sauvegarde ; titres de section ≤ 20 caractères
  (sinon le tableau ne se charge plus) ; secrets d'intégration chiffrés AES-256-CBC avec
  `SECRET_ENCRYPTION_KEY` ; pings des outils protégés par NPM en URL interne (`http://sonarr:8989/ping`…).
- L'UI d'onboarding est sur l'hôte (8766) ; NPM doit cibler `172.18.0.1:8766`, pas un conteneur.
