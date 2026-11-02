# Catalogue et suppressions

Rapport du catalogue jamais regardé, liste rouge et suites d'une suppression. Sommaire de toutes les tâches :
[AUTOMATION.md](../AUTOMATION.md).

## catalogue_report — 24 h (2026-10-08)

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
- Pour **agir** sur ce rapport (exempter, supprimer), voir `red_list` ci-dessous et la page `/accounts/liste-rouge`.

## red_list — 24 h (2026-10-20)

Liste rouge : films et séries que personne n'a lancés depuis `min_idle_days` jours, sur la page d'admin
`/accounts/liste-rouge` (lien depuis `/accounts` et la section catalogue de `/status.html`). Même lecteurs que
`catalogue_report` (Arrs, tous les comptes Jellyfin avec `UserId`, Playback Reporting).

- **Rouge** = dernière activité il y a au moins `min_idle_days` jours ; l'activité est la plus tardive de l'arrivée (film :
  fiche ou fichier ; série : fiche, première et dernière diffusion, import du dernier épisode), de la dernière lecture
  d'un compte (`LastPlayedDate`) ou de Playback Reporting (≥ 60 s), et de l'horloge « vu sans date » (`undated_since`,
  démarrée à la première observation). Un titre présent sur le VPS et la seedbox n'est rouge que si **toutes** ses copies
  le sont ; sans identifiant TMDB/TVDB, sans date d'arrivée ou absent de Jellyfin : jamais rouge (« Non évalués »). Voie
  russe évaluée et étiquetée (`include_russian`). Sections « Bientôt rouges » (`soon_days`, 14) et « Exemptés ».
- **La tâche ne fait que calculer** (une fois par jour, ou « Recalculer » sur la page) ; elle marque aussi « interrompu »
  un travail coupé par un redémarrage et rejoue les étapes en attente (corbeille de l'Arr, média Seerr : 5 par passage,
  abandonnées après 14 j ; le média Seerr est gardé si le titre est revenu dans un Arr).
- **Exempter** : permanent, jusqu'à « Retirer l'exemption ».
- **Nettoyer** (liste : les `max_titles_per_clean` plus gros, 50 ; ou « Nettoyer ce titre ») : page de confirmation avec
  la liste exacte et son empreinte (refus si la liste a changé entre-temps), un seul travail à la fois (verrou partagé avec
  « Recalculer »), suivi sur `…/travail?id=`. Le travail lit le quota de la seedbox, refait un **calcul complet** (refus
  si un côté Arr est injoignable ou si le calcul est incomplet) et ne garde que les titres confirmés, rouges maintenant et
  non exemptés. Par titre : revérifications (exemption, lecture en cours, rangement par `anime_library` depuis moins de
  6 h, fiche inchangée : même identifiant et même dossier), fiche sauvegardée (`backups/liste-rouge-<date>/`, 600),
  tailles exclusives mesurées (`nlink` sur le VPS ; script python fixe par ssh sur la seedbox, chemins par l'entrée
  standard), `DELETE` de la fiche avec `deleteFiles` **jamais rejoué** (délai `arr_delete_timeout_secs` ; après une erreur
  la fiche est relue, un 404 vaut réussite, sinon ce côté s'arrête), dossier du titre vidé de la corbeille de l'Arr après
  `recycle_wait_secs` (gardes : chemin absolu sans `..`, strictement sous une corbeille connue, jamais un lien), torrents
  d'origine (sauf encore utilisés ; délai C411 respecté), `vfs/forget` + `vfs/refresh` sur la seedbox, média Seerr retiré
  quand plus aucune copie ne reste. À la fin : un seul `Library/Media/Updated` « Deleted » (jamais `DELETE /Items`) et une
  alerte admin récapitulative.
- **Garde-fous ajoutés le 22/10** : titre écarté si ses copies (côté, fiche, dossier) diffèrent de la liste confirmée ;
  média Seerr retiré seulement quand les dossiers du titre ont disparu (sinon étape `seerr` rejouée, montage tombé =
  attendre) ; sur le VPS, torrents retenus par un fichier encore dans la corbeille revus quand la reprise la vide ;
  reprise d'une corbeille seedbox seulement si l'Arr déclare toujours la corbeille notée à la suppression.
- **Simulation** tant que `clean_dry_run = true`, **titre seul compris** : rien n'est supprimé ni écrit hors de l'état,
  le travail dit ce qu'il ferait. `--dry-run` de homelabd l'emporte toujours. En suppression réelle, la confirmation
  propose aussi « Simuler d'abord ». La simulation compte les fichiers du titre comme déjà supprimés : un torrent lié au
  seul titre est annoncé retiré, comme pour de vrai (avant le 22/10, il était compté « partagé »).
- Réglages `[tasks.red_list]` : `min_idle_days` (30 au moins), `include_russian`, `max_titles_per_clean` (200 au
  plus), `clean_dry_run`, `arr_delete_timeout_secs` (300), `recycle_wait_secs` (90), `empty_recycle`,
  `add_import_exclusion` (false : un membre peut redemander le titre), `ssh_host`, `jobs_kept` (20), `soon_days` (14).

## deletion_cleanup — 5 min

Suite d'une suppression faite dans Jellyfin (bouton « Supprimer » ; médias montés en écriture pour ça).
Un fichier connu d'un Arr est tenu pour supprimé seulement si **trois signaux** concordent : absent du
disque (`NotFound`, pas une erreur d'E/S), absent de Jellyfin (`GET /Items?Fields=Path`), et déjà absent
au passage précédent (`confirm_after_secs`). Garde-fous : imports de moins de `min_file_age_mins` ignorés ;
côté seedbox, montage vérifié et cache rclone rafraîchi (`vfs/refresh`) avant de conclure ; plus de
`abort_if_missing_titles_over` titres manquants d'un coup ⇒ rien n'est fait (disque ou montage) ;
`max_titles_per_run` titres par passage ; une machine injoignable est sautée, l'autre continue.
Chemins : côté VPS, `vps_paths` donne chaque racine en triplet (chemin dans l'Arr, sur l'hôte, dans Jellyfin) ; côté
seedbox, `[seedbox] media_root`, `mount_point` et `jellyfin_root` jouent le même rôle.

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
