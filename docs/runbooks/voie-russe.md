# Voie russe (RuTracker)

À lire avant de toucher au tag `russe`, aux dossiers `Russian` de la seedbox, au Jackett ou au FlareSolverr de la
seedbox, à la tâche `russian_search` ou au bouton « Chercher en russe » de Mon compte. Mise en place le 26/09 pour un
membre ; c'est la seule exception à « C411 seul » et à « aucune recherche depuis les Arrs ». Tâches :
[AUTOMATION.md, russian_search](../AUTOMATION.md#russian_search--10-min) et
[anime_library](../AUTOMATION.md#anime_library--5-min).

## Principe

- **C'est un CHOIX du membre, jamais la langue TMDB.** Un film russe demandé normalement suit la voie classique (C411, VF
  si elle existe). Le membre autorisé choisit le dossier `…/Russian` ou `…/Russian Movies` à la demande, ou passe sa
  demande en russe depuis Mon compte.
- **RuTracker** est déclaré dans le **Jackett de la seedbox** (`app-jackett`, `172.17.0.1:16129` sur la seedbox, compte
  `RUTRACKER_USERNAME/PASSWORD` dans `.env` — mot de passe collé dans une conversation, à changer), derrière le
  **FlareSolverr de la seedbox** (`app-flaresolverr`, port 16111) : RuTracker présente un défi Cloudflare à l'IP de la
  seedbox ; sans lui, « Challenge detected but FlareSolverr is not configured ».
- Les Arrs de la seedbox ne joignent **pas** le Prowlarr du VPS : indexeur Torznab « RuTracker » dans Sonarr (id 10) et
  Radarr (id 8) de la seedbox, **tag `russe`** (ne sert qu'à ces fiches), RSS + recherche automatique. C411 reste sans tag
  et sans recherche : une recherche de l'Arr sur une fiche `russe` n'interroge que RuTracker.
- Tracker `t-ru.org` dans `[tasks.tracker_ratio] secondary` (ratio compté chez eux). L'hébergeur de la seedbox arrête les
  torrents sans drapeau `private` une fois terminés : les torrents RuTracker ne partagent donc pas (ratio du compte à
  surveiller ; **ne pas contourner** la règle de l'hébergeur).
- Bazarr : `Excluded Tags` = `russe` sur Sonarr et Radarr (une fiche russe ne reçoit pas de sous-titres automatiques).

## Tâches

- **`anime_library`** : fiche dans un dossier russe (`[tasks.anime_library] seedbox_ru_series_root` /
  `seedbox_ru_movies_root`) ⇒ tag `russe` posé et, si elle est incomplète, recherche `SeriesSearch` / `MoviesSearch` au
  plus toutes les `ru_search_retry_hours` (`state.russian_searches`) ; tag `russe` posé à la main hors du dossier ⇒ fiche
  déplacée (`russian_choice`). Un titre arrivé dans un dossier russe sans qu'aucun de ses demandeurs soit autorisé est
  remis en classique (`russian_folder_allowed`) : **Jellyseerr ne contrôle pas le dossier à la création d'une demande**,
  seule son interface le masque.
- **`series_search` / `movie_search`** ignorent la voie russe (`russian_route`).
- **`russian_search`** (10 min) : Sonarr/Radarr cherchent par le titre anglais (« The Interns » → 0, « Интерны » → 18) ;
  la tâche cherche par le **titre original TMDB** dans le Jackett de la seedbox (joignable du VPS par
  `[tasks.russian_search] jackett_url`, clé `SEEDBOX_JACKETT_API_KEY`), plage d'épisodes en **numérotation absolue**
  (`E1-120`, `S4E61-268`, `(101-120)`), torrent `homelab:russe` ajouté **arrêté** avec **seuls les fichiers manquants**
  (`Интерны_060.avi`, `S2E02` → absolu par l'Arr), puis démarré ; import `ManualImport` copy puis analyse complète de
  Jellyfin. Seedbox injoignable : côté sauté, passage réussi.

## Jellyfin et Jellyseerr

- Bibliothèques « Séries russes » `1de6d599992bb110f4cab5814bc899be` et « Films russes »
  `4b5799916c780f2a26b93b2970081b95`, **données au seul compte autorisé**, pas dans `JELLYFIN_LIB_EXTRA`. Elles restent
  visibles des deux comptes protégés (`EnableAllFolders`) ; l'admin non protégé a été passé en liste explicite sans elles
  le 27/09. Activées dans Jellyseerr (6 bibliothèques). Limite connue : Jellyseerr affiche « disponible » à tous pour un
  titre russe.
- Vignettes des deux bibliothèques : `backups/russe-20260926/lib-ru-*.png` (images TMDB : hors du dépôt public). Logos
  Fanart : langue russe pour ces fiches (voir [jellyfin-serveur-et-extensions.md](jellyfin-serveur-et-extensions.md)).

## Le choix côté membre (Mon compte)

- Bouton « Chercher en russe » / « Voie russe · revenir au classique » sur **ses propres** cartes de l'onglet Demandes
  (script Mon compte, `GET|POST /compte/api/route`), seulement pour les comptes de `HOMELABD_RUSSIAN_USERS` (`.env`) ; le
  serveur vérifie que la demande est la sienne (Jellyseerr `requestedBy`, `serviceId` 1).
- **Aucun effet sur les autres membres** : passage en russe refusé (bouton masqué, 409) si un autre membre a demandé le
  titre ou s'il a déjà des fichiers (il disparaîtrait de Séries/Films).
- Retour au classique = tag retiré + fiche remise dans `seedbox_default_series_root` / `seedbox_default_movies_root` +
  `series_search` / `movie_search` lancés.
- **Choix à la demande** (27/09, voulu par le propriétaire) : `JellyseerrShowAdvanced = true` dans Jellyfin Enhanced
  (global) ; le script Mon compte **cache le bloc** à tous (CSS `gc-ru-css`) sauf au compte autorisé, pour qui il ne reste
  qu'un choix « Version » (Classique / Russe) ; il **enveloppe `JE.jellyseerrAPI.requestMedia/requestTvSeasons`** : les
  réglages avancés ne partent QUE pour un dossier russe choisi par un compte autorisé, sinon la demande part sans réglages
  (profil et dossier Anime automatiques). Sauvegarde de la config du plugin :
  `backups/russe-20260926/jellyfin-enhanced-config-before.json`.
- Tests : banc de non-régression versionné `tools/tests/compte-russe/` (26 contrôles, contre-épreuve `--env MUTATE=1`,
  voir [tools/README.md](../../tools/README.md)) ; ancien banc `backups/russe-20260926/ru_filter_test.js`.

## Mesure

- Le rapport `catalogue_report` (`/status.html`) suit la voie russe : épisodes disponibles et lus (au 08/10 : 120
  disponibles, 0 lu ; l'essai de 54 s du 26/09 est sous le seuil de 60 s de Playback Reporting). Demander au membre si les
  séries lui conviennent reste une action humaine, à la charge du propriétaire.

## Sauvegardes

`backups/russe-20260926/` (config d'avant, droits des comptes, vignettes, banc).
