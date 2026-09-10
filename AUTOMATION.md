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

## Watcher auto_import (continu)

inotify non récursif sur `paths.downloads` (create, close_write, moved_to), chaque nom traité
au plus une fois par 2 min. Vidéo (`mkv/mp4/avi`, hors `.!qB`/`.part`) : attente 5 s, puis
classification : `S01E02`, `1x02`, `Season`, `Saison`, `Complete` ⇒ série, sinon film.
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
Le mot de passe (16 caractères alphanumériques) n'apparaît jamais dans les logs.

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
