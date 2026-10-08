# CLAUDE.md

Guide pour Claude Code dans ce dépôt : ce qu'il est, les commandes, les interdits, les valeurs en vigueur et où lire
la suite. Le détail (procédures, pourquoi, historique) est dans [docs/runbooks/](docs/runbooks/README.md) : **avant de
toucher un domaine, lire son runbook** (§ 5).

## 1. Ce qu'est ce dépôt

`/opt/homelab` est **à la fois** le dépôt git (branche `main`, remote `HaradasCYB/groscailloux-homelab`, **public**) et
le répertoire de production : compose, configuration (`homelab.toml`) et code Rust sont versionnés ; l'état des services
(`<service>/`), `library/`, `backups/`, `state/`, `logs/` et `.env` sont ignorés par git. « Déployer » = `docker compose up
-d` pour les conteneurs, `cargo build` + `install` + `systemctl restart homelabd` pour l'automatisation (`sudo ./setup.sh`
fait tout). Deux machines : le **VPS** (lecture, recherche, automatisation, bibliothèque historique) et la **seedbox**
(tous les téléchargements, montée sur le VPS par rclone).

| Document | Contenu |
| --- | --- |
| [docs/INFRA.md](docs/INFRA.md) | vue d'ensemble avec schémas : machines, parcours d'une demande, stockage, résilience |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | services, flux, réseau, sécurité des accès, homelabd en bref |
| [docs/AUTOMATION.md](docs/AUTOMATION.md) | chaque tâche de homelabd, ses états, ses réglages et ses garde-fous |
| [docs/DEPLOY.md](docs/DEPLOY.md) | hôte neuf, mise à jour, VPN, seedbox, sauvegarde et restauration, démarrage |
| [docs/SECRETS.md](docs/SECRETS.md) | chaque variable de `.env` (noms seulement) et leur rotation |
| [docs/ONBOARDING.md](docs/ONBOARDING.md) | arrivée d'un membre, côté admin |
| [docs/JELLYFIN-12.md](docs/JELLYFIN-12.md) | migration 10.11 → 12.1 et retour arrière |
| [docs/runbooks/](docs/runbooks/README.md) | un runbook par domaine + historique des incidents + table « ancien CLAUDE.md → nouvel emplacement » |
| [tools/README.md](tools/README.md) | outils versionnés : hors pic (`offpeak.sh`), bancs (`bench.sh`), tests |
| [CHANGELOG.md](CHANGELOG.md) | versions livrées |

## 2. Commandes

```bash
docker compose config --quiet             # TOUJOURS avant un up -d
docker compose up -d [svc]                # recrée seulement ce qui a changé
docker compose ps ; docker compose logs -f <svc>

cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test
cargo build --release --target x86_64-unknown-linux-musl -j4      # dans /opt/homelab ; laisser 2 vCPU à Jellyfin
sudo install target/x86_64-unknown-linux-musl/release/homelab{d,ctl} /usr/local/bin/ && sudo systemctl restart homelabd

homelabctl check | list | status | run <tâche> [--dry-run] | onboard | accounts | subs | vpn | backup | chat announce <fichier>
journalctl -u homelabd -f                 # passages calmes en debug : RUST_LOG=debug pour tout voir
curl -s 127.0.0.1:8766/health             # battement de l'ordonnanceur + version (git describe)

tools/offpeak/offpeak.sh … --dry-run      # seul outil « hors pic, si personne ne regarde » (voir tools/README.md)
tools/bench/bench.sh <scénario> <appareils>   # seul lanceur de banc d'interface (comptes zz_ toujours supprimés)
```

## 3. Interdits et règles dures

### Secrets et dépôt public
- Secrets **uniquement dans `.env`** : jamais en dur dans compose, TOML, code, scripts ou docs ; les scripts de `scripts/` sourcent `.env`.
- **N'afficher aucun secret** : ni `.env`, ni configuration de service, ni URL avec jeton, ni réponse `settings/main` de Jellyseerr (elle contient la clé), ni colonne `password` de NPM ou Homarr ; masquer les chaînes hexadécimales longues.
- Ne jamais coller `.env`, `backups/` ou une configuration de service dans un outil externe ; un secret collé dans une conversation est compromis : le régénérer.
- **Dépôt public** : aucun pseudo de membre, IP, domaine ni e-mail (écrire « un membre », « l'admin », « l'adresse publique de la seedbox (`SEEDBOX_PUBLIC_URL` de `.env`) »).
- Adresse, compte et dossier de la seedbox : `.env` (`SEEDBOX_PUBLIC_URL`, `SEEDBOX_HOME`, `SEEDBOX_USER`, `SEEDBOX_SFTP_HOST`), cités `${…}` dans `homelab.toml` ; domaine d'onboarding : `ONBOARD_PUBLIC_URL`. Aucune valeur de `.env` compilée dans un binaire (les releases sont publiques).
- **Jamais de `token=`** dans un lien, une redirection ou un mail ; **jamais d'identifiant ni de mot de passe dans un mail** ; jetons et mots de passe jamais journalisés.
- Erreur journalisée = `format!("{e:#}")`, et `.map_err(reqwest::Error::without_url)` d'abord si l'URL porte un secret (webhook, `apikey=`, `ApiKey=`).
- `backups/` : rien de lisible par « autres » ; après un lot, `sudo find /opt/homelab/backups -perm -o+r ! -type l -exec chmod o-rwx {} +` ; dossier à secrets en 700/600.
- Une seule clé API Jellyfin depuis le 07/10 (« Jellyseerr » = `JELLYFIN_API_KEY` ; « claude-setup » révoquée) : toute nouvelle clé se note dans SECRETS.md.

### Données et suppressions
- **Jamais de purge globale** de file ou de torrents : suppression ciblée et plafonnée (`max_actions_per_run`).
- **Jamais de DELETE en boucle non vérifié** : `GET /Devices?userId=` ignore le filtre (tous les membres déconnectés le 15/09) ; vérifier le nombre et `LastUserId` de chaque élément.
- `DELETE /Items/<id>` de Jellyfin **efface le disque** : jamais pour « nettoyer » une vue ou une bibliothèque.
- `deletion_cleanup` ne touche que les médias Jellyseerr « en attente » ou « en cours » sans demande, jamais « disponible » ou « partiel ».
- Déplacement ou déménagement de titres en masse hors de `anime_library` : `deletion_cleanup` dans `tasks.disabled` pendant toute l'opération.
- Import d'un téléchargement non demandé par l'Arr : `ManualImport` en `importMode: copy`, **jamais `auto`** ; candidat choisi par **chemin exact** ; `torrent_import` ne remplace jamais un fichier et n'importe jamais un fichier incomplet.
- **`homelabctl` n'écrit jamais l'état** ; toute modification manuelle de `state/homelabd.json` se fait **daemon arrêté**.
- Mesurer « exclusif / partagé » (inodes) avant d'annoncer un gain d'espace.

### Docker, VPN, système
- `docker compose config --quiet` avant tout `up -d`.
- Images épinglées **`tag@sha256`**, pas de `:latest` nu, et `diun/images.yml` en cohérence (un bloc `- name:` par image).
- **Pas de `chown -R /opt/homelab`** (npm/ et homarr/ root, grafana/ 472, guacamole/mysql 999).
- **`qBittorrent.conf` : conteneur arrêté avant d'éditer**, sinon il écrase le fichier.
- Jamais `172.18.0.0/16` dans `bypass_auth_subnet_whitelist` de qBittorrent (seulement `127.0.0.0/8` et `172.18.0.1/32`) ; garder `web_ui_reverse_proxy_enabled`.
- **Recréer `gluetun` = recréer `qbittorrent`** (`up -d --force-recreate --no-deps qbittorrent`) puis reposer le port transféré.
- `COMPOSE_PROFILES` changé seulement par `homelabctl vpn` ; jamais les deux profils (vpn, novpn) en même temps.
- Nouveau service : port publié en `"127.0.0.1:<port>:<port>"` et `cpu_shares: 512`.
- Arrêter un service volontairement = l'ajouter à `[tasks.stack_health] ignore` (+ restart homelabd), sinon il est relancé sous 5 min.
- **SSH du VPS (sshd, clés, mots de passe, fail2ban), port 81 de NPM et listes d'accès NPM : on n'y touche pas** (signaler seulement).
- **Redémarrages (VPS, Jellyfin, montage rclone, conteneurs qui coupent la lecture) : hors pic et sans lecture en cours**, par `tools/offpeak/offpeak.sh`.
- Scripts : `head -n N` et `tail -n N` (coreutils uutils : `tail -25 a b` échoue) ; `.env` sans expression shell ; hook gluetun en POSIX sh avec `wget`.
- Tout dossier volumineux sous `/opt/homelab` → `[backup] excludes` ; toute nouvelle base SQLite d'un service → `[backup] sqlite`.
- Instance d'essai jamais laissée en marche après son banc, et sa copie de base supprimée avec elle.

### homelabd (code, configuration, installation)
- Changement de comportement = **d'abord `homelab.toml`** ; une valeur réglable ne s'écrit que là, la doc cite la clé.
- **`deny_unknown_fields`** : une clé ajoutée à `homelab.toml` impose d'installer **homelabd ET homelabctl** dans la foulée ; ne pas fusionner dans `main` une clé nouvelle avant la fenêtre d'installation.
- **Installer depuis l'arbre de travail de `/opt/homelab`, jamais depuis un worktree** ; une cible cargo (`CARGO_TARGET_DIR`) par worktree.
- Après un redémarrage de homelabd : `git status --short`, `curl` d'une route d'une autre session (ex. `/premium`), `homelabctl check`.
- Nouvelle tâche : module dans `tasks/`, `registry()`, `[tasks.<nom>]` dans `config.rs` + `homelab.toml`, dry-run, tests de la décision, paragraphe dans AUTOMATION.md.
- homelabd est cloisonné (`ProtectSystem=strict`) : tout nouveau dossier écrit va dans `ReadWritePaths` de `systemd/homelabd.service`.
- Un GET **à effet** (recherche ou téléchargement Prowlarr, `release` d'un Arr, canari) n'est jamais rejoué : `.send()`, pas `send_retry`.
- Une alerte n'est notée « signalée » que si elle est partie (`alerts::retry_later`) ; jamais de seuil `/health` par tâche.

### Arrs, indexeurs, Jellyseerr
- **Aucune recherche depuis Sonarr/Radarr** (C411 en RSS seulement) : chercher par `/recherche` ; ne pas remettre la recherche à la demande dans Jellyseerr (`preventSearch`).
- **Viser un profil de qualité par son NOM, jamais par son numéro** (VPS 6 = seedbox 7 = FR-friendly H.264).
- Tout ce qui est neuf passe par la seedbox : remettre le VPS en service exige `auto_sides` **et** le RSS de l'indexeur **et** `rssSyncInterval`.
- Ne jamais écarter une release parce qu'elle est en x265 ; garder le garde-fou d'égalité de saison de `series_candidate`.
- Nouveau marqueur de langue = code (`langs_of`) **et** formats des 4 Arrs ; changer le profil anime = aussi `activeAnimeProfileId` dans Jellyseerr.
- Toute règle par tracker vise les deux domaines de C411 (`c411.org` et `tk.c411.tw`).
- Prowlarr sans application liée ; avant de retirer un service, chercher qui l'appelle (`grep -r <nom>:<port>`, `baseUrl` des indexeurs).
- **Jellyseerr (Seerr 3.2) : `GET settings/jellyfin/library` sans paramètre et `?sync=true` sans `?enable=` désactivent toutes les bibliothèques** ; lire par `GET /api/v1/settings/jellyfin`.
- Historique d'un titre : `history/movie?movieId=` et `history/series?seriesId=` (`history?movieId=` ignore le filtre).
- Voie russe = choix du membre, jamais la langue TMDB ; ne pas contourner l'arrêt des torrents publics par l'hébergeur de la seedbox.
- Flux C411 d'autobrr désactivé : le réactiver remet le bruit d'origine (décision du propriétaire).

### Jellyfin
- **Aucune tâche lourde entre 13 h et 05 h** ; trickplay jamais pendant une analyse ; **aucune option qui lit la vidéo à l'ajout** d'un titre ni qui écrit dans les dossiers médias.
- **Pas de `Refresh` récursif d'une série** pour une image ou par commodité (il relit les épisodes par le lien seedbox) : seulement dans les recettes d'identification et de remplacement, hors pic.
- **Intro Skipper : interdits en production** « Exécuter » la tâche, `POST /Intros/ScanSeason`, `DELETE /Intros/Show/…`, `POST /Intros/ExcludedTimestamps/Clear`, EraseTimestamps, `POST /Intros/AnalyzerActions/UpdateSeason` ; jamais rallumer `ScanRecap` ; refaire le gel avant toute mise à jour du plugin.
- **Mises à jour d'extensions : manuelles**, en vérifiant `targetAbi` (le catalogue garde une compilation 10.11 du même numéro) ; JavaScript Injector ne purge jamais le script d'une extension retirée.
- Jamais d'analyse complète lancée par-dessus une autre (elles s'annulent) ; `GET /Items` toujours avec `UserId`.
- Jamais d'édition à la main de `system.xml` Jellyfin démarré ; garder `EnableLegacyAuthorization = true`, `EnableEmbeddedTitles = false`, `MaxActiveSessions = 0`, aucun `RemoteClientBitrateLimit`.
- Playback Reporting : `MaxDataAge = -1` dans la configuration **nommée** (`0` effacerait tout).
- `EnableAllFolders` seulement pour les deux comptes protégés.
- Toute nouvelle extension qui ouvre une websocket reproduit le piège corrigé par `gc-socket.js` (une seule websocket par page en 12.x).
- Ne jamais convertir un sous-titre ASS en SRT pour l'usage principal.
- Une session sans `PlayableMediaTypes` peut être un lecteur (Android TV après un redémarrage de Jellyfin) : ne jamais l'écarter pour cette seule raison.

### Interface et scripts injectés
- Un élément se contrôle **par sa taille à l'écran** (`getBoundingClientRect`), jamais par sa seule présence.
- Jamais retirer un élément dessiné par React ; jamais de `window.confirm/alert/prompt` ; tout ce qui est TV sous `.layout-tv` ou l'agent, jamais par le nombre de cœurs.
- Le CSS ne s'édite pas dans l'interface : `branding/jellyfin/groscailloux-tv.css` + `scripts/jellyfin-branding-apply.sh` ; scripts par `sudo scripts/jellyfin-js-apply.py`.
- NPM : éditer la base **et** le fichier `.conf` ensemble ; `location ^~ /gc-chat/` (le `^~` est obligatoire) ; captures nommées dans les `set`.
- Homarr : base modifiée Homarr arrêté et sauvegardé, titres de section ≤ 20 caractères, aucun widget de demandes, d'utilisateurs ou de sessions sur le tableau public.

### Seedbox
- rclone : Jellyfin lie le **parent** `/mnt/seedbox` ; pas de `--vfs-read-ahead` sans mesure ; la clé SFTP n'a ni écriture ni création (lecture + suppression).
- Déménagement VPS → seedbox : hors pic, un flux, `--bwlimit 15000`, jamais pendant une lecture du titre.
- `ssh seedbox cmd args` recolle les arguments : passer les chemins par l'entrée standard.
- Montage : hôte et compte dans la configuration générée au démarrage (`scripts/seedbox-rclone-conf.sh` → `/run/homelab-seedbox-mount/rclone.conf`) ; **aucune option `--sftp-*` ajoutée, retirée ou changée dans l'unité** (le dossier du cache VFS change : 120 Go abandonnés) ; `rclone/rclone.conf` n'est qu'un modèle (`seedbox.invalid`).

### Bancs et essais
- Jamais en production : comptes `zz_` toujours supprimés, **jamais un vrai groupe SyncPlay**, pas de banc de 19:00 à 00:00 ni pendant une lecture.
- `tools/bench/bench.sh` est le seul lanceur de banc et `tools/offpeak/offpeak.sh` le seul exécutant hors pic : `--dry-run` d'abord, rien qui chevauche un créneau `lot3-*`.
- Tester un mail avec un compte temporaire dont l'adresse est celle de l'expéditeur (`SMTP_FROM`), jamais une adresse inventée.

### Git et versions
- Tout changement visible pour les membres ou l'admin = une ligne dans `CHANGELOG.md` (section de la version en cours, avec ses commits ; 1.0.x corrections, 1.x.0 nouveautés).
- Pousser après chaque lot, après avoir cherché secrets et pseudos dans le diff ; **jamais de `--force` sur `main`**.
- Un tag `v1.x.y` publie une release GitHub **publique** (binaires musl + `SHA256SUMS`, `release.yml`) : avant de le pousser, vérifier que le binaire ne contient aucune valeur de `.env` (`strings`).

## 4. Valeurs en vigueur

Une seule source de vérité par valeur : en cas d'écart avec une doc, c'est la source qui a raison (et la doc qu'on corrige).

| Valeur | En vigueur (09/10) | Source de vérité |
| --- | --- | --- |
| Budget C411 | 40 requêtes/h **par clé**, 2 clés ; 10 gardées pour `/recherche` ; clé en 429 mise de côté 15 min | `homelab.toml` `[indexers]` `c411_max_per_hour`, `manual_reserve`, `cooldown_after_429_mins` |
| Filet Prowlarr | 45 requêtes/h par indexeur C411 | Prowlarr, `queryLimit` de « C411 » et « C411 (2) » |
| Indexeurs de Prowlarr | C411, C411 (2), Nyaa.si, World-torrent (secours) | Prowlarr `GET /api/v1/indexer` ; `[indexers] fallback`, `fallback_anime` |
| Rythme des recherches | séries : 6 requêtes par passage de 10 min, 5 s d'écart, 60 épisodes au plus par saison ; films : 3 par passage de 5 min | `[tasks.series_search]` `max_queries_per_run`, `query_gap_secs`, `max_grabs_per_season` ; `[tasks.movie_search]` `max_per_run` |
| Plafonds de taille | 3 Go par épisode, 15 Go par film | `[indexers]` `max_gb_per_episode`, `max_gb_per_movie` |
| Côté qui télécharge | seedbox seule | `[downloads] auto_sides` |
| Profils de qualité | FR-friendly H.264 : VPS 6, seedbox 7 | Arrs `GET /api/v3/qualityprofile` (par nom) ; `[seedbox] quality_profile_id` |
| Comptes | 25 premium, 2 lectures simultanées, sessions illimitées | `[accounts]` `max_premium`, `max_playbacks_per_user`, `max_devices_per_user` |
| Langue des comptes | audio `fre`, sous-titres `fre` en Smart ; mode VO = `jpn` | `[accounts]` `audio_language`, `subtitle_language`, `subtitle_mode`, `vo_audio_language` |
| Saut du lecteur | 10 s | `[accounts]` `skip_forward_ms`, `skip_back_ms` |
| PayPal | **live** depuis le 20/09 | `.env` `PAYPAL_ENV` |
| Abonnements | cycle réel ; grâce 3 j ; essai 7 j ; marge PayPal 36 h ; fiches hors PayPal à la main | `[subscriptions]` `cycle_dry_run`, `grace_days`, `trial_days`, `paypal_margin_hours` |
| Jellyfin | 12.1 | `docker-compose.yml`, image du service `jellyfin` |
| Tmpfs de transcodage | 4 Go, `mem_limit` 6g ; purge chaque minute, urgence à 85 % | `docker-compose.yml` (`jellyfin`) ; `systemd/jellyfin-transcodes-purge.timer` |
| Montage seedbox | cache 120G (dossier `seedbox{9oylk}`), 80G libres gardés, blocs 4M, 32 connexions | `systemd/homelab-seedbox-mount.service` ; modèle `rclone/rclone.conf` + `.env` `SEEDBOX_SFTP_HOST`, `SEEDBOX_USER` |
| Disque du VPS | alerte 85 %, suppression 95 %, journal seul 98 % | `[tasks.disk_pressure]` `alert_pct`, `hard_pct`, `crit_pct` |
| Quota seedbox | alerte 85 % | `[tasks.seedbox_health] quota_alert_pct` |
| Tâche en échec | alerte après 6 échecs de suite et 30 min | `[alerts]` `fail_streak`, `fail_minutes` |
| Boucles HLS | 20 demandes d'un segment en 5 min ; 30 ffmpeg par titre et par heure | `[tasks.hls_loop_watch]` `threshold`, `max_jobs_per_item_hour` |
| Sauvegarde d'état | dimanche 04:30 (+ jusqu'à 15 min), 4 gardées | `systemd/homelab-backup.timer` ; `[backup] keep_last` |
| Langue d'origine | films ; séries à partir du 10/10 07:05 (minuteur `lot4-ol-series`) ; 07:30–11:30 | `[tasks.original_language]` |
| Mode VO | `jpn` ; « Langue d'origine » native préparée, **coupée** | `[accounts]` `vo_audio_language`, `vo_native`, `vo_native_clients`, `vo_native_ignored_clients`, `vo_native_days` |
| Tchat | annonces de plus de 14 j lues d'office pour un compte neuf | `[chat] new_member_read_days` |
| Tâches de homelabd | 32 (dont `tba_bypass`, désactivée, et `vo_native_guard`, inactive tant que `vo_native = false`) + l'observateur `auto_import` | `crates/homelab-core/src/tasks/mod.rs` `registry()` ; `[tasks] disabled` |
| Ports ouverts à Internet | 80/443 (NPM), 6881 (BitTorrent), 81 (admin NPM) ; le reste sur 127.0.0.1 | `docker-compose.yml` |

Mesures de référence (pas des réglages) : un seul transcodage 1080p tient en temps réel ; lecture directe 65 % (04/10) ;
lien seedbox ~8–10 Mo/s par connexion — voir [lecture-et-transcodage.md](docs/runbooks/lecture-et-transcodage.md) et
[seedbox-et-rclone.md](docs/runbooks/seedbox-et-rclone.md).

## 5. Runbooks : avant de toucher X, lire…

| Si tu touches… | Lire |
| --- | --- |
| Sonarr, Radarr, Prowlarr, C411, profils, choix des releases, imports, Jellyseerr, `/recherche`, autobrr | [arrs-et-indexeurs.md](docs/runbooks/arrs-et-indexeurs.md) |
| Tag `russe`, RuTracker, Jackett de la seedbox | [voie-russe.md](docs/runbooks/voie-russe.md) |
| Seedbox, quota, montage rclone, déménagement, ménage, remplacement de fichiers | [seedbox-et-rclone.md](docs/runbooks/seedbox-et-rclone.md) |
| Transcodage, tmpfs, Chromecast, AirPlay, télés, langues, sous-titres, lecture qui saccade | [lecture-et-transcodage.md](docs/runbooks/lecture-et-transcodage.md) |
| Serveur Jellyfin, bibliothèques, identification, extensions, Intro Skipper, journaux | [jellyfin-serveur-et-extensions.md](docs/runbooks/jellyfin-serveur-et-extensions.md) |
| Calque CSS, scripts `branding/jellyfin/`, interface 12.1, télé, « Lire sur », SyncPlay | [jellyfin-interface.md](docs/runbooks/jellyfin-interface.md) |
| Comptes, onboarding, mails, abonnements, PayPal, Mon compte | [comptes-et-abonnements.md](docs/runbooks/comptes-et-abonnements.md) |
| « Aide et annonces » (tchat) | [tchat.md](docs/runbooks/tchat.md) |
| NPM (hôtes, configuration avancée, site par défaut) | [npm.md](docs/runbooks/npm.md) |
| Homarr | [homarr.md](docs/runbooks/homarr.md) |
| Code Rust, `homelab.toml`, installation, état, alertes, Discord, pages d'admin | [homelabd.md](docs/runbooks/homelabd.md) |
| Compose, images, diun, gluetun, qBittorrent, redémarrage, bureau VNC | [vps-docker-et-systeme.md](docs/runbooks/vps-docker-et-systeme.md) |
| Sauvegardes, `backups/`, instances d'essai | [sauvegardes.md](docs/runbooks/sauvegardes.md) |
| Bancs d'interface, travaux hors pic, `tools/`, lot 3 | [outils-bancs-et-hors-pic.md](docs/runbooks/outils-bancs-et-hors-pic.md) |
| « Pourquoi cette règle ? » | [incidents.md](docs/runbooks/incidents.md) |

## 6. En cours au 09/10

- **Lot 3** (mises à jour de NPM, Seerr, Arrs du VPS, outils, Homarr, gluetun, rclone, MySQL, puis redémarrage du VPS) :
  exécutant `backups/lot3-20261008/run.sh`, minuteurs `lot3-J1` à `lot3-J3` (09–11/10, 08:05–12:30) et `lot3-REBOOT`
  (12/10, 04:10–07:00). Après chaque jour : valider compose et diun dans git, reporter `NOTES-DOC.txt` dans les runbooks.
- Instantané Jellyfin 10.11.8 supprimé le 10/10 à 12:00 par un minuteur transitoire (perdu si le VPS redémarre avant).
- Décidé le 08/10 : séries de `original_language` (sam. 10/10 07:05, journal `backups/lot4-20261008/ol-series-on.log`) ;
  flux C411 d'autobrr laissé coupé ; date d'échéance gardée dans Mon compte ; releases `v1.*` publiques (dès 1.24.0).
- Mode VO « Langue d'origine » : préparé et coupé ; activation sur décision du propriétaire une fois OriginalLanguage
  remplie (procédure : [lecture-et-transcodage.md](docs/runbooks/lecture-et-transcodage.md) § 7). À mesurer avant :
  lecteur ExoPlayer de « Jellyfin for Android ».
- Montage seedbox : passe sur la configuration générée à son prochain redémarrage (J3 du lot 3, 11/10) ; contrôler que
  le cache reste `seedbox{9oylk}`. Ancien cache sans suffixe (`cache/rclone/{vfs,vfsMeta}/seedbox`, ~20 Go, inutilisé
  depuis le 19/09) à supprimer ensuite.
- En attente du propriétaire : import CSV des abonnés historiques ; reste du lot 4.
- À valider en séance réelle : Chromecast `high10` → `high`, compression des listes HLS, SyncPlay en 12.1 dans Jellyfin
  Desktop (avant de retirer `gc-syncplay.js`).
- Provisoire : Collection Sections recompilée (à remplacer par Home Screen Sections), garde NPM de Home Screen Sections (à
  retirer quand l'amont corrige).
