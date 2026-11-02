# Jellyfin

Bibliothèques, identification, langues, sous-titres et ajouts de homelabd dans l'interface de Jellyfin. Sommaire de
toutes les tâches : [AUTOMATION.md](../AUTOMATION.md).

## anime_library — 5 min
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

## identity_check — 30 min

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

## original_language — 10 min, 07:30–11:30 (2026-10-08)

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
  [runbooks/lecture-et-transcodage.md](../runbooks/lecture-et-transcodage.md#langue-dorigine-original_language-lot-4-0810-et-préférence-native)).

## trending — 6 h

Rangée « Tendances cette semaine » de l'accueil Jellyfin. Classement lu dans Playback Reporting
(`POST user_usage_stats/submit_custom_query`, SQL en lecture) : épisodes regroupés par série, spectateurs
distincts puis heures ; un spectateur compte à partir de `min_minutes` sur le titre. Semaine trop calme :
complétée par `fallback_days`. Tient à jour la collection `collection_name` (création, puis ajouts et
retraits ciblés). Collection et non playlist : une playlist qui reçoit une série y déplie ses épisodes.

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

## Langue par membre (v1.19, 2026-09-21)

Chaque compte reçoit à l'onboarding (`[accounts] audio_language`, `subtitle_language`, `subtitle_mode`) : audio
préféré `fre`, sous-titres `fre`, mode **Smart** (sous-titres seulement quand l'audio n'est pas en français),
`PlayDefaultAudioTrack = false` (Jellyfin choisit la piste française d'un MULTi au lieu de la piste « par défaut » du
fichier). Rattrapage du 2026-09-21 sur les comptes sans préférence (sauvegarde `backups/jellyfin-language-20260921/`).
Dans « Mon compte », le membre choisit « Français quand il existe » ou « Toujours en VO, sous-titres français »
(`POST /compte/api/language` : audio `[accounts] vo_audio_language` (`jpn`) + sous-titres `Always`, mémorisation des
pistes coupée ; dans les clients web, le script Mon compte bascule ensuite sur la piste de la langue d'origine, voir
[runbooks/lecture-et-transcodage.md](../runbooks/lecture-et-transcodage.md#7-langue-audio-et-sous-titres-des-comptes)).

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
« pas rangé », un torrent rangé en partie (`partial`) « Téléchargé en partie : N épisode(s) pas rangé(s), l'administrateur
est prévenu » : `%` = 1 − Σ sizeleft / Σ size, ETA = `timeleft`/`eta` max + 2 min d'import + un passage de
`seedbox_refresh`), *ajout* (fichier
présent côté Arr, pas encore vu par Jellyfin), *disponible* (statut média Jellyseerr). Le lien demande ↔ torrent est
l'étiquette, le lien torrent ↔ décision de `torrent_import` est le hash (`state_key`), jamais le nom : la release prise et
le torrent arrivé ne s'appellent pas pareil. Un torrent déjà rangé (`imported`, `arr_managed`) sort de la file
(`shelved`) mais compte encore (2026-10-21) : « Téléchargement lancé, en attente de qBittorrent » ne s'affiche plus que
**sans torrent** (pas encore arrivé, retiré, qBittorrent injoignable) ; un torrent rangé il y a moins de 2 min + un
passage de `seedbox_refresh` donne « Téléchargé, ajout à la médiathèque », plus tard (saison toujours incomplète)
« Téléchargé en partie : le reste sera recherché dans … » (« … est recherché » après un épisode seul). Les fichiers
déjà là (`has_file`) priment sur un torrent bloqué ou rangé en partie. Le script « Mon compte »
détecte les cartes `.je-request-card` de Jellyfin Enhanced et y insère une barre + texte, rafraîchis toutes les
30 s ; sur téléviseur, pourcentage seul.

## Avertissements de l'onglet Téléchargements (09/10)

`GET /compte/api/downloads` (jeton Jellyfin, cache 30 s partagé, une seule lecture des files à la fois, 20 s au plus par
Arr) : la file brute des 4 Arrs (`queue_records_detailed` : `includeSeries`/`includeEpisode`, `includeMovie`), traduite
par `homelab_core::queue_explain` (pur et testé) en `{ source, title, year, episode, episode_title, badge, text, detail }`
pour les seuls éléments bloqués ; `detail` (message d'origine nettoyé) n'est envoyé qu'aux administrateurs Jellyfin. Une
file illisible est sautée ; si aucune n'a répondu, l'échec (502) est resservi 60 s sans relire les Arrs. Journal : un
`warn` par file illisible (et pour l'échec complet) au plus toutes les 15 min, `debug` entre-temps. Côté script, une
seule requête en vol et 25 s d'attente après une erreur. Les promesses d'action automatique suivent la configuration :
`id_match_import` actif (son `interval_secs`) et élément qu'il traite (`is_id_match_blocked`), `stuck_handler` actif et
son `pattern` qui reconnaît le message (`stall_secs`). Deux éléments que la carte de Jellyfin Enhanced ne distingue pas
(même source, titre, épisode) ne sont gardés qu'avec la même note. Le script « Mon compte » remplace le badge de statut
de la carte `.je-download-card` et ajoute une ligne `.gc-dl` (toutes les 3 s, 6 s sur téléviseur où seule la première
phrase s'affiche) ; familles : [arrs-et-indexeurs.md](../runbooks/arrs-et-indexeurs.md#10-avertissements-de-la-file-onglet-téléchargements).
Aucune écriture d'état, aucune action sur les Arrs.

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
