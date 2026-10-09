# Seedbox et montage rclone

À lire avant d'agir sur la seedbox (applis, quota, ménage, déménagement de titres) ou sur le montage rclone. Mise en
place et arrêt de la seedbox : [DEPLOY.md](../DEPLOY.md#seedbox-optionnelle). Schémas : [INFRA.md](../INFRA.md).

## 1. Accès et applis

- Accès admin : `ssh seedbox` (clé `~/.ssh/seedbox_ed25519`) ; applis via `app-<x> start|restart|backup…` (pas de
  `status`). Adresse publique des applis : voir `[seedbox]` de `homelab.toml` (`radarr_url`, `sonarr_url`, `qbit_url`,
  `bazarr_url`) et `[tasks.russian_search] jackett_url` ; jamais en dur dans une doc.
- Ports locaux sur la seedbox : Sonarr 16126, Radarr 16127, Bazarr 16131, Jackett 16129, FlareSolverr
  **172.17.0.1**:16111, autobrr 16123, qBittorrent 16141 (natif, `127.0.0.1` seulement). Les applis tournent en
  conteneurs Docker et joignent qBittorrent par le proxy HTTPS de l'hébergeur.
- qBittorrent de la seedbox vu de homelabd : `[seedbox] qbit_url` + `SEEDBOX_QBIT_PASSWORD`.
- Un `ssh seedbox cmd args` recolle les arguments en une seule ligne shell : passer les chemins par l'entrée standard.
- `du -sh ~` sur la seedbox renvoie 0 (`~` est un lien symbolique) : `du -sh ~/`.
- **Arrs et Bazarr de la seedbox** (07/10) : Sonarr et Radarr journalisent en Info (en debug : 25 h d'historique) ;
  Sonarr 4.0.20 et Radarr 6.4.4 à installer hors pic. Bazarr : fournisseur tvsubtitles retiré (403 permanents depuis
  l'IP de l'hébergeur), `Excluded Tags` = `russe` sur Sonarr et Radarr. API Bazarr : `POST /api/system/settings` en
  formulaire, une clé `settings-<section>-<clé>` par valeur (répétée pour une liste), sans
  `languages-enabled`/`languages-profiles` ; un POST pendant une recherche Bazarr répond 504 et **n'écrit pas**
  `config.yaml` (relire le fichier, ou `app-bazarr restart` puis reposter). `config/host` d'un Arr contient le hash du mot
  de passe et la clé : sauvegarde en 600. Retour : `backups/lot1-20261007/arrs/ROLLBACK.sh`.

## 2. Redémarrage de l'hôte de la seedbox

- Un redémarrage de l'hôte partagé laisse Sonarr, Radarr, Bazarr, Jackett, FlareSolverr, autobrr et unpackerr
  **arrêtés** (qBittorrent et le montage repartent). Relance manuelle :
  `ssh seedbox 'app-sonarr start; app-radarr start; app-bazarr start; app-jackett start; app-flaresolverr start; app-autobrr start; app-unpackerr start'`.
- **Automatisé** (02/10) : crontab de la seedbox → `~/.local/bin/homelab-apps-watch.sh` (source versionnée
  `scripts/seedbox/homelab-apps-watch.sh`, `@reboot` + toutes les 5 min ; crontab d'avant dans
  `~/.local/state/crontab-avant-20261002.txt`). Côté VPS, `seedbox_health` alerte après 10 min.
- `seedbox_refresh`, `monitor_sync`, `russian_search`, `deletion_cleanup` et `torrent_import` **sautent** le côté
  injoignable (« arr unreachable: side skipped this run », passage réussi) : `seedbox_health` reste la seule alerte.
  `monitor_sync` : tant qu'un Sonarr est muet, l'autre ne suit aucune nouvelle saison (il peut en retirer).

## 3. Espace

- **Espace = le quota du compte** (`quota -s` sur la seedbox : 3,7 To), pas le `df` du disque partagé. Le quota est aussi
  lu par homelabd (`quota.json` écrit toutes les 15 min, alerte à `[tasks.seedbox_health] quota_alert_pct`, 85 %).
- Les corbeilles des Arrs (`media/Movies/.recycle`, `media/TV Shows/.recycle`, purge 14 j) peuvent contenir des fichiers
  **partagés par inode avec la médiathèque** : toujours mesurer « exclusif / partagé » avant d'annoncer un gain.

## 4. Montage rclone

- rclone monte `~/media` dans **`/mnt/seedbox/media`** ; Jellyfin lie le **parent** `/mnt/seedbox` (rslave) et voit
  `/seedbox/media`. Lier le point de montage FUSE lui-même casse la reprise après coupure.
- Réglages (source de vérité : `systemd/homelab-seedbox-mount.service`) : cache VFS `full` de **120G** avec
  `--vfs-cache-min-free-space 80G` (rclone évince seul avant le seuil `hard_pct` de `disk_pressure`),
  `--vfs-cache-max-age 168h`, `--vfs-read-chunk-size 4M` (un bloc est livré entier : 8M = 0,67 s au premier Mio),
  `--vfs-read-chunk-streams 4`, `--sftp-connections 32`, API RC sur `127.0.0.1:5572` sans authentification (jamais hors
  boucle locale). **Pas de `--vfs-read-ahead`** tant que ce n'est pas mesuré (palier B : le refus historique visait le
  doublement de bloc du mode `streams = 0`).
- **Hôte et compte (depuis le 09/10)** : hors du dépôt public, `SEEDBOX_SFTP_HOST` et `SEEDBOX_USER` dans `.env`. À chaque
  démarrage, `scripts/seedbox-rclone-conf.sh` (ExecStartPre) écrit `/run/homelab-seedbox-mount/rclone.conf` (dossier
  700, fichier 600) depuis le modèle `rclone/rclone.conf`, qui contient `host = seedbox.invalid` et n'est jamais lu tel
  quel. Commande à la main (montage en marche, en root) : `rclone --config /run/homelab-seedbox-mount/rclone.conf lsd
  seedbox:` ; en `deploy`, générer une copie 600 : `scripts/seedbox-rclone-conf.sh rclone/rclone.conf <fichier>`.
- **Nom du cache VFS** : rclone ajoute au nom du remote un suffixe, hachage des options de backend passées en ligne de
  commande ou par variable (`seedbox{9oylk}` = `--sftp-connections 32` seul, en 1.72.1 comme en 1.75.1). Ce nom est le
  dossier `cache/rclone/vfs/…` et `vfsMeta/…`. Ajouter, retirer ou changer une option `--sftp-*`, ou une variable
  `RCLONE_SFTP_*` / `RCLONE_CONFIG_SEEDBOX_*`, crée un nouveau dossier : les 120 Go sont abandonnés et jamais purgés. Un
  nouveau réglage SFTP va dans le modèle, sans effet sur le nom. Contrôle : `curl -s -X POST 127.0.0.1:5572/vfs/stats |
  jq -r .diskCache.path` doit finir par `seedbox{9oylk}/media`. Ancien cache sans suffixe (`cache/rclone/{vfs,vfsMeta}/seedbox`,
  ~20 Go, abandonné le 19/09 à l'ajout de `--sftp-connections`) : à supprimer par son nom exact, jamais `seedbox*`.
- **Clé rclone** : fichier `~/.ssh/seedbox_sftp_ro` (nom historique), autorisée côté seedbox avec le commentaire
  `homelab-sftp-rd` et `sftp-server -P write,mkdir,rename,…` = **lecture + suppression, aucune écriture** (le bouton
  « Supprimer » de Jellyfin doit pouvoir effacer ; un `open` en création peut laisser un fichier vide). Sauvegarde
  `~/.ssh/authorized_keys.bak-20260915` sur la seedbox.
- **Écriture coincée** : une écriture sur `/mnt/seedbox` réussit en local puis reste dans le cache (`cache/rclone/vfsMeta`,
  rclone réessaie toutes les 5 min, « permission denied ») : arrêter le montage, retirer les entrées (vfsMeta + vfs),
  relancer.
- **Nouveau dossier invisible** : le montage ne voit pas un nouveau dossier tant que le **dossier parent** n'a pas été
  rafraîchi (`vfs/refresh dir=Movies` puis `dir=Movies/<titre>`).
- **Fichier fantôme = Jellyfin bloqué** : un fichier supprimé sur la seedbox mais resté dans le cache de répertoires ;
  le `ffprobe` de Jellyfin qui l'ouvre ne rend jamais la main (SFTP bloqué, état D, `kill -9` sans effet) et plus aucun
  titre nouveau n'apparaît. Diagnostic : `ps -eo pid,stat,etime,args | grep jellyfin-ffmpeg/ffprobe` (état `D`, heures) et
  `rc core/stats` (transfert à 0). Remède : `vfs/forget` puis redémarrer `homelab-seedbox-mount`. **Automatisé** :
  `scripts/seedbox-mount-watch.sh` (minuteur `seedbox-mount-watch`, 5 min) : ffprobe > 15 min → forget ; toujours
  bloqué → redémarrage du montage s'il n'y a pas de lecture seedbox, d'office après 60 min ; alerte Discord admin ;
  `--check`. Après une suppression faite à la main sur la seedbox : `vfs/forget` + `vfs/refresh` récursif aussitôt.
- **Lien** (mesuré le 20/09) : le VPS reçoit ~8–10 Mo/s **par connexion** quelle que soit la source (RTT seedbox 97 ms),
  l'agrégat monte avec le nombre de flux (4 ssh ≈ 30 Mo/s). Une lecture = un flux ≈ 65 Mbit/s : assez, sans marge pour
  un à-coup. Les « 500 Mbit/s » du 18/09 étaient l'agrégat de nombreuses connexions (trickplay). L'hôte de la seedbox
  est partagé (charge 45–60, 128 cœurs).
- **Montage absent ou vide** : l'analyse Jellyfin écrit « Library folder … is inaccessible or empty, skipping » et ne
  supprime rien (vérifié en 10.11.8 sur une instance jetable) ; les titres reviennent au retour du montage.

## 5. Jellyfin et la seedbox

- « Films » et « Séries » (et « Anime », « Films d'animation ») ont chacune deux dossiers (`/media/…` et
  `/seedbox/media/…`) ; plus de bibliothèques « (Seedbox) ». Les nouvelles demandes vont aux Arrs de la seedbox
  (Jellyseerr id 1).
- Un import seedbox est signalé à rclone puis à Jellyfin par `seedbox_refresh`. Un titre **nouveau** (série nouvelle,
  saison nouvelle, nouveaux épisodes dans une saison seedbox, déplacement entre bibliothèques) n'apparaît qu'après une
  analyse complète de la médiathèque (`POST /Library/Refresh`, ~7 min) ; ne pas en lancer une par-dessus une autre
  (voir [jellyfin-serveur-et-extensions.md](jellyfin-serveur-et-extensions.md#3-bibliothèques-et-analyses)).

## 6. Déménager un titre du VPS vers la seedbox

- `scripts/move-to-seedbox.py` (`--list` numérote, `--titles a-b`, `--worker i/n`, `--dry-run`). Ordre immuable : rsync
  (`-a --partial`, ssh admin, `nice`/`ionice`) → vérification nom + taille de chaque fichier → fiche de l'Arr seedbox
  **créée non surveillée** (ou fiche existante) → `RescanSeries`/`RescanMovie`, contrôle du nombre de fichiers, puis
  surveillance de ce qui a un fichier → **seulement alors** suppression côté VPS (fiche + fichiers, torrents liés s'ils ont
  fini de partager depuis 7 j) → `Library/Media/Updated` Deleted/Created.
- Pièges : rsync ≥ 3.2.4 protège lui-même le chemin distant (**pas de guillemets** : `seedbox:/home/x/y z`, sinon
  `mkdir ".../'/home/…'"`) ; parent à
  rafraîchir dans rclone (§ 4) ; `deletion_cleanup` dans `tasks.disabled` pendant toute l'opération (une fiche seedbox
  fraîche dont le montage ne voit pas encore les fichiers serait « sans fichier ») ; jamais pendant une lecture du titre
  (`/Sessions`). Jellyfin recrée l'élément (nouvel id) : l'état « vu » suit les identifiants TMDB/TVDB. Deux workers en
  parallèle ont mis un Sonarr seedbox en « database is locked » (500) : le script réessaie.
- **Hors pic seulement, un flux, `--bwlimit 15000`** : deux rsync à 20 Mo/s ont empêché un membre de lire un film de la
  seedbox (20/09). Le transfert reprend sur ce qui reste (`state-0.json`). L'ancien minuteur `move-to-seedbox-offpeak`
  n'existe plus : programmer par `tools/offpeak/offpeak.sh` ([outils-bancs-et-hors-pic.md](outils-bancs-et-hors-pic.md)).

## 7. Supprimer, faire le ménage, remplacer

- **Supprimer des titres pour de vrai** (modèle `backups/seedbox-cleanup-20260925/delete.py`, `--dry-run` d'abord, fiches
  sauvegardées) : fiche Radarr/Sonarr **avec** fichiers, torrents liés (par inode, sauf délai C411), **dossier du titre
  dans la corbeille de l'Arr** (sinon rien n'est libéré avant 14 j), fiche média Jellyseerr, puis `vfs/refresh` et
  `Library/Media/Updated`. « Jamais regardé » = ni `PlaybackActivity` ni `Played`/`IsResumable` d'aucun compte ; la date
  d'ajout de Jellyfin n'est pas fiable (éléments recréés par les déménagements) : prendre `added` de l'Arr. Le rapport
  `catalogue_report` de `/status.html` donne la liste (lecture seule, l'admin tranche).
- **Ménage des torrents sans catégorie** (`scripts/seedbox-cleanup.py`) : repérés par inode ; un torrent dont **aucun**
  fichier n'est relié à `media/` (hors `.recycle`) est retiré avec ses fichiers, un torrent partiellement relié est laissé,
  jamais un torrent de catégorie `sonarr`/`radarr`. Règle C411 : fini depuis moins de 7 j et ratio < 1 = **reporté**
  (`deferred.json`, vide depuis le 27/09 ; l'ancien minuteur `seedbox-cleanup-deferred` n'existe plus).
- **L'hébergeur arrête les torrents « publics »** : toutes les 5 min, `~/.config/.stop_pub/qbittorrent/qbt_pub.py` lit le
  drapeau `private` du `.torrent` et, s'il manque, bride l'envoi et arrête le torrent une fois terminé (« Torrent
  stopped »). Concerne RuTracker, World-torrent, Nyaa. **Ne pas contourner** (conditions d'utilisation) ; C411 est privé.

### Remplacer un titre par une version plus légère

- `torrent_import` examine **tout** torrent complet de la seedbox : tant que l'ancien fichier est là, il note le nouveau
  « rien à importer » **pour de bon** (et a déjà créé une fiche Radarr fausse pour un nom ambigu). Donc : télécharger,
  supprimer l'ancien fichier par l'Arr (fiche gardée), `vfs/forget` + `vfs/refresh` récursif **aussitôt** (sinon
  Jellyfin ouvre l'ancien fichier et se bloque, voir § 4), puis importer soi-même en `ManualImport` copy — aperçu
  `manualimport?folder=` **sans** id de fiche, épisodes « Unknown Series » lus par `SxxEyy`, packs « `04. Titre.mkv` »
  d'une seule saison lus par le numéro en tête.
- Chercher par identifiant **sans saison** (les « INTEGRALE » n'apparaissent pas par saison) ; écarter HDR/DV
  (transcodage sans GPU) ; ne remplacer un MULTi que par un MULTi (séries comprises) ; écarter les fichiers incomplets.
- Avant de remplacer un fichier au même chemin : supprimer ses segments Intro Skipper « User »
  ([jellyfin-serveur-et-extensions.md](jellyfin-serveur-et-extensions.md#intro-skipper)).
- Après coup, Jellyfin : `Library/Media/Updated` ne suffit pas pour une grosse série dont tous les noms changent : `POST
  /Items/<id série>/Refresh?Recursive=true` par série, puis comparer le nombre d'épisodes Jellyfin à `episodeFileCount` ;
  série, saison ou épisodes **nouveaux** : seulement par l'analyse complète. Remettre les nouveaux éléments dans leurs
  collections ([jellyfin-serveur-et-extensions.md](jellyfin-serveur-et-extensions.md#5-collections-après-un-remplacement-de-fichier)).
- **REPACK ou PROPER importée par l'Arr** (le fichier change de nom, ex. « … Proper.mkv ») : `ManualImport` **copy** de
  l'élément de file (`manualimport?downloadId=` **sans** `seriesId`, candidat par chemin exact ; l'Arr met l'ancien
  fichier à la corbeille, raison `Upgrade`), puis `vfs/forget` + `vfs/refresh` du **dossier**, puis
  `Library/Media/Updated` du **dossier** seulement (`Modified`). **Jamais l'ancien chemin en `Deleted`** : Jellyfin le
  rouvre et rclone le refait apparaître depuis son cache VFS (fantôme vu le 09/10, Carrie S01E02) ; si c'est arrivé,
  refaire `vfs/forget` + `vfs/refresh` du dossier. Jellyfin rattache d'abord le nouveau fichier comme 2e version de
  l'épisode, puis ne garde que lui (~2 min).
- Outils : `backups/codec-replace-20260927/` (version à jour : `verify.py`, `grab.py`, `replace.py`, `--dry-run` d'abord ;
  1er lot dans `backups/codec-replace-20260926/`).

### Série introuvable sur C411 mais publiée en vidéo

`yt-dlp` autonome dans `~/bin` de la seedbox, URL **`/embed/video/<id>`** (la page normale ne propose que 480p),
`-f hls-720` réessayé, remux MKV `-c copy` (audio marqué dans sa langue), puis `ManualImport` en **`move`** (pas de
torrent). Scripts : `backups/kukhnya-20260926/` (`dl.py` sur la seedbox, `import.py` depuis le VPS, épisodes terminés
seulement).
