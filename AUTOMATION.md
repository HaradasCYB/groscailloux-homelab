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
   `map_episodes`. **Jamais de remplacement** : un fichier dont un épisode (ou le film) a déjà un fichier est
   écarté (« déjà présent »). Le 2026-09-17, « The.Final.Season.E01 », sans saison, a été lu S01E01 et la
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
des tailles différentes : le segment était régénéré sous elle). Jellyfin ne journalise que « non-keyframe breaks » ;
NPM voit chaque requête. La tâche relit les `tail_bytes` derniers octets de `npm_access_log` (horodatages **UTC**),
garde les requêtes `/videos/<id>/hls1/…/<n>.(ts|mp4|m4s)` de la fenêtre `window_secs`, et compte par (client, média,
segment) en fenêtre glissante ; `≥ threshold` ⇒ `warn!` + mail admin, une seule fois par (client, média) tant que la
boucle dure. Le résumé porte aussi les relances HLS du jour (« non-keyframe breaks » dans le journal Jellyfin) et
les 5xx sur segments — les repères de l'audit lecture du 2026-09-18. Rien n'est modifié.

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
épisode seul pris → 2 h, erreur → 1 h) et **absentes de l'autre machine** ; séries ajoutées le plus récemment d'abord. Par saison, via Prowlarr
(indexer « C411 » déclaré dans Prowlarr, même clé) : `search?type=tvsearch&query={TmdbId:<tmdbId>}{Season:<n>}`
→ releases dont l'attribut **`tmdbId` est celui de la fiche** (le nom ne compte pas) → `GET parse` de Sonarr
(saison, pack, épisodes, qualité) → garde-fous : marqueur FR (VFF > MULTi > FRENCH > VOSTFR), qualité autorisée
par le profil et ≤ 1080p, au moins une source, épisodes manquants. Pack si la moitié de la saison manque,
sinon épisodes ; tri langue, résolution, H.264, sources. Rien par identifiant (série sans `tmdbId`, releases
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
taille…) est respecté. **Rythme** : `max_queries_per_run` requêtes C411 (2 par passage, 12/h), `query_gap_secs`
(15 s) d'écart ; filet de sécurité : 25 requêtes/heure sur C411 dans Prowlarr. La clé C411 est partagée par
Prowlarr et les 4 Arrs (RSS ~16/h) : le 2026-09-17, le 429 est tombé vers 50 requêtes dans l'heure, pendant
le rattrapage de l'arriéré (35 requêtes homelabd + RSS + une recherche Sonarr).
Résumé : `grabbed=1 none=2 pending=13`.

### movie_search — 5 min
Recherche des films **suivis, sans fichier, sortis, hors file d'attente**, dès le passage qui suit la demande
(`missing_hours` 0, passage toutes les 5 min) : depuis le 2026-09-17, Radarr n'a plus aucune recherche (C411 en
RSS seulement) et le RSS ne ramène que les nouveautés — un film ancien ne peut venir que d'ici. Même mécanique que
`series_search` : `{TmdbId:<id>}` (type `movie`), `tmdbId` vérifié, `parse` Radarr, garde-fous, `release/push`,
sinon qBittorrent + `homelab:movie=<id>`. Au plus `max_per_run` films (3) par passage ; un échec (indexeur en
panne) est retenté après `error_retry_hours` (1), « aucune release » après `retry_after_hours` (72).

### indexer_unblock — 10 min
Après des échecs (429, délais), Sonarr et Radarr mettent un indexeur en pause, jusqu'à 24 h ; aucune API ne
lève la pause (`indexerstatus` → 404). Lecture seule de la table `IndexerStatus` des 4 Arrs (VPS : `rusqlite`
sur `sonarr|radarr/config/*.db` ; seedbox : `ssh seedbox` + `sqlite3 -readonly` sur
`<seedbox_apps_dir>/<app>/<app>.db`). Un indexeur en pause dont le **dernier échec date d'au moins
`quiet_mins`** (60 : fenêtre de limite de C411 ; plus tôt il se rebloquerait) est remis en service :
application arrêtée (`docker compose stop` / `app-<app> stop`), base copiée (`backups/arr-db-<app>-<date>.db`
ou `<app>.db.homelab-<date>` sur la seedbox, mode 600, gardées 7 jours), `UPDATE IndexerStatus SET
DisabledTill = NULL, EscalationLevel = 0, …`, application relancée et vérifiée (`system/status`). Au plus un
arrêt par application par `app_cooldown_mins` (60), même après un échec. Un indexeur débloqué
`max_unblocks_per_day` fois (3) en 24 h n'est plus touché : mail à l'admin (site mort, Cloudflare…). Chaque
déblocage est signalé par mail (`CHAT_ADMIN_EMAIL`). homelabd est cloisonné (`ProtectSystem=strict`) :
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

### Ancien budget (remplacé)
`homelab_core::budget` : un seul compteur horaire glissant (`state.c411_queries`) pour `series_search`,
`movie_search` et la page `/recherche`. `[indexers] c411_max_per_hour` (20) moins `manual_reserve` (6) pour les
tâches de fond, la réserve restant à la page. Avant le 2026-09-17, chacun avait son plafond (12/h, 1/h, 6/h)
sans voir les autres.

### anime_library — 30 min
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
Jellyfin identifie un dossier **d'après son nom** : un titre proche de celui d'un spin-off part sur la mauvaise
fiche (*Attack on Titan* → *Junior High School*, *Slime* → *Slime Diaries*, *The Walking Dead* → *Dead City*).
La tâche compare, pour chaque fiche des 4 Arrs, l'identifiant (TVDB pour les séries, TMDB pour les films) à
celui de l'élément Jellyfin qui porte le même chemin (`map_path`), et corrige les écarts : `RemoteSearch` avec
le bon identifiant, `Apply`, puis rafraîchissement complet des métadonnées. Au plus `max_fixes_per_run` (3) par
passage, jamais un titre en cours de lecture, `dry_run` respecté. Un identifiant absent d'un côté ne conclut
rien. Premier passage le 2026-09-17 : 6 titres corrigés sur 215.

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

## Recherche manuelle (`/recherche`)

Page de homelabd (hôte d'onboarding, liste NPM « admin-outils » + jeton) : chercher un titre des 4 Arrs, choisir
une saison ou le film, voir les releases, en télécharger une. Elle remplace la recherche de Sonarr/Radarr pour les
**animés** : celle-ci interroge chaque indexeur avec chaque titre connu, épisode par épisode (le 2026-09-17,
plusieurs minutes, délai dépassé du proxy de la seedbox — « timed out » — et 429 de C411 pour une heure).

- Logique dans `homelab_core::manual_search`, rendu dans `crates/homelabd/src/search_page.rs` (serveur, sans
  JavaScript : la page de résultats se recharge seule tant que la recherche tourne, rien ne bloque une requête HTTP).
- **C411 par identifiant TMDB** (une requête par saison ou par film, via Prowlarr), plafond propre à la page
  (`[manual_search] max_queries_per_hour`, 6/h ; s'ajoute aux 12/h de `series_search` et 1/h de `movie_search`,
  sous la limite de 25/h de Prowlarr). Plafond atteint, indexeur en pause ou fiche sans identifiant : la page le dit.
- **Nyaa.si** en plus pour un animé (fiche dans un dossier anime ou de type anime) : `nyaa_queries` (2) requêtes
  texte, un titre chacune. Nyaa ne donne que des liens **magnet** : `send_release` les ajoute directement au
  qBittorrent du même côté avec l'étiquette `homelab:`, lue par `torrent_import`.
- **Rien n'est filtré** : toutes les releases sont montrées, triées comme le choix automatique (sans écart d'abord,
  puis bonne œuvre et bonne saison, langue, saison complète, résolution, H.264, sources) et **marquées** :
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
Les torrents terminés qu'aucune fiche n'a voulus (`no_match` de `torrent_import`, 15 au plus, les plus récents
d'abord) sont listés sous le tableau des tâches : sans ça, un téléchargement fini restait invisible.

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
