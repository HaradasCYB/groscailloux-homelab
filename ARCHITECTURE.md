# Architecture

> Schémas (physique, parcours d'une demande, stockage) : [docs/INFRA.md](docs/INFRA.md).

Un hôte, un réseau bridge `homelab` (172.18.0.0/16), tout en bind mounts sous `/opt/homelab`.
Les services s'adressent par nom de conteneur (`http://radarr:7878`) ; depuis l'hôte
(homelabd) par `localhost:<port hôte>`.

## Services

| Service | Rôle | État |
|---|---|---|
| jellyfin | lecture, transcodage logiciel (x264 ; éviter HEVC) | `jellyfin/config`, cache tmpfs |
| jellyseerr (seerr) | demandes utilisateurs → Sonarr/Radarr | `jellyseerr/config` |
| sonarr / radarr | séries / films : recherche, import, renommage | `*/config`, `library:/data` |
| jackett | **source des indexers publics** de Sonarr/Radarr (URLs `http://jackett:9117/api/v2.0/indexers/<id>/results/torznab/`) ; ne pas retirer tant que les Arrs pointent dessus | `jackett/config` |
| flaresolverr | résolution des défis Cloudflare pour Jackett (1337x, eztv) ; pas de port publié | — |
| prowlarr | gestion d'indexers ; **aucune application synchronisée** aujourd'hui, les indexers publics passent par Jackett et C411 par son API Torznab directement dans les Arrs | `prowlarr/config` |
| gluetun | WireGuard ProtonVPN, port forwarding NAT-PMP, netns de qbittorrent | `gluetun/` |
| qbittorrent | client torrent dans le netns gluetun (`network_mode: service:gluetun`) | `qbittorrent/config` |
| qbittorrent-direct | même client sans VPN (profil `novpn`) | idem |
| pyload | téléchargements directs | `pyload/config` |
| filebrowser | explorateur web de `library/` | `filebrowser/` |
| npm | reverse proxy TLS (Let's Encrypt), entrée publique 80/443 | `npm/data`, `npm/letsencrypt` (root) |
| duckdns | DNS dynamique | — |
| guacamole / guacd / guacdb | bureau distant navigateur, MySQL 8.0 | `guacamole/mysql` |
| homarr | deux tableaux : **« Groscailloux-TV »** public (accueil : Regarder, Demander, nouveautés, à venir, demandes) et **« Operations »** privé (admin : automatisation homelabd, ressources, lectures, téléchargements VPS + seedbox, conteneurs, mises à jour d'images, Grafana partagé, outils) ; dispositions mobile / tablette / bureau ; français, sombre ; intégrations (VPS + seedbox) dont les secrets sont chiffrés en base avec `SECRET_ENCRYPTION_KEY` (AES-256-CBC) — modifier la base Homarr arrêté, après sauvegarde | `homarr/` (root) |
| portainer | UI Docker | `portainer/` |
| influxdb / telegraf / grafana | métriques hôte + conteneurs, rétention 30 j | `influxdb/`, `grafana/` (uid 472) |
| glances | monitoring live | — |
| diun | notification mail des nouveaux tags d'images (`diun/images.yml`) | `diun/` |
| homelabd (hôte, pas un conteneur) | automatisation + UI d'onboarding sur `:8766` | `state/` |

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

**Seedbox (acquisition + stockage déportés).** Une seedbox partagée (`tofino.usbx.me`, Toronto,
3,7 To, outillage Ultra.cc `app-*`) héberge qBittorrent, Radarr, Sonarr, Jackett (+ FlareSolverr),
Bazarr, Unpackerr et autobrr. Les Arrs y rangent par hardlink dans `~/media/{Movies,TV Shows}`.
Jellyseerr envoie les **nouvelles demandes** à ces Arrs (serveurs par défaut, id 1) ; ceux du VPS
(id 0) gardent la bibliothèque existante. Le VPS monte `~/media` en **lecture seule** (rclone SFTP,
clé restreinte à `sftp-server -R`) sur `/mnt/seedbox/media` ; Jellyfin le voit sous
`/seedbox/media` : `/seedbox/media/Movies` et `/seedbox/media/TV Shows` sont un **second dossier**
des bibliothèques « Films » et « Séries » (une seule bibliothèque par type pour les utilisateurs).
`seedbox_refresh` (homelabd) signale chaque nouvel import à rclone puis à Jellyfin.
Si la seedbox tombe : les titres venant de la seedbox restent affichés mais ne se lisent plus ;
Jellyfin ne les purge pas (« Library folder … is inaccessible or empty, skipping », vérifié en
10.11.8 sur scan de bibliothèque et scan global) ; le reste de Jellyfin et le pipeline VPS ne sont
pas affectés.
Lien mesuré : RTT 96 ms, ~12 Mo/s par lecture (1080p et 4K WEB OK, remux 4K limite) ; pas de
transcodage 4K HEVC (VPS sans GPU). Sur la seedbox, les apps tournent en conteneurs Docker et
joignent qBittorrent (natif, `127.0.0.1` seulement) via `https://kakaouette.tofino.usbx.me/qbittorrent`
et Jackett/FlareSolverr via `172.17.0.1:<port>`.

**Lecture fluide.** 98 % des lectures sont en lecture directe (Playback Reporting, 30 j) : le
buffering vient de l'acheminement, pas du transcodage. Donc :
- rclone : `chunk_size = 255k` (SFTP ; 5 → 16 Mo/s par flux, 22–24 Mo/s via le montage), blocs de
  lecture de 8 Mo (reprise après un saut : 20 Mo en ~3 s au lieu de 4), cache VFS 20 Go, **pas** de `--vfs-read-ahead` (il fait télécharger 256 Mo à chaque ouverture, analyses comprises) ;
- **rien ne lit la vidéo à l'ajout d'un titre** : Intro Skipper `AutoDetectIntros=false`, pas de
  « Screen Grabber » dans les sources d'images, `SaveLocalMetadata=false` (pas de NFO ni d'images à
  côté des médias, la seedbox est en lecture seule) ; normalisation audio (LUFS) à 07:00 ;
- Jellyfin : trickplay en images clés seulement et **jamais pendant un scan** ; tâches lourdes
  (scan 05:00, segments 05:15, trickplay 05:30 max 6 h, Intro Skipper 06:00 max 3 h) dans la fenêtre
  sans lecture 05–13 h ; `cpu_shares` 2048 pour jellyfin, 512 pour les tâches de fond ;
- Jellyseerr `availability-sync` quotidien (05:00) ; Telegraf toutes les 30 s.

**Santé du stack.** `stack_health` (homelabd) relance les services arrêtés, redémarre les
`unhealthy` et ceux dont la sonde applicative échoue (Guacamole : login test). Guacamole est en
`restart: "no"` : seul compose le lance, après `guacdb` healthy.

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

## Sécurité des accès web

- NPM : liste d'accès **« admin-outils »** (authentification HTTP, utilisateur `groscailloux`) devant
  Sonarr, Radarr, qBittorrent, Prowlarr, Jackett, Grafana, Portainer, pyLoad, Guacamole (y compris le
  `location /guacamole/` de sa config avancée). Restent directs : Jellyfin, Jellyseerr, Homarr,
  Filebrowser (sa propre connexion), onboarding (jeton). Grafana : `/public-dashboards/`,
  `/api/public/`, `/public/` (statiques) et `/apis/` (authentifié par Grafana) sont exemptés, pour le
  tableau partagé intégré dans Homarr.
- qBittorrent (VPS) : dispense d'authentification limitée à `127.0.0.0/8` et `172.18.0.1/32`
  (homelabd depuis l'hôte). **Jamais `172.18.0.0/16`** : NPM est dans ce réseau, qBittorrent était
  ouvert à Internet sans mot de passe jusqu'au 2026-09-12.
- homelabd : `/status` et `/status.html` exigent `HOMELABD_STATUS_TOKEN` (le sous-domaine d'onboarding est public).
- Page « Comptes » (`/accounts` de l'hôte onboarding) : liste « admin-outils » dans la config avancée
  NPM (`location /accounts`) **et** `HOMELABD_ONBOARD_TOKEN` exigé par homelabd (fermée s'il n'est pas défini).

## Contraintes d'exploitation

- **C411 seul en automatique** sur les 4 Arrs (RSS + recherche auto) ; tous les autres indexers en
  recherche interactive seulement (toujours proposés quand on cherche à la main).
- Profils « FR-friendly H.264 » (VPS 6, seedbox 7) : **1080p au plus, jamais de 2160p** (CPU du VPS) ;
  VFF > MULTi > FRENCH, H.264 préféré ; VO/VOSTFR en dernier recours (`No French Marker` -2000,
  `minFormatScore` -9999) ; rejets durs (langues étrangères, CAM/TS, sample, 3D) à -100000.
- Jamais de purge globale de queue : chaque suppression est ciblée par id/titre.
- Arrêter qBittorrent avant d'éditer `qBittorrent.conf` (sinon il l'écrase à l'arrêt).
- Pas d'accélération matérielle : préférer x264 à x265 pour les releases FR/MULTi.
- Ne jamais `chown -R /opt/homelab` : npm, homarr (root), grafana (472), mysql (999).
- `SECRET_ENCRYPTION_KEY` ne sert qu'à Homarr.
