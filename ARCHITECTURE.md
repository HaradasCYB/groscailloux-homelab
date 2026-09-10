# Architecture

Un hôte, un réseau bridge `homelab` (172.18.0.0/16), tout en bind mounts sous `/opt/homelab`.
Les services s'adressent par nom de conteneur (`http://radarr:7878`) ; depuis l'hôte
(homelabd) par `localhost:<port hôte>`.

## Services

| Service | Rôle | État |
|---|---|---|
| jellyfin | lecture, transcodage logiciel (x264 ; éviter HEVC) | `jellyfin/config`, cache tmpfs |
| jellyseerr (seerr) | demandes utilisateurs → Sonarr/Radarr | `jellyseerr/config` |
| sonarr / radarr | séries / films : recherche, import, renommage | `*/config`, `library:/data` |
| prowlarr | gestion d'indexers ; **aucune application synchronisée** aujourd'hui, les indexers sont configurés directement dans les Arrs | `prowlarr/config` |
| gluetun | WireGuard ProtonVPN, port forwarding NAT-PMP, netns de qbittorrent | `gluetun/` |
| qbittorrent | client torrent dans le netns gluetun (`network_mode: service:gluetun`) | `qbittorrent/config` |
| qbittorrent-direct | même client sans VPN (profil `novpn`) | idem |
| pyload | téléchargements directs | `pyload/config` |
| filebrowser | explorateur web de `library/` | `filebrowser/` |
| npm | reverse proxy TLS (Let's Encrypt), entrée publique 80/443 | `npm/data`, `npm/letsencrypt` (root) |
| duckdns | DNS dynamique | — |
| guacamole / guacd / guacdb | bureau distant navigateur, MySQL 8.0 | `guacamole/mysql` |
| homarr | dashboard (seul utilisateur de `SECRET_ENCRYPTION_KEY`) | `homarr/` (root) |
| portainer | UI Docker | `portainer/` |
| influxdb / telegraf / grafana | métriques hôte + conteneurs, rétention 30 j | `influxdb/`, `grafana/` (uid 472) |
| glances | monitoring live | — |
| diun | notification mail des nouveaux tags d'images (`diun/images.yml`) | `diun/` |
| onboarder | **obsolète** : UI Flask remplacée par homelabd:8766, retiré à la bascule | — |

Trois conteneurs montent `docker.sock` en lecture (homarr, portainer, glances/telegraf) ;
glances tourne `privileged`. Toutes les images sont pinnées `tag@sha256`.

## Flux

**Acquisition → lecture.** Jellyseerr → Sonarr/Radarr → indexers → qBittorrent (via gluetun)
ou pyLoad. Tout descend dans `library/downloads` (`/downloads` dans les conteneurs). Sonarr et
Radarr importent par hardlink vers `library/media/{tvshows,movies}` (`/data/media/...` chez
eux, `/media` en lecture seule chez Jellyfin). Le hardlink permet de supprimer un torrent et ses
fichiers sans toucher à la bibliothèque.

**Dépôts directs.** `homelabd` surveille `library/downloads` (inotify) : nouvelle vidéo →
classification série/film → parse + lookup Arr → ajout si absent → `DownloadedEpisodesScan` /
`DownloadedMoviesScan`. Archives zip/rar extraites puis supprimées.

**Garde-fous qBittorrent** (mutex partagé dans homelabd) : share limits par tracker, remplacement
des téléchargements bloqués > 8 h, suppression des torrents arrêtés les plus anciens quand le
disque dépasse 95 %.

**Utilisateurs.** Un compte = Jellyfin (mot de passe) + import Jellyseerr + mail. Trois
entrées : `homelabctl onboard`, l'UI web homelabd (`/onboard`, token), et le poller qui
convertit les users créés dans l'UI Jellyseerr.

**Observabilité.** Telegraf (hôte via `/hostfs`, Docker via socket) → InfluxDB bucket
`metrics` (30 j) → Grafana.

## Réseau et exposition

NPM termine TLS pour `<service>.<domaine>.duckdns.org` et proxifie vers les conteneurs ; pour
l'UI homelabd, vers la passerelle `172.18.0.1:8766`. qBittorrent n'est joignable que par
gluetun (8080/6881 publiés sur le conteneur gluetun). Le hook `hooks/qbit-update-port.sh`
(monté `/gluetun/scripts`) reçoit le port forwardé et l'applique via l'API WebUI locale.

## Contraintes d'exploitation

- Profil Sonarr `6` : `minFormatScore=-9999`, FR préféré, VO/VOSTFR acceptés en dernier recours.
- Indexer C411 en recherche interactive uniquement dans Sonarr (pas de RSS/auto).
- Jamais de purge globale de queue : chaque suppression est ciblée par id/titre.
- Arrêter qBittorrent avant d'éditer `qBittorrent.conf` (sinon il l'écrase à l'arrêt).
- Pas d'accélération matérielle : préférer x264 à x265 pour les releases FR/MULTi.
- Ne jamais `chown -R /opt/homelab` : npm, homarr (root), grafana (472), mysql (999).
- `SECRET_ENCRYPTION_KEY` ne sert qu'à Homarr.
