# Automatisation : homelabd et homelabctl

`homelabd` (systemd `homelabd.service`, user `deploy`) exécute chaque tâche dans sa propre
boucle tokio, à l'intervalle de `homelab.toml`, avec un jitter initial de 0–30 s et un
timeout de 10 min par passage. Un mutex sérialise toute mutation de qBittorrent, un autre les
onboardings. `HOMELABD_DRY_RUN=1` (ou `--dry-run`) : aucune écriture, les tâches loggent ce
qu'elles feraient. Les logs vont dans le journal : `journalctl -u homelabd -f`.

**Journal et état (2026-10-07)** : `run_done`, `run_failed` et `run_timeout` donnent la durée (`ms`). Un passage
**calme** (réussi, sans action, résumé identique au passage réussi précédent) écrit son `run_done` en `debug` :
c'étaient 88 % des lignes (4 344 lignes entre 01:15 et 10:25 le 07/10, 312 le 08/10) ; `RUST_LOG=debug` pour tout
voir. L'état est sérialisé en mémoire puis écrit en un appel par un écrivain unique ; la simple tenue (début d'un
passage, fin d'un passage calme) part avec la sauvegarde suivante (`update_lazy`), au plus tard au tour suivant de
`StateStore::run_lazy_flusher` (60 s, lancé par homelabd), ou à l'arrêt. `/health` répond 503 `scheduler_stale` quand
plus aucune boucle de tâche n'a tourné depuis 20 min (max(2 × 600 s, plus petit intervalle + 600 s)) et donne la version
(`git describe`, voir `build.rs`).

`homelabctl run <tâche> [--dry-run]` exécute la même implémentation une fois ;
`homelabctl status` lit `state/homelabd.json` (dernier passage, erreurs, items suivis) : la tenue d'un passage sans
rien de neuf y arrive dans la minute, d'où `running? depuis HH:MM` (début récent sans fin enregistrée : en cours ou fini
depuis moins d'une minute) et `interrompu? JJ/MM HH:MM` (début plus vieux que `RUN_TIMEOUT`, 600 s : daemon arrêté en
plein passage) ; `/status.html` lit la mémoire du démon. Sous chaque tâche, une ligne ↳ donne les erreurs récentes
(« 2 aujourd'hui, 5 sur 7 j · dernière le JJ/MM HH:MM : message »), seulement si la dernière date de moins de 7 jours
ou si la tâche échoue en ce moment : `errors` est un total depuis l'origine.

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
`max_actions_per_run` arrêts par passage. Remplace `MaxActiveSessions`, laissé à 0 (voir [runbooks/comptes-et-abonnements.md](runbooks/comptes-et-abonnements.md#1-comptes)).

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
Relance **ratée** (`up`/`restart`/`recreate` en échec, ou `post_exec` raté après la relance) : message Discord admin
« Relance en échec », au plus un par service et par `alert_every_secs` (3600 ; mémoire `state.stack_failures`).
Avant le 2026-10-07, seule une relance réussie prévenait.

**Sonde qBittorrent vue de l'hôte** (2026-10-02) : `http://localhost:8080/api/v2/app/version`. qBittorrent partage le
réseau de gluetun ; quand gluetun est recréé, il reste « healthy » (sa sonde interne passe) mais injoignable. Sonde en
échec alors que gluetun est sain → `action = "recreate"` (`up -d --force-recreate --no-deps qbittorrent`), puis
`post_exec` dans gluetun : le hook `qbit-update-port.sh` repose le port transféré. Toute sonde accepte maintenant
`action` (`restart` par défaut, ou `recreate`) et `post_exec_in` / `post_exec`.

### seedbox_health — 5 min

Santé de la seedbox vue du VPS : `ping` de Sonarr et Radarr, version de qBittorrent, historique Bazarr, lecture du
montage rclone (`mount_check`). Un service injoignable depuis `alert_after_mins` (10) → **mail + Discord admin**, une
seule fois par panne ; un second message à son retour (« panne d'environ 16 h 25 »). Décision pure `decide`, testée.
La relance, elle, est faite **sur la seedbox** par son crontab : `scripts/seedbox/homelab-apps-watch.sh` (copié dans
`~/.local/bin/`), `@reboot` (toutes les applis, 2 min après le démarrage) et toutes les 5 min (une appli qui ne
répond pas deux fois de suite → `app-<x> start`, sans effet si elle tourne déjà) ; journal
`~/.local/state/homelab-apps-watch/watch.log`, `--dry-run` pour voir. Origine : le 01/10, l'hôte de la seedbox a
redémarré et Sonarr, Radarr, Bazarr, Jackett, FlareSolverr, autobrr et unpackerr sont restés arrêtés 16 h sans alerte.

**Quota** (2026-10-07) : le quota du compte seedbox (`quota.json`, écrit par la seedbox toutes les 15 min, lu par le
montage après un `vfs/refresh`, module `quota.rs` partagé avec `/status.html`) est dans le résumé (« quota 57 % ») ;
alerte admin à `quota_alert_pct` (85), une par franchissement (voir « Alertes admin »). Une panne n'est notée
« signalée » que si l'alerte est partie : sinon nouvel essai au passage suivant (l'état est écrit après l'envoi).

### Chien de garde de homelabd (hors homelabd)

`systemd/homelabd-watchdog.timer` (toutes les 2 min, 5 min après le démarrage) lance en root
`scripts/homelabd-watchdog.sh` : `/health` sans réponse 3 fois de suite (≈ 6 min) → `systemctl restart homelabd` et
message Discord admin (webhook lu dans `.env`) ; un second message quand il répond de nouveau ; `--check` pour voir.
homelabd est en `Restart=always` (relancé même après un arrêt « propre » imprévu) ; le chien de garde couvre le cas
d'un homelabd vivant mais figé : depuis le 2026-10-07, `/health` répond 503 quand l'ordonnanceur ne tourne plus
(20 min sans tour de boucle), d'où une relance au bout d'environ 26 min. Le script écrit par `logger -p
user.warning|notice` ; l'unité est en `LogLevelMax=notice` et sous `OnFailure=homelab-alert@%n.service`.

### cert_watch — 24 h (2026-10-07)

Un seul certificat Let's Encrypt (joker, défi DNS-01) sert tous les hôtes de NPM ; NPM le renouvelle seul dès J-30
mais ne remonte rien en cas d'échec, et le canari passe par `localhost:8096` (ni NPM ni TLS). Depuis l'hôte, vers
`connect` (`127.0.0.1:443`) avec le nom de l'hôte public de Jellyfin (`JELLYFIN_PUBLIC_URL`, jamais en dur) :
date d'expiration du certificat présenté (connexion sans vérification, pour la lire même expiré ; décodage DER
minimal de `notAfter`), `GET <hôte public><health_path>` (chaîne NPM → Jellyfin), puis la même requête avec
vérification complète (certificat reconnu pour ce nom). 3 tentatives espacées de 30 s, nouvel essai sur erreur de
connexion et sur HTTP ≥ 500 (`should_retry`), seule la dernière compte. Défaut (certificat à moins de `warn_days` = 21
jours, chaîne cassée, NPM injoignable) : un message mail + Discord qui les regroupe, répété chaque jour tant que ça
dure, **mais jamais deux fois en moins de 20 h pour le même défaut** (2026-10-08 : la tâche tourne aussi à chaque
démarrage de homelabd, et chaque redémarrage réalertait ; voir « Mémoire des surveillances quotidiennes » plus bas).
Résumé : « certificat valide encore 49 j, /health 200 ».

### backup_watch — 24 h (2026-10-07)

Âge de la dernière archive `backups/homelab-state-*.tar.zst` : plus de `max_age_days` (8) jours, ou aucune archive ⇒
alerte admin. Couvre le minuteur `homelab-backup.timer` (dimanche) qui ne tourne plus du tout ; un échec franc de
l'unité prévient déjà par `OnFailure=homelab-alert@`. `homelabctl backup` poste lui-même, en fin de sauvegarde, les
avertissements de copie SQLite à l'admin. Même mémoire que `cert_watch` : « trop ancienne » puis « aucune archive »
sont deux défauts différents, l'âge qui grandit n'en fait pas un nouveau.

### diun_watch — 24 h (2026-10-07)

Relit `diun/images.yml` avec le contrôle strict de `homelabctl check` (`homelab_core::diun`) : clé en double, clé
orpheline, entrée sans `name`, nom en double, tabulation ; chaque image du compose doit avoir son entrée `repo:tag`
(digest ignoré) ; une entrée inutilisée n'est qu'un avertissement. Listes YAML en bloc acceptées sous une clé sans
valeur en ligne. Fichier invalide ⇒ alerte admin à chaque passage, sauf si les **mêmes** erreurs (empreinte
`alerts::fingerprint`) ont déjà été signalées il y a moins de 20 h. Du 18/09 au 07/10, deux groupes de clés
orphelines (retrait de Jackett/FlareSolverr) ont rendu diun aveugle sans que rien ne le signale.

Ces trois tâches passent aussi ~30 s après chaque démarrage de homelabd : l'heure du contrôle quotidien est celle du
dernier redémarrage.

### id_match_import — 5 min
Radarr/Sonarr bloquent l'import quand le nom de la release ne correspond pas au titre de la fiche,
même si l'indexer a fourni l'id au grab : message « Found matching movie/series via grab history,
but release was matched to … by ID ». Fréquent avec les releases C411 titrées en français.
Pour chaque item de queue (tous les Arrs, VPS et seedbox) `status=completed`,
`trackedDownloadState=importBlocked` avec ce message : `GET /api/v3/manualimport?downloadId=…&movieId|seriesId=…`,
puis `ManualImport` (importMode auto → hardlink) des seuls fichiers **sans rejet** ; séries :
uniquement les fichiers dont l'épisode est identifié, les autres restent en manuel (avertissement
dans les logs). Au plus 10 téléchargements par passage.

### torrent_import — 2 min
Torrents ajoutés **à la main** dans qBittorrent (VPS, et seedbox si `[seedbox] qbit_url` est défini), ou par
`series_search` / `movie_search` quand l'Arr refuse la release : ils arrivent dans Jellyfin sans passer par
Jellyseerr. Pour chaque torrent terminé pas encore jugé (`state.torrent_import`, clé `vps:<hash>` /
`seedbox:<hash>`), dans l'ordre :
1. `GET /api/v3/history?downloadId=<HASH en majuscules>` sur le Radarr et le Sonarr du côté : un
   enregistrement ⇒ l'Arr gère ce téléchargement (`arr_managed`) ;
2. `torrents/files` : aucune vidéo hors `sample` ⇒ `no_video` ; VPS : toutes les vidéos ont déjà
   plus d'un lien (importées) ⇒ `already_linked` ;
3. **étiquette `homelab:series=<id>` ou `homelab:movie=<id>`** (posée par `series_search` / `movie_search`) :
   la fiche est connue, pas d'analyse de nom. Sinon : série si le nom ou un fichier porte un marqueur
   d'épisode/saison, sinon film ; `parse` du nom (puis du premier fichier vidéo) → choix d'abord parmi les
   **fiches déjà suivies** (elles portent leurs titres alternatifs : « Shingeki.no.Kyojin.S02 » retrouve
   « Attack on Titan »), puis `lookup` TVDB/TMDB ; `matching` : titre identique après normalisation, année,
   saison ; à défaut premier résultat s'il commence par le même mot (« approximate match ») ; rien ⇒ `no_match` ;
4. la fiche a des fichiers dans les Arrs de **l'autre machine** ⇒ `dup_other_side` (pas de doublon Jellyfin) ;
5. fiche absente ⇒ ajout **non surveillé**, sans recherche (racine et profil du côté : `auto_import`
   pour le VPS, `[seedbox] radarr_root|sonarr_root|quality_profile_id`) ; série : attente de ses
   épisodes (`series_ready_secs`) ;
6. `GET /api/v3/manualimport?folder=<content_path>` **sans id de fiche** (avec l'id, Sonarr liste les fichiers
   déjà rangés de la série et aucun du torrent ; pour une fiche vide il répond 500) et sans `downloadId`
   (liste vide pour un téléchargement non suivi), candidats filtrés sur le **chemin du torrent**, rejets
   d'identification ignorés (`Unknown Movie/Series`, `matched … by ID`). Épisodes : **ceux de l'Arr quand il a
   reconnu cette fiche** (il connaît saisons et numérotation absolue), sinon `parse` du nom de fichier →
   `map_episodes`, puis la numérotation fansub (`Erased S01 - 06`). En dernier recours, et seulement si
   l'étiquette nomme une saison (`homelab:series=<id>:season=<n>`, posée par `series_search`), un **numéro nu**
   (`Angels of Death - 07 (…).mkv` → S01E07, `bare_episode`) est lu dans cette saison. Sonarr ne lit cette forme
   que pour un animé : *Angels of Death* (2021), une série classique, était restée « téléchargée mais pas rangée »
   le 2026-10-03. `bare_episodes` refuse **en bloc** dans trois cas : les fichiers ne portent pas tous le même
   titre, deux fichiers ont le même numéro, ou un numéro dépasse la saison (numérotation absolue). Jamais pour un
   fichier que Sonarr attribue à une autre fiche. **Numéro en tête** (`05. Titre de l'épisode.mkv`, sans nom de série
   ni `SxxEyy`, 2026-10-08, `numbered_pack`) : lu dans la saison de l'étiquette, **seulement si Sonarr n'a rien lu dans
   aucun nom du torrent** (`sonarr_read_something`), décidé pour le torrent entier et refusé en bloc au moindre doute
   (forme mixte, doublon, trou, numéro hors saison, suite qui ne commence pas à 1 après une autre saison) ; un titre qui
   contient un autre nombre (`Show 13`, `E13`) reste du ressort de Sonarr. **Jamais de remplacement** : un fichier dont
   un épisode (ou le film) a déjà un fichier est écarté (« déjà présent »). Le 2026-09-17, « The.Final.Season.E01 », sans saison, a été lu S01E01 et la
   saison 1 d'une série écrasée (réparée en réimportant ses fichiers d'origine). Puis `ManualImport` en
   **`importMode: copy`** (= hardlink). Jamais `auto` : pour un téléchargement non suivi, `auto` = déplacement.
Rien d'importable ⇒ `nothing_importable`. Erreur (Arr injoignable…) ⇒ `retry`, `error` après `max_attempts`.
Au plus `max_per_run` torrents coûteux par passage, les plus récents d'abord. Aucune modification des
torrents. Jellyfin : LibraryMonitor (VPS) ou `seedbox_refresh` (seedbox). Le watcher `auto_import` ignore les
vidéos qui appartiennent à un torrent qBittorrent.
Résumé : `files=3 arr_managed=67 already_linked=8 imported=2 no_match=1 pending=0`.

### hls_loop_watch — 5 min
Boucle HLS = un client qui redemande sans fin le même segment d'un flux transcodé, ce que le membre voit comme
« ça charge » (le 13/09/2026, une TV webOS a redemandé deux segments ~950 fois en 6 min, tous servis en 200 avec
des tailles différentes : le segment était régénéré sous elle). NPM voit chaque requête. La tâche relit les
`tail_bytes` derniers octets de `npm_access_log` (horodatages **UTC**), garde les requêtes
`/videos/<id>/hls1/…/<n>.(ts|mp4|m4s)` de la fenêtre `window_secs`, et compte par (client, média, segment) en
fenêtre glissante ; `≥ threshold` ⇒ `warn!` + mail admin, une seule fois par (client, média) tant que la boucle dure.
**Rafales de ffmpeg** (2026-10-04) : un lecteur peut aussi faire relancer son flux en boucle sans redemander le même
segment — télé Samsung, 116 remux en une heure le 30/09 ; Chromecast, 42 lancements en 22 min le 04/10 — et la règle
des segments ne le voit pas. Chaque lancement laisse un `FFmpeg.<Transcode|Remux|DirectStream>-<date>_<heure>_<id>_<n>.log`
(heure locale) dans `cleanup.jellyfin_log_dir` : la tâche compte les lancements des 60 dernières minutes par titre, sans
les titres du canari (`state.canary.items`), et alerte au-delà de `max_jobs_per_item_hour` (30 ; lecture normale :
17 au plus). Depuis le 2026-10-07, **une alerte par rafale**, mémorisée dans `state.burst_alerts` (id compact → date)
et réarmée quand le compteur horaire du titre retombe sous le seuil : un redémarrage ne répète plus l'alerte, une
seconde rafale le même jour alerte. Une rafale n'est notée signalée que si l'alerte est partie (ou sans canal).
Les alertes (rafales et boucles de segments) donnent le **titre** Jellyfin (« Série S17E03 — Titre », « Film
(année) »), l'identifiant restant pour le journal ; repli sur « média <id> » si Jellyfin ne répond pas. Le
dédoublonnage des boucles de segments (client, média) reste en mémoire : il contient l'adresse d'un membre. L'ancien
repère « non-keyframe breaks » a disparu en 12.x (et ne comptait que le canari, qui demandait `BreakOnNonKeyFrames`).
Le résumé porte le plus grand nombre de lancements d'un titre dans l'heure et les 5xx sur segments. Rien n'est modifié.
La ligne « passage » du journal n'est en `info` que s'il y a une boucle, une rafale ou des 5xx (`is_noteworthy`, 2026-10-08) :
sinon `debug` (111 lignes en 9 h pour « rien à signaler »). De même la ligne « ok » de `disk_pressure`.

### series_search — 10 min (remplace unknown_series_grab)
Les séries se cherchent **par identifiant TMDB**, par homelabd, plus par Sonarr. Pourquoi : Sonarr interroge
C411 avec ses propres titres (« Shingeki no Kyojin », « Attack on Titan ») alors que C411 range la série sous
« L'Attaque des Titans » ; et pour un animé il envoie 3 à 4 requêtes par épisode (~90 pour une saison), ce qui
déclenche le **429** de C411 et une pause de l'indexeur qui s'allonge jusqu'à 24 h. Mesuré le 2026-09-17 sur
la saison 4 : `{TmdbId:1429}{Season:4}` → 5 releases, toutes justes ; « Attack on Titan » → 1 ; identifiant IMDb
→ 100 releases sans rapport (C411 l'ignore). **Jellyseerr est en `preventSearch`** sur les deux Sonarr : une
demande crée la fiche et les saisons suivies, sans recherche. Sonarr garde le RSS, l'import et le suivi.

Pour chaque Sonarr : `GET wanted/missing` → saisons avec épisodes diffusés manquants, hors file d'attente, pas
cherchées récemment (`state.unknown_series` : sans candidat → `retry_after_hours` 24 h, pack pris → 7 j,
épisodes pris → `episode_retry_mins` (5 min), erreur → 1 h) et **absentes de l'autre machine** ; séries ajoutées le plus récemment d'abord. Par saison, via Prowlarr
(deux clés, indexeurs « C411 » et « C411 (2) » de Prowlarr, voir « Budget de l'indexer » plus bas) : `search?type=tvsearch&query={TmdbId:<tmdbId>}{Season:<n>}`
→ releases dont l'attribut **`tmdbId` est celui de la fiche** (le nom ne compte pas) → `GET parse` de Sonarr
(saison, pack, épisodes, qualité) → garde-fous : marqueur FR (VFF > MULTi > FRENCH > VOSTFR), qualité autorisée
par le profil et ≤ 1080p, au moins une source, épisodes manquants. Pack si la moitié de la saison manque,
sinon épisodes ; tri : langue, résolution, au moins deux sources, codec, sources, audio (voir « Codec » plus bas). **Pas de pack avant la fin de la diffusion** (2026-10-07) :
quand un pack serait pris, les épisodes de la fiche sont relus (un GET) ; s'il reste un épisode **suivi** dont la date
de diffusion est connue et à venir (`future_episodes`), le pack est écarté (« pack écarté (épisodes à venir : 2,
3) ») et les épisodes sortis sont pris un à un (`choose_episodes`). C'est la `FullSeasonSpecification` de Sonarr, qui
ne joue pas côté seedbox (la release part directement dans qBittorrent). Un épisode sans date ne bloque jamais. Le
plafond de taille d'un pack est jaugé sur les manquants **datés** (`SeasonTodo.dated_missing`, `choose_sized`), plus
sur les épisodes sans date de `undated_missing` : une saison toute sans date refuse donc tout pack de plus de
`max_gb_per_episode` et part épisode par épisode ; seule une lecture du `.torrent` des packs permettrait mieux.
Black Clover S02 (03/10) : pack de 51 fichiers (S02E52-102, 14,2 Go) pris pour une saison de 13 épisodes dont 1
diffusé. Rien par identifiant (série sans `tmdbId`, releases
sans attribut) : `text_queries` noms essayés en texte libre (Jellyseerr FR et original, Sonarr, alternatifs),
titre parsé identique exigé, release d'un autre identifiant écartée.

**Identifiant incomplet → texte libre en plus** (2026-09-18) : `uncovered()` compare les épisodes manquants aux
épisodes couverts par les candidats. S'il en reste, les noms de la série sont essayés **en complément** de
l'identifiant (`how = "tmdb+texte"`), avec les mêmes garde-fous. Vu sur Bleach S17 : `{TmdbId:30984}{Season:17}`
renvoie 41 releases qui couvrent E01–26 et E41–48, mais **jamais E27–40** — ce cours n'est publié que sous
`BLEACH.Thousand-Year.Blood.War.S03`. Attention, Sonarr n'analyse correctement que le premier cours : `S01` →
saison 17, mais `S02` → saison 2 et `S03` → saison 3 de Bleach. Le garde-fou d'égalité de saison les refuse, ce
qui évite d'écraser deux vraies saisons ; ce qui reste introuvable est enregistré (`uncovered` dans
`state.unknown_series`) et listé sur `/status.html` sous « Saisons sans release », pour une reprise à la main
depuis `/recherche`.

**Pack d'un cours** (2026-09-18) : si le trou persiste et que la fiche est un **animé**, `cour_pack` retient
les releases que l'Arr rattache à cette fiche mais à une autre saison ; `torrent_file::files` lit la liste de
leurs fichiers dans le `.torrent` (téléchargé chez Prowlarr, aucune annonce au tracker) et `offset_mapping`
n'accepte que la certitude : trou d'un seul tenant, autant de fichiers vidéo que d'épisodes manquants,
numérotés en suite, aucun épisode déjà pourvu. Le décalage part dans l'étiquette
`homelab:series=<id>:season=<n>:offset=<k>:eps=<from>-<to>`, que `torrent_import` applique seul
(`EpisodeSource::OursOnly` : ni les épisodes de Sonarr ni `map_episodes`, tous deux lisant la mauvaise saison).
Le pack n'est jamais confié à l'Arr. Réglages : `cour_packs`, `cour_max_files`.

**Intégrale** (2026-10-03) : un pack sans numéro de saison (`Space.Dandy.INTEGRALE.MULTI.VFF.1080p…`) n'est jamais
renvoyé par `{TmdbId}{Season}`, et le `parse` de Sonarr n'en lit rien, pas même la qualité. *Space Dandy*, demandé par
un membre, restait donc « 0 candidat » alors que C411 l'avait. Quand **rien d'autre n'est acceptable** pour une saison
(et jamais pendant une panne de C411), `try_integrale` reprend les intégrales vues dans les résultats (même `tmdbId`,
`season_less_pack` : aucune saison lisible ou pack de plusieurs saisons). S'il n'y en a pas, il fait **une** requête
`{TmdbId}` sans saison (type `tvsearch`), seulement si le dernier épisode manquant est sorti depuis
`integrale_min_age_days` (14). La qualité est lue en remplaçant le marqueur par `S01` (`integrale_quality_title`).
Le classement suit les mêmes clés que `choose`, puis le `.torrent` des deux meilleures est lu (aucune annonce).
Chaque fichier vidéo passe au `parse` de Sonarr, scene mapping compris : `02x01` → S01E14. `integrale_pick` ne garde
que les fichiers dont **tous** les épisodes manquent dans CETTE saison ; deux fichiers pour un même épisode, c'est un
refus. Le torrent est ajouté **arrêté** ; les fichiers non retenus sont désélectionnés (`file_ids`, chemin exact sous
la racine), puis il démarre avec l'étiquette `homelab:series=<id>:season=<n>`, et `torrent_import` importe. Si
l'intégrale est déjà dans qBittorrent pour une autre saison, les fichiers rejoignent sa sélection et son passage dans
`torrent_import` est effacé, pour qu'il les importe à leur tour. Une intégrale dont les fichiers ne portent pas le nom
de la série (« 01. Titre.mkv ») est écartée : l'API `parse` de Sonarr ne prend pas de chemin. Réglages :
`integrale_packs`, `integrale_max_files` (300), `integrale_min_age_days`.

**Le VPS ne cherche plus** (2026-09-18) : `[downloads] auto_sides = ["seedbox"]` retire les Arrs du VPS de la
boucle, et leur indexer C411 est en `enableRss = false`. Le VPS n'entame plus aucun téléchargement.
**Envoi** — Arr du **VPS** : `POST /api/v3/release/push` (Sonarr suit, importe, renomme) avec le lien Prowlarr
réécrit pour les conteneurs (`prowlarr_url_for_arrs` = `http://prowlarr:9696` : le lien renvoyé par Prowlarr,
`http://localhost:9696/…`, désigne le conteneur lui-même, et Sonarr accepte l'envoi puis échoue en silence
« Connection refused », vu le 2026-09-17), puis vérification que la saison (ou le film) apparaît dans sa file
**par identifiants** (le titre de la file est le nom interne du torrent). Arr de la **seedbox** (Prowlarr du VPS
injoignable), refus d'identification ou indexeur bloqué (`bypassable_rejection`), ou envoi accepté mais pas
mis en file → `.torrent` récupéré par Prowlarr et ajouté au qBittorrent du même côté avec l'étiquette
`homelab:series=<id>:season=<n>`, importé ensuite par `torrent_import`. Tout autre refus (liste noire,
taille…) est respecté. **Rythme** : au plus `max_queries_per_run` requêtes C411 par passage (6, soit 36/h au pire),
`query_gap_secs` (5 s) d'écart, `max_new_per_run` en plus pour les demandes de moins d'une heure, le tout sous le budget
par clé (« Budget de l'indexer » plus bas) ; filet de sécurité : Prowlarr coupe à 45 requêtes/heure par indexeur C411.
Historique : le 2026-09-17, avec une seule clé partagée par Prowlarr et les 4 Arrs, le 429 est tombé vers 50 requêtes
dans l'heure, pendant le rattrapage de l'arriéré ; d'où les deux clés et leurs compteurs.
Résumé : `grabbed=1 none=2 pending=13`.

**Codec** (2026-09-26) : à langue, résolution et partage égaux (≥ 2 sources), le **x265** passe devant le x264 et
l'**AV1** en dernier (`codec_rank`, clé `(langue, résolution, sources ≥ 2, codec, sources)`, même clé pour
`movie_search` et `/recherche`), puis l'**audio** (`audio_rank` : AAC/E-AC3/AC3/Opus > FLAC > DTS > DTS-HD/TrueHD,
le DTS n'est lu ni par les Chromecast ni par l'appli iOS). Un x264 reste pris quand c'est la seule version française bien partagée.

### movie_search — 5 min
Recherche des films **suivis, sans fichier, sortis, hors file d'attente**, dès le passage qui suit la demande
(`missing_hours` 0, passage toutes les 5 min) : depuis le 2026-09-17, Radarr n'a plus aucune recherche (C411 en
RSS seulement) et le RSS ne ramène que les nouveautés — un film ancien ne peut venir que d'ici. Même mécanique que
`series_search` : `{TmdbId:<id>}` (type `movie`), `tmdbId` vérifié, `parse` Radarr, garde-fous, `release/push`,
sinon qBittorrent + `homelab:movie=<id>`. Au plus `max_per_run` films (3) par passage ; un échec (indexeur en
panne) est retenté après `error_retry_hours` (1), « aucune release » après `retry_after_hours` (72).

**Films français sans date numérique** (`awaiting_vod`, 2026-09-25) : sans date numérique, Radarr croit un film
disponible 90 jours après la salle ; en France la VOD arrive 4 mois après (chronologie des médias) et C411 n'a rien
avant. Un film en langue originale française, sans `digitalRelease` ni `physicalRelease`, sorti en salle depuis
moins de `min_days_after_cinema` (110) jours n'est pas cherché (résumé : « N en attente de la VOD ») ; l'onglet
Demandes affiche « Au cinéma depuis le … : la VOD arrive environ 4 mois après (vers le …) ». Le RSS reste actif.
Les films étrangers (date numérique américaine connue) sont déjà gérés par Radarr (`isAvailable`).

### indexer_unblock — 5 min
Après des échecs (429, délais), Sonarr et Radarr mettent un indexeur en pause, jusqu'à 24 h ; aucune API ne
lève la pause (`indexerstatus` → 404). Lecture seule de la table `IndexerStatus` des 4 Arrs (VPS : `rusqlite`
sur `sonarr|radarr/config/*.db` ; seedbox : `ssh seedbox` + `sqlite3 -readonly` sur
`<seedbox_apps_dir>/<app>/<app>.db`). Un indexeur en pause dont le **dernier échec date d'au moins
`quiet_mins`** (15 : fenêtre de limite de C411 ; plus tôt il se rebloquerait) est remis en service :
application arrêtée (`docker compose stop` / `app-<app> stop`), base copiée (`backups/arr-db-<app>-<date>.db`
ou `<app>.db.homelab-<date>` sur la seedbox, mode 600, gardées 7 jours), `UPDATE IndexerStatus SET
DisabledTill = NULL, EscalationLevel = 0, …`, application relancée et vérifiée (`system/status`). Au plus un
arrêt par application par `app_cooldown_mins` (60), même après un échec. Un indexeur débloqué
`max_unblocks_per_day` fois (10) en 24 h n'est plus touché : alerte admin (site mort, Cloudflare…), notée
« déjà signalée » (24 h) seulement si elle est partie. Chaque déblocage est signalé (mail + Discord admin).

**Site de C411 en panne** (2026-10-07) : avant de lever la pause d'un indexeur C411 (nom commençant par
`[manual_search] c411_indexer`), le site est sondé (`indexer::c411_reachable` : `caps` par Prowlarr, sans clé ni
quota ; pas `c411_up`, dont la branche `indexerstatus` reflète la clé de recherche et bloquerait la levée de la clé RSS
sur un 429). Site en panne (503, page HTML en 200, injoignable) ⇒ `Decision::Outage` : aucun arrêt d'Arr, aucune
copie de base, résumé « C411 en panne : pause laissée (n) ». Une alerte au début de l'incident (marqueur `c411_outage`
dans `state.indexer_alerts`, posé **seulement si l'alerte est partie** — ou si aucun canal n'est configuré —, sinon le
passage suivant la retente) ; tant qu'il existe, le site est sondé à chaque passage. Il n'est retiré qu'après
**6 h sans panne constatée** (2026-10-08, `OUTAGE_HOLD_SECS`) : chaque passage en panne rafraîchit la date du marqueur
(`OutageStep::Hold`, mutation différée), et une sonde réussie sur un marqueur plus récent ne fait rien (`Quiet`). Un C411 qui
clignote (503, 200, 503…) ne réalerte donc pas à chaque aller-retour ; la panne suivante, 6 h après le retour, est
signalée. Prowlarr absent ou injoignable = inconnu : comportement d'avant. Les autres
indexeurs ne sont pas concernés. Du 30/09 au 02/10 : 34 arrêts inutiles des Arrs seedbox. **Purge** des copies
`*.homelab-*` de plus de 7 jours de la seedbox : une fois par 24 h (un seul ssh, marqueur `purge:seedbox`, antidaté de
23 h en cas d'échec pour réessayer dans l'heure), plus seulement après un déblocage. `indexer_alerts` porte donc trois
sortes de clés : `application:id`, `c411_outage`, `purge:seedbox`. homelabd est cloisonné (`ProtectSystem=strict`) :
`backups/`, `sonarr/config` et `radarr/config` sont dans `ReadWritePaths` de `systemd/homelabd.service`.
Premier passage le 2026-09-17 : C411 (Sonarr seedbox, niveau 9, bloqué jusqu'à 21 h 33) et U2P, WorldTorrent,
JK-nortorrent (Sonarr VPS) remis en service.

### Indexer : RSS d'un côté, recherches de l'autre
Les 4 Arrs gardent C411 en **RSS seulement** (avec `C411_RSS_API_KEY`) : ils ne lancent plus aucune recherche,
donc ils ne peuvent plus déclencher la limite d'API (une recherche de saison d'animé = une requête par épisode).
Les recherches passent par Prowlarr, avec l'autre clé, sous le budget commun ci-dessous. `indexer_unblock` passe
toutes les 5 min et lève une pause 15 min après le dernier échec (10 fois par jour au plus).

### Budget de l'indexer (commun)
`homelab_core::indexer` : deux clés déclarées dans Prowlarr, un compteur horaire **par clé**
(`c411_max_per_hour`, 40), `manual_reserve` (10) gardées pour `/recherche`, bascule immédiate sur l'autre clé
si l'une répond 429 (mise de côté `cooldown_after_429_mins`). `search_tmdb` et `search_text` sont les deux
seules portes d'entrée : tâches et page passent par elles.

Historique : avant le 2026-09-17, chaque tâche avait son plafond (12/h, 1/h, 6/h) sans voir les autres ; un compteur
commun (20/h, réserve 6) l'a remplacé ce jour-là, puis un compteur par clé (deux clés).

### russian_search — 10 min
Voie russe : Sonarr et Radarr cherchent par leur titre (anglais), RuTracker range par le titre russe. Pour chaque
fiche de la voie russe à qui il manque des fichiers : titre original TMDB (Jellyseerr) → Jackett de la seedbox
(RuTracker) → film de la bonne année ≤ 1080p, ou release dont la plage d'épisodes en numérotation absolue couvre le
plus d'épisodes manquants → `.torrent` ajouté au qBittorrent de la seedbox (`homelab:russe`), fichiers inutiles
désélectionnés → une fois complet, `ManualImport` copy (épisode par `SxxEyy` ou numéro absolu) et analyse complète de
Jellyfin. `max_per_run` 2, `retry_hours` 24 (1 h après une erreur), état `russian_title`. Un torrent déjà présent
(même release) est repris tel quel. Seedbox injoignable (qBittorrent, liste ou épisodes d'un Arr) : le côté est sauté,
passage réussi (« seedbox injoignable »), au lieu d'une erreur à chaque passage (2026-10-07).

### anime_library — 5 min
**Voie russe** (2026-09-26) : une fiche de la seedbox rangée dans `seedbox_ru_series_root` / `seedbox_ru_movies_root`
(dossier choisi dans Jellyseerr par le membre, permission « demandes avancées ») reçoit le tag `russe` et, s'il lui
manque des fichiers, une recherche de l'Arr (`SeriesSearch` / `MoviesSearch`, au plus toutes les
`ru_search_retry_hours`) qui n'interroge que RuTracker (seul indexer tagué `russe`). Un tag `russe` posé à la main
hors du dossier fait déplacer la fiche. La langue TMDB seule ne décide jamais. `series_search` et `movie_search`
ignorent ces fiches (`russian_route`).

**Après un déplacement, analyse complète de la médiathèque** (`scan_after_move`, au plus une toutes les
`scan_min_gap_mins` = 20, un déplacement pendant l'attente est rattrapé au passage suivant) : un titre passé d'une
bibliothèque seedbox à une autre n'est créé par Jellyfin que par cette analyse (`Library/Media/Updated` et le
rafraîchissement du dossier ne suffisent pas ; *Your Name* invisible 45 min le 2026-09-26).

Pose aussi `seriesType = anime` sur toute série rangée dans Anime (numérotation absolue), et rattrape celles
qui étaient restées en « standard » (`RescanSeries` derrière, contrôle des fichiers).

Range l'**animation japonaise** dans deux bibliothèques Jellyfin dédiées : « Anime » (séries : `/media/anime` +
`/seedbox/media/Anime`) et « Films d'animation » (films : `/media/anime-films` + `/seedbox/media/Anime Movies`).
Dossiers racines des Arrs : `/anime` et `/anime-films` sur le VPS (liens de `sonarr|radarr/config/custom-cont-init.d/symlinks.sh`
vers `/data/media/anime*`), `~/media/Anime` et `~/media/Anime Movies` sur la seedbox.

- **Classement** (`homelab_core::anime`) : fiche TMDB lue par Jellyseerr (`tv/{id}`, `movie/{id}`), en cache
  `recheck_days` (30 ; 1 jour pour une fiche introuvable) dans `anime_class`. Anime = genre Animation (16) **et**
  origine japonaise (langue `ja`, ou pays `JP` seul / majoritaire en japonais), ou Animation + mot-clé `anime`
  sans autre langue. Le type « anime » de Sonarr n'est **pas** utilisé (*Bleach*, *Re:Zero*, *FMA: Brotherhood*
  y étaient en « standard ») ; les prises de vues réelles japonaises et l'animation américaine, chinoise ou
  française restent dans Séries/Films. Tags manuels prioritaires : `anime` ⇒ rangé, `pas-anime` ⇒ jamais déplacé.
- **Déplacement** : fiche hors du dossier anime classée anime, sans téléchargement en cours ni fichier en lecture
  dans Jellyfin (`Sessions` : `en_lecture`, passage suivant ; Jellyfin injoignable ⇒ rien) ⇒ tag `anime` +
  `PUT series/editor` ou `movie/editor` (`rootFolderPath`, `moveFiles`) ; renommage sur le même disque, hardlinks
  et torrents intacts. Au plus `max_moves_per_run` (5) par passage, `max_lookups_per_run` (150) fiches TMDB lues.
  Puis attente des commandes de déplacement (4 min au plus), `vfs/refresh` rclone de l'ancien et du nouveau
  dossier (seedbox) et `Library/Media/Updated` pour Jellyfin. Le déplacement est noté dans `anime_moves` avant
  l'appel : `deletion_cleanup` ignore la fiche pendant 6 h.
- **Jamais dans l'autre sens** : une fiche du dossier anime classée non-anime est seulement signalée
  (`a_verifier`) ; fiche sans identifiant TMDB : `inconnu`, laissée en place.
- **Jellyfin après un déplacement** : côté VPS la surveillance en temps réel suffit (la fiche passe de bibliothèque
  en une minute) ; côté **seedbox** (montage rclone), `Library/Media/Updated` rafraîchit l'ancienne fiche sans créer
  la nouvelle : le titre disparaît de Séries/Films et n'apparaît dans Anime qu'à l'analyse de la médiathèque (05 h).
  La réidentification peut se tromper (le 2026-09-17 : *L'Attaque des Titans* → spin-off, *Slime* → *Slime
  Diaries*) : comparer `ProviderIds.Tmdb` des bibliothèques Anime / Films d'animation au `tmdbId` de l'Arr.
- `only_tmdb` limite la tâche à quelques titres (essai) ; l'essai à blanc lit tout et liste tout sans plafond.
- Migration du 2026-09-17 : 27 fiches (23 séries, 4 films) en 5 lots, `deletion_cleanup` suspendu, tailles et
  nombres de fichiers identiques avant/après (Arr et disque), aucune suppression dans l'historique des Arrs ;
  pilote avec un compte ordinaire temporaire : épisode vu et reprises conservés.
- Nouvelles demandes : Jellyseerr range déjà les séries qu'il reconnaît comme animés (`activeAnimeDirectory`,
  `animeTags` des serveurs Sonarr) ; la tâche rattrape le reste et les films.

### identity_check — 30 min

Deuxième motif depuis le 2026-09-22 : une fiche affichée sous un **nom de release** (`looks_like_release`, marqueurs
entiers `1080p`, `WEBRip`, `x265`, `MULTi`…). Les bibliothèques sont passées à `EnableEmbeddedTitles = false`, mais
les fiches déjà créées gardent ce nom : seul `RemoteSearch` + `Apply` le remplace. Une fiche n'est réessayée qu'une
fois par mois (`state.renamed_items`), interrupteur `[tasks.identity_check] fix_release_names`. 30 films réparés le
jour même (Matrix Reloaded, Harry Potter, A Quiet Place…).
Jellyfin identifie un dossier **d'après son nom** : un titre proche de celui d'un spin-off part sur la mauvaise
fiche (*Attack on Titan* → *Junior High School*, *Slime* → *Slime Diaries*, *The Walking Dead* → *Dead City*).
La tâche compare, pour chaque fiche des 4 Arrs, l'identifiant (TVDB pour les séries, TMDB pour les films) à
celui de l'élément Jellyfin qui porte le même chemin (`map_path`), et corrige les écarts : `RemoteSearch` avec
le bon identifiant, `Apply`, puis rafraîchissement complet des métadonnées. Au plus `max_fixes_per_run` (3) par
passage, jamais un titre en cours de lecture, `dry_run` respecté. Un identifiant absent d'un côté ne conclut
rien. Premier passage le 2026-09-17 : 6 titres corrigés sur 215.

### original_language — 10 min, 07:30–11:30 (2026-10-08)

But : la préférence audio « Langue d'origine » de Jellyfin 12 (`AudioLanguagePreference = "OriginalLanguage"`) choisit la
piste d'après la métadonnée `OriginalLanguage` de la fiche, vide sur 262 fiches sur 275 au 08/10. La tâche y écrit la
langue d'origine TMDB lue par Jellyseerr, par l'éditeur de métadonnées (`POST /Items/{id}` avec le seul corps
`update_body`), **films d'abord**, du plus récent au plus ancien, puis séries si `series = true` (**`false` tant que le
propriétaire n'a pas accepté** qu'une série réécrive la classification de ses saisons et épisodes). Les épisodes
héritent de leur série et ne sont jamais écrits.

- **Fenêtre** `window_start`–`window_end` (07:30–11:30) : après l'analyse de 05:00, les segments et les images de chapitre,
  avant midi. Aucune vidéo lue ; tout passage pendant une analyse de la médiathèque est sauté ; jamais une fiche en
  lecture ; jamais de Refresh.
- Par passage : `max_movies_per_run` (4) ou `max_series_per_run` (1) ; `retry_hours` (24) pour une fiche sans langue TMDB
  ou en échec net.
- Issues : `written`, `unknown`, `locked`, `children_differ` (une série dont un enfant a sa propre classification n'est
  pas écrite : Jellyfin la lui recopierait), `error`, plus `posted` (écriture partie sans réponse nette). **Une fiche
  écrite, ou `posted`, n'est jamais réécrite d'office** : un retour arrière tient.
- État `state.original_language` : ancienne et nouvelle valeur, enfants dont la classification vide a reçu celle de la
  série. Résumé : à remplir, sans TMDB, en attente de nouvel essai, en lecture, remises à vide depuis l'écriture,
  écritures non confirmées, séries hors liste, écritures constatées après coup.
- Lecture de la médiathèque : avec ET sans compte puis filtre sur `SeriesId` (`GET /Items` avec un compte regroupe une
  série présente dans deux dossiers ; sans compte, la liste des films est incomplète).
- `--dry-run` montre le prochain passage, même hors fenêtre. Clés : `interval_secs`, `enabled`, `series`, `window_start`,
  `window_end`, `max_movies_per_run`, `max_series_per_run`, `retry_hours`.
- Retour arrière : `backups/original-language-20261008/rollback.py` (voir
  [runbooks/lecture-et-transcodage.md](runbooks/lecture-et-transcodage.md#langue-dorigine-original_language-lot-4-0810-et-préférence-native)).

### seedbox_refresh — 5 min (si `[seedbox] enabled`)
Lit l'historique `downloadFolderImported` (eventType 3) des Radarr/Sonarr de la seedbox depuis le
dernier id traité (`state.seedbox_history` ; la première passe initialise le curseur sans rejouer).
Pour chaque nouvel import : chemin `media_root/…` → dossier relatif, `POST <rclone_rc>/vfs/refresh`
sur ce dossier et ses parents, puis `POST /Library/Media/Updated` à Jellyfin avec le chemin
`jellyfin_root/…` (Jellyfin ne scanne que ces dossiers). Montage absent → avertissement, le
curseur n'avance pas. Arr de la seedbox injoignable (2026-10-07) : sauté (« arr unreachable: side skipped this run »,
curseur inchangé), l'autre est traité, résumé « seedbox injoignable » / « <arr> injoignable ».

### Arrs multi-instances
Avec la seedbox activée, `stuck_handler` traite aussi les queues des Arrs seedbox,
`tba_bypass` scanne aussi `seedbox.sonarr_downloads` avec le Sonarr seedbox, et `monitor_sync`
**route chaque demande Jellyseerr selon `media.serviceId`** (`jellyseerr_vps_sonarr_id` → Sonarr
VPS, `jellyseerr_sonarr_id` → Sonarr seedbox) : les ids de séries diffèrent entre instances.

### tracker_ratio — 30 min
Share limits par tracker via `POST /api/v2/torrents/setShareLimits`.
`unlimited` (`c411.org`, `c411.tw`) → ratio -1 / temps -1 ; `secondary` (yggleak, u2p, ygg.gratis, `t-ru.org` = RuTracker) et
`public` (liste de trackers publics) → 2.0 / 14 j ; défaut → 1.0 / 7 j.
Ne touche un torrent que si sa limite actuelle diffère (ratio comparé à 2 décimales).

### stuck_handler — 5 min
`GET /api/v3/queue?pageSize=200` sur Sonarr et Radarr. Un item dont `errorMessage` matche
`stalled|metadata|no connections` est suivi (`state.stuck`). Passé `stall_secs` (8 h) :
`DELETE /api/v3/queue/{id}?removeFromClient=true&blocklist=true` → l'Arr relance une
recherche. Max 5 par passage. Les items disparus de la queue sont oubliés.

### disk_pressure — 15 min
`statvfs` sur `paths.base`, arrondi comme `df`. À `alert_pct` (85, 2026-10-07) : alerte admin, une par franchissement
(réarmée `capacity_rearm_pts` sous le seuil), bien avant `hard_pct` qui supprime sans prévenir. < 95 % : rien. 95–98 % : supprime avec
fichiers (`POST /api/v2/torrents/delete`, `deleteFiles=true`) jusqu'à 5 torrents en état
`stoppedUP`, les plus anciens d'abord — les hardlinks de `library/media` survivent.
≥ 98 % : log d'erreur, aucune action automatique.

### trending — 6 h

Rangée « Tendances cette semaine » de l'accueil Jellyfin. Classement lu dans Playback Reporting
(`POST user_usage_stats/submit_custom_query`, SQL en lecture) : épisodes regroupés par série, spectateurs
distincts puis heures ; un spectateur compte à partir de `min_minutes` sur le titre. Semaine trop calme :
complétée par `fallback_days`. Tient à jour la collection `collection_name` (création, puis ajouts et
retraits ciblés). Collection et non playlist : une playlist qui reçoit une série y déplie ses épisodes.

### catalogue_report — 24 h (2026-10-08)

Rapport « catalogue jamais regardé » (décidé par le propriétaire : un rapport sur `/status.html`, l'admin tranche).
**Lecture seule** : ~52 requêtes Jellyfin, 4 aux Arrs, 3 à Jellyseerr, aucun fichier lu, rien de supprimé ni d'écrit ailleurs
que dans `state.catalogue` (4 à 11 Ko : `catalogue`, `unmatched`, `never_all`, `never_aged`, `backlog`, `russian`).

- **Arrivée** = date `added` de l'**Arr** (jamais celle de Jellyfin, recréée à chaque déménagement), repoussée à
  `movieFile.dateAdded` pour un film et à `firstAired` pour une série (demandée avant sa sortie, elle n'arrive qu'avec ses
  épisodes ; `firstAired` est en RFC 3339 comme `added`).
- **Vu** = `IsPlayed` ou `IsResumable` d'**un** compte quelconque (tous les comptes, désactivés compris, lus avec `UserId`)
  ou une ligne Playback Reporting de 60 s ou plus. Une erreur sur un compte fait échouer le passage (un « vu » incomplet
  gonflerait le « jamais vu ») ; un côté Arr injoignable garde le rapport précédent.
- Rapprochement Arr ↔ Jellyfin par dossier (`map_path`/`side_maps` d'`identity_check`), puis par identifiant TMDB/TVDB
  unique ; sinon la fiche est **non évaluée** (« je ne sais pas » n'est pas « jamais vu »).
- Demandeur = **sorte** seulement (`membre` l'emporte sur `admin`, sinon `aucune` ; `inconnu` si Jellyseerr est muet ou si
  la fiche n'a pas d'identifiant TMDB), jamais un pseudo. Arriéré des séries déjà commencées à part ; ligne **voie russe**
  (épisodes disponibles / lus), indépendante du seuil.
- Réglages `[tasks.catalogue_report]` : `min_age_days` (60), `max_listed` (40), `max_backlog_listed` (15),
  `user_pause_ms` (150). `--dry-run` calcule et journalise sans écrire.
- **Limite connue** : les fiches Arr de la seedbox datent de la création des Arrs (12/09) ; elles n'atteignent 60 jours
  qu'à partir du 11/11/2026. Au 08/10, la liste de décision ne contient que 6 films du VPS (22 Go) ; premier calcul :
  277 titres (2,29 To), 165 jamais commencés (0,84 To), arriéré de 29 séries (0,71 To), voie russe 120 épisodes
  disponibles, 0 lu. Pas d'historique : seul le dernier rapport est gardé.

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
- **Demande retirée dans Jellyseerr** (média « en attente » ou « en cours » sans demande) : fiche Arr et
  fichiers supprimés, torrents traités comme ci-dessus, média Jellyseerr retiré. Les médias « disponible » et
  « partiel » sont intouchables (le scan Jellyfin en crée un par titre de la bibliothèque). Garde-fous :
  Jellyseerr injoignable ⇒ rien, abandon au-delà de `abort_if_missing_titles_over` d'un coup,
  `max_titles_per_run` par passage, jamais un titre en cours de lecture.
- **Série entière** : idem côté Sonarr. **Saison entière** : épisodes non surveillés, saison non
  surveillée, notée dans `deletions.seasons` : `monitor_sync` ne la re-surveille plus, sauf demande
  Jellyseerr créée après la suppression. **Épisodes isolés** : non surveillés.
- **Torrents** : sources = historique du titre (`history/movie`, `history/series` filtré sur les épisodes
  supprimés, jamais un évènement d'un autre titre) + nom de release (`originalFilePath`, `sceneName`).
  Gardé s'il sert encore (lien physique d'un de ses fichiers sur le VPS, ou autre import du même
  `downloadId` qui a encore son fichier). C411 ou tracker inconnu : retiré avec ses fichiers seulement à
  ratio `c411_min_ratio` ou `c411_min_seed_days` de seed (en attente dans `deletions.pending_torrents`,
  revu à chaque passage) ; autres trackers : tout de suite.
- **Fichiers des séries de la seedbox en cache** (2026-10-07) : la liste `episodefile` d'une série est gardée en
  mémoire tant que sa signature (`episodeFileCount`, `sizeOnDisk`, dossier) ne change pas, au plus
  `episode_files_cache_mins` (60 ; 0 = sans cache). Une série dont un fichier semble absent est toujours relue fraîche
  et re-sondée avant toute conclusion : rien n'est décidé sur une liste en cache (des fichiers renommés gardent même
  nombre et même taille). Avant : 83 requêtes par le proxy de la seedbox à chaque passage ; maintenant une fois par
  heure. Le VPS n'est pas concerné.

Testé le 2026-09-14 de bout en bout sur un film jetable du VPS (import Radarr, scan, `DELETE /Items`,
fiche Radarr supprimée, aucun torrent touché).

### monitor_sync — note (2026-09-13)
Une saison demandée n'est suivie que si elle n'a pas déjà des fichiers **sur l'autre machine**
(rapprochement par tvdbId) : sans ça, une nouvelle demande routée vers la seedbox y faisait suivre
toutes les saisons historiques, y compris celles présentes sur le VPS (doublons).

### tba_bypass — 5 min (désactivée : dans `tasks.disabled` depuis `episodeTitleRequired = never`, gardée comme filet)
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
Les séries inconnues de Jellyseerr ne sont pas touchées. Un Sonarr injoignable est sauté, l'autre traité
(2026-10-07) ; tant qu'un côté est muet, l'autre ne connaît pas ses saisons déjà présentes : il n'ajoute **aucun**
nouveau suivi (risque de doublon Jellyfin) mais peut en retirer, rattrapé au passage suivant. Jellyseerr injoignable
reste une erreur.

### user_poller — 60 s
`GET /api/v1/user?take=200` : users `userType == 2` (locaux), non admin, email valide, créés
depuis < 10 min, email pas encore traité (`state.onboarded`). Si un user Jellyfin du même nom
existe → marqué et ignoré. Sinon `DELETE /api/v1/user/{id}` puis onboarding unifié.

### cleanup — 24 h
Fichiers de `jellyfin/cache/transcodes` > 1 j ; dossiers vides de `library/downloads`
(profondeur ≤ 3) > 2 j ; fichiers des `.recycle` Sonarr/Radarr > 14 j ; logs Jellyfin > 7 j.

## Recherche manuelle (`/recherche`)

Page de homelabd (hôte d'onboarding, liste NPM « admin-outils » + jeton) : chercher un titre des 4 Arrs, choisir
une saison ou le film, voir les releases, en télécharger une. Elle remplace la recherche de Sonarr/Radarr pour les
**animés** : celle-ci interroge chaque indexeur avec chaque titre connu, épisode par épisode (le 2026-09-17,
plusieurs minutes, délai dépassé du proxy de la seedbox — « timed out » — et 429 de C411 pour une heure).

- Logique dans `homelab_core::manual_search`, rendu dans `crates/homelabd/src/search_page.rs` (serveur, sans
  JavaScript : la page de résultats se recharge seule tant que la recherche tourne, rien ne bloque une requête HTTP).
- **C411 par identifiant TMDB** (une requête par saison ou par film, via Prowlarr), dans le budget par clé : la page
  dispose de la réserve `[indexers] manual_reserve` (10 par heure et par clé). Puis, si l'identifiant ne donne rien ou
  pour une saison, `[manual_search] text_queries` (2) noms de la fiche en texte libre (cours d'animés publiés sous leur
  propre titre, marqués « autre saison »). Plafond atteint, indexeur en pause ou fiche sans identifiant : la page le dit.
- **Nyaa n'est plus interrogé par la page** depuis le budget commun du 2026-09-17 (le bandeau de la page et la doc de
  `manual_search::run` le disent encore : texte périmé) ; il ne sert qu'en secours automatique pendant une panne de
  C411. Un lien **magnet** (secours) est ajouté directement au qBittorrent du même côté avec l'étiquette `homelab:`.
- **Rien n'est filtré** : toutes les releases sont montrées, triées comme le choix automatique (sans écart d'abord,
  puis bonne œuvre et bonne saison, langue, saison complète, résolution, codec, sources) et **marquées** :
  « sans français », « VOSTFR », « plus de 1080p », « hors profil », « aucune source », « autre saison »,
  « saison inconnue », « titre non reconnu ». L'admin garde la main : un pack VOSTFR reste téléchargeable.
- « Télécharger » passe par `series_search::send_release` (même chemin que les recherches automatiques) ;
  `dry_run` journalise sans rien envoyer. Résultats gardés `results_ttl_mins` (30) en mémoire.

## Vue d'ensemble des membres (Jellyfin Enhanced)

Chaque membre voit les mêmes onglets que l'admin : **Demandes** (toutes, avec leur demandeur) et **Calendrier**
(VPS et seedbox). Réglages, tous globaux (les nouveaux comptes en héritent) :
`DownloadsFilterByUserRequests = false`, `CalendarFilterByLibraryAccess = false`, `SonarrInstances` et
`RadarrInstances` avec les deux machines. Côté Jellyseerr, le droit **« voir les demandes » (bit 16384)** est posé
sur les comptes actifs, dans `defaultPermissions`, et à chaque activation (`accounts::granted_bits`, réglage
`[accounts] jellyseerr_view_requests`) : sans lui, Jellyseerr ne renvoie que les demandes du membre. Ce droit est
en lecture seule — ni validation ni refus. Le plugin garde 30 min en cache le lien compte Jellyfin ↔ compte
Jellyseerr : un changement de droits met ce temps à se voir.

## Page /status : « Rien ne bouge »
Les torrents terminés qu'aucune fiche n'a voulus (`no_match` et `nothing_importable` de `torrent_import`, 15 au plus,
les plus récents d'abord) sont listés sous le tableau des tâches : sans ça, un téléchargement fini restait invisible.
Autres sections : « Saisons sans release » (`uncovered` de `series_search`), « Canari de lecture », « Catalogue jamais
regardé » (`catalogue_report`).
Depuis le 2026-10-07 : « Dernière erreur » sous chaque tâche (même règle que la ligne ↳ de `homelabctl status`),
section « Alertes admin · N livrée(s), M non livrée(s) » (dernières alertes, canaux), et tout texte venu d'ailleurs
(nom de torrent, côté, titre de série, objet d'alerte) échappé au rendu.

## Alertes admin (2026-10-07)

Un seul point d'entrée, `alerts::admin` : mail (`CHAT_ADMIN_EMAIL`, repli `GUIDE_CONTACT_EMAIL`) **et** salon Discord
admin (`[discord] admin_alerts`). Chaque envoi laisse une ligne « alerte envoyée » (objet et canaux seulement, jamais
l'adresse, l'URL du webhook ni le corps), ou « alerte NON livrée » / « alerte sans canal », et une entrée dans
`state.alerts` (compteurs livrées/non livrées, 20 dernières). Rien en dry-run. Une alerte envoyée par `homelabctl`
(sauvegarde) n'y figure pas : la CLI n'écrit pas l'état.

- **Règle de réessai** : un appelant qui note « déjà signalé » ne le fait que si au moins un canal a pris l'alerte
  **ou si aucun n'est configuré** (`!alerts::retry_later(sent, alerts::configured(ctx))`) : sans canal, réessayer à chaque
  passage ne ferait qu'écrire un avertissement de plus. Appliquée par `scheduler::settle`, `hls_loop_watch`,
  `indexer_unblock` (alerte « toujours en panne » et « C411 en panne »), `seedbox_health` et `alerts::capacity`
  (alignement du 2026-10-08 : les trois derniers ne regardaient que `delivered`, et l'alerte « C411 en panne » ignorait
  le résultat de l'envoi).
- **Mémoire des surveillances quotidiennes** (2026-10-08) : `cert_watch`, `backup_watch` et `diun_watch` passent aussi à
  chaque démarrage de homelabd. `alerts::watch` garde dans `state.watch_alerts` (par tâche : date + clé du défaut,
  `#[serde(default)]`, compatible avec l'état d'avant) la dernière alerte **partie** ; il retient la suivante tant que
  c'est le même défaut (`watch_due`) et qu'il s'est écoulé moins de `WATCH_REPEAT_SECS` (20 h, donc le passage
  planifié du lendemain passe toujours). Un défaut différent (autre clé : `cert_watch` = ses `Problem::kind` triés,
  `backup_watch` = `stale`/`missing`, `diun_watch` = empreinte des erreurs) alerte aussitôt ; un passage sain
  (`watch_clear`) efface la mémoire, une rechute alerte normalement. Une alerte non livrée alors qu'un canal existe
  n'est pas notée : la tâche réessaie à son passage suivant.
- **Tâche en échec répété** (`[alerts]`) : une alerte quand une tâche échoue `fail_streak` (6) fois de suite **et**
  depuis au moins `fail_minutes` (30) minutes (une tâche toutes les 20 s échoue 6 fois en 2 min sans que ce soit une
  panne), puis un message « Tâche rétablie » à son retour. Les erreurs isolées ne préviennent pas. Au plus
  `fail_alerts_max` (3) alertes de ce type par `fail_alerts_window_mins` (10) : au-delà, la série n'est pas marquée
  signalée et repart au prochain échec. La place est **réservée dans la même mise à jour d'état que la décision**
  (`AlertStats::reserve_streak`, 2026-10-08) : `alerts::admin` n'enregistre l'alerte qu'après l'envoi (SMTP puis Discord),
  et deux tâches au seuil à quelques secondes d'écart voyaient toutes deux de la place. La réservation (`reserved`, jamais
  écrite sur disque) est rendue dès l'envoi fini (`release_streak`) : l'alerte enregistrée, livrée ou non, compte à sa place ;
  une réservation orpheline (envoi interrompu) sort de la fenêtre d'elle-même. Mémoire dans `RunInfo` (`fail_streak`, `fail_since`, `streak_alerted`,
  `last_error`, `last_error_at`, `errors_by_day` sur 14 jours).
- **Capacité** : `alerts::crossing` (pure) + `alerts::capacity` : une alerte par franchissement de
  `tasks.disk_pressure.alert_pct` ou `tasks.seedbox_health.quota_alert_pct` (85 ; 0 = désactivé), réarmée
  `capacity_rearm_pts` (3) points sous le seuil, mémoire `state.capacity_alerts`.
- **Unités systemd** : `systemd/homelab-alert@.service` + `scripts/homelab-alert.sh` (webhook lu dans `.env`, jamais
  affiché ; titre « <unité> en échec », résultat et 20 dernières lignes du journal ; au plus un message par unité et
  par heure, le suivant dit combien d'échecs ont été tus ; `--dry-run`). `OnFailure=homelab-alert@%n.service` sur
  `homelab-backup`, `homelabd-watchdog`, `seedbox-mount-watch` et `jellyfin-transcodes-purge`. Les minuteurs à la
  minute sont en `LogLevelMax=notice` : seules les lignes préfixées `<5>`/`<4>` par leurs scripts restent au journal
  (purge silencieuse quand il n'y a rien à faire, `find -ignore_readdir_race`, conteneur arrêté = sortie propre,
  `SuccessExitStatus=2` pour « saturé après purge », déjà signalé par le script). `homelab-seedbox-mount.service` :
  `SuccessExitStatus=143` (un arrêt voulu n'est plus un échec ; un SIGTERM externe ne relance plus rclone).
- **Journal système** : `systemd/journald-homelab.conf` (`SystemMaxUse=2G`, `MaxFileSec=1day`, `MaxRetentionSec=45day`)
  posé par `homelabctl install` dans `/etc/systemd/journald.conf.d/homelab.conf` (réécrit seulement s'il diffère),
  puis `sudo systemctl restart systemd-journald` (à lancer soi-même, ne coupe aucun service). Avant : 500 Mo, et le
  fichier `user-1000` (homelabd, rclone) ne tournait presque jamais, d'où 1 à 10 jours d'historique en dents de scie.

## Pages d'administration (2026-09-23, portes du 2026-10-07)

`crates/homelabd/src/admin_auth.rs` + `client_addr.rs`. Session par cookie (`/connexion`, voir [runbooks/homelabd.md](runbooks/homelabd.md#5-pages-dadministration-crateshomelabdsrcadmin_authrs-client_addrrs)) ; en plus :

- **Adresse du client** : dernier saut de `X-Forwarded-For`, seulement si le pair TCP est dans `[web] trusted_proxies`
  (172.18.0.0/16) et n'est pas une adresse de l'hôte (127.0.0.1, 172.18.0.1 : `bind` UDP d'essai, en cache). Avec
  `[web] trusted_proxy_container = "npm"`, seul le pair à l'adresse actuelle de ce conteneur est « confirmé » :
  adresse relue en tâche de fond (`docker inspect --type container`, toutes les 60 s, et 10 s après un pair inconnu du
  réseau, par exemple NPM recréé) ; les requêtes lisent un instantané et n'attendent jamais Docker (sauf la toute
  première, 4 s au plus). Docker muet ou conteneur vide : le réseau seul décide, pour les limites par adresse.
- **IP de la maison** (`HOMELABD_ADMIN_TRUSTED_IPS`) : session d'office seulement depuis un NPM **confirmé** ; Docker
  muet ⇒ formulaire `/connexion` (le cookie d'un an couvre l'entre-deux), une seule ligne « docker inspect en échec »
  par panne.
- **Hôte** : les chemins d'administration (`/`, `/accounts*`, `/recherche*`, `/status`, `/status.html`, `/onboard`,
  `/connexion`, `/admin`, `/admin/*`) répondent 404 si `Host` n'est ni l'hôte de `ONBOARD_PUBLIC_URL` ni `localhost`
  ou une adresse locale ou privée ; sans `ONBOARD_PUBLIC_URL`, aucun filtre (avertissement au démarrage). Les pages
  publiques (`/premium*`, `/inscription`, `/bienvenue`, `/premiers-pas`, `/guide`, `/paypal/webhook`, `/don`, `/chat`,
  `/compte`, `/health`) restent servies partout.
- **API de la CLI** (`/admin/*`) : pair local (127.0.0.1, ::1) sans `X-Forwarded-For` seulement, sinon 404 ; un refus
  = une ligne `warn` par adresse et par 15 min (100 adresses au plus), les suivants en `debug` ; jeton incorrect =
  `warn`. `homelabctl` appelle `http://127.0.0.1:<port de listen>` avec `x-onboard-token`.
- **Échecs** : jetons comparés à temps constant partout (`admin_auth::token_matches`) ; un POST sans session sur un
  chemin d'administration compte ses 401 comme `/connexion` (10 par IP et par 15 min, puis 429) ; le plafond global
  (100) ne bloque jamais un appel local. `POST /onboard` est fermé si aucun jeton n'est configuré.
- **Jeton absent du HTML** : la couche remplace le jeton par un jeton de formulaire (HMAC du jeton) dans les pages, et
  fait l'inverse dans un POST qui a une session. Contrôle : le code source de `/accounts` ne contient plus le jeton.

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

Trois entrées, un seul flux (`onboard::run`) : `homelabctl onboard <user> <email> [--password]` (passe par l'API
du daemon), le formulaire admin `/` (session d'administration, voir « Pages d'administration »), et la **page publique
`/inscription`** (`[onboard] public_signup`) ; `user_poller` reprend aussi les « Add User » de Jellyseerr.
Le compte Jellyfin est créé avec un mot de passe aléatoire **jamais communiqué**, importé dans Jellyseerr
(e-mail, droits), puis suspendu sauf `accounts.new_accounts_premium`.

**Lien de bienvenue** (`homelab_core::welcome`, 2026-09-20) : le mail ne contient qu'un bouton vers
`/bienvenue/<jeton>` (jeton de 32 octets aléatoires, stocké **haché** dans `state.welcome_links`, valable
`[onboard] link_ttl_mins`, un seul usage, un seul lien actif par compte). La page fait choisir le mot de passe
(8 caractères min, sans le pseudo ; `POST /Users/{id}/Password` en admin, Jellyseerr suit), consomme le lien et
affiche les accès (Jellyfin, Jellyseerr, guide). Lien expiré ou consommé → page « recevoir un nouveau lien »
(`POST /bienvenue/renouveler`, réponse identique que l'adresse soit connue ou non, `renew_per_hour` par adresse) :
c'est aussi le « mot de passe oublié ». À l'**activation** depuis `/accounts`, mail « ton compte est actif » (lien
`activated`, même page si le mot de passe n'est pas encore défini). `/accounts` montre l'état du lien et permet
de le renvoyer ; `homelabctl accounts link <compte>` idem ; `homelabctl mail-test <adresse>` envoie le mail avec
un lien de démonstration (page en lecture seule). Les liens vivent dans l'état **du daemon** : la CLI passe par
`POST /admin/link` et `/admin/mail-test` (jeton en en-tête). `cleanup` purge les liens finis depuis > 7 j.

**Inscription publique** : pseudo + e-mail + case « validé par l'admin » + champ leurre ; rate limit IP
(`web.rate_limit_secs`), plafond `max_signups_per_day` (toutes adresses), adresse déjà connue → réponse neutre sans
création, pseudo pris → message. Le compte est créé suspendu, le membre reçoit son lien, l'admin un mail
« nouveau compte à activer » (`CHAT_ADMIN_EMAIL`, repli `GUIDE_CONTACT_EMAIL`).

**Mails** (`mail.rs`) : `curl smtps` vers le SMTP de `.env`, message MIME construit ici — `Date`, `Message-ID`,
`Reply-To` (contact), sujet/noms RFC 2047, corps quoted-printable UTF-8, `multipart/alternative` texte + HTML
pour les membres (`assets/mail/welcome.html`), texte seul pour l'admin. Jamais d'identifiant ni de mot de passe
dans un mail.

## Discord

Webhooks (aucun bot) : `DISCORD_WEBHOOK_MEMBERS` (salon des membres) et `DISCORD_WEBHOOK_ADMIN` (salon privé) dans
`.env` ; `[discord] announcements` / `admin_alerts` dans `homelab.toml`. `homelab_core::discord` : embeds avec limites
de l'API, reprise sur 429, URL masquée dans les journaux. `homelabctl discord apply` crée ou met à jour, dans les 4
Arrs, « Discord membres » (imports : Radarr `onDownload`/`onUpgrade`, Sonarr `onImportComplete`/`onUpgrade`) et
« Discord admin » (`onHealthIssue`/`onHealthRestored`/`onManualInteractionRequired`), et active l'agent Discord de
Jellyseerr (demande, validée d'office, refusée, disponible) ; `test` poste un message d'essai sur chaque salon et
déclenche le test Jellyseerr ; `remove` retire tout. Ce que homelabd poste lui-même : annonces du tchat (membres),
alertes admin via `alerts::admin` (mail + Discord, voir « Alertes admin »), relances de `stack_health`. L'URL d'un
webhook est un secret : retirée des erreurs (`without_url`) et masquée dans les journaux.

## Guide des nouveaux membres

`GET /guide` (public, sans jeton, sur l'onboarder) : `crates/homelabd/assets/guide.html`, captures
intégrées, affiches floutées. Le fichier ne contient ni adresse ni contact (dépôt public) : `guide.rs`
remplace `{{JELLYFIN_URL}}`, `{{JELLYSEERR_URL}}` (et `_HOST`) et `{{CONTACT}}` depuis `.env`
(`JELLYFIN_PUBLIC_URL`, `JELLYSEERR_PUBLIC_URL`, `GUIDE_CONTACT_EMAIL`, `GUIDE_CONTACT_DISCORD`). Pour le
modifier : sources et captures dans `backups/guide-draft-20260915/` (`guide.final.tmpl.html`,
`build_final.py <asset> <aperçu>`, script de captures `shots.js` avec un compte ordinaire temporaire,
aucune demande envoyée), puis rebuild de homelabd. Validé par l'utilisateur le 2026-09-15.

## Aide à la qualité de lecture

`branding/jellyfin/gc-quality-helper.js`, déposé dans JavaScript Injector (« Groscailloux Qualité ») par
`sudo scripts/jellyfin-js-apply.py`, qui synchronise aussi les scripts du tchat et de « Lire sur »
(sauvegarde de la configuration du plugin avant écriture). Le script relève l'état du `<video>` toutes les
2 s : image figée ou tampon vide alors que la lecture n'est ni en pause ni en recherche = un blocage (les
événements `waiting` ne suffisent pas, hls.js les absorbe) ; les 10 premières secondes ne comptent pas et un
même blocage n'est compté qu'une fois par 15 s. Au 3ᵉ blocage en 3 minutes, un bandeau **dans la page**
(jamais `window.confirm`) propose de réduire la qualité : il pilote le menu du lecteur (roue crantée →
Qualité) et choisit le palier le plus haut sous 2 Mbit/s, ce qui garde la position et mémorise le choix pour
l'appareil. Aucun réglage serveur n'est touché : pas de `RemoteClientBitrateLimit`, la lecture directe reste
la règle. Sur téléviseur, les relevés passent à 4 s, le bandeau donne le focus au bouton, se ferme à la touche Retour et s'efface après 15 s. Banc d'essai : compte ordinaire temporaire, navigateur jetable, connexion bridée à 2 Mbit/s
(`Network.emulateNetworkConditions`) — mesuré le 2026-09-16 : 4,98 → 1,56 Mbit/s, 57 s d'image par minute
contre 15 avant.

## Tchat des membres

« Aide et annonces » (nom de la bulle et du panneau depuis le 2026-10-08) : bulle en haut à droite de l'interface web de
Jellyfin (navigateur, Jellyfin Desktop, applis Android/iPhone ; pas sur les télés ni les applis natives). Salons
`annonces` (modérateurs seulement), `entraide` (tout le monde ; l'ancien salon `discussion` y a été fusionné le 08/10 par
`merge_discussion`, à l'ouverture de `chat.db`, et `Channel::parse` accepte encore « discussion » comme synonyme), et un
fil privé `prive:<id Jellyfin>` par membre, lisible par lui et par les modérateurs (`[chat] moderators`). Un compte neuf
voit comme lus les messages publics de plus de `[chat] new_member_read_days` (14) jours (`ChatStore::init_reads`, une
fois). Règles côté client (sondages, focus, Échap, bandeau) : [runbooks/tchat.md](runbooks/tchat.md).

- **Chemin** : script chargé par JavaScript Injector (`branding/jellyfin/gc-chat-loader.js`) →
  `/gc-chat/app.js` → API `/gc-chat/api/*`, publiées par NPM (hôte Jellyfin) vers homelabd `/chat/*`.
- **API** : `GET /me` (salons, non-lus, dernière annonce non lue), `GET|POST /messages`,
  `DELETE /messages/{id}`, `POST /read`, `GET /private` (modérateurs). 401 sans session valide, 403 hors
  droits ou hors `beta_users`, 429 au-delà du débit (1 message / 3 s, 30 / 10 min).
- **Règles** (`homelab_core::chat`, testées) : texte nettoyé, 2 000 caractères ; suppression de son message
  pendant 15 min, de tout message pour un modérateur (message conservé, marqué supprimé).
- **Mails** (nommés « Aide et annonces ») : toutes les minutes, s'il y a de nouveaux messages d'entraide ou privés de
  membres et que le dernier récapitulatif a plus de `moderator_mail_interval_mins` (15), un mail à `CHAT_ADMIN_EMAIL`. Annonce avec « envoyer aussi par
  mail » : un mail par compte actif ayant une adresse valide dans Jellyseerr (2 s d'écart), sauf l'auteur.
- **Messages privés de l'admin** : onglet Privé → « Nouveau message privé », un ou plusieurs membres actifs
  (`GET /members`, `POST /direct`, modérateurs seulement) ; chacun reçoit le message **séparément** dans son
  fil privé (personne ne voit les autres destinataires), avec mail facultatif (`direct_mail`, adresse
  Jellyseerr). Côté membre : badge et bannière « Message de l'admin » sur l'accueil (prioritaire sur
  l'annonce), qui disparaît une fois le message lu ou quand il répond.
- **Client** : messages toutes les 5 s panneau ouvert (10 s sur télé), `/me` en filet à 120 s (20 s quand aucun salon ni
  fil n'est interrogé) ; panneau fermé : 60 s, rien pendant une lecture, 3 min après 5 min sans activité ; `/messages` et
  `/read` renvoient l'état des non-lus ; bulle masquée pendant la lecture ; bannière de la dernière annonce non lue sur
  l'accueil ; aucun HTML de message interprété.
- **Annonce depuis la ligne de commande** : `homelabctl chat announce [fichier] [--author <modérateur>] [--mail]
  [--no-discord]` (texte lu dans le fichier ou sur l'entrée standard) → `POST /admin/chat/announce` (jeton
  `HOMELABD_ONBOARD_TOKEN`, hors du préfixe `/gc-chat/` publié par NPM). L'auteur doit être dans `[chat] moderators`
  et exister dans Jellyfin ; le texte est nettoyé comme un message ordinaire puis **découpé** en messages ≤ `max_chars`
  entre deux paragraphes (`chat::split_parts`, testé) ; Discord (salon des membres) et le mail facultatif reçoivent le
  texte entier, une fois. `--dry-run` affiche le nombre de messages sans rien publier.
- **Couper** : `[chat] enabled = false` (+ restart homelabd) et désactiver le script dans JavaScript Injector.

### playback_canary — 15 min

Un vrai transcodage HLS (PlaybackInfo forcé h264/aac 2 Mbit/s, appareil `gc-canary`, compte admin) sur un petit
épisode, en alternance VPS puis seedbox : `master.m3u8` → `main.m3u8` → deux segments. Échec si un segment est
vide ou tronqué (tmpfs plein), si le premier segment dépasse `max_first_segment_secs` (20 s), ou si Jellyfin refuse
le transcodage. Alerte admin (mail + Discord) au premier échec, message de retour à la normale, état dans
`state.canary` et ligne « Canari de lecture » sur `/status.html`. Sauté si `skip_if_transcodes_at_least` (2)
transcodages de membres sont déjà en cours. Le transcodage est arrêté proprement (`DELETE /Videos/ActiveEncodings`).
Résumé d'un passage réussi : **`lecture OK`**, toujours identique (2026-10-08) : le côté et la latence changeaient à chaque
passage, aucun n'était « calme » et une ligne `run_done` partait en `info` toutes les 15 min ; ils restent dans
`state.canary.last_detail` (/status.html) et dans le journal en `debug` (le retour après un échec reste en `info`).

## Langue par membre (v1.19, 2026-09-21)

Chaque compte reçoit à l'onboarding (`[accounts] audio_language`, `subtitle_language`, `subtitle_mode`) : audio
préféré `fre`, sous-titres `fre`, mode **Smart** (sous-titres seulement quand l'audio n'est pas en français),
`PlayDefaultAudioTrack = false` (Jellyfin choisit la piste française d'un MULTi au lieu de la piste « par défaut » du
fichier). Rattrapage du 2026-09-21 sur les comptes sans préférence (sauvegarde `backups/jellyfin-language-20260921/`).
Dans « Mon compte », le membre choisit « Français quand il existe » ou « Toujours en VO, sous-titres français »
(`POST /compte/api/language` : audio `[accounts] vo_audio_language` (`jpn`) + sous-titres `Always`, mémorisation des
pistes coupée ; dans les clients web, le script Mon compte bascule ensuite sur la piste de la langue d'origine, voir
[runbooks/lecture-et-transcodage.md](runbooks/lecture-et-transcodage.md#7-langue-audio-et-sous-titres-des-comptes)).

### Mode VO « Langue d'origine » (`vo_native`, préparé le 2026-10-08, coupé par défaut)

`[accounts] vo_native = true` fait poser au mode VO la préférence native de Jellyfin 12 « Langue d'origine »
(`AudioLanguagePreference = "OriginalLanguage"`) au lieu de `vo_audio_language` : le serveur choisit la piste d'origine de
chaque titre (métadonnée `OriginalLanguage`, remplie par `original_language`, héritée de la série). Tant que l'interrupteur
est à `false`, rien ne change (Mon compte, onboarding, garde). Code : `homelab_core::vo_native` (décisions pures testées).

- **Règle sans régression** : un compte en VO ne passe en « Langue d'origine » que si aucun client hors de
  `vo_native_clients` (jellyfin-web et les applis qui l'embarquent) n'apparaît pour lui, ni dans ses lectures depuis
  `vo_native_days` (60) jours (Playback Reporting), ni dans ses sessions ouvertes, ni dans ses appareils enregistrés
  (`GET /Devices` lu en entier, filtré sur `LastUserId`). Sinon il est **gardé** sur `vo_audio_language` (`jpn`), comme
  aujourd'hui : le bogue de Jellyfin 12.1 (piste d'origine marquée « Original » sans être par défaut) donnerait la VF à
  l'appli Android TV / Fire TV, qui n'impose pas d'index. Toute session compte, capacités déclarées ou non : l'appli
  Android TV ne déclare `PlayableMediaTypes` qu'à son démarrage à froid et Jellyfin les oublie à chaque redémarrage
  (2 sessions Android TV sur 2 sans capacités en prod le 09/10). Seuls les services qui ne lisent jamais sont écartés,
  par leur nom (`vo_native_ignored_clients` : Seerr, Jellyseerr). Une des trois sources illisible = gardé. Ajouter
  `Jellyfin Android TV` à `vo_native_clients` est le compromis possible, au choix du propriétaire. Un compte gardé
  par un vieil appareil peut être promu après sa déconnexion (Mon compte, « Appareils connectés »), puis `vo-native`.
- **Mon compte** (`POST /compte/api/language`) et **onboarding** (s'il est réglé sur la VO) passent par
  `vo_native::decide` ; chaque décision est notée dans `state.vo_native` (avant, après, origine, clients hors liste).
- **Migration** : `homelabctl accounts vo-native --dry-run` (plan par compte, plus la médiathèque : part des films et
  épisodes dont la langue d'origine est connue, titres avec piste japonaise sans langue d'origine, titres touchés par le
  bogue ; ~15 Mo lus, hors soirée), puis sans option → `POST /admin/vo-native` : refusé si `vo_native = false` ; sauvegarde de
  la configuration complète des comptes qui changent (`backups/vo-native-<date>-native-avant.json`, 0600) AVANT tout
  changement, puis un compte à la fois (`AudioLanguagePreference`, `PlayDefaultAudioTrack` et mémorisation des pistes
  coupés). Relancer la commande plus tard promeut les comptes gardés dont l'historique est redevenu sûr.
- **Retour** : `homelabctl accounts vo-classic [--dry-run]` remet `vo_audio_language` sur tout compte VO en « Langue
  d'origine » (sauvegarde `…-classic-avant.json`), puis `vo_native = false` et redémarrage de homelabd, puis `vo-classic`
  une seconde fois (un membre a pu rechoisir la VO entre-temps).
- Le script Mon compte reste le filet : il ne fait rien quand la piste reçue n'est pas la française, lit la langue
  d'origine sur la fiche Jellyfin et ne demande TMDB que pour une fiche sans langue d'origine. Banc :
  `tools/bench/bench.sh --offline tools/tests/compte-vo/compte-vo.js desktop tv`.

### vo_native_guard — 30 s

Avec `vo_native = true` : un compte en VO réglé sur « Langue d'origine » qui a une session ouverte (même sans capacités
déclarées) ou un appareil enregistré sur un client hors de `vo_native_clients` (appli Android TV ou Fire TV, Chromecast,
client tiers) revient sur `vo_audio_language` (`state.vo_native`, origine `garde`) : en général avant sa première
lecture sur ce client ; sinon dès cette lecture (sondage toutes les 30 s). Reste un trou étroit : la première lecture
d'un appareil neuf lancée moins de 30 s après sa connexion, sur un titre touché par le bogue ou sans langue d'origine,
part en VF. Jamais l'inverse (la promotion passe par `homelabctl accounts vo-native`). Un compte en mode `fr` qui a
choisi « Langue d'origine » lui-même n'est pas concerné ; un service de `vo_native_ignored_clients` (Seerr) non plus.
Interrupteur coupé : aucun appel. Aucun compte en « Langue d'origine » : une lecture (`/Users`) ; sinon trois
(`/Users`, `/Sessions`, `/Devices`). Appareils illisibles : sessions seules, passage en échec. Passage sans rien à
faire : « rien à garder » (journal en `debug`). Clé : `[tasks.vo_native_guard] interval_secs`.

## Suivi des demandes dans l'onglet Demandes (v1.19)

`GET /compte/api/requests` (jeton Jellyfin, cache 20 s) : pour chaque demande Jellyseerr approuvée, l'**étape** et
l'**avancement** (`homelab_core::requests_progress`, pur et testé) : *recherche* (état `unknown_series` /
`movie_search`, prochaine tentative d'après les délais de `series_search`/`movie_search`, « introuvable » si des
épisodes sont sans release), *téléchargement* (files des 4 Arrs par `externalServiceId` **et** torrents étiquetés
`homelab:series=<id>[:season=<n>]` / `homelab:movie=<id>` des deux qBittorrent — côté seedbox c'est le seul chemin,
`series_search`/`movie_search` n'y passent jamais par la file de Sonarr/Radarr ; un hash déjà en file Arr ne compte
qu'une fois, un torrent fini attend `torrent_import` (« ajout »), un torrent que `torrent_import` a rejeté affiche
« pas rangé » : `%` = 1 − Σ sizeleft / Σ size, ETA = `timeleft`/`eta` max + 2 min d'import + un passage de
`seedbox_refresh`), *ajout* (fichier
présent côté Arr, pas encore vu par Jellyfin), *disponible* (statut média Jellyseerr). Le script « Mon compte »
détecte les cartes `.je-request-card` de Jellyfin Enhanced et y insère une barre + texte, rafraîchis toutes les
30 s ; sur téléviseur, pourcentage seul.

## Sous-titres (seedbox)

**Le problème** (mesuré du 19 au 21/09) : Jellyfin extrait une piste incrustée en relisant **tout le fichier** par le
lien seedbox — 108 à 757 s par épisode ; le lecteur web attend (les sous-titres arrivaient après plusieurs minutes) ou
abandonne dès qu'on change de piste (499 dans le journal NPM). Sur le VPS c'est quelques secondes. Inventaire seedbox :
493 fichiers avec piste française **ASS** (animés stylés), 1 103 **SRT**, 41 PGS (image), 282 sans piste française.

### subtitle_sync — 5 min

Pour chaque item Jellyfin sous `/seedbox/media` (compte admin, `Fields=Path,MediaStreams,DateCreated`, **les plus
récents d'abord**) ayant une piste française incrustée texte sans fichier externe correspondant, la tâche lance **sur la
seedbox** (ssh `[tasks.indexer_unblock] ssh_host`, script `scripts/seedbox/gc-extract-sub.sh` installé dans `~/bin/`)
une extraction **à codec identique** (`ffmpeg -c:s copy`, `nice`/`ionice`, un à la fois, disque local, rien sur le
lien) : la piste choisie par ffprobe d'après codec et drapeaux (les index Jellyfin bougent dès qu'un externe existe) →
`.fr.default.ass` (complète, marquée `default` : Jellyfin la met devant), `.fr.forced.ass`, `.fr.hi.ass`
(malentendants, à part), `.fr.srt` pour une piste SRT ; d'une ASS complète, `gc-ass2srt.py` dérive un **`.fr.srt` sans
les panneaux** (styles Sign/Title/OP/ED…, balises retirées) pour AirPlay et les téléviseurs, régénéré à chaque
extraction. Puis `vfs/refresh` du dossier et `jellyfin::refresh_streams` (`Items/{id}/Refresh?MetadataRefreshMode=
FullRefresh&ReplaceAllMetadata=false&ImageRefreshMode=None`, **seul moyen** de voir un fichier annexe ; ni l'analyse de
05 h, ni `Library/Media/Updated`, ni un Refresh « Default », vérifié le 21/09). `max_per_run` 40 items et `max_seconds` 420 par
passage (le planificateur coupe une tâche à 600 s ; une extraction lit tout le fichier, ~30 s), jamais un item en
cours de lecture (repris au passage suivant), dry-run = liste. Résultat côté membre : l'ASS externe est
rendu par libass **exactement comme l'incrusté**, sans attente ; en VO Jellyfin le choisit d'office ; en audio français
la piste forcée incrustée reste le choix (mode Smart). Nouveaux imports : l'item apparaît par `seedbox_refresh`, le
passage suivant extrait (≤ 5 min).

**Lecture de la médiathèque** (2026-10-07) : balayage complet une fois par jour, au premier passage qui suit
`full_scan_hour` (5, heure locale) et au premier passage après un démarrage ; c'est le seul qui élague
`subtitle_tries`. Les autres passages relisent ce que Jellyfin a sauvé depuis le début du passage précédent moins
1 h (`MinDateLastSaved`, compte admin, vérifié sur 12.1), le reliquat du passage précédent (au-delà de `max_per_run`,
en lecture, budget atteint, échec passager) et les essais dont la relance est due (`Ids=`, par 80). Mémoire en
processus, mise à jour seulement après un passage abouti (un échec élargit la fenêtre suivante). Avant : 9 pages de
1,8 Mo et ~14 s de CPU Jellyfin toutes les 5 min pour « rien à extraire » 98 fois sur 100 ; maintenant ~0,4 s. Le
balayage complet ajoute « ; balayage complet » au résumé ; après extraction : « X à reprendre, W en relance
différée ». Un passage court oublie les essais des éléments qu'il vient de relire et qui n'ont plus rien à extraire
(`prune_seen`, 2026-10-08) : « N en attente de relance » ne gonfle plus après des extractions (16 au lieu de 3 le 08/10
après 13 extractions, jusqu'au balayage de 05:00), et ces éléments ne sont plus relus par `Ids=` six heures plus tard ;
un élément non relu garde son essai.

Bazarr (seedbox, `[seedbox] bazarr_url`) garde son rôle d'origine : profil « Français (+anglais) »,
`use_embedded_subs = true` (une piste incrustée compte), `audio_exclude = True` sur le français, six fournisseurs dont
OpenSubtitles.com (compte saisi dans son interface) — il ne télécharge que ce qui manque vraiment. Sonarr/Radarr seedbox
le préviennent à l'import (connexion « Bazarr », webhook, 21/09). Sauvegarde d'avant : `backups/bazarr-20260921/`.
Pièges : un `POST /api/system/settings` pendant une recherche Bazarr répond 504 et **n'écrit pas** `config.yaml`
(base verrouillée) : relire le fichier après, ou redémarrer Bazarr (`app-bazarr restart`) et reposter. `[seedbox]
bazarr_url` + `SEEDBOX_BAZARR_API_KEY` : client `clients::bazarr` (historique), gardé pour la page d'état.

## Abonnés et cycle premium (v1.18, 2026-09-20)

Une fiche par compte dans `state/subscriptions.db` (SQLite, sauvegardée) : statut (`essai`, `actif`, `échéance
dépassée`, `suspendu`, `offert`, `exempt`, `à qualifier`), échéance, source (`paypal`, `manual`, `trial`, `import`),
abonnement PayPal lié, code de parrainage, historique horodaté. Module `homelab_core::subscriptions` (décisions pures,
testées) + `subscription_ops` (ce qui touche Jellyfin/PayPal/mails, toujours via `accounts::set_premium`).

- **Page `/premium`** : le membre saisit son nom de compte, PayPal reçoit ce nom en `custom_id` ; à l'approbation, la
  page poste `/premium/lier` qui relit l'abonnement chez PayPal, le rattache et **active tout de suite** (page
  `/premium/merci`). `/premium/activate` : rattachement par identifiant `I-…` (ou demande à l'admin, comme avant).
- **Webhook `POST /paypal/webhook`** (hôte public de `/premium`, sans liste d'accès) : signature vérifiée par
  `verify-webhook-signature` (`PAYPAL_WEBHOOK_ID`), chaque événement traité une fois (`paypal_events`).
  `ACTIVATED`/`RE-ACTIVATED`/`PAYMENT.SALE.COMPLETED` → paiement (échéance = prochaine facturation PayPal, sinon
  `period_days`) ; `CANCELLED`/`SUSPENDED`/`EXPIRED`/`PAYMENT.FAILED` → note dans l'historique, le compte va au bout
  de sa période ; remboursement → alerte admin. 5xx = PayPal réessaie.
- **`subscription_cycle` — 1 h** : fiches créées pour les comptes qui n'en ont pas (actifs → « à qualifier », jamais
  suspendus tant que l'admin n'a pas tranché). `decide` (décision pure, testée) ne suit que deux sortes de fiches
  (décision du propriétaire, 2026-10-08) :
  - **essai de l'inscription publique** (`is_trial` : source `trial` posée par `start_trial`, statut essai ou grâce) :
    rappel J-1 (jamais un palier égal à la durée de l'essai), grâce de `grace_days` à l'échéance, puis suspension ;
  - **fiche liée à PayPal** (`paypal_sub_id`) : qui prélève (ACTIVE) → une information de renouvellement sans lien au
    palier le plus lointain, marge `paypal_margin_hours`, puis grâce et suspension ; arrêtée chez PayPal → rappels J-7/J-1
    avec lien, grâce, suspension ;
  - **toute autre fiche est gérée à la main** (`managed_by_hand` : actif ou offert posé par l'admin, essai posé par
    l'admin avec `subs set --status trial`, import, exempté, à qualifier, ancienne grâce manuelle) : ni rappel ni
    suspension ; à l'échéance `decide` renvoie `ManualDue`, `run_cycle` regroupe les `ManualDue` d'un passage en **une**
    alerte admin (« Abonnés : échéance à gérer à la main ») et note `due_noted` (colonne de `state/subscriptions.db` +
    événement caché du membre) seulement si l'alerte est partie ; `/accounts` affiche « À gérer (échéance passée) ». En
    `cycle_dry_run`, une ligne dans le récapitulatif, sans note.
  Suspension = `set_premium(false)`, mail « accès en pause », rien de supprimé. Comptes protégés, `[subscriptions] exempt`
  et fiches gardées (compte Jellyfin absent) toujours sautés. Récapitulatif Discord admin à chaque passage qui a agi.
  `cycle_dry_run = true` : tout est annoncé, rien n'est appliqué. Depuis le 2026-10-07 :
  - un essai ne reçoit pas un palier au moins égal à sa durée (pas de J-7 pour un essai de 7 jours ; le J-1 part) ;
  - **abonnement PayPal automatique** (lié et pas arrêté chez PayPal, `auto_renews` ; statut inconnu = actif) : une
    seule information J-7 **sans lien** (« ton abonnement se renouvelle le JJ/MM », `Action::RenewalNotice`), grâce
    seulement `paypal_margin_hours` (36) après la date de facturation ; à la suspension, mail « prélèvement non
    passé, mets à jour ton moyen de paiement dans PayPal » sans lien (un lien = second abonnement) ;
  - **fiches orphelines** (compte Jellyfin disparu) : celles sans rien à perdre (à qualifier, suspendu ou exempté,
    sans échéance ni PayPal : comptes de banc) sont retirées ; les autres, `max_orphan_removals_per_run` (3) au plus
    par passage. Au-delà, ou si Jellyfin renvoie une liste vide ou autre chose qu'une liste (`users()` échoue), aucune
    n'est retirée, le cycle les ignore (ni mail ni suspension), l'admin reçoit une alerte par fiche (événement
    `orphan_held`) et doit relever la clé le temps d'un passage ;
  - un parrainage ne prolonge jamais une fiche offerte, exemptée ou à qualifier sans échéance (ligne d'historique,
    rien compté dans le plafond annuel).
- **`subscription_reconcile` — au démarrage puis 24 h** (l'heure du contrôle est celle du dernier redémarrage de
  homelabd) : relit chaque abonnement PayPal lié ; décision pure `reconcile_decision` :
  - `Apply` : paiement constaté (`last_payment.time` > `paypal_paid_at` + 12 h), jamais déduit de `next_billing_time` ;
    fiche en retard ou importée par CSV : appliqué comme avant ;
  - `Baseline` : fiche d'avant ce suivi (sans `paypal_paid_at`) déjà alignée sur la date PayPal : le dernier paiement
    devient la référence, sans ligne d'historique ;
  - `PendingCharge` : prochaine facturation absente, facturation passée depuis plus de `paypal_margin_hours`, échéance
    de la fiche + marge dépassée sans paiement, ou fiche sans référence dont la facturation a été repoussée de plus de
    `period_days` + 3 j ⇒ alerte « Prélèvement PayPal en attente », une par abonnement et par jour UTC
    (`paypal_events` `pending:<id>:<jour>`), rien n'est prolongé ;
  - `Stopped` : CANCELLED, SUSPENDED, EXPIRED ou inconnu de PayPal (404) noté sur la fiche (`paypal_status`), ligne
    d'historique « abonnement annulé chez PayPal — accès jusqu'au JJ/MM » et information à l'admin au changement ;
    rien n'est suspendu avant l'échéance payée (le cycle suit, sans marge).
  Résumé : « vérifiés=3 corrigés=0 en attente=0 arrêtés=0 ». Un échec de `on_payment` ne bloque plus le passage.
- **Second abonnement** : si le compte trouvé (par `custom_id` ou adresse) a déjà un AUTRE abonnement ACTIVE chez
  PayPal (relu par `find_subscription` ; 404 ne bloque pas), rien n'est rattaché ni prolongé (`Payment::Duplicate`),
  l'admin est alerté une fois par jour et par abonnement, `/premium/lier` renvoie vers `/premium/activate?err=deja`.
  Le remboursement ou la résiliation se font à la main dans PayPal (argent réel). Changer d'abonnement remet
  `paypal_status`/`paypal_paid_at` à zéro ; les webhooks les tiennent à jour.
- **Essai et parrainage** : `/inscription` active le compte en « essai » `trial_days` jours (mail de fin d'essai par
  le cycle) ; code de parrainage facultatif à l'inscription ; au premier paiement du filleul, `referral_days` offerts
  aux deux, plafond `referral_cap_days_per_year`.
- **« Mon compte » dans Jellyfin** : script JavaScript Injector « Groscailloux Mon compte »
  (`branding/jellyfin/gc-account-loader.js`) → `/gc-compte/app.js` (NPM hôte 1 → homelabd `/compte/`) : statut,
  échéance, bouton d'abonnement pré-rempli (masqué tant que l'abonnement PayPal se renouvelle seul ; lien court sans
  clé), appareils connectés (déconnexion vérifiée sur `LastUserId`), lien de
  changement de mot de passe (page `/bienvenue`), code de parrainage, historique. Identité = jeton Jellyfin
  (`/Users/Me`, cache 5 min). Téléviseurs : lecture seule, Retour ferme.
- **Lien signé** : les mails (rappels, suspension) mènent à `/premium?compte=X&k=…`, `k` = HMAC-SHA256 de
  `HOMELABD_ONBOARD_TOKEN` sur `gc-premium-v1|<compte en minuscules>`, 16 hexa, comparé à temps constant. Seul un lien
  signé affiche « déjà un abonnement PayPal actif » à la place du bouton (fiche locale, aucun appel PayPal) : sans
  `k` ou avec une clé fausse, page normale, et la page publique ne dit plus qui paie. Renouveler le jeton rend les
  clés des anciens mails caduques, sans risque (le serveur refuse toujours un second abonnement).
- **Admin** : colonne « Abonnement » sur `/accounts` (statut, échéance, source) et décision par compte (prolonger de
  N jours, actif, offert, exempté, à qualifier, suspendu) ; `homelabctl subs list | set <compte> --status … [--days N]
  | extend <compte> --days N | link <compte> --sub I-… | import <csv PayPal> | paypal [--webhook <url>]`.
- **Secrets** : `PAYPAL_ENV` (sandbox|live), `PAYPAL_CLIENT_ID`/`PAYPAL_SECRET`/`PAYPAL_PLAN_ID`/`PAYPAL_WEBHOOK_ID`
  (ou `PAYPAL_SANDBOX_*`), `PREMIUM_PUBLIC_URL`. En service : **live** ; `/premium` montre le bouton de l'application
  REST. Si l'application repassait en sandbox, la page publique reprendrait le bouton Live historique (`DONATION_*`)
  et `/premium?test=1` montrerait le bouton sandbox.
- **Admin** (`/accounts`) : la source indique aussi « paypal · PayPal annulé/suspendu/expiré/introuvable ».

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

- **Page** `onboarder.<domaine>/accounts` (session `/connexion`, voir « Pages d'administration »), derrière la
  connexion NPM « admin-outils » : un interrupteur par compte, compteur « N premium / max ». Formulaires POST sans
  JavaScript, jeton en champ caché (anti-CSRF). Lien « Comptes » dans le tableau Homarr Opérations.
- **CLI** : `homelabctl accounts list | on <compte> | off <compte> | limits` (`--dry-run` respecté).
- **Suspension** : politique relue puis seuls `IsDisabled` et `MaxActiveSessions` changent (bibliothèques
  intactes) ; lectures en cours arrêtées ; permissions Jellyseerr sauvegardées dans `state.accounts` puis
  mises à 0 (une session Jellyseerr ouverte survit à la suspension Jellyfin).
- **Activation** : refusée au-delà de `accounts.max_premium` ; permissions Jellyseerr restaurées (à
  défaut de sauvegarde, celles par défaut de Jellyseerr).
- **Comptes protégés** (`[accounts] protected` : les deux comptes de l'admin) : affichés avec un badge,
  sans interrupteur ni suppression, hors plafond. Les autres admins sont gérés normalement.
- **Suppression** : bouton « Supprimer » → page de confirmation (`GET /accounts/delete`) → `POST
  /accounts/delete` : compte Jellyfin puis compte Jellyseerr (ses demandes partent avec). CLI :
  `homelabctl accounts delete <compte> --yes`.
- **Plafonds** (`[accounts]`) : 25 comptes premium, 2 lectures simultanées par compte. Dimensionnés
  pour 6 vCPU sans GPU (un seul transcodage 1080p en temps réel) et le lien seedbox (~8–10 Mo/s par connexion, mesuré le
  20/09) ; pic mesuré le 2026-09-14 : 4 lectures simultanées pour 13 comptes. Lecture directe : 65 % des lectures (30 j
  au 04/10). Pas de limite de débit par utilisateur : elle forcerait des transcodages.

## Autres commandes

- `homelabctl vpn status|on|off` : voir [DEPLOY.md](DEPLOY.md) (profils compose).
- `sudo homelabctl backup` : voir [DEPLOY.md](DEPLOY.md) ; aussi `homelab-backup.timer`.
- `homelabctl check` : ping Sonarr, Radarr, qBittorrent, Jellyfin, Jellyseerr ; présence SMTP,
  token, dossier surveillé ; Arrs et qBittorrent de la seedbox, montage ; `diun/images.yml` en contrôle strict (✗ et
  code de sortie ≠ 0 si invalide).
- `sudo homelabctl install` : copie `systemd/*` dans `/etc/systemd/system`, enable ; pose aussi
  `systemd/journald-homelab.conf` (puis `sudo systemctl restart systemd-journald`, à lancer soi-même).

## Clients HTTP (2026-10-07)

- **GET coupé rejoué une fois** (`clients::SendRetry::send_retry`) : seulement un GET, seulement sur une coupure de
  transport (`is_request() || is_connect()`, jamais un délai dépassé), une seule fois ; ligne `info` « GET coupé :
  nouvelle tentative (une seule) » avec le chemin (sans requête) et la chaîne de causes, sans l'URL. Appliqué aux
  clients Arr, Bazarr, Jellyfin, Jellyseerr, Prowlarr et qBittorrent, **sauf** les GET à effet : recherches,
  téléchargements et magnets Prowlarr (quota C411), `release` des Arrs, `get_bytes` de Jellyfin (la playlist du canari
  démarre une conversion). PayPal n'est pas concerné. Vu le 08/10 : un GET de Radarr seedbox rejoué après
  « connection closed before message completed ».
- **Jellyseerr** (Node, keep-alive 5 s) a son propre client avec `pool_idle_timeout` 4 s ; pas de délai global (4 s
  imposerait une poignée de main TLS vers la seedbox à presque chaque requête).
- **qBittorrent 5.2** : le cookie de session `QBT_SID_<port>` est accepté comme `SID` (nom renvoyé tel quel).
- **Erreurs** : journalisées avec toute leur chaîne (`format!("{e:#}")`) ; une URL qui porte un secret (webhook
  Discord, Jackett `apikey=`, liens Prowlarr, playlists de conversion Jellyfin `ApiKey=`) est retirée avant le contexte
  (`reqwest::Error::without_url`).

## Hook gluetun (reste en shell)

`hooks/qbit-update-port.sh` tourne **dans** le conteneur gluetun (busybox, pas de curl), appelé
par `VPN_PORT_FORWARDING_UP_COMMAND` avec le port forwardé : attend que qBittorrent réponde
sur 127.0.0.1:8080 puis `setPreferences {listen_port}`. Log sur stdout de gluetun.

## Tests

`cargo test` (unitaires : classification, politiques de tracker, décisions stuck/disk, filtre
TBA, règle monitor_sync, candidats du poller, validateurs d'onboarding, état, config).
`cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`. CI : `.github/workflows/rust.yml`.
