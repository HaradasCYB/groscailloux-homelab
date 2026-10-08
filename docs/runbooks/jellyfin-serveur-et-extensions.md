# Jellyfin : serveur, bibliothèques et extensions

À lire avant de toucher à la configuration du serveur Jellyfin, à une bibliothèque, à une extension (installation, mise à
jour, réglage) ou à Intro Skipper. Interface et scripts injectés : [jellyfin-interface.md](jellyfin-interface.md).
Lecture et transcodage : [lecture-et-transcodage.md](lecture-et-transcodage.md). Migration 10.11 → 12.1 :
[JELLYFIN-12.md](../JELLYFIN-12.md).

## 1. Version et migration 12.1

- **Jellyfin 12.1 en service depuis le 03/10 à 23:01** (source de vérité : image du service `jellyfin` dans
  `docker-compose.yml`). Procédure et constats : [JELLYFIN-12.md](../JELLYFIN-12.md).
- **Instantané 10.11.8** : `backups/jellyfin-snapshot-10.11.8-20261003/` (root 700). Retour arrière :
  `sudo …/rollback.sh --yes` (~2 min ; tout ce qui a changé depuis la bascule est perdu). Suppression le **10/10 à 12:00**
  par le minuteur **transitoire** `jellyfin-snapshot-purge` (perdu si le VPS redémarre avant : supprimer alors à la main).
- **`EnableLegacyAuthorization = true`** dans `config/system.xml` : la migration le met à `false`, et alors `X-Emby-Token`
  et `?api_key=` répondent 401 ; toutes les applis ouvrent leur websocket par `api_key` et le Chromecast lit ses flux
  ainsi. Le couper un jour = passer d'abord homelabd, `scripts/` et nos scripts à `Authorization: MediaBrowser
  Token="…"` (accepté par 10.11 et 12.x).
- **Jamais d'édition à la main de `system.xml` Jellyfin démarré** : `GET /System/Configuration`, changer le seul champ,
  `POST` de l'objet entier.
- **Redémarrer Jellyfin** : hors pic, sans lecture en cours. Une appli restée ouverte (Jellyfin Desktop) garde l'ancienne
  page (nouveau CSS, anciens scripts : pas de rangées, ancien logo, SyncPlay qui envoie des titres sans id → « Guid can't be
  empty ») : faire recharger (Ctrl+R) ; un redémarrage coupe aussi les groupes SyncPlay.

## 2. Réseau

- **`KnownProxies = 172.18.0.0/16`** (réseau Docker entier) dans la configuration réseau, lu **au démarrage seulement**.
  Avant le 15/09 il valait l'ancienne IP de NPM : Jellyfin voyait tout le monde comme NPM, donc comme réseau local.
  Aucune limite de débit « distant » n'est réglée, ce qui compte si ça change.
- `PublishedServerUrl` = `${JELLYFIN_PUBLIC_URL}` (compose) ; port 8096 publié sur `127.0.0.1` seulement.

## 3. Bibliothèques et analyses

| Bibliothèque | Dossiers | Id (si utile) |
| --- | --- | --- |
| Films, Séries | `/media/movies`, `/media/tvshows` + `/seedbox/media/Movies`, `/seedbox/media/TV Shows` | `JELLYFIN_LIB_FILMS`, `JELLYFIN_LIB_SERIES` (`.env`) |
| Anime | `/media/anime` + `/seedbox/media/Anime` | `0c41907140d802bb58430fed7e2cd79e`, dans `JELLYFIN_LIB_EXTRA` |
| Films d'animation | `/media/anime-films` + `/seedbox/media/Anime Movies` | `bebdce85c5b682ddbce0412f41cff060`, dans `JELLYFIN_LIB_EXTRA` |
| Collections | collections (sagas, rangées d'accueil) | dans `JELLYFIN_LIB_EXTRA` |
| Séries russes, Films russes | dossiers `Russian` de la seedbox | voir [voie-russe.md](voie-russe.md) |

- Un membre ordinaire a la liste explicite des 5 bibliothèques (Films, Séries, Anime, Films d'animation, Collections ;
  `JELLYFIN_LIB_EXTRA` et `EnabledFolders`), pas de TV en direct, pas de téléchargement, verrouillage après 5 échecs
  (modèle `non_admin_policy`). **Seuls les deux comptes protégés ont `EnableAllFolders = true`.** Ordre du menu
  (`OrderedViews`) : Films, Séries, Anime, Films d'animation, Collections, posé à la création du compte.
- La bibliothèque « Collections » est donnée à tous les comptes : sans elle, un compte ordinaire ne voit ni les sagas ni
  les rangées de collections.
- `JELLYFIN_LIB_EXTRA` (`.env`) = Anime, Films d'animation et Collections : l'onboarding les donne à chaque nouveau
  compte avec Films et Séries. Une bibliothèque à donner à tous = son id dans `JELLYFIN_LIB_EXTRA` **et** dans les
  `EnabledFolders` des comptes existants ; les bibliothèques russes n'y sont jamais.
- Animés : rangés par `anime_library` d'après **TMDB** (genre Animation + origine japonaise), jamais d'après le type
  « anime » de Sonarr. Forcer : tag `anime` ou `pas-anime` dans l'Arr (`pas-anime` = id 4 sur le Sonarr seedbox, posé le
  07/10 sur deux fiches sans TMDB ; journaliser un « inconnu » une fois par jour reste à faire dans le code). **Tout
  déplacement en masse hors de cette tâche : `deletion_cleanup` dans `tasks.disabled` pendant ce temps.** Jellyseerr :
  `activeAnimeDirectory` + `animeTags` des deux Sonarr sur le dossier et le tag anime.
- **Une bibliothèque créée par l'API reste vide, et un titre déplacé d'une bibliothèque seedbox à une autre
  n'apparaît pas**, tant qu'une analyse complète de la médiathèque n'est pas passée (ni `Items/{id}/Refresh`, ni
  `Library/Media/Updated`, ni un redémarrage). `anime_library` la lance lui-même après un déplacement (`scan_after_move`,
  au plus une toutes les `scan_min_gap_mins` = 20) et **ne relance pas tant qu'une autre tourne**
  (`library_scan_running`). **Ne jamais lancer une analyse à la main par-dessus une autre** : `Library/Refresh` annule
  celle en cours (des analyses annulées après 25 à 65 min, titres jamais affichés). Ranger en masse avant 13 h, ou attendre
  l'analyse de 05 h.
- **Supprimer une bibliothèque** (constaté en 10.11) : elle reste dans les vues jusqu'au redémarrage ; ses
  `CollectionFolder` restent en base (visibles des comptes `EnableAllFolders`). Ordre complet : supprimer → analyse
  complète → redémarrage de Jellyfin → Jellyseerr `?sync=true` + `?enable=` (voir le piège Jellyseerr dans
  [arrs-et-indexeurs.md](arrs-et-indexeurs.md#7-jellyseerr)).
- **`DELETE /Items/<id>` efface le disque** (c'est le bouton « Supprimer ») : jamais pour « nettoyer » une vue ou une
  bibliothèque.
- **`GET /Items` sans `UserId` renvoie une liste incomplète** (des épisodes importés le matin absents ; sans compte, 107
  films au lieu de 184) : toute lecture de la médiathèque par l'API passe par un compte (`UserId=<admin>`).
- **`EnableEmbeddedTitles = false`** sur Films, Séries, Anime et Films d'animation (22/09) : avec cette option, Jellyfin
  affichait le nom de release écrit dans la métadonnée `title` du MKV (30 films concernés). Une fiche déjà créée garde ce
  nom : ni `Refresh` (même `FullRefresh` + `ReplaceAllMetadata`) ni un rafraîchissement d'images ne le remplacent, **seul
  `RemoteSearch` + `Apply`** ; c'est ce que fait `identity_check` (`fix_release_names`). Sauvegarde
  `backups/jellyfin-libs-20260922/virtualfolders-before.json`.
- **Images manquantes** : un rafraîchissement ciblé (`ImageRefreshMode=FullRefresh`, `MetadataRefreshMode=None`) n'avait
  rien ramené de TMDB pour ~22 titres obscurs ; Fanart (05/10) a ajouté 5 logos (`fanart-logos.py`, `RemoteImages/Download`
  seulement : aucune fiche rafraîchie, aucune vidéo relue ; langues fr/en/sans, russe pour la voie russe). Ne pas relancer
  la chasse sans nouveau fournisseur. **Jamais de `Refresh` sur une série pour une image** : il est récursif (épisodes
  sondés par le lien seedbox).
- **Historique de lecture** : Playback Reporting en conservation illimitée depuis le 05/10 (`MaxDataAge = -1` dans la
  configuration **nommée** `GET|POST /System/Configuration/playback_reporting`, pas dans `/Plugins/<id>/Configuration` ;
  **0 effacerait tout**).

## 4. Identification des titres

- Jellyfin identifie un dossier **d'après son nom** et confond les spin-offs (un film au titre court, une série proche
  d'un spin-off). `identity_check` (30 min) compare chaque fiche Jellyfin à l'identifiant de Sonarr/Radarr, qui fait foi,
  et corrige seule (`RemoteSearch` + `Apply` + métadonnées, 3 titres au plus par passage, jamais pendant une lecture).
- **À la main** — film : `POST /Items/RemoteSearch/Movie` (ProviderIds Tmdb) puis `/Items/RemoteSearch/Apply/{id}`, puis
  le job Jellyseerr `jellyfin-full-scan` (le scan « recently added » ne revoit pas un titre de la veille ; tant que
  l'identifiant TMDB diffère, la demande reste « en cours »). Série : `POST /Items/RemoteSearch/Series` (ProviderIds
  Tvdb), `Apply` (peut dépasser 3 min : vérifier ensuite plutôt que relancer), puis `Refresh` récursif en `FullRefresh` +
  `ReplaceAllMetadata` pour rattacher les épisodes.
- Après une réidentification ou un déplacement vers Anime, comparer `ProviderIds` à l'identifiant de l'Arr : la
  réidentification peut repartir sur un spin-off.

## 5. Collections après un remplacement de fichier

- Le nouveau fichier crée un nouvel élément ; les collections gardent un **lien mort** vers l'ancien chemin (« Unable to
  find linked item »). Remettre le nouvel élément dans ses collections (`POST /Collections/<id>/Items?ids=`) ; les liens
  morts partent avec la tâche « Nettoyer les collections et les listes de lecture » (au démarrage).

## 6. Extensions

- **Mises à jour manuelles** : la tâche « Mettre à jour les extensions » n'a plus de déclencheur (sauvegarde
  `backups/jellyfin-tuning-20261004/`) ; une mise à jour automatique s'activait au redémarrage suivant, sans vérification.
- **Le catalogue ne remplace pas une compilation 10.11 par la compilation Jellyfin 12 du même numéro** (cas de JavaScript
  Injector 4.0.0.0 et NotifySync 5.8.4.0) : à chaque mise à jour, vérifier `targetAbi` dans `meta.json` et le dépôt
  « jf12 » de l'auteur. Extensions d'IAmParadox27 (Home Screen Sections, Plugin Pages, Collection Sections) : un même numéro
  existe pour plusieurs ABI ; installer par le catalogue du serveur (`/Packages`) et vérifier `targetAbi`.
- **JavaScript Injector** active aussi les scripts d'autres extensions qui s'y enregistrent et **ne les purge jamais** :
  une extension retirée laisse son script dans `PluginJavaScripts` (à retirer à la main).
- **Lot du 05/10** (validé, appliqué sans surveillance à 08:47, coupure 52 s) : Jellysleep et GetAvatar **retirés** ;
  JavaScript Injector et NotifySync sur leurs **compilations Jellyfin 12** (même GUID, configurations reprises ;
  NotifySync sur sa branche `jellyfin-12`) ; **Fanart 15.0.0.0** juste après TheMovieDb dans les sources d'images Movie et
  Series (actif aussi sur les collections, choix du propriétaire). Outils, paquets épinglés (MD5 + SHA-256) et retour
  arrière : `backups/jellyfin-plugins-20261005/` (`apply-restart-batch.sh`, `rollback-restart-batch.sh <dossier
  apply-prod-*> --yes`, `prepare-test2.sh` pour une instance d'essai sur :18097).
- **Collection Sections** (rangées Tendances, Anime, Les mieux notés, Films français) : pas de version 12.x publiée ;
  **recompilée le 04/10** contre Jellyfin 12.1.0 depuis le code relu de la copie communautaire
  (DD00031/jellyfin-plugin-collection-sections, 3541f2e : cible 12 + 3 petits fichiers, aucune logique changée), même
  GUID ; installée en production le 04/10. Paquet, `install.sh` (refus si lecture en cours) et `rollback.sh` (ancienne
  2.3.10.0) dans `backups/jellyfin-collectionsections-20261004/`. **Provisoire** : à la sortie de la fonction équivalente
  dans Home Screen Sections, basculer et retirer ce paquet. Une copie compilée pour 12.0 n'est pas sûre en 12.1.
- **Home Screen Sections** : ordre des rangées **global** (identique pour tous les comptes), modifié par
  `POST /Plugins/b8298e012697407ab44daa8dc795e850/Configuration` (sans redémarrage) ; « Mes médias » (`MyMedia`) en
  tête (`OrderIndex 0`, sauvegarde `backups/jellyfin-ui-20260920-mymedia/`). **Contrôler comme l'appli** :
  `GET /HomeScreen/Sections?UserId=…&Language=fr&Page=1&NumResultsPerPage=4&PageHash=<uuid v4>` ; l'appel sans
  pagination est en cache **24 h par compte** (`CacheTimeoutSeconds`) et montre l'ancien ordre. Faille 3.0.2 (issue amont #298) fermée par NPM :
  [npm.md](npm.md#garde-de-home-screen-sections).
- **Jellyfin Enhanced** (réglages serveur) : page Téléchargements rafraîchie toutes les **120 s**
  (`DownloadsPollIntervalSeconds` ; à 30 s, deux pages admin restées ouvertes faisaient 63 % du trafic Jellyfin) ;
  `ThemeSelectorEnabled = false` ; `JellyseerrShowNetworkDiscovery` coupé (il exige une clé TMDB absente : 928 réponses
  503/jour) ; `JellyseerrShowAdvanced = true` (voie russe) ; région = France ; calendrier en 24 h
  (`CalendarTimeFormat = 17:00/17:30`). Vue d'ensemble des membres : [jellyfin-interface.md](jellyfin-interface.md#10-vue-densemble-des-membres).
- **NotifySync et les websockets** : voir « Une seule connexion temps réel par page » dans
  [jellyfin-interface.md](jellyfin-interface.md#9-une-seule-connexion-temps-réel-par-page).

### Intro Skipper

- **Version 12.0.4, segments figés et réglages sobres** (05/10, décision du propriétaire). La migration 1.10.11.19 →
  12.0.4 avait importé les analyses sans `ConfigHash` : toute la médiathèque était à refaire (~700 Go par le lien seedbox en
  8 à 9 matinées), alors que 1 948 des 2 026 titres en file avaient déjà leurs segments. Aucune base en ligne ne convenait
  (TheIntroDB : 403 Cloudflare depuis l'IP du VPS, 45 % des séries ; chapitres : 24 % ; AniSkip : 34 %).
  **Gel** : 3 635 segments passés en Source « User » par l'API du plugin (`PUT /Episode/{itemId}/Segments/{segmentId}`,
  bornes identiques) ; un segment User n'est **jamais** recalculé. Les 130 génériques suspects des séries non animées
  (> 180 s ou début avant la moitié du fichier) sont restés automatiques pour être refaits.
- **Réglages** : `ScanRecap = false`, `AnalysisLengthLimit = 6`, `MaximumCreditsDuration = 300`,
  `MaximumMovieCreditsDuration = 600`, `PathExclusions` = les 5 dossiers de films (les génériques de films existants
  restent servis, les nouveaux films n'en ont plus), `SeriesExclusions` = Mentalist (intros de ~8 s introuvables) et
  Brooklyn Nine-Nine (génériques introuvables). Récaps existants toujours servis (aucune appli ne propose de passer un
  récap par défaut). Croisière attendue ~60 Go par semaine au lieu de ~86.
- **Règle vérifiée dans le code** : un changement de réglage ne remet en file que ce qui a été analysé sous le hash
  courant, jamais un segment User ; `AnalysisLengthLimit` entre dans les hashes Intro/Recap, `Maximum*CreditsDuration`
  dans celui des génériques ; les `Scan*` et les exclusions dans aucun. **Rallumer `ScanRecap` remettrait toute la
  médiathèque en file pour les récaps.**
- **INTERDIT en production** (efface aussi les segments User) : « Exécuter » la tâche à la main, `POST /Intros/ScanSeason`
  (bouton d'analyse d'une saison), `DELETE /Intros/Show/…`, `POST /Intros/ExcludedTimestamps/Clear`, EraseTimestamps,
  `POST /Intros/AnalyzerActions/UpdateSeason`. Fermer toute page de réglages du plugin ouverte avant un changement par
  l'API (elle renverrait l'ancienne configuration).
- **Corriger un segment figé faux** : `DELETE /Episode/{itemId}/Segments/{segmentId}` (réanalysé au passage suivant, sauf
  s'il garde un autre segment User du même type ; jamais sur un élément exclu).
- **Fichier remplacé au même chemin** : supprimer d'abord ses segments User. **Refaire le gel avant toute mise à jour du
  plugin.**
- Contrôle d'un passage : `grep -E '\[Mode: (Introduction|Credits)\] Analyzing' log_<date>.log` (« [Mode: Preview] » est
  normal : chapitres, sans ffmpeg), tâche « Completed », `rclone_core.bytes` de 03 à 07 h UTC autour de 60 Go.
- Au démarrage, « ffmpeg did not exit within 2000ms » quand la machine est chargée : sans gravité (`FFmpegVersionGate`
  revérifie au prochain usage), un avertissement reste dans sa page de réglages jusqu'au redémarrage suivant.
- Outils, journal et sauvegardes : `backups/introskipper-sobre-20261005/` (`freeze.py --dry-run`, `apply-config.py
  --restore`, `prod-20261005/` : journal CSV, copies de la base avant/après, procédure de retour arrière).

## 7. Journaux

- `jellyfin/config/config/logging.json` (copie de `logging.default.json`) : Playback Reporting et Collection Sections en
  `Warning`, traductions manquantes de Home Screen Sections en `Error`.
- **Durée de garde = `LogFileRetentionDays` de `config/system.xml`** : 14 jours depuis le 04/10 (la tâche quotidienne
  « Supprimer les fichiers journaux » efface tout ce qui dépasse dans `config/log` : journaux principaux, `FFmpeg.*`,
  `upload_*`). Le `retainedFileCountLimit` (14) de `logging.json` ne compte que les `log_*.log`. Sauvegarde
  `backups/jellyfin-mobile-ui-20261004/`.
- `upload_*.log` = journaux envoyés par les clients (`ClientLog/Document`, lignes « gc-airplay … ») : les lire avant toute
  hypothèse sur ce que fait l'appli iPhone.
