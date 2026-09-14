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
- **Indexers** : C411 seul en automatique (RSS + auto) sur les 4 Arrs, les autres en interactif
  seulement. **Profils** FR-friendly : 1080p max (jamais 2160p), `minFormatScore=-9999`, FR d'abord,
  VO/VOSTFR en dernier recours. **Jellyfin** : pas de GPU, préférer x264 à HEVC.
- **C411** : `animeCategories=[5070]` et `animeStandardFormatSearch=true` dans les deux Sonarr, sinon
  une série de type « anime » n'interroge jamais C411 (et, C411 étant le seul indexer en auto, rien ne part).
  Son API limite le débit : un **429** met C411 en pause **1 h** dans l'Arr (« API Request Limit reached »).
  Éviter les rafales de recherches interactives (une recherche anime = une requête par épisode).
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
  (`SaveLocalMetadata`, `SaveSubtitlesWithMedia`, trickplay avec le média : tous à `false`).
  Une suppression dans Jellyfin est suivie par `deletion_cleanup` (fiche Arr, Jellyseerr, torrent).
- **Comptes** : premium = compte Jellyfin actif, non-premium = `IsDisabled` ; passer par
  `homelab_core::accounts` (page `/accounts`, `homelabctl accounts`) qui garde les permissions Jellyseerr.
  `accounts.protected` (Haradas, LeGrosCailloux) : jamais suspendus ni supprimés par la page ; les autres
  admins sont gérés comme tout le monde. Plafonds dans `[accounts]` (25 premium, 2 lectures par
  compte) ; pas de `RemoteClientBitrateLimit` (forcerait des transcodages).
- **Historique Arr** : `GET history?movieId=` / `?seriesId=` n'existe pas, le filtre est ignoré et tout
  l'historique revient. Utiliser `history/movie?movieId=` et `history/series?seriesId=` (seul `downloadId`
  filtre vraiment `GET history`).
- **Profils compose** : `COMPOSE_PROFILES=vpn|novpn` dans `.env`, changé uniquement par
  `homelabctl vpn`. `gluetun`+`qbittorrent` et `qbittorrent-direct` ne coexistent jamais.
- **Nouvelle tâche** : un module dans `crates/homelab-core/src/tasks/`, `impl Task`, ajout dans
  `registry()`, section `[tasks.<nom>]` dans `config.rs` + `homelab.toml`, dry-run respecté,
  tests unitaires de la décision, paragraphe dans AUTOMATION.md.
- **Changement de comportement** = changement de `homelab.toml` (seuils, intervalles) avant
  changement de code. Les valeurs par défaut du code doivent rester égales à celles du TOML.
- **Reboot** : `homelab-stack.service` relance compose ; vérifier `docker compose ps` et
  `systemctl status homelabd` après.
- `scripts/` ne contient plus que des outils ponctuels (les anciens scripts bash planifiés ont été
  retirés) : `jellyfin-branding-apply.sh` et `jellyfin-ui-rollback.sh` (voir « Interface Jellyfin »).

## Interface Jellyfin (« Groscailloux TV », 2026-09-14)

- **Thème** : ElegantFin **épinglé** + calque maison, source `branding/jellyfin/groscailloux-tv.css`, appliqué par
  `scripts/jellyfin-branding-apply.sh` (sauvegarde l'ancien, refuse tout `@import` en `@main/@master/@latest`).
  Ne pas éditer le CSS dans l'interface. Monter ElegantFin = changer le tag, repasser le banc d'essai
  (captures bureau + téléphone), puis appliquer. Jellyfin 10.11 lit ce CSS dans `Branding/Configuration`
  (champ `CustomCss`), plus `Branding/Css`.
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
  ElegantFin).
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

- `.env` est lu par bash (`.` ), compose et dotenvy : pas d'expression shell, guillemets seulement
  autour des valeurs avec espaces.
- Le hook `hooks/qbit-update-port.sh` s'exécute dans l'image gluetun (busybox) : POSIX sh,
  `wget` uniquement.
- `GET /api/v3/manualimport` de Sonarr dure ~25 s sur un gros `/downloads` (timeout 5 min).
  Avec `folder=` un fichier **situé dans le dossier d'une série**, Sonarr renvoie tous les fichiers de
  la saison : toujours choisir le candidat par **chemin exact**, jamais le premier (le 12/09, S17E41 a
  été rattaché à E48 par erreur, corrigé en réimportant chaque fichier vers son épisode).
- Prowlarr n'a aucune application configurée : les indexers vivent dans Sonarr/Radarr et les
  publics passent par **Jackett** (+ FlareSolverr pour Cloudflare). Avant de retirer un service,
  vérifier qui l'appelle : `grep -r <nom>:<port>` dans les configs et les champs `baseUrl` des
  indexers Arr (`GET /api/v3/indexer`) — le retrait de Jackett/FlareSolverr le 2026-09-10 a coupé
  les indexers publics pendant deux jours.
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
