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

homelabctl check | list | status | run <task> --dry-run | onboard | vpn | backup | chat announce <fichier>
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
  Une saison notée « introuvable » puis **complétée** (pack de cours, import manuel) sort de la liste au passage
  suivant de `series_search` (`stale_uncovered`, 2026-09-19 : Bleach S17 27-40 restait affichée à 48/48).
- **Les cours d'un animé sont récupérés seuls** (2026-09-18, v1.12.0) : quand la recherche laisse un trou dans
  une saison d'**animé**, `series_search` repère les packs que l'Arr rattache à **cette** fiche mais à une autre
  saison (`cour_pack`), télécharge leur `.torrent` par Prowlarr (lecture seule, **aucune annonce au tracker**),
  en lit la liste des fichiers (`homelab_core::torrent_file`, décodeur bencode maison) et n'agit que si
  `offset_mapping` est certain : trou d'un seul tenant, autant de fichiers vidéo que d'épisodes manquants,
  numérotés en suite continue, aucun épisode déjà pourvu. La correspondance voyage dans l'étiquette
  qBittorrent `homelab:series=<id>:season=<n>:offset=<k>:eps=<from>-<to>` ; `torrent_import` l'applique en
  court-circuitant **et** les épisodes proposés par Sonarr **et** `map_episodes` (`EpisodeSource::OursOnly`),
  les deux lisant la saison annoncée par les fichiers. Un pack de cours n'est **jamais** proposé à l'Arr
  (`goes_straight_to_qbittorrent`) : il l'accepterait comme la saison qu'il croit lire. Trois pièges traités :
  l'indexer donne aux packs l'identifiant TMDB **du cours** (313552) et non de la série (30984) — le refus par
  identifiant est donc contourné **pour ce seul chemin**, la sécurité venant de la table d'alias de l'Arr et de
  la lecture du `.torrent` ; le titre du cours est un titre alternatif qui arrivait après le titre principal et
  que `text_queries` (2) n'atteignait jamais (`gap_names` le fait passer devant) ; et le pack **S01** est déjà
  mappé correctement par le scene mapping TVDB (50/50 en saison 17), donc `cour_pack` l'écarte. Interrupteur
  `[tasks.series_search] cour_packs`. Le 2026-09-18, Bleach S17 est passée de 34 à **48/48** épisodes diffusés,
  sans qu'aucune autre saison ne bouge.
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
  VO/VOSTFR en dernier recours.
- **Le codec source n'est PAS un critère de charge** (mesuré le 2026-09-18, à corriger dans les têtes) : avec les
  réglages réels de Jellyfin (`superfast`, 4 threads, pas de GPU), un transcodage 1080p tourne à **1,85×
  que la source soit h264 ou HEVC** — le coût est dans l'**encodage x264**, pas dans le décodage (décodage seul :
  h264 6,25×, HEVC 6,74×). L'ancienne note « HEVC 0,72× → préférer x264 » était fausse. Sur 7 jours, les 19
  transcodages venaient **tous de sources h264**, aucun d'une source HEVC, alors que le HEVC est 53 % des 1 494
  épisodes : les clients des membres lisent le HEVC en direct. Garder `h264` comme départage en fin de tri
  (`choose`) ne coûte rien, mais **ne jamais écarter une release parce qu'elle est en x265**, surtout quand c'est
  la seule en français (cas courant des animés sur C411).
- **La vraie limite : UN SEUL transcodage 1080p à la fois** (mesuré le 2026-09-18) : 1 flux 1,42× ; **2 flux
  0,82×/0,85×** ; 3 flux 0,60×/0,69× — dès deux transcodages simultanés on passe sous le temps réel et ça
  saccade pour tout le monde. D'où l'intérêt de la lecture directe (98 %) et de `gc-quality-helper.js`.
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
  `animeStandardFormatSearch=true` restent dans les deux Sonarr pour le RSS. Films : `movie_search` cherche **dès le passage suivant** (`missing_hours = 0`, passage toutes les 5 min) — Radarr
  n'a plus de recherche depuis le 2026-09-17 et le RSS ne ramène que les nouveautés (Matrix a attendu 4 h le
  2026-09-19 avec l'ancien délai de 24 h). Filet : C411 limité à 25 requêtes/heure
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
- **Audit lecture du 2026-09-18** (v1.14.0) — les trois causes mesurées et ce qui a été fait :
  1. **Trickplay tournait 6 h chaque matin sans jamais finir** (« Cancelled after 360 minutes » à 11:30, 256 items
     sur ~1 840) et Intro Skipper jusqu'à 3 h : **506–521 Mbit/s entrants de 05 h à 08 h**, ~1,2 To/matin à
     travers un cache rclone de 20 Go → cache vidé chaque matin, tout repartait sur le réseau le soir. Trickplay
     passé **hebdomadaire (dimanche 05:30, butée 6 h)**, Intro Skipper à 05:30 (`ProcessThreads 2`,
     `MaxParallelism 1`, `ScanCommercial false`), images de chapitre 06:30 en `P480` (extraction désactivée par
     bibliothèque : tâche vide), `LibraryScanFanoutConcurrency 2`. Fenêtre en semaine : tout est fini à 08:30.
  2. **rclone** (`systemd/homelab-seedbox-mount.service`) : cache **120G** avec `--vfs-cache-min-free-space 80G`
     (rclone évince seul avant `disk_pressure hard_pct 95`), `--vfs-cache-max-age 168h`, `--vfs-read-chunk-size
     4M` (un chunk est livré entier : 8M = 0,67 s au premier Mio), `--sftp-connections 32`. `--vfs-read-ahead`
     reste absent (palier B, à mesurer) : le refus historique visait le doublement de chunk du mode
     `streams = 0`, pas cette option. **Le lien seedbox est > 500 Mbit/s entrant** (mesuré), pas ~190.
  3. **Le tmpfs `/cache/transcodes` (2 Go) se remplit de segments abandonnés** : Jellyfin ne les efface pas tous à la fin
     d'un job (28 jobs, 926 fichiers, 2 Go le 2026-09-20 à 17:55 ; les plus vieux de la veille). Plein, ffmpeg écrit des
     segments **vides** servis en 200 (`[Length 0]` dans le journal NPM) : le lecteur les télécharge à toute vitesse et
     n'affiche jamais rien (« chargement infini », un membre bloqué 4 essais ; la lecture directe passe, tout remux ou
     transcodage échoue). Diagnostic : `docker exec jellyfin df -h /cache/transcodes`. Purge horaire par timer transitoire
     `scripts/jellyfin-transcodes-purge.sh` (unités `systemd/jellyfin-transcodes-purge.{service,timer}`, **chaque
     minute**, installées par `homelabctl install`) : routine = jobs sans ffmpeg actif vieux de > 2 min (un job terminé
     n'est jamais réutilisé) ; **urgence** automatique dès 85 % (tout ce qui n'a pas de ffmpeg actif, puis les segments
     > 10 min des jobs actifs) + message Discord admin ; `--check` pour voir, `--urgent` à la main, `--force` refusé tant
     qu'un ffmpeg tourne. Mécanisme mesuré : le tmpfs se remplit **par paliers à la fin de chaque job** (~400 Mo laissés
     par un 1080p), pas au fil d'une lecture (`EnableSegmentDeletion`, 300 s gardées + 180 s d'avance = ~480 s × débit
     par job actif). **Un job actif plus gros que le tmpfs ne se purge pas** (remux 4K 60 Mbit/s ≈ 3,6 Go) : tmpfs porté
     à **4 Go** et `mem_limit 6g` (compose, appliqué hors pic le 2026-09-21 04:30). « Chargement infini » sur tout ce qui
     transcode = `--check` d'abord.
  4. **Jellyfin** : tmpfs 2 Go compté dans `mem_limit 4g` → `ThrottleDelaySeconds 180`, `SegmentKeepSeconds 300`
     (retour arrière 5 min sans relance ffmpeg ; 720 après la sortie du tmpfs, palier B) ; **`cpus: 4` retiré**
     (deux transcodages passaient à 0,82× ; `cpu_shares` arbitre) ; healthcheck explicite `start_period 180s`
     (l'image en a un à 0 : une migration de base au boot était redémarrée par `stack_health` après ~3,5 min).
  `cpu_shares` posés sur npm (2048), gluetun/jellyseerr/guacdb/guacd/guacamole (512), duckdns (256) ; homarr
  `cpus: 1`. HoverTrailer et Media Bar jouent des bandes-annonces **YouTube** (zéro CPU serveur) : seul
  `EnableThemeVideoFallback` touchait Jellyfin, mis à `false`. **Plafond à l'acquisition** : 110 Mo/min sur les
  qualités 1080p des 4 Arrs (`qualitydefinition`, ≈ 14,7 Mbit/s), `[indexers] max_gb_per_movie 15`,
  `max_gb_per_episode 3` ; pas de plafond par utilisateur (il forcerait des transcodages). Observabilité :
  Telegraf lit l'API RC rclone (`rclone_vfs`, `rclone_core`) et la tâche **`hls_loop_watch`** lit le journal
  NPM (UTC) pour signaler un client qui redemande le même segment ≥ 20 fois en 5 min (mail admin). Les
  changements qui coupent la lecture (restart du montage, recréation des conteneurs) s'appliquent **hors pic**
  par timer transitoire (`backups/playback-audit-*/apply-offpeak.sh`). Port 8096 et `PublishedServerUrl` gardés :
  compteur `iptables -nvL DOCKER | grep dpt:8096` à 0, à relire après 7 jours avant de fermer.
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
- **Langue d'affichage** : jellyfin-web la garde **dans l'appareil** (localStorage `<userId>-language` et
  `<userId>-datetimelocale`, `userSettings.language()` lit avec `enableOnServer = false`) — la copie dans les
  `DisplayPreferences` du serveur **n'est jamais lue** (vérifié le 2026-09-19 : posée à `fr` côté serveur, l'appli
  restait en anglais). Sans clé, c'est la langue de l'appareil : un membre sous Windows en anglais voyait « Home »,
  « Favorites », « Ends at 11:09 PM », alors que « Découvrir/Demandes/Calendrier » (renommés par notre CSS) et les
  titres des rangées (Home Screen Sections) restaient en français. Script **public** `branding/jellyfin/gc-lang.js`
  (« Groscailloux Langue ») : pose `fr` pour les comptes connus de l'appareil (`jellyfin_credentials`) avant le
  démarrage de l'appli, et à la première connexion pose la clé puis recharge une fois (garde `sessionStorage`).
  Une clé `language` déjà présente = choix du membre, rien n'est touché. Test : `backups/cast-tests-20260919/
  lang_test.js` (navigateur en-US, 6 cas, `NO_INJECT=1` après déploiement).
- **Saut du lecteur Jellyfin** : `skipForwardLength` / `skipBackLength` à **10 s** (`[accounts] skip_forward_ms`
  et `skip_back_ms`, posés à la création d'un compte par `jellyfin::set_skip_lengths`). Jellyfin met **30 s en
  avant** par défaut. Le bouton d'avance rapide **et** les flèches gauche/droite passent par le même réglage
  (`playbackManager.fastForward(skipForwardLength())` dans `playback-video.*.chunk.js`) : il n'y a donc qu'une
  valeur à changer. C'est un `DisplayPreferences` **par compte** (`usersettings`, client `emby`), pas un réglage
  serveur : les 16 comptes existants ont été convertis le 2026-09-18 (sauvegarde
  `backups/jellyfin-skip-20260918-155125/`), et un compte créé ensuite le reçoit à l'onboarding. Une appli déjà
  ouverte garde l'ancienne valeur en cache jusqu'au rechargement. Ni les raccourcis de Jellyfin Enhanced
  (lettres et chiffres seulement) ni nos scripts injectés ne touchent aux flèches.
- **Onboarding et mails (2026-09-20, v1.16.0)** : le mail de bienvenue finissait en **spam** — `curl smtps` sans
  `Date`/`Message-ID`, identifiant + mot de passe en clair, « Jellyseerr Groscailloux », accents retirés, liens
  duckdns. `mail.rs` construit maintenant un MIME complet (`Date`, `Message-ID`, `Reply-To`, RFC 2047, quoted-printable,
  `multipart/alternative`) ; `SMTP_FROM_NAME` = « Groscailloux ». **Jamais d'identifiant ni de mot de passe dans un
  mail** : un bouton vers `/bienvenue/<jeton>` (`homelab_core::welcome`, jeton haché SHA-256 dans
  `state.welcome_links`, `[onboard] link_ttl_mins` = 60, usage unique) où le membre choisit son mot de passe ; page
  expirée → renvoi par adresse (réponse neutre, `renew_per_hour`). Page publique `/inscription` (`public_signup`,
  honeypot, rate limit IP, `max_signups_per_day`, adresse connue → neutre) ; l'admin reçoit « nouveau compte à
  activer » et le membre « ton compte est actif » à l'activation depuis `/accounts` (colonne « Lien », bouton
  Renvoyer). **Les liens vivent dans l'état du daemon** : `homelabctl onboard|accounts link|mail-test` passent par
  `POST /onboard`, `/admin/link`, `/admin/mail-test` (jeton `HOMELABD_ONBOARD_TOKEN` en en-tête) — un lien émis par
  la CLI dans le fichier d'état serait invisible du daemon et écrasé à sa prochaine sauvegarde (vu le 2026-09-20).
  Test sans polluer : compte temporaire dont l'adresse est **celle de l'expéditeur** (`SMTP_FROM`, aucun rebond) ;
  jamais d'adresse inventée (rebonds = réputation Gmail). Jetons et mots de passe jamais journalisés.
- **Discord (2026-09-20, v1.17.0)** : deux **webhooks** dans `.env` (`DISCORD_WEBHOOK_MEMBERS` = salon des membres,
  `DISCORD_WEBHOOK_ADMIN` = salon privé, repli sur membres ; `DISCORD_ROLE_MEMBERS` facultatif), **jamais dans le
  dépôt ni les journaux** (`discord::mask`). `homelab_core::discord` poste les embeds de homelabd (annonces du tchat →
  membres ; `alerts::admin` = mail **et** Discord admin pour `hls_loop_watch`, `indexer_unblock`, inscription à
  activer, mail de bienvenue non parti, récap du tchat ; `stack_health` → Discord seulement). `homelabctl discord
  apply` configure par API les 4 Arrs (connexions « Discord membres » : Radarr `onDownload/onUpgrade`, Sonarr
  **`onImportComplete`** — un message par téléchargement, jamais `onDownload` = un par épisode ; « Discord admin » :
  santé) et l'agent Discord de Jellyseerr (types 2|8|64|128 : demande, disponible, refusée, validée d'office) ;
  `test` poste un essai ; `remove` retire tout ; sauvegardes JSON dans `backups/discord-<date>/` (contiennent
  l'URL : 600). Chemins Arr toujours préfixés `api/v3/` (la base de `ArrClient` est la racine). Un webhook collé
  dans une conversation est compromis : le recréer dans Discord (Modifier le salon → Intégrations → Webhooks) et
  relancer `apply`.
- **Abonnés (v1.18, 2026-09-20)** : `homelab_core::subscriptions` (fiches SQLite `state/subscriptions.db`, décisions pures
  `decide` testées) + `subscription_ops` (tout passage par `accounts::set_premium`) ; tâches `subscription_cycle` (1 h,
  `cycle_dry_run = true` la première semaine) et `subscription_reconcile` (24 h) ; `[subscriptions]` dans le TOML.
  PayPal : `PAYPAL_ENV` + `PAYPAL_*` (live) ou `PAYPAL_SANDBOX_*` dans `.env` — les identifiants fournis le 2026-09-20
  sont ceux du **sandbox** (application « APP-9R85… ») et ont été collés dans une conversation : à régénérer, et les
  identifiants **Live** restent à fournir ; tant que l'application est en sandbox, la page publique `/premium` garde le
  bouton Live historique (`DONATION_*`, activation manuelle) et `/premium?test=1` montre le bouton sandbox. Webhook
  `/paypal/webhook` sur l'hôte public de `/premium` (hôte NPM 20, sans liste d'accès), signature vérifiée chez PayPal,
  id dans `PAYPAL_WEBHOOK_ID` (`homelabctl subs paypal --webhook <url>` le crée). « Mon compte » = script Injector
  « Groscailloux Mon compte » → `/gc-compte/` (NPM hôte 1, base **et** `1.conf`, sauvegarde `backups/npm-20260920-gc-compte/`)
  → homelabd `/compte/`. Statut « à qualifier » = compte actif sans abonnement connu : **jamais suspendu par le cycle**,
  l'admin tranche sur `/accounts`. Aucun secret ni identifiant PayPal dans le dépôt, les journaux ou les pages.
- **Lot « lecture et suivi » (v1.19, 2026-09-21)** : `playback_canary` (15 min, transcodage réel de 2 segments, alerte
  admin au premier échec, `state.canary`) ; **langue par compte** posée à l'onboarding (`[accounts] audio_language
  = "fre"`, `subtitle_language = "fre"`, `subtitle_mode = "Smart"`, `PlayDefaultAudioTrack = false`) et rattrapée le
  21/09 sur 16 comptes (sauvegarde `backups/jellyfin-language-20260921/`), choix « VO sous-titrée » dans « Mon
  compte » ; **avancement des demandes** dans l'onglet Demandes de Jellyfin Enhanced (`/compte/api/requests`,
  `homelab_core::requests_progress`, cartes `.je-request-card` + `data-tmdb-id`) — clés d'état
  `unknown_series` = `<arr>:<seriesId>:<saison>`, `movie_search` = `<arr>:<movieId>` avec `<arr>` = `sonarr`,
  `radarr`, `sonarr-seedbox`, `radarr-seedbox`, Jellyseerr `serviceId` 0 = VPS, 1 = seedbox, `externalServiceId` =
  id Arr ; **sous-titres** = Bazarr de la seedbox (profil « Français (+anglais) », `subsync` off), rien sur le VPS.
  **Sous-titres incrustés = 2 minutes d'extraction par le lien** (2026-09-21) : Jellyfin lit tout le fichier seedbox
  pour extraire une piste incrustée (111 s pour 1,6 Go), le lecteur web abandonne (499) et le membre n'a rien. Bazarr
  (seedbox) a `use_embedded_subs = false` et extrait chaque piste française en `.fr.srt` externe sur place ; ne pas
  remettre ce réglage à `true`. La tâche **`subtitle_sync`** (5 min, historique Bazarr → `vfs/refresh` → FullRefresh de
  la fiche) est ce qui fait apparaître ces fichiers dans Jellyfin : ni l'analyse de 05 h, ni `Library/Media/Updated`
  ne voient un fichier annexe déposé à côté d'une vidéo existante. Un membre qui bascule l'audio en VO **en cours de lecture** garde la piste de
  sous-titres choisie au départ (« Forcé » en mode Smart) : c'est jellyfin-web, pas une panne ; « Toujours en VO »
  dans Mon compte sélectionne la piste complète d'office.
  **Les grabs côté seedbox ne passent jamais par la file de Sonarr/Radarr** (pas de Prowlarr là-bas : ajout direct au
  qBittorrent avec l'étiquette `homelab:`) : la barre lit aussi les torrents étiquetés des deux qBittorrent
  (`requests_progress::homelab_tag`/`from_torrent`), sinon Black Clover à 46 % s'affichait « recherche, prochaine
  tentative dans 7 j » (2026-09-21).
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
  Pas de tchat dans les applis natives (Android TV, Swiftfin), **ni sur les téléviseurs** (2026-09-19 : agent
  webOS/Tizen… ou classe `layout-tv` → `gc-chat-loader.js` n'insère pas `app.js`, et `app.js` se retire quand même
  s'il est chargé).
  **Téléviseurs** (webOS, Tizen, Android TV — détectés par l'agent, l'absence de pointeur ou ≤ 2 cœurs) :
  boucle à 3 s au lieu de 1 s, sondages espacés (10 s ouvert / 180 s fermé), ni ombre ni animation, et les
  bandeaux (annonce, message privé, aide à la qualité) se ferment à la touche **Retour** (keyCode 461 webOS,
  10009 Tizen) **et** s'effacent seuls au bout de 12 s : sans pointeur, la croix est inatteignable. Mesuré le
  2026-09-16 sur un téléviseur simulé (processeur bridé 6x) : nos scripts coûtent ~2 points de processeur sur
  l'accueil et rien de mesurable en lecture — la lenteur vient du client webOS lui-même. **Disposition TV**
  (`layout-tv`, appli webOS = client web du serveur) : jellyfin-web laisse l'en-tête **défiler hors écran** dès qu'une
  carte a le focus (page décalée de ~600 px, `.skinHeader` en `position: relative`) — au pointeur Magic Remote,
  Rechercher/Tchat/Notifications/Profil sont inaccessibles ; et à 1080p les cinq onglets chevauchaient la cloche
  NotifySync et le bouton Aléatoire de Jellyfin Enhanced. Depuis le 2026-09-19 le calque épingle l'en-tête et
  resserre onglets et boutons sur `.layout-tv` (sondes `backups/cast-tests-20260919/tv_*.js` : agent webOS 1080p,
  boîtes et chevauchements mesurés, navigation ↑ vérifiée). **Interface TV distincte (v1.15.0)** : script
  **public** `branding/jellyfin/gc-tv.js` (« Groscailloux TV », `RequiresAuthentication: false` → `public.js`,
  exécuté dès le chargement, avant la connexion et avant Media Bar) : sur agent TV il neutralise Media Bar
  (`window.slideshowPure.SlideshowManager.loadSlideshowData` et `CONFIG.enableTrailers`, objets exposés en fin de
  `slideshowpure.js`) et retire `#randomItemButton` (JE) et `.headerSyncButton` ; le bloc `.layout-tv` du calque
  cache `#slides-container`, remet `.homeSectionsContainer` à `top: 1.6em` (le calque le décale de 80vh sous le
  bandeau), cache les rangées HSS non essentielles (classe = SectionId : `gc-tendances`, `BecauseYouWatched…`,
  `gc-mieux-notes`, `Genre-…`, `gc-films-fr`, `DiscoverMovies/TV`, `MyJellyseerrRequests`, `WatchAgain`), les blocs
  secondaires des fiches (`#similarCollapsible`, genres/tags/studios, Elsewhere `.streaming-lookup-container`,
  `.audio-languages-container`) et `.gc-chat-btn`. Règle : **tout ce qui est TV est sous `.layout-tv` ou gardé par
  l'agent utilisateur — jamais par le nombre de cœurs** (un vieux portable y passerait). Vérification :
  `tv_home_lite.js` (DEVICE=tv|desktop|iphone, candidats injectés par `CANDIDATE_JS`/`CANDIDATE_CSS`) — bureau et
  iPhone doivent être identiques avant/après. Media Bar charge encore `youtube.com/iframe_api` au chargement (avant
  tout script injecté), sans lecteur : négligeable. Les pages « Demandes » et fiches d'un **admin** déclenchent
  des rafales Jellyfin Enhanced (`arr/series-slugs` par carte, réservé aux admins : 249 requêtes/min vues depuis la
  TV) ; `JellyseerrShowNetworkDiscovery` coupé le 2026-09-19 (928 réponses 503/jour : il exige une clé TMDB dans le
  plugin, absente). **Jamais de `window.confirm/alert/prompt`** dans
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
  (JavaScript Injector, déployé avec `scripts/jellyfin-js-apply.py`) : **seulement ses propres appareils
  connectés**, quel que soit le réseau. Jusqu'au 2026-09-19 il exigeait aussi la même adresse publique que
  l'appareil courant ; or un iPhone derrière le **Relais privé iCloud** (ou un VPN) joint le serveur par deux
  adresses à la fois — la box (86.194.x) et un relais DataPacket (79.127.x), à 7 s d'écart dans le journal NPM —
  et sa propre TV disparaissait du menu (vu sur le compte admin). Condition réseau retirée, garde-fou par compte
  conservé et testé (`node` sur la liste réelle de `/Sessions`). Le menu ne peut lister qu'un appareil **ouvert
  et connecté** avec le même compte : Desktop fermé ou TV éteinte = liste vide, ce n'est pas une panne. Jellyfin
  garde la liste des cibles en mémoire pendant la vie de la page : le filtre doit être chargé avant la première
  ouverture du menu. **Google Cast n'existe que dans Chrome et l'appli Android** ; ailleurs jellyfin-web met une
  note « (Google Cast non pris en charge) » sous le titre — un `<p class="actionSheetText">`, pas une entrée, la
  liste étant vide (vérifié en test). `branding/jellyfin/gc-airplay.js` (« Groscailloux AirPlay », v1.14.2) la
  remplace sur iPhone/iPad/Mac par une entrée **AirPlay** (v2 le 2026-09-19 : vidéo en cours →
  `webkitShowPlaybackTargetPicker()` dans le clic ; sinon la feuille est fermée, le bouton Lire **natif** de la
  fiche est cliqué et le sélecteur est tenté dès que la vidéo est prête ; le rappel « icône AirPlay du lecteur »
  n'est affiché que si le sélecteur refuse), la retire ailleurs, et écrit « Aucun autre appareil connecté avec ce
  compte » quand la liste est vide. **Le bouton « Lire » du bandeau Media Bar (`.slide .btnPlay`) est inopérant
  dans l'appli iPhone** : il fait `POST /Sessions/{id}/Playing` vers sa propre session (v3, 2026-09-19, journal
  client « video: none within 8000ms ») — depuis l'accueil, le script clique le bouton « Détails » de la
  diapositive active (`#slides-container .slide.active[data-item-id] .detail-button`), attend le `.btnPlay` de
  la fiche, puis lance ; sans titre affiché, il ferme la feuille et l'explique en une ligne. Les dialogues jellyfin-web 10.11 se ferment **uniquement** par
  `history.back()` (entrée `history.state.usr.dialogs[]`) : clic synthétique sur le fond et Escape n'ont aucun
  effet ; sans entrée d'historique (WebView), le script retire la feuille lui-même. Chaque appui envoie ses étapes
  à `ClientLog/Document` (`jellyfin/config/log/upload_*.log`, lignes « gc-airplay … ») : lire ce journal avant
  toute hypothèse sur ce que fait l'appli iPhone. Une version déployée est **remplacée** par la suivante
  (`VERSION`, override de `__gcAirPlayDone`) : le lot `private.js` peut porter les deux pendant un test. Test :
  `backups/cast-tests-20260919/airplay_test.js` (puppeteer, compte temporaire, script candidat injecté par
  interception de `/JavaScriptInjector/private.js` ; UA iPhone + simulateur du sélecteur, 22 cas dont accueil, fiche et page sans titre ; relance avec
  `NO_INJECT=1` après déploiement).
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
- **Codec : HEVC et H.264 sont à égalité** (2026-09-18). Retiré du classement de `choose` et de
  `best_movie_release`, et les formats personnalisés `HEVC 10-bit`, `HEVC 8-bit`, `H.264` et `AV1` sont à **0**
  dans les 4 profils FR-friendly (sauvegarde `backups/arr-codec-neutral-20260918-104404/`). Motif : mesures du
  jour — h264 et HEVC transcodent tous deux à 1,85×, le coût est l'encodage x264 et pas le décodage, et les 19
  transcodages de la semaine venaient tous de sources h264 alors que le HEVC est 53 % de la médiathèque.
  Pénaliser le x265 revenait à refuser la seule version française disponible (cas courant des animés sur C411).
  Le nom « FR-friendly H.264 » des profils est resté, il ne décrit plus le codec.
- **Langue des ANIMÉS** (2026-09-18, demandé par l'utilisateur) : **MULTi 4 > VOSTFR 3 > VF 2 > FRENCH 1 >
  VO 0** (`lang_rank_for(title, anime)`, `seriesType == "anime"`). Un MULTi porte les deux pistes audio ; la
  VOSTFR garde l'audio japonais. Un « MULTI.VFF » compte comme MULTi pour un animé, comme VF pour le reste
  (comportement d'origine préservé). Même ordre côté Sonarr : le profil **« Anime - MULTi/VOSTFR »** (VPS 7,
  seedbox 8) a été réparé — il rejetait tout HEVC 10 bits et tout doublage français (`minFormatScore = 0` avec
  `HEVC 10-bit -10000` et `FRENCH -500`) — puis aligné (MULTi 3000, VOSTFR 2000, VFF 1000, FRENCH 500, sans
  marqueur français −2000, codecs à 0, `minFormatScore = -9999`, mêmes qualités ≤ 1080p que FR-friendly). Les
  **26 fiches animées** (20 seedbox + 6 VPS) y sont passées, sans perte (429 et 119 fichiers avant comme après).
  Sauvegarde `backups/arr-anime-profile-20260918-142927/`. **Ne pas oublier `activeAnimeProfileId` dans
  Jellyseerr** (VPS 7, seedbox 8, sauvegarde `backups/jellyseerr-sonarr-20260918/before-anime-profile.json`) :
  basculer les fiches existantes ne suffit pas, les **nouvelles demandes** arrivaient encore sur FR-friendly
  (Frieren, le 2026-09-18). Jellyseerr a un profil séparé pour les animés, repéré par `animeTags`.
- **Nommage des fansubs** : `Erased S01 - 06 VOSTFR [1080p][X265].mkv` — Sonarr lit `S01` comme une **saison
  entière**, ne voit jamais le « - 06 », refuse chaque fichier (« Single episode file contains all episodes in
  seasons ») **et**, si on ignore ce rejet, propose les 12 épisodes pour le premier fichier. Le 2026-09-18, les
  2,11 Gio d'Erased sont restés complets et non importés, en état **définitif** (`nothing_importable`), invisibles.
  Corrigé : `series_search::fansub_episode` lit ce numéro, le rejet passe dans `is_identification_rejection`, et
  dès qu'un fichier est lu ainsi le torrent bascule en `EpisodeSource::OursOnly` — **sinon Sonarr gagne et un
  seul fichier est rattaché aux 12 épisodes** (vu en production avant le correctif). `no_match` **et**
  `nothing_importable` remontent maintenant dans « Rien ne bouge » sur `/status.html`.
- **Langue** : `lang_rank` classe VF 4 > MULTi 3 > FRENCH 2 > VOSTFR 1 > **VO 0** ; une release sans français
  n'est prise qu'en dernier recours (`[indexers] allow_no_french`), quand aucune française n'est acceptable.
  Plafonds de taille du choix automatique : `max_gb_per_episode` (6) et `max_gb_per_movie` (25) — sinon un pack
  de 134 Go à une seule source peut gagner contre un 27,8 Go bien partagé. Un refus « blocked till … » compte
  comme une **erreur** (nouvelle tentative dans l'heure), plus comme « aucun candidat » (24 h).
- **Titre supprimé puis redemandé : le torrent est réutilisé, pas retéléchargé** (2026-09-18). `deletion_cleanup`
  garde les torrents en partage (C411 : ratio 1 ou 7 j) alors que les fichiers médias, eux, sont supprimés.
  Redemandé, `torrents/add` répondait « Fails. » (déjà présent) et **rien ne s'importait** : Bleach S17, 0/20
  épisodes pris. `series_search` cherche maintenant l'`infoHash` de la release (fourni par Prowlarr) parmi les
  torrents du qBittorrent concerné ; s'il est là et complet, il le **réétiquette** vers la nouvelle fiche
  (`qbit::retag`) et efface son enregistrement `torrent_import` (sinon `is_candidate` le saute, il est marqué
  `imported`). L'import repart en lien physique en quelques minutes, sans un octet réseau.
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
- **Recréer `gluetun` = recréer `qbittorrent`** : qBittorrent est en `network_mode: service:gluetun` ; quand gluetun
  est recréé (changement de compose, `cpu_shares`…), compose laisse qbittorrent « Up » **sur l'espace réseau de
  l'ancien conteneur** : injoignable de partout (Homarr, NPM, homelabd `localhost:8080`), alors que son healthcheck
  interne reste vert. Le 2026-09-19, l'application hors pic de 04:30 a coupé qBittorrent du VPS 11 h (tracker_ratio,
  torrent_import côté VPS, tuile Homarr rouge). Toujours `docker compose up -d --force-recreate --no-deps
  qbittorrent` après un `up -d` qui a recréé gluetun, puis reposer le port transféré (`/tmp/gluetun/forwarded_port` →
  `setPreferences listen_port`, le hook ne rejoue pas seul).
- **Sauvegarde d'état (`homelabctl backup`, tâche `backup`)** : tout dossier volumineux sous `/opt/homelab` doit être
  dans `[backup] excludes` — le 2026-09-20, `cache/rclone` (fichiers **creux** de plusieurs centaines de Go) a produit
  une archive de **149 Go** (au lieu de 3) et poussé le disque à 92 %. Après un nouveau dossier de cache ou de données
  massives : l'ajouter aux exclusions, puis contrôler la taille de l'archive suivante et son manifeste (`.list.gz`).
- **Reboot** : `homelab-stack.service` relance compose ; vérifier `docker compose ps` et
  `systemctl status homelabd` après.
- `scripts/` ne contient plus que des outils ponctuels (les anciens scripts bash planifiés ont été
  retirés) : `jellyfin-branding-apply.sh`, `jellyfin-ui-rollback.sh` (voir « Interface Jellyfin »),
  `jellyseerr-rotate-key.py` (voir « Pièges connus »), `jellyfin-js-apply.py` (scripts JavaScript Injector, dont
  « Groscailloux Mon compte »),
  `move-to-seedbox.py` et `seedbox-cleanup.py` (voir « Seedbox »).

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
  avec la liste complète des bibliothèques. **Le `GET settings/jellyfin/library` sans paramètre désactive lui aussi
  tout** (vérifié le 2026-09-20 dans `settings.json`) : lire l'état par `GET /api/v1/settings/jellyfin`
  (champ `libraries`), et n'appeler `?enable=<4 ids>` qu'en écriture. Un `?sync=true` peut rester bloqué plusieurs
  minutes : `--max-time 60`.
- **Déménager un titre du VPS vers la seedbox** : `scripts/move-to-seedbox.py` (`--list` numérote, `--titles a-b`,
  `--worker i/n` pour paralléliser, `--dry-run`), ordre immuable : rsync (`-a --partial`, ssh admin, `nice`/`ionice`)
  → vérification nom+taille de chaque fichier → fiche de l'Arr seedbox **créée non surveillée** (ou fiche existante,
  ex. Law & Order S10–13 ajoutées à côté de S1–17) → `RescanSeries`/`RescanMovie`, contrôle du nombre de fichiers,
  puis surveillance de ce qui a un fichier → **seulement alors** suppression VPS (fiche + fichiers, torrents liés
  s'ils ont fini de partager depuis 7 j) → `Library/Media/Updated` Deleted/Created. Pièges : rsync ≥ 3.2.4 protège
  lui-même le chemin distant (**pas de guillemets** : `seedbox:/home/x/y z`, sinon `mkdir ".../'/home/…'"`) ; le
  montage rclone **ne voit pas un nouveau dossier** tant que le **dossier parent** n'a pas été rafraîchi
  (`vfs/refresh dir=Movies` puis `dir=Movies/<titre>`) ; `deletion_cleanup` dans `tasks.disabled` pendant toute
  l'opération (une fiche seedbox fraîche dont le montage ne voit pas encore les fichiers serait « sans fichier ») ;
  jamais pendant une lecture du titre (`/Sessions`). Jellyfin recrée l'item (nouvel id) : l'état « vu » suit les
  identifiants TMDB/TVDB. Débit mesuré : ~17 Mo/s par flux, ~25 Mo/s à deux. Deux workers en parallèle ont mis un
  Sonarr seedbox en « database is locked » (500) : le script réessaie. **Hors pic seulement, un flux, `--bwlimit 15000`**
  (timer `move-to-seedbox-offpeak`, 08:30–12:30, `RuntimeMaxSec=4h`) : le 2026-09-20 à 17:28, avec deux rsync à
  20 Mo/s, un membre n'a pas pu lire un film de la seedbox (4 essais, segments HLS servis puis requête suivante jamais
  aboutie, cache rclone figé une minute). Mesuré après arrêt : **le VPS reçoit ~8–10 Mo/s par connexion** quelle que
  soit la source (OVH 8,6 Mo/s, seedbox 7–10 Mo/s, RTT seedbox 97 ms), l'agrégat monte avec le nombre de flux (4 ssh =
  30 Mo/s ; les « 500 Mbit/s » du 18/09 étaient l'agrégat trickplay). Une lecture = un flux ≈ 65 Mbit/s : assez, mais
  sans marge pour un à-coup ; l'hôte seedbox est partagé (charge 45–60, 128 cœurs). Le transfert reprend seul le
  lendemain 08:30 sur ce qui reste (`state-0.json`), `deletion_cleanup` reste coupée jusqu'à la fin.
- **Ménage de la seedbox** (`scripts/seedbox-cleanup.py`, 2026-09-20) : les torrents **sans catégorie** (ajoutés à la
  main les 11–12/09 : ISO, logiciels, musique, PDF, sport, docs) sont repérés par inode — un torrent dont **aucun**
  fichier n'est relié à `media/` (hors `.recycle`) est retiré avec ses fichiers ; un torrent partiellement relié est
  laissé ; jamais un torrent de catégorie `sonarr`/`radarr`. Règle C411 : fini depuis < 7 j et ratio < 1 = **reporté**
  (`deferred.json`, timer transitoire `seedbox-cleanup-deferred` quotidien à 13:05). Les corbeilles Arr (`.recycle`,
  purge 14 j) peuvent contenir des fichiers **partagés par inode avec la médiathèque** (Attack on Titan, 112 Go
  affichés, 0 libéré) : toujours mesurer « exclusif / partagé » avant d'annoncer un gain. `du -sh ~` sur la seedbox
  renvoie 0 (`~` est un lien symbolique) : `du -sh ~/`.

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
- **Deux sessions dans le dépôt** : le binaire installé doit être construit depuis **l'arbre de travail tel quel**
  (`cargo build … -j4` dans `/opt/homelab`), jamais depuis un arbre indexé/worktree qui exclut les fichiers non
  validés d'une autre session — le 2026-09-19, cinq installs ainsi construits ont retiré les routes `/premium`
  (pages non validées d'une autre session) du binaire en service, 404 sur le lien public jusqu'à ce que
  l'utilisateur le remarque. Le worktree ne sert qu'à `fmt/clippy/test` de ce qu'on committe. Après un restart,
  `git status --short` puis `curl 127.0.0.1:8766/<route de l'autre session>`.
- **homelabd est cloisonné** (`ProtectSystem=strict`) : tout nouveau dossier écrit par une tâche va dans
  `ReadWritePaths` de `systemd/homelabd.service` (sinon « Read-only file system », vu le 2026-09-17).
- Jellyfin 10.11 : une bibliothèque supprimée (API ou UI) reste dans les vues des utilisateurs,
  même après un scan global, jusqu'au redémarrage de Jellyfin (`docker compose restart jellyfin`). Après le
  redémarrage, les membres ne la voient plus, mais ses `CollectionFolder` restent en base (visibles des comptes
  `EnableAllFolders`, admins compris) : il faut **une analyse complète** (qui les retire de la base, 131 s le 2026-09-20)
  **puis un second redémarrage** (les vues sont en mémoire), et Jellyseerr ne les oublie qu'après son propre `?sync=true`
  (+ `?enable=`). Ordre complet : supprimer la bibliothèque → analyse → redémarrage → Jellyseerr sync + enable.
- **Comptes « toutes les bibliothèques »** : seuls les admins protégés doivent avoir `EnableAllFolders = true` ; un membre
  ordinaire a la liste explicite des 5 bibliothèques (Films, Séries, Anime, Films d'animation, Collections), pas de TV en
  direct, pas de téléchargement, verrouillage après 5 échecs (modèle `non_admin_policy`). Ardus et caca, créés avant
  l'onboarding v2, ont été réalignés le 2026-09-20 (sauvegarde `backups/jellyfin-ui-20260920-mymedia/*-before.json`).
- **Rangée « Mes médias » en tête** (Home Screen Sections `MyMedia`, `OrderIndex 0` depuis le 2026-09-20, demandé par
  l'admin : jusque-là 17ᵉ et dernière, chargée au défilement, invisible sur téléphone sans tout faire défiler). L'ordre
  du plugin est **global** (identique pour les 17 comptes, vérifié par `GET /HomeScreen/Sections?userId=`) ; il se
  modifie par `POST /Plugins/b8298e012697407ab44daa8dc795e850/Configuration` (sans redémarrage). **Contrôler comme
  l'appli** : `GET /HomeScreen/Sections?UserId=…&Language=fr&Page=1&NumResultsPerPage=4&PageHash=<uuid v4>` — l'appel
  sans pagination est mis en cache **24 h par compte** (`CacheTimeoutSeconds`) et montre l'ancien ordre après un
  changement, alors que les applis (nouveau `PageHash` à chaque accueil) voient le nouveau tout de suite. Sauvegarde
  `backups/jellyfin-ui-20260920-mymedia/`.
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
  **Widget « Téléchargements »** : `limitPerIntegration` est appliqué **côté serveur, avant** le filtre « masquer
  les terminés » du client — avec 10 et 315 torrents finis sur la seedbox, les 10 envoyés étaient tous terminés
  et le widget restait vide malgré des téléchargements en cours (2026-09-19). Passé à 500 (options de l'item
  `83gkiwxp5m1hwbu9iymgj53g`, sauvegarde `backups/homarr-db-20260919-165303-downloads-limit.sqlite`).
- L'UI d'onboarding est sur l'hôte (8766) ; NPM doit cibler `172.18.0.1:8766`, pas un conteneur.
