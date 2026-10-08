# Architecture

> Schémas (physique, parcours d'une demande, stockage) : [INFRA.md](INFRA.md).

Un hôte, un réseau bridge `homelab` (172.18.0.0/16), tout en bind mounts sous `/opt/homelab`.
Les services s'adressent par nom de conteneur (`http://radarr:7878`) ; depuis l'hôte
(homelabd) par `localhost:<port hôte>`.

## Services

| Service | Rôle | État |
|---|---|---|
| jellyfin | lecture, transcodage logiciel (un seul 1080p à la fois ; HEVC et H.264 coûtent pareil) | `jellyfin/config`, tmpfs de transcodage 4 Go |
| jellyseerr (seerr) | demandes utilisateurs → Sonarr/Radarr | `jellyseerr/config` |
| sonarr / radarr | séries / films : recherche, import, renommage | `*/config`, `library:/data` |
| prowlarr | **C411 seul**, sans application liée : sert aux recherches de homelabd (`series_search`, `movie_search`, `/recherche`) ; les indexers des Arrs sont déclarés directement (C411 en RSS) | `prowlarr/config` |
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
| homelabd (hôte, pas un conteneur) | automatisation (24 tâches) + pages web sur `:8766` (onboarding, comptes, recherche, état, Mon compte, tchat, premium) | `state/` |

Trois conteneurs montent `docker.sock` en lecture (homarr, portainer, glances/telegraf) ;
glances tourne `privileged`. Toutes les images sont pinnées `tag@sha256`.

## Flux

**Bibliothèque VPS.** Historique : Sonarr et Radarr du VPS importaient par hardlink de `library/downloads` vers
`library/media/{tvshows,movies}` (`/media` chez Jellyfin, en écriture pour le bouton « Supprimer »). Depuis le
18/09, le VPS ne prend plus aucune release (`auto_sides = ["seedbox"]` + RSS coupé) ; il garde ses fiches et ses
fichiers. Le hardlink permet de supprimer un torrent sans toucher à la bibliothèque.

**Dépôts directs.** `homelabd` surveille `library/downloads` (inotify) : nouvelle vidéo →
classification série/film → parse + lookup Arr → ajout si absent → `DownloadedEpisodesScan` /
`DownloadedMoviesScan`. Archives zip/rar extraites puis supprimées.

**Seedbox (tous les téléchargements).** Une seedbox partagée (quota de 3,7 To, outillage Ultra.cc `app-*`)
héberge qBittorrent, Radarr, Sonarr, Bazarr, Unpackerr et autobrr. Les Arrs y rangent par hardlink dans
`~/media/{Movies,TV Shows,Anime,Anime Movies}`. Jellyseerr leur envoie **toutes les demandes** (serveurs par
défaut, id 1, `preventSearch`) ; homelabd cherche les releases (C411 par TMDB) et les leur pousse, ou les ajoute
directement au qBittorrent de la seedbox avec une étiquette `homelab:`. Le VPS monte `~/media` (rclone SFTP, clé
limitée à la lecture et à la suppression) sur `/mnt/seedbox/media` ; Jellyfin le voit sous
`/seedbox/media` : `/seedbox/media/Movies` et `/seedbox/media/TV Shows` sont un **second dossier**
des bibliothèques « Films » et « Séries » (une seule bibliothèque par type pour les utilisateurs).
`seedbox_refresh` (homelabd) signale chaque nouvel import à rclone puis à Jellyfin.
Si la seedbox tombe : les titres venant de la seedbox restent affichés mais ne se lisent plus ;
Jellyfin ne les purge pas (« Library folder … is inaccessible or empty, skipping », vérifié en
10.11.8 sur scan de bibliothèque et scan global) ; le reste de Jellyfin et le pipeline VPS ne sont
pas affectés.
Lien mesuré : RTT ~97 ms, 8 à 10 Mo/s par connexion (~30 Mo/s à quatre). Sur la seedbox, les apps tournent en
conteneurs Docker et joignent qBittorrent (natif, `127.0.0.1` seulement) par le proxy HTTPS de l'hébergeur.

**Lecture fluide.** 65 % des lectures (et des heures) sont en lecture directe, mesuré le 04/10/2026 sur 30 j
(lectures ≥ 60 s, Playback Reporting) : remux 18 % du temps, son seul 7,5 %, vidéo 9,4 % — le Chromecast fait
63 % des conversions vidéo, l'appli iOS remuxe presque tout (MKV) ou convertit le son (DTS). Le buffering vient
surtout de l'acheminement, et le remux passe lui aussi par le lien seedbox. Donc :
- rclone : `chunk_size = 255k` (SFTP), 32 connexions, blocs de lecture de 4 Mo, cache VFS 120 Go avec 80 Go
  d'espace libre gardé (éviction avant le seuil de `disk_pressure`), **pas** de `--vfs-read-ahead` ;
- **rien ne lit la vidéo à l'ajout d'un titre** : Intro Skipper `AutoDetectIntros=false`, pas de
  « Screen Grabber » dans les sources d'images, `SaveLocalMetadata=false` (pas de NFO ni d'images à
  côté des médias) ; normalisation audio (LUFS) à 07:00 ;
- Jellyfin : trickplay **plus généré** (sans déclencheur depuis le 25/09 : tout arrive sur la seedbox) ; tâches
  lourdes (scan 05:00, segments, Intro Skipper 05:30) dans la fenêtre sans lecture 05–13 h ; `cpu_shares` 2048
  pour jellyfin, 512 pour les tâches de fond ; tmpfs de transcodage purgé chaque minute ;
- Jellyseerr `availability-sync` quotidien (05:00) ; Telegraf toutes les 30 s.

**Santé du stack.** `stack_health` (homelabd) relance les services arrêtés, redémarre les
`unhealthy` et ceux dont la sonde applicative échoue (Guacamole : login test). Guacamole est en
`restart: "no"` : seul compose le lance, après `guacdb` healthy.

**Garde-fous qBittorrent** (mutex partagé dans homelabd) : share limits par tracker, remplacement
des téléchargements bloqués > 8 h, suppression des torrents arrêtés les plus anciens quand le
disque dépasse 95 %.

**Utilisateurs.** Un compte = Jellyfin + import Jellyseerr + mail avec un lien à usage unique (`/bienvenue/<jeton>`,
60 min) où le membre choisit son mot de passe, puis les premiers pas. Entrées : `/inscription` (publique, compte à
activer), `homelabctl onboard`, la page « Créer un compte » (`/`, session admin) et le poller qui convertit les
comptes créés dans l'UI Jellyseerr. Procédure : [ONBOARDING.md](ONBOARDING.md).

**Observabilité.** Telegraf (hôte via `/hostfs`, Docker via socket) → InfluxDB bucket
`metrics` (30 j) → Grafana.

## Réseau et exposition

NPM termine TLS pour `<service>.<domaine>.duckdns.org` et proxifie vers les conteneurs ; pour
l'UI homelabd, vers la passerelle `172.18.0.1:8766`. qBittorrent n'est joignable que par
gluetun (8080/6881 publiés sur le conteneur gluetun). Le hook `hooks/qbit-update-port.sh`
(monté `/gluetun/scripts`) reçoit le port forwardé et l'applique via l'API WebUI locale.

## Sécurité des accès web

- NPM : liste d'accès **« admin-outils »** (authentification HTTP, utilisateur `groscailloux`) devant
  Sonarr, Radarr, qBittorrent, Prowlarr, Grafana, Portainer, pyLoad, Guacamole (y compris le
  `location /guacamole/` de sa config avancée). Restent directs : Jellyfin, Jellyseerr, Homarr,
  Filebrowser (sa propre connexion), onboarding (jeton). Grafana : `/public-dashboards/`,
  `/api/public/`, `/public/` (statiques) et `/apis/` (authentifié par Grafana) sont exemptés, pour le
  tableau partagé intégré dans Homarr.
- qBittorrent (VPS) : dispense d'authentification limitée à `127.0.0.0/8` et `172.18.0.1/32`
  (homelabd depuis l'hôte). **Jamais `172.18.0.0/16`** : NPM est dans ce réseau, qBittorrent était
  ouvert à Internet sans mot de passe jusqu'au 2026-09-12.
- homelabd : les pages d'administration (`/`, `/accounts*`, `/recherche*`, `/status*`) passent par une couche
  commune (`crates/homelabd/src/admin_auth.rs`) : session par cookie signé (HMAC du jeton, un an), ouverte par
  `/connexion` ou d'office depuis `HOMELABD_ADMIN_TRUSTED_IPS` (vue par NPM confirmé par Docker seulement) ; le jeton
  ne passe jamais dans une adresse ni dans une page (jeton de formulaire dérivé). Les routes `/admin/*` (CLI) ne
  répondent qu'à un appel local (127.0.0.1, sans `X-Forwarded-For`) avec le jeton en en-tête. Les chemins
  d'administration ne sont servis que sur l'hôte de `ONBOARD_PUBLIC_URL` (404 ailleurs, dont l'hôte premium).
  L'adresse du client (`client_addr.rs`) n'est lue dans `X-Forwarded-For` que si le pair TCP est NPM
  (`[web] trusted_proxies` + `trusted_proxy_container`) : confiance à trois niveaux (ignoré / réseau / confirmé par
  Docker), instantané de l'adresse de NPM tenu par une tâche de fond.
- Page de don : sous-domaine `don.` → homelabd ; la config avancée NPM ne laisse passer que `/don`
  (`/` redirige, tout le reste renvoie 404). Sans lien avec le service (ni Jellyfin, ni Homarr, ni mail).
- Page « Comptes » (`/accounts` de l'hôte onboarding) : liste « admin-outils » dans la config avancée
  NPM (`location /accounts`) **et** session d'administration homelabd.

## homelabd en bref

- `homelab-core` : la logique (clients Jellyfin/Arrs/qBittorrent/Prowlarr/Jellyseerr, tâches, état, abonnements,
  tchat, `html::esc`), testée par des fonctions pures ; `homelabd` : le démon (planificateur, pages web, API
  « Mon compte » `/compte/api/*`, tchat `/chat/*`, webhook PayPal) ; `homelabctl` : la CLI.
- Planificateur : une boucle par tâche, jamais deux passages simultanés d'une même tâche, une panique n'arrête
  qu'un passage ; battement de cœur lu par `/health` (503 après 20 min sans tour de boucle). L'état
  (`state/homelabd.json`) n'est écrit que par le démon : sérialisé en mémoire sous le verrou, puis écrit en UN appel
  hors du verrou par un écrivain unique numéroté (jamais un état plus ancien après un plus récent ; fichier
  temporaire, `fsync`, renommage, `fsync` du dossier ; une sérialisation identique n'est pas réécrite). `update` rend
  la main une fois sur disque ; `update_lazy` (tenue des passages) part avec la sauvegarde suivante, au plus tard au tour de `run_lazy_flusher` (60 s), `flush` à l'arrêt.
  Jusqu'au 07/10, serde_json écrivait directement dans le fichier : ~38 000 `write` de 4 octets par sauvegarde, 66 % du
  CPU du démon dans le noyau. `homelabctl` l'ouvre en lecture seule et passe par `POST /admin/run` et `/admin/accounts`.
- Alertes : `alerts::admin` (mail + Discord admin), trace de livraison dans l'état, règles dans
  [AUTOMATION.md](AUTOMATION.md#alertes-admin-2026-10-07).
- Configuration : `homelab.toml` refuse les clés inconnues ; un test vérifie que les défauts du code sont ceux du
  fichier.

## Contraintes d'exploitation

- **C411 est le seul indexeur**, en **RSS seulement** dans les 4 Arrs (seconde clé) ; aucune recherche depuis
  les Arrs : homelabd cherche par TMDB via Prowlarr, dans un budget horaire par clé. Le VPS ne prend plus aucune
  release (`[downloads] auto_sides = ["seedbox"]`, RSS coupé).
- Profils « FR-friendly H.264 » (VPS 6, seedbox 7) : **1080p au plus, jamais de 2160p** (CPU du VPS) ;
  VFF/VOF > MULTi > FRENCH, codec neutre (HEVC et H.264 à 0) ; VO/VOSTFR en dernier recours (`No French Marker`
  -2000, `minFormatScore` -9999) ; rejets durs (langues étrangères, CAM/TS, sample, 3D) à -100000. Animés : profil
  « Anime - MULTi/VOSTFR » (MULTi > VOSTFR > VF). Toujours viser un profil par son **nom** : les numéros diffèrent.
- Jamais de purge globale de queue : chaque suppression est ciblée par id/titre.
- Arrêter qBittorrent avant d'éditer `qBittorrent.conf` (sinon il l'écrase à l'arrêt).
- Pas d'accélération matérielle : un seul transcodage 1080p tient en temps réel ; le codec source ne change pas
  le coût (c'est l'encodage x264 qui coûte), donc jamais écarter une release parce qu'elle est en x265.
- Ne jamais `chown -R /opt/homelab` : npm, homarr (root), grafana (472), mysql (999).
- `SECRET_ENCRYPTION_KEY` ne sert qu'à Homarr.
