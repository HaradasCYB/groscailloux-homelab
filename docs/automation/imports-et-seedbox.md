# Imports, seedbox et files des Arrs

Ce qui range un téléchargement fini, rafraîchit le montage de la seedbox et tient les files et le suivi des Arrs.
Sommaire de toutes les tâches : [AUTOMATION.md](../AUTOMATION.md).

## id_match_import — 5 min
Radarr/Sonarr bloquent l'import quand le nom de la release ne correspond pas au titre de la fiche,
même si l'indexer a fourni l'id au grab : message « Found matching movie/series via grab history,
but release was matched to … by ID ». Fréquent avec les releases C411 titrées en français.
Pour chaque item de queue (tous les Arrs, VPS et seedbox) `status=completed`,
`trackedDownloadState=importBlocked` avec ce message : `GET /api/v3/manualimport?downloadId=…&movieId|seriesId=…`,
puis `ManualImport` (importMode auto → hardlink) des seuls fichiers **sans rejet** ; séries :
uniquement les fichiers dont l'épisode est identifié, les autres restent en manuel (avertissement
dans les logs). Au plus 10 téléchargements par passage.

## torrent_import — 2 min
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
   `map_episodes`, puis la numérotation fansub (`Erased S01 - 06`). **Animé pris pour une saison** (étiquette
   `season=`) : un `SxxEnnn` dont le couple saison/épisode n'existe pas dans la fiche, mais dont `nnn` est le numéro
   absolu d'un seul épisode de **cette** saison, est rattaché par l'absolu (`sxxe_absolute_episodes`). Le 2026-10-18,
   18 des 25 fichiers d'un pack de One Piece S23 (`S01E1156`, absolu 1156 = S23E01) étaient restés « Invalid season or
   episode ». Ce refus de Sonarr cède alors devant notre correspondance, seulement quand Sonarr a reconnu cette fiche
   sans proposer d'épisode. Un absolu hors de la saison de l'étiquette reste refusé (Black Clover S02, 2026-10-03 :
   `S02E73` = S01E73). En dernier recours, et seulement si
   l'étiquette nomme une saison (`homelab:series=<id>:season=<n>`, posée par `series_search`), un **numéro nu**
   (`Angels of Death - 07 (…).mkv` → S01E07, `bare_episode`) est lu dans cette saison. Sonarr ne lit cette forme
   que pour un animé : *Angels of Death* (2021), une série classique, était restée « téléchargée mais pas rangée »
   le 2026-10-03. `bare_episodes` refuse **en bloc** dans trois cas : les fichiers ne portent pas tous le même
   titre, deux fichiers ont le même numéro, ou un numéro dépasse la saison (numérotation absolue). Jamais pour un
   fichier que Sonarr attribue à une autre fiche. **Numéro en tête** (`05. Titre de l'épisode.mkv`, sans nom de série
   ni `SxxEyy`, 2026-10-08, `numbered_pack`) : lu dans la saison de l'étiquette, **seulement si Sonarr n'a rien lu dans
   aucun nom du torrent** (`sonarr_read_something`), décidé pour le torrent entier et refusé en bloc au moindre doute
   (forme mixte, doublon, trou, numéro hors saison, suite qui ne commence pas à 1 après une autre saison) ; un titre qui
   contient un autre nombre (`Show 13`, `E13`) reste du ressort de Sonarr. **Saison forcée par l'étiquette**
   (`tag_season_pack`, 2026-10-28) : série non animée prise pour la saison N (`season=`) sur laquelle Sonarr bute par sa
   **numérotation scène** (table XEM) — au moins une vidéo refusée « Episode NxYY was unexpected considering the …
   folder name ». Si chaque vidéo hors extrait et bonus porte un seul `SNNEyy` littéral, lu de même par le `parse` de
   Sonarr (sans absolu), sans doublon, sans épisode inconnu ni plus de fichiers que la saison N n'a d'épisodes, **tout**
   le torrent va en N×yy (`EpisodeSource::OursOnly`) et ce refus cède (journal `info` « saison forcée par l'étiquette »).
   Au moindre doute, rien n'est rangé par ce chemin **et** aucune proposition de Sonarr hors de la saison N n'est
   importée (`TagSeasonGuard::refuse_off_season` : vidéo de côté, `warn`). Sans ce refus, Sonarr garde la main même
   s'il range ailleurs : une release nommée en numérotation scène (`Lupin.S03` = Partie 3) est alors juste. Lupin (2021)
   le 28/10 : `Lupin.2021.S03` (TVDB saison 3) rangé en 2x01…2x07, `Lupin.S02` en 1x06…1x10.
   **Jamais de remplacement** : un fichier dont
   un épisode (ou le film) a déjà un fichier est écarté (« déjà présent »), et deux fichiers du torrent ne visent jamais
   le même épisode : le premier est importé, le second écarté (« déjà pris »). Le 2026-09-17, « The.Final.Season.E01 », sans saison, a été lu S01E01 et la
   saison 1 d'une série écrasée (réparée en réimportant ses fichiers d'origine). Puis `ManualImport` en
   **`importMode: copy`** (= hardlink). Jamais `auto` : pour un téléchargement non suivi, `auto` = déplacement.
Rien d'importable ⇒ `nothing_importable`. Erreur (Arr injoignable…) ⇒ `retry`, `error` après `max_attempts`.
**Import partiel** (`partial`, 2026-10-21) : des fichiers sont rangés mais des vidéos qui comptent restent de côté
(`left_out`, d'après les refus de `fresh_files`). Ne comptent pas : les extraits (« Sample »), les épisodes déjà couverts
(« déjà présent », « déjà pris », « Not an upgrade »…) et les bonus repérés dans le nom ou le dossier (`NCOP`, `NCED`,
`OP1`, `ED2`, `creditless`, trailer, featurette, extras, menu, `PV`). Le 2026-10-18, un pack One Piece S23 avait 7 épisodes
rangés sur 25 et était noté `imported` sans un mot ; la carte de la demande est restée deux jours sur « en attente de
qBittorrent ». Désormais : détail « 7 fichier(s) rangé(s), 18 vidéo(s) de côté », ligne `warn` « import partiel », liste
(40 au plus, nom du fichier seul) dans `state.torrent_import`, et **une alerte admin par torrent** (`alert_pending`,
écrit avec la décision ; noté parti seulement si `!alerts::retry_later`, sinon redonné au passage suivant). Le torrent est
**réexaminé** toutes les 6 h (`PARTIAL_RETRY_SECS`) tant que `attempts < max_attempts` (3 fois), toujours en `copy`, par
chemin exact, sans remplacement : un correctif de correspondance installé entre-temps range seul le reste, et un partiel
fini à la main **pendant ces réexamens** (≈ 18 h) passe en `imported` ; après, l'entrée reste `partial` (la retirer de
l'état, daemon arrêté). Un réexamen en échec (Arr injoignable, aucun fichier proposé, fiche disparue : jamais recréée)
laisse le partiel tel quel ; un partiel dont le torrent attend son retrait par `deletion_cleanup`
(`deletions.pending_torrents`) n'est plus réexaminé. Seul `sxxe_absolute_episodes` lève « Invalid season or episode »
(jamais un numéro nu), sans extrait (`sample`), et jamais pour un numéro qui tient aussi dans la saison de l'étiquette.
Une ligne proposée par l'Arr passe avant les nôtres pour un même épisode, quel que soit l'ordre du disque. Retour à un ancien
binaire : `partial` y est lu comme définitif et affiché « pas rangé ».
Au plus `max_per_run` torrents coûteux par passage, les plus récents d'abord. Aucune modification des
torrents. Jellyfin : LibraryMonitor (VPS) ou `seedbox_refresh` (seedbox). Le watcher `auto_import` ignore les
vidéos qui appartiennent à un torrent qBittorrent.
Résumé : `files=3 arr_managed=67 already_linked=8 imported=2 no_match=1 pending=0`.

## seedbox_refresh — 5 min (si `[seedbox] enabled`)
Lit l'historique `downloadFolderImported` (eventType 3) des Radarr/Sonarr de la seedbox depuis le
dernier id traité (`state.seedbox_history` ; la première passe initialise le curseur sans rejouer).
Pour chaque nouvel import : chemin `media_root/…` → dossier relatif, `POST <rclone_rc>/vfs/refresh`
sur ce dossier et ses parents, puis `POST /Library/Media/Updated` à Jellyfin avec le chemin
`jellyfin_root/…` (Jellyfin ne scanne que ces dossiers). Montage absent → avertissement, le
curseur n'avance pas. Arr de la seedbox injoignable (2026-10-07) : sauté (« arr unreachable: side skipped this run »,
curseur inchangé), l'autre est traité, résumé « seedbox injoignable » / « <arr> injoignable ».

## Arrs multi-instances
Avec la seedbox activée, `stuck_handler` traite aussi les queues des Arrs seedbox,
`tba_bypass` scanne aussi `seedbox.sonarr_downloads` avec le Sonarr seedbox, et `monitor_sync`
**route chaque demande Jellyseerr selon `media.serviceId`** (`jellyseerr_vps_sonarr_id` → Sonarr
VPS, `jellyseerr_sonarr_id` → Sonarr seedbox) : les ids de séries diffèrent entre instances.

## tracker_ratio — 30 min
Share limits par tracker via `POST /api/v2/torrents/setShareLimits`.
`unlimited` (`c411.org`, `c411.tw`) → ratio -1 / temps -1 ; `secondary` (yggleak, u2p, ygg.gratis, `t-ru.org` = RuTracker) et
`public` (liste de trackers publics) → `public_ratio` / `public_time_min` (en minutes) ; défaut → `default_ratio` /
`default_time_min`.
Ne touche un torrent que si sa limite actuelle diffère (ratio comparé à 2 décimales).

## stuck_handler — 5 min
`GET /api/v3/queue?pageSize=200` sur Sonarr et Radarr. Un item dont `errorMessage` matche
l'expression `pattern` (`stalled|metadata|no connections`) est suivi (`state.stuck`). Passé `stall_secs` (8 h) :
`DELETE /api/v3/queue/{id}?removeFromClient=true&blocklist=true` → l'Arr relance une
recherche. `max_actions_per_run` par passage. Les items disparus de la queue sont oubliés.

## monitor_sync — note (2026-09-13)
Une saison demandée n'est suivie que si elle n'a pas déjà des fichiers **sur l'autre machine**
(rapprochement par tvdbId) : sans ça, une nouvelle demande routée vers la seedbox y faisait suivre
toutes les saisons historiques, y compris celles présentes sur le VPS (doublons).

## tba_bypass — 5 min (désactivée : dans `tasks.disabled` depuis `episodeTitleRequired = never`, gardée comme filet)
`GET /api/v3/manualimport?folder=/downloads&filterExistingFiles=true` (Sonarr, timeout
5 min). Candidat = exactement un rejet, commençant par « Episode has a TBA title », série et
épisode identifiés, `episodeFileId == 0`. Alors `POST /api/v3/command` `ManualImport`
(importMode auto). Contourne le refus d'import des épisodes anime fraîchement diffusés dont
TVDB n'a pas encore le titre.

## monitor_sync — 10 min
Jellyseerr ajoute les séries sans `addOptions.monitor` ⇒ Sonarr monitore tout. La tâche lit
`GET /api/v1/request?filter=all` (paginé), construit `sonarrSeriesId → saisons demandées`
(statuts DECLINED/FAILED exclus), puis pour chaque série Sonarr concernée :
`monitored = demandée OR a des fichiers`, S00 jamais modifiée, `PUT /api/v3/series/{id}`.
Les séries inconnues de Jellyseerr ne sont pas touchées. Un Sonarr injoignable est sauté, l'autre traité
(2026-10-07) ; tant qu'un côté est muet, l'autre ne connaît pas ses saisons déjà présentes : il n'ajoute **aucun**
nouveau suivi (risque de doublon Jellyfin) mais peut en retirer, rattrapé au passage suivant. Jellyseerr injoignable
reste une erreur. Une saison suivie parce qu'elle a des fichiers sans être demandée garde ses mises à niveau (RSS de
Sonarr) mais n'est jamais cherchée par `series_search` (1.30.2, voir
[series_search](recherche-et-indexeurs.md#series_search--10-min-remplace-unknown_series_grab), « Saisons demandées seulement »).

## Watcher auto_import (continu)

inotify non récursif sur `paths.downloads` (create, close_write, moved_to), chaque nom traité
au plus une fois par 2 min **et un seul traitement à la fois par nom** (verrou « en cours »).
Vidéo (`mkv/mp4/avi`, hors `.!qB`/`.part`) : attente `[auto_import] settle_secs` puis **attente d'une taille stable**
(`archive_stable_checks` relevés identiques à 5 s d'écart, 6 h max) — un envoi Filebrowser/scp écrit par morceaux, un
scan trop tôt importerait (voire déplacerait) un fichier incomplet. Fichier d'un torrent qBittorrent
⇒ laissé à `torrent_import`. Classification : `S01E02`, `1x02`, `S01`, `Season`, `Saison`,
`Complete`, `Intégrale` ⇒ série ; sinon `GET /api/v3/parse` Sonarr : série suivie + épisodes
identifiés (ex. numérotation absolue « Bleach - 48 ») ⇒ série ; sinon film.
Série : `GET /api/v3/parse` → `series/lookup` → `GET /series?tvdbId` → `POST /series` si
absent (profil `QUALITY_PROFILE_ID`, monitor all, pas de recherche) → `DownloadedEpisodesScan`.
Film : idem côté Radarr (`movie/lookup`, `tmdbId`, `DownloadedMoviesScan`).
Archive `zip/rar` : attente de taille stable (`archive_stable_checks` relevés à 5 s, au plus
`archive_max_wait_secs`), extraction
(`unzip` / `unrar` / `7z`), suppression de l'archive, scan du dossier extrait d'après sa
première vidéo.
