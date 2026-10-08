# Arrs, indexeurs et choix des releases

À lire avant de toucher un Arr (VPS ou seedbox), Prowlarr, un indexeur, un profil de qualité, Jellyseerr ou les
règles de choix de `series_search` / `movie_search`. Le détail de chaque tâche (étapes, états, résumés) est dans
[AUTOMATION.md](../AUTOMATION.md) ; ce runbook dit **ce qu'il ne faut pas casser et pourquoi**. La voie russe a son
propre runbook : [voie-russe.md](voie-russe.md).

## 1. Qui cherche, qui télécharge

- **Tout ce qui est neuf passe par la seedbox** (depuis le 18/09) : deux verrous.
  - `homelab.toml` `[downloads] auto_sides = ["seedbox"]` : `series_search` et `movie_search` ignorent les Arrs du VPS.
  - Côté Arr du VPS : `enableRss = false` sur l'indexeur C411 de Sonarr et Radarr (sauvegarde
    `backups/vps-rss-off-20260918-093803/`) et, depuis le 07/10, `rssSyncInterval = 0` (96 avertissements « No available
    indexers » par jour ; sauvegarde `backups/lot1-20261007/arrs/`). L'état de santé `IndexerRssCheck` en erreur est normal.
  - Le VPS garde ses fiches, ses fichiers, `torrent_import` et `deletion_cleanup` ; il ne prend plus aucune release.
  - **Remettre le VPS en service** = remettre `"vps"` dans `auto_sides` **et** rallumer le RSS de l'indexeur **et** remettre
    `rssSyncInterval = 15` (`config/indexer` : GET puis PUT de ce seul champ). Un seul des trois ne suffit pas.
  - Jellyseerr envoie toutes les demandes à la seedbox (`isDefault` sur les serveurs id 1).
- **Aucune recherche depuis Sonarr/Radarr** : C411 y est en **RSS seulement** (`enableAutomaticSearch` et
  `enableInteractiveSearch` à `false` sur les 4 Arrs depuis le 17/09). L'avertissement « No indexers available with
  Automatic Search enabled » est normal et voulu. Seule exception : la voie russe (RuTracker, Arrs de la seedbox).
  Pourquoi : un bouton « Search » sur une saison d'animé interroge C411 épisode par épisode et titre par titre
  (~30 requêtes d'un coup, 3 à 4 par épisode) → 429 « API Request Limit reached » → pause de l'indexeur qui s'allonge
  jusqu'à 24 h, et comme C411 est seul, « All indexers are unavailable ».
- **Séries : homelabd cherche par identifiant TMDB** (`series_search`). Jellyseerr est en **`preventSearch`** sur les deux
  Sonarr : une demande crée la fiche et ses saisons suivies, sans recherche. C411 renvoie les releases d'une série par
  `{TmdbId}{Season}` quel que soit leur nom (japonais, anglais, français) et **ignore l'identifiant IMDb** (100 releases
  sans rapport). Sonarr garde le RSS, l'import et le suivi. **Ne pas remettre la recherche à la demande dans
  Jellyseerr.** `animeCategories=[5070]` et `animeStandardFormatSearch=true` restent dans les deux Sonarr pour le RSS.
- **Films : `movie_search`** cherche dès le passage qui suit la demande (`[tasks.movie_search] missing_hours = 0`,
  passage toutes les 5 min) : Radarr n'a plus de recherche et son RSS ne ramène que les nouveautés.
- **Recherche manuelle** : page `/recherche` de homelabd, jamais la recherche de Sonarr/Radarr (voir § 8).

## 2. Indexeurs et clés C411

### Inventaire (vérifié le 08/10 par `GET /api/v1/indexer` de Prowlarr et les notes du lot 4)

| Où | Indexeurs | Rôle |
| --- | --- | --- |
| Prowlarr du VPS | « C411 » (id 4) et « C411 (2) » (id 6), `queryLimit` 45 chacun ; « Nyaa.si » (id 12) et « World-torrent » (id 10), sans limite | recherches de homelabd seulement (`series_search`, `movie_search`, `/recherche`) ; Nyaa et World-torrent = secours pendant une panne de C411 |
| Sonarr/Radarr du VPS | C411, RSS coupé | aucune prise |
| Sonarr/Radarr de la seedbox | C411 en RSS (`C411_RSS_API_KEY`) ; « RuTracker » (Torznab via le Jackett de la seedbox, tag `russe`, Sonarr id 10, Radarr id 8) | RSS ; voie russe |
| autobrr (seedbox) | flux C411 (troisième clé) **désactivé depuis le 08/10** | aucun (§ 9) |

- **Prowlarr n'a aucune application liée** : sinon il réécrirait les indexeurs des Arrs. Les indexeurs des Arrs sont
  déclarés directement. La clé C411 n'est pas lisible par l'API des Arrs (champ masqué) : elle vient de leur base.
- **Plus de Jackett ni de FlareSolverr sur le VPS** (retirés du compose le 17/09 avec les 34 indexeurs publics, qui ne
  servaient qu'en interactif ; sauvegarde `backups/indexers-20260917-204443/`). Le Jackett et le FlareSolverr **de la
  seedbox** servent la voie russe : ne pas les confondre.
- **Avant de retirer un service**, chercher qui l'appelle : `grep -r <nom>:<port>` dans les configs et les champs
  `baseUrl` des indexeurs des Arrs (`GET /api/v3/indexer`). Le retrait de Jackett/FlareSolverr le 10/09 avait coupé les
  indexeurs publics pendant deux jours.

### Budget et clés

- Source de vérité : `homelab.toml` `[indexers]` et `homelab_core::indexer` (seules portes : `search_tmdb`, `search_text`).
- **Deux clés dans Prowlarr**, un compteur horaire **par clé** : `c411_max_per_hour` (40) chacune, soit 80/h ;
  `manual_reserve` (10) gardées pour `/recherche`. Chaque recherche prend la clé la moins chargée.
- Une clé qui répond 429 est mise de côté `cooldown_after_429_mins` (15) et la requête repart **aussitôt sur l'autre** :
  tant qu'une clé répond, rien ne s'arrête.
- Filet : Prowlarr coupe à 45 requêtes/h par indexeur (`queryLimit`).
- Rythme des tâches : `[tasks.series_search] max_queries_per_run` (6 par passage de 10 min, 36/h au pire) et
  `query_gap_secs` (5) ; `[tasks.movie_search] max_per_run` (3 films par passage de 5 min).
- « C411 (2) » utilise la clé RSS des Arrs (`C411_RSS_API_KEY`) : le RSS de la seedbox (deux Arrs, toutes les 15 min)
  n'est pas compté par homelabd. La **troisième clé** (autobrr) n'est comptée nulle part.
- `[manual_search]` n'a que `c411_indexer`, `text_queries` et `results_ttl_mins` : la page `/recherche` puise dans le
  même budget (la réserve), sans plafond propre.

### C411 en panne, indexeur en pause

- **C411 annonce sur deux domaines** : `c411.org` **et** `tk.c411.tw`. Toute règle par tracker vise les deux
  (`[tasks.tracker_ratio] unlimited`, décision « torrent » de `deletion_cleanup`).
- **Panne ≠ « aucune release »** : pendant une panne (503) ou une maintenance (page HTML « Incident en cours » servie en
  200), Prowlarr répond par une liste vide en 200. Sur une liste vide, `indexer::indexer_down` regarde `indexerstatus`
  (échec < 10 min ou `disabledTill` à venir) et sonde `<baseUrl>/api?t=caps` (sans clé, sans quota) : panne ⇒ erreur,
  nouvel essai au bout de `error_retry_hours`. Un refus « blocked till … » compte aussi comme une erreur (nouvel essai
  dans l'heure), pas comme « aucun candidat » (24 h).
- **Secours public pendant une panne** (seulement si `indexer::is_outage`) : `[indexers] fallback = "World-torrent"`
  (films et séries), `fallback_anime = "Nyaa.si"` (animés : Nyaa d'abord, puis World-torrent).
  - World-torrent ne cherche que par **titre français**, et la ponctuation le perd : titre TMDB français (Jellyseerr)
    nettoyé + année (Jellyseerr `movie_details`), puis début du titre + année (`movie_search::fallback_queries`) ; séries : deux premiers noms connus
    sans ponctuation (`series_search::fallback_candidates`, `fallback_names`).
  - Pas d'identifiant TMDB chez eux : une release n'est gardée que si le `parse` de l'Arr la rattache à **cette** fiche
    (et cette saison) ; mêmes règles de langue, de taille et de codec ; **français seulement** pour les séries
    (`lang_rank_for` > 0 : la plupart des releases Nyaa sont sous-titrées en anglais).
  - Le lien « .torrent » de World-torrent est une redirection 301 vers un magnet : `send_release` passe par le magnet.
  - Secours sans candidat acceptable = `fallback_none`, refait au bout de `fallback_retry_hours` (12) ; **dès que C411
    répond** (sonde `indexer::c411_up`, sans quota, une fois par passage), la saison ou le film repart sur C411
    (`series_search::fallback_due`). Aucun « trou » n'est affiché sur `/status.html` pendant une panne.
  - Torrents publics : l'hébergeur de la seedbox les arrête une fois terminés (§ 7 de [seedbox-et-rclone.md](seedbox-et-rclone.md)).
- **Indexeur mis en pause par un Arr** (429, délais) : jusqu'à 24 h ; ni `testall` ni un réenregistrement ne lèvent la
  pause (pas d'API : `indexerstatus` → 404), aucune recherche ni `release/push` ne passe. L'état est dans la table
  `IndexerStatus` ; `indexer_unblock` s'en charge (application arrêtée ~20 s, base sauvegardée, `quiet_mins` 15 après le
  dernier échec, au plus `max_unblocks_per_day` 10 en 24 h, puis alerte seulement). Pendant la pause, `series_search`
  passe par qBittorrent (étiquette `homelab:`).
- **Site C411 en panne** : `indexer_unblock` sonde le site avant de lever une pause (`indexer::c411_reachable`, `caps`
  sans clé ni quota ; pas `c411_up`, qui lit l'état de la clé de recherche). Site en panne ⇒ rien n'est arrêté, résumé
  « C411 en panne : pause laissée », une alerte par incident (marqueur `c411_outage` dans `state.indexer_alerts`,
  retiré après 6 h sans panne constatée). Copies `*.homelab-*` de la seedbox purgées une fois par jour
  (marqueur `purge:seedbox`). Détail : [AUTOMATION.md, indexer_unblock](../AUTOMATION.md#indexer_unblock--5-min).

## 3. Profils et réglages des Arrs

- **Viser un profil par son NOM, jamais par son numéro** : les numéros diffèrent d'une machine à l'autre.

  | Profil (relu par `GET /api/v3/qualityprofile` le 08/10) | VPS | Seedbox |
  | --- | --- | --- |
  | FR-friendly H.264 (Sonarr et Radarr) | 6 | 7 (`[seedbox] quality_profile_id`) |
  | Anime - MULTi/VOSTFR (Sonarr) | 7 | 8 |
  | Anime - JAP/VOSTFR (Radarr, ancien profil : `minFormatScore` 0, HEVC −10000) | 7 | 8 |

  Le nom « FR-friendly H.264 » est historique : il ne décrit pas le codec (§ 4). Les deux Radarr gardent l'ancien profil
  « Anime - JAP/VOSTFR » : **n'y mettre aucun film** (il refuse le HEVC et préfère la VOSTFR).
- **FR-friendly** : 1080p au plus (jamais 2160p : pas de GPU), `minFormatScore = -9999`, français d'abord (formats VFF,
  VOF, MULTi, FRENCH), `No French Marker` −2000 (VO/VOSTFR en dernier recours), rejets durs (langues étrangères, CAM/TS,
  sample, 3D) à −100000, `upgradeAllowed = false`.
- **Anime - MULTi/VOSTFR** (Sonarr ; réparé le 18/09 : il rejetait tout HEVC 10 bits et tout doublage français avec
  `minFormatScore = 0`, `HEVC 10-bit -10000` et `FRENCH -500` ; sauvegarde `backups/arr-anime-profile-20260918-142927/`) :
  MULTi 3000, VOSTFR 2000, VFF 1000, FRENCH 500, sans marqueur français −2000,
  `minFormatScore = -9999`, mêmes qualités ≤ 1080p. Jellyseerr a un profil séparé pour les animés, repéré par
  `animeTags` : **`activeAnimeProfileId`** des deux Sonarr de Jellyseerr = VPS 7, seedbox 8 (sinon les nouvelles
  demandes d'animés partent en FR-friendly ; sauvegarde `backups/jellyseerr-sonarr-20260918/before-anime-profile.json`).
- **Codec et audio côté Arrs** (26/09, sur FR-friendly et Anime des 4 Arrs) : `HEVC 10-bit`/`HEVC 8-bit` +200,
  `H.264` +100, `AV1` 0 (sous les marches de langue, ≥ 500) ; « Audio DTS » −50 et « Audio DTS-HD/TrueHD » −100 (titre de
  release). Sauvegardes `backups/arr-codec-priority-20260926/` (dont `*-customformat-before-audio.json`).
- **Marqueurs de langue** : « VOF » (version originale française) compte comme VF (`langs_of`) et figure dans les formats
  n° 4 « No French Marker », « VFF » et « FRENCH » des 4 Arrs (sauvegarde `backups/arr-cf-vof-20260923/`). **Un nouveau
  marqueur de langue = code (`langs_of`) ET formats des 4 Arrs.**
- **Réglages des 4 Arrs** (audit du 18/09, sauvegarde `backups/arr-settings-20260918-072920/`) :
  `episodeTitleRequired = never` (les animés tout juste sortis ont un titre « TBA » ; `tba_bypass` est donc dans
  `tasks.disabled`, gardée comme filet) ; `importExtraFiles = true` avec `srt,ass,ssa,sub,idx` (les sous-titres externes
  des VOSTFR étaient jetés) ; `animeEpisodeFormat` avec `{absolute:000}` ; `rssSyncInterval = 15` sur la seedbox (le
  tracker sert le RSS avec un cache de 5 min et un ETag) ; corbeille `/data/media/*/.recycle` aussi sur le VPS, purgée à
  14 j par `cleanup`.
- **Plafond à l'acquisition** : 110 Mo/min sur les qualités 1080p des 4 Arrs (`qualitydefinition`, ≈ 14,7 Mbit/s),
  aligné sur `[indexers] max_gb_per_episode` / `max_gb_per_movie`.
- **Type « anime » obligatoire** pour une série rangée dans Anime : `anime_library` pose `seriesType = anime` (au
  déplacement et en rattrapage), sinon Sonarr ne lit pas la numérotation absolue (« Bleach - 367 »).
- **Dossiers** : `seriesFolderFormat = {Series Title} [tvdbid-{TvdbId}]`, `movieFolderFormat` avec `[tmdbid-…]`
  (identification Jellyfin) ; les anciens dossiers n'ont pas été renommés. `enableSeasonFolders = true` sur les deux
  Sonarr de Jellyseerr (sauvegarde `backups/jellyseerr-sonarr-20260918/`) ; les fiches restées à plat le restent tant
  qu'on ne les renomme pas ; les tâches de homelabd créent leurs fiches avec `seasonFolder: true`.
- **Historique d'un titre** : `GET history?movieId=` / `?seriesId=` n'existe pas (filtre ignoré, tout revient). Utiliser
  `history/movie?movieId=` et `history/series?seriesId=` ; seul `downloadId` filtre vraiment `GET history`.
- **Seedbox** : Sonarr et Radarr y journalisent en Info ; `config/host` d'un Arr contient le hash du mot de passe et la
  clé (sauvegarde en 600). Voir [seedbox-et-rclone.md](seedbox-et-rclone.md).
- **Montée de version d'un Arr du VPS** : le lot 3 la prépare (`backups/lot3-20261008/<service>/apply.sh`, avec
  `check.sh` et `rollback.sh`) ; à reporter ici après son exécution (voir
  [outils-bancs-et-hors-pic.md](outils-bancs-et-hors-pic.md#5-exécutant-du-lot-3-jusquau-1210)).

## 4. Choix d'une release (`choose`, `best_movie_release`, `/recherche`)

- **Clé de tri** : langue > résolution > au moins deux sources > **codec** > sources > **audio**.
  - Langue (`lang_rank`) : VF 4 > MULTi 3 > FRENCH 2 > VOSTFR 1 > VO 0. **Animés** (`lang_rank_for(title, anime)`,
    `seriesType == "anime"`) : MULTi 4 > VOSTFR 3 > VF 2 > FRENCH 1 > VO 0 ; un « MULTI.VFF » compte comme MULTi pour un
    animé, comme VF pour le reste.
  - Codec (`series_search::codec_rank`) : HEVC 2 > H.264 ou non indiqué 1 > AV1 0 (décision du 26/09 : un 1080p HEVC
    pèse 1,1–1,3 Go/h contre 3,4–4,25 Go/h en H.264 ; l'AV1, aussi léger, est mal lu par les vieux clients).
  - Audio (`audio_rank`) : AAC/E-AC3/AC3/Opus ou non indiqué 3 > FLAC 2 > DTS 1 > DTS-HD MA/DTS:X/TrueHD 0 (80 % des
    lectures d'une piste DTS réencodaient le son : Chromecast, appli iOS).
- Le x265 n'est jamais **exigé** : un x264 en VF passe devant un x265 sans français, un x265 à une seule source derrière un
  x264 bien partagé. **Ne jamais écarter une release parce qu'elle est en x265** (souvent la seule en français pour un
  animé) : le codec source ne change pas le coût d'un transcodage ([lecture-et-transcodage.md](lecture-et-transcodage.md)).
- Une release sans français n'est prise qu'en dernier recours (`[indexers] allow_no_french`), quand aucune française
  n'est acceptable.
- **Plafonds de taille** du choix automatique : `[indexers] max_gb_per_episode` (3) et `max_gb_per_movie` (15) ; sinon un
  pack de 134 Go à une source gagnait contre un 27,8 Go bien partagé. Au-delà, la release reste visible sur `/recherche`.
- **Œuvres dérivées** : un titre qui contient `mini`, `specials`, `OVA`, `recap`, `abridged`, `junior`… absent des titres
  de la fiche est écarté du choix automatique (`series_search::derivative`) ; le repli en texte libre exige en plus que
  l'Arr rattache la release à **cette** fiche.

## 5. Séries : saisons, packs et numérotations

- **Une saison entière en une requête** : sans pack, `series_search` prend **une release par épisode manquant** dans le
  même lot de résultats (`choose_episodes`, plafond `[tasks.series_search] max_grabs_per_season`, 60), puis reprend
  `episode_retry_mins` (5) après. Avant, un épisode toutes les 2 h.
- **Jamais de pack de saison** tant qu'un épisode suivi a une date de diffusion à venir (`future_episodes` : l'Arr ne
  voit pas les packs envoyés directement à qBittorrent). Le plafond de taille d'un pack est jaugé sur les manquants
  **datés** (`choose_sized`) : saison toute sans date ⇒ pack > `max_gb_per_episode` refusé, épisodes un à un.
- **Couverture partielle** : une recherche par identifiant peut ne couvrir qu'une partie d'une saison.
  `series_search::uncovered` compare les épisodes manquants aux candidats ; s'il en reste, le texte libre est lancé **en
  plus** (`how = "tmdb+texte"`) ; ce qui reste introuvable est noté (`uncovered`) et affiché sur `/status.html`
  (« Saisons sans release »). Une saison complétée ensuite sort de la liste au passage suivant (`stale_uncovered`).
- **Cours d'animés publiés sous leur propre titre** (ex. `BLEACH.Thousand-Year.Blood.War.S01/S02/S03` = Bleach S17) :
  Sonarr ne rattache bien que S01 (→ saison 17) ; **S02 → saison 2 et S03 → saison 3**. Le garde-fou d'égalité de saison
  de `series_candidate` les refuse : **ne pas le retirer** (il éviterait d'écraser deux vraies saisons). Ces releases
  apparaissent sur `/recherche` marquées « autre saison ».
- **Packs de cours récupérés seuls** (`cour_pack`, animés seulement, interrupteur `[tasks.series_search] cour_packs`) :
  packs que l'Arr rattache à cette fiche mais à une autre saison ; `.torrent` lu chez Prowlarr (aucune annonce au
  tracker, `homelab_core::torrent_file`) ; action seulement si `offset_mapping` est certain (trou d'un seul tenant, autant
  de vidéos que de manquants, suite continue, aucun épisode déjà pourvu). Correspondance dans l'étiquette
  `homelab:series=<id>:season=<n>:offset=<k>:eps=<from>-<to>`, appliquée par `torrent_import` en court-circuitant Sonarr
  **et** `map_episodes` (`EpisodeSource::OursOnly`). Un pack de cours n'est **jamais** proposé à l'Arr
  (`goes_straight_to_qbittorrent`). Pièges traités : l'indexeur donne au pack l'identifiant TMDB **du cours**
  (refus par identifiant contourné pour ce seul chemin) ; le titre du cours est un titre alternatif (`gap_names` le fait
  passer devant les `text_queries`) ; le pack S01 déjà bien mappé par TVDB est écarté.
- **Intégrales** (pack sans numéro de saison, jamais renvoyé par `{TmdbId}{Season}`) : `series_search::try_integrale`,
  seulement quand rien d'autre n'est acceptable (interrupteur `integrale_packs`) ; requête `{TmdbId}` sans saison si le
  dernier manquant a au moins `integrale_min_age_days` (14) jours ; chaque fichier passe au `parse` de Sonarr (scene
  mapping : `02x01` → S01E14) ; torrent ajouté **arrêté**, autres fichiers désélectionnés, puis démarrage. Introuvable
  quand même : recherche Prowlarr en texte sans catégorie, puis la même recette à la main
  ([AUTOMATION.md, series_search](../AUTOMATION.md#series_search--10-min-remplace-unknown_series_grab)).
- **Épisodes sans date** (TheTVDB date tard les séries françaises ; Sonarr exclut de `wanted/missing` tout épisode sans
  date) : `series_search` lit aussi les séries dont une saison suivie, commencée depuis moins de
  `undated_window_days` (730), est incomplète (`undated_missing`).
- **Fansubs `S01 - 06`** : Sonarr lit `S01` comme une saison entière et refuse chaque fichier (« Single episode file
  contains all episodes in seasons »), ou, si on ignore ce rejet, propose les 12 épisodes pour le premier fichier.
  `series_search::fansub_episode` lit le numéro, le rejet passe dans `is_identification_rejection`, et dès qu'un fichier
  est lu ainsi le torrent bascule en `EpisodeSource::OursOnly` (sinon Sonarr gagne et un fichier est rattaché à 12
  épisodes). `no_match` et `nothing_importable` apparaissent dans « Rien ne bouge » sur `/status.html`.
- **Numéro nu `Titre - 07`** : Sonarr ne le lit que pour un animé. `torrent_import::bare_episode` le lit dans la saison de
  l'étiquette (`season=`, posée par `series_search`), et seulement là ; `bare_episodes` refuse tout le pack si les titres
  diffèrent, si deux fichiers ont le même numéro ou si un numéro dépasse la saison. `S01 - 06` reste à la lecture fansub.
- **Numéro en tête `05. Titre de l'épisode`** (`numbered_pack`) : seulement si Sonarr n'a rien lu dans aucun nom du torrent
  (`sonarr_read_something` : aucun `parsedEpisodeInfo`, aucun épisode proposé par `manualimport`, ni fansub ni
  `map_episodes`), avec `season=` dans l'étiquette, refusé en bloc au moindre doute (forme mixte, doublon, trou,
  numéro hors saison, suite qui ne commence pas à 1 après une autre saison) ; un titre qui contient un autre nombre est
  écarté ; la source reste `ArrFirst`.
- Pistes par défaut fausses dans un pack (ex. piste russe par défaut) : corriger sur des **copies** (`mkvpropedit`),
  jamais sur les fichiers du torrent, qui doivent rester intacts pour le partage.

## 6. Imports

- **Import d'un téléchargement que l'Arr n'a pas demandé** : `ManualImport` en `importMode: copy` (lien physique), jamais
  `auto` (= déplacement : le torrent perd ses fichiers). `GET manualimport` **sans** `downloadId` (liste vide sinon) et
  **sans** id de fiche (avec l'id, l'Arr renvoie les fichiers déjà rangés). C'est ce que fait `torrent_import`.
- `GET /api/v3/manualimport` de Sonarr dure ~25 s sur un gros `/downloads` (délai 5 min). Avec `folder=` un fichier
  situé dans le dossier d'une série, Sonarr renvoie tous les fichiers de la saison : **toujours choisir le candidat par
  chemin exact**, jamais le premier.
- **`torrent_import` ne remplace jamais un fichier** : épisode ou film déjà présent ⇒ fichier écarté ; la correspondance
  d'épisodes de l'Arr prime sur l'analyse du nom. Après tout import manuel, vérifier l'historique (`episodeFileDeleted`,
  raison `Upgrade`).
- **Jamais d'import d'un fichier incomplet** : un torrent « terminé » peut contenir des fichiers désélectionnés après le
  début du téléchargement, donc tronqués. `torrent_import::incomplete_paths` écarte tout fichier à `progress < 1` ou
  priorité 0 (`TorrentFile.progress/priority`) ; un ajout avec sélection de fichiers se fait torrent **arrêté**
  (`add_torrent_with(…, true)`), puis désélection, **puis** démarrage. Sauvegarde de l'incident : `backups/fix-20260927/`.
- **`torrent_import` examine tout torrent complet de la seedbox**, quelle que soit son étiquette : tant qu'un ancien
  fichier est là, il note le nouveau « rien à importer » pour de bon. Une entrée en `nothing_importable` est
  **définitive** (`is_final`) : un correctif ne la rouvre pas, il faut la retirer de l'état, **daemon arrêté**.
- **Doublons** : rien n'est importé côté seedbox qui existe déjà sur le VPS (`dup_other_side`).
- **Titre supprimé puis redemandé** : `deletion_cleanup` garde les torrents en partage (C411 : ratio 1 ou 7 j) alors que
  les fichiers sont supprimés ; redemandé, `torrents/add` répondait « Fails. » (déjà présent) et rien ne s'importait.
  `series_search` cherche l'`infoHash` de la release parmi les torrents du qBittorrent concerné ; présent et complet, il le
  **réétiquette** (`qbit::retag`) et efface son enregistrement `torrent_import` (sinon `is_candidate` le saute).
- **Remplacer un titre par une version plus légère** : procédure dans [seedbox-et-rclone.md](seedbox-et-rclone.md#remplacer-un-titre-par-une-version-plus-légère).

## 7. Jellyseerr

- **Demandes validées d'office** pour tous (bit 128, `[accounts] jellyseerr_auto_approve`, posé à la création et à
  l'activation ; `defaultPermissions = 160`). Garde-fou : quota par défaut 10 films + 10 saisons / 7 j
  (`defaultQuotas`, admins et gestionnaires exemptés). Sauvegarde d'avant : `backups/jellyseerr-settings-main-20260915-094557.json`.
- **Vue d'ensemble des membres** : droit « voir les demandes » (bit 16384) sur les comptes actifs, dans
  `defaultPermissions` et à l'activation (`[accounts] jellyseerr_view_requests`) ; réglages Jellyfin Enhanced dans
  [jellyfin-interface.md](jellyfin-interface.md#10-vue-densemble-des-membres).
- **Une réponse de `settings/main` contient la clé API** : ne jamais l'afficher (filtrer les champs).
- **Job « Download Sync »** toutes les 5 min (`0 */5 * * * *`, 07/10) au lieu de chaque minute :
  `POST /api/v1/settings/jobs/<id>/schedule` avec `{"schedule": "…"}` (`cronSchedule` → 400). Les barres d'avancement
  des membres viennent de homelabd, pas de ce job.
- **Bibliothèques Jellyfin dans Jellyseerr (Seerr 3.2)** : ne jamais appeler `settings/jellyfin/library?sync=true` sans
  renvoyer `?enable=` avec la liste complète ; **le `GET settings/jellyfin/library` sans paramètre désactive lui aussi
  tout** (vérifié le 20/09). Lire l'état par `GET /api/v1/settings/jellyfin` (champ `libraries`) ; n'appeler
  `?enable=<ids>` qu'en écriture ; `--max-time 60` (un `?sync=true` peut rester bloqué). Six bibliothèques activées
  (Films, Séries, Anime, Films d'animation, Séries russes, Films russes). Le lot 3 prépare Seerr 3.5.0, qui fait
  disparaître ce piège : à réécrire après son exécution (`backups/lot3-20261008/NOTES-DOC.txt`).
- **Suppression d'une demande** : `deletion_cleanup` supprime la fiche Arr **et ses fichiers**, les torrents devenus
  inutiles, puis la fiche média — **seulement pour les médias « en attente » (2) ou « en cours » (3) sans demande**. Un
  scan Jellyfin crée une fiche média pour tout ce qui est déjà dans la bibliothèque, toutes « disponible » ou
  « partiel » : les toucher effacerait la médiathèque.
- **Titre mal identifié par Jellyfin** ⇒ demande restée « en cours » : recette dans
  [jellyfin-serveur-et-extensions.md](jellyfin-serveur-et-extensions.md#4-identification-des-titres).
- **Rotation de la clé API Jellyseerr** : `sudo scripts/jellyseerr-rotate-key.py` (régénère, puis met à jour `.env`
  `JELLYSEERR_API_KEY` + restart homelabd, `JellyseerrApiKey` de Jellyfin Enhanced **et** de Home Screen Sections, et
  l'intégration Homarr, Homarr arrêté ; ~12 s de coupure, aucune clé affichée). Nouveau consommateur de la clé = l'ajouter
  au script.

## 8. Recherche manuelle (`/recherche`)

- Passer par la page **`/recherche`** de homelabd (hôte d'onboarding, liste NPM « admin-outils » + session), jamais par
  la recherche de Sonarr/Radarr sur un animé (plusieurs minutes, « timed out » du proxy de la seedbox à 300 s, 429).
- Une requête C411 par identifiant TMDB (dans la réserve du budget) ; Nyaa pour les animés (liens **magnet** ajoutés au
  qBittorrent du côté concerné avec l'étiquette `homelab:`). Toutes les releases sont montrées et marquées (VOSTFR,
  hors profil, autre saison…) ; le choix reste à l'admin. Détail : [AUTOMATION.md](../AUTOMATION.md#recherche-manuelle-recherche).

## 9. autobrr (seedbox) : flux C411 désactivé le 08/10

- autobrr (natif, `127.0.0.1:16123` sur la seedbox, base `~/.apps/autobrr/autobrr.db`) lisait le flux Torznab C411 toutes
  les 10 min (`t=caps` puis `t=search&extended=1&limit=100`, 288 requêtes/jour) et poussait **chaque** release, quelle que
  soit sa catégorie, à Radarr **et** Sonarr de la seedbox (filtre unique sans critère). Du 02/10 au 08/10 : 7 483
  releases, ~15 000 poussées refusées, 3 acceptées (gain ~15 min sur le RSS), ~440 avertissements « Unable to parse » par
  jour dans les Arrs. Il utilise une **troisième clé C411**, stockée seulement dans sa base, **comptée nulle part**.
- **Pourquoi pas deux filtres par catégorie** : autobrr 1.86.0 (dernière version) ne transmet pas au filtre les
  `torznab:attr category` de C411 (`[match category] not matching: got  want: …`) ; un filtre par catégorie paraît actif
  et ne fait rien ; trier sur le nom est impossible (packs XXX, jeux et musique portent résolution et codec).
- **Décision** : flux C411 désactivé (`PATCH /api/feeds/1/enabled`) ; le RSS des Arrs (15 min) reste la voie d'entrée.
  L'appli reste en marche (le chien de garde de la seedbox la garde) ; filtre 1 et ses deux actions intacts : **réactiver
  le flux remet aussitôt le bruit d'origine**. Clé d'API autobrr « homelab » créée le 08/10 (dans sa base seulement).
  Sauvegarde dans `~/.local/state/autobrr-avant-20261008/` sur la seedbox (700/600, secrets : à supprimer après
  quelques jours).
- **Retour arrière** : réactiver le flux (interface : Réglages → Flux → C411, ou `PATCH /api/feeds/1/enabled` avec
  `{"enabled": true}` et l'en-tête `X-API-Token`, réponse 204). Par la base seulement si l'API ne répond pas (efface aussi
  la clé « homelab » et les releases enregistrées depuis).
- **Si la réactivité devient nécessaire** : C411 respecte `cat=` avec des sous-catégories exactes ; seule séparation fiable
  = à la source, deux flux (champ « catégories » du flux), chacun avec son indexeur Torznab, un filtre sans critère et une
  seule action (Radarr pour 2030/2060, Sonarr pour 5000/5070/5080) ; 4 requêtes par cycle sur la troisième clé. Levier plus
  simple : `rssSyncInterval` à 10 sur les Arrs de la seedbox. **À décider par le propriétaire** ; la troisième clé peut
  aussi être révoquée chez C411.
- **Pièges de l'API autobrr** : cookie limité à `Path=/autobrr/` (utiliser l'en-tête `X-API-Token`) ; API locale sous
  `/api/…` ; `POST`/`PUT /filters` exigent `resolutions`, `codecs`, `sources`, `containers` à `[]` (sinon 500) ; autobrr
  s'arrête au premier filtre qui retient une release et n'enregistre pas celles qu'aucun filtre ne retient ; un indexeur
  Torznab se crée par `POST /indexer`, puis le flux par `POST /feeds` avec `indexer_id`.
