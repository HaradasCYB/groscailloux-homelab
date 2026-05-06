# ARCHITECTURE.md

Stack monolithique : 23 services Docker partagent un réseau `homelab` (bridge), bind-mount sur `/opt/homelab/<service>/` pour leur état, et `/opt/homelab/{downloads,media}/` pour les données partagées.

## Vue d'ensemble

```
                                                    ┌──────────────────────┐
                                                    │  Internet (peers,    │
                                                    │  trackers, indexers) │
                                                    └──────┬───────────────┘
                                                           │
                                                  ┌────────▼────────┐
                                                  │   gluetun       │
                                                  │  (ProtonVPN +   │
                                                  │   NAT-PMP PF)   │
                                                  └────────┬────────┘
                                                           │ network_mode: service:gluetun
                                                  ┌────────▼────────┐
┌──────────────┐    ┌──────────────┐              │  qBittorrent    │◄─── peers
│  Jellyseerr  │───▶│  Sonarr/     │─grab via──▶  │   (port 8080)   │     entrants via
│  (requests)  │    │  Radarr      │              │                 │     port forwardé
└──────────────┘    └──────┬───────┘              └─────────────────┘
                           │ search via
                    ┌──────▼───────┐
                    │  Prowlarr +  │
                    │   Jackett    │
                    └──────────────┘
                           │
                    ┌──────▼───────┐
                    │ FlareSolverr │ (bypass Cloudflare)
                    └──────────────┘
```

## Pipeline 1 — Acquisition

**Acteurs** : Jellyseerr → Sonarr/Radarr → Prowlarr → qBittorrent → import

```
1. User demande un media via Jellyseerr UI
2. Jellyseerr POST → Sonarr/Radarr (selon type)
3. Sonarr/Radarr cherchent sur les indexers via Prowlarr/Jackett
4. Meilleure release sélectionnée selon Quality Profile + Custom Formats
5. .torrent envoyé à qBittorrent (catégorie sonarr/radarr)
6. qBittorrent télécharge via le tunnel VPN (gluetun)
7. À 100%, Sonarr/Radarr "Completed Download Handling" :
   - Hardlink vers /movies/<Title (Year)>/<file>.mkv
   - Marque hasFile=true
   - Le torrent reste dans qBit (removeCompletedDownloads: false) → seed continu
```

**Garde-fous** :
- `qbit-stuck-handler` (timer 5min) : si stalled > 8h → blocklist + re-search
- `qbit-tracker-ratio-policy` (timer 30min) : applique limits per-tracker
- `qbit-disk-pressure-handler` (timer 15min) : pause/delete si disque > 90%/95%

## Pipeline 2 — Drop manuel (pyload / DDL)

**Acteurs** : pyload → /downloads → auto-import.sh → Sonarr/Radarr

```
1. Drop fichier dans /opt/homelab/downloads (via pyload, scp, etc.)
2. inotifywait détecte le nouveau fichier
3. auto-import.sh :
   - Parse nom via Sonarr/Radarr /api/v3/parse → titre + année
   - Lookup TMDB/TVDB → tmdbId/tvdbId
   - Si pas dans library : POST /api/v3/movie ou /series (searchForXxx: false)
   - Trigger DownloadedXxxScan → hardlink vers /media
4. Jellyfin scanne /media → media disponible
```

**Garde-fous** :
- `tba-import-bypass` (timer 5min) : bypass titre TBA Sonarr pour anime récents
- `jellyseerr-sonarr-monitor-sync` (timer 10min) : aligne saisons monitored avec demandes

## Pipeline 3 — Streaming

**Acteurs** : Jellyfin → /media (read-only) → users

```
1. User accède à Jellyfin via URL publique (NPM TLS)
2. Auth Jellyfin (creds locaux ou OIDC)
3. Stream via HLS/Direct Play
4. (Si transcoding nécessaire) ffmpeg → /cache/transcodes (tmpfs 2 GB)
```

**Auth users** : Jellyfin = source de vérité du mdp. Jellyseerr délègue son auth à Jellyfin via "Use your Jellyfin account" tab → un seul mdp pour les 2 services.

## Pipeline 4 — Observabilité

**Acteurs** : Telegraf → InfluxDB → Grafana (+ Glances + Dashdot pour vues légères)

```
Telegraf (collecteur, host metrics + Docker metrics)
   │
   ▼
InfluxDB v2 (org=homelab, bucket=metrics)
   │
   ▼
Grafana (dashboards)
```

Dashdot et Glances offrent des vues web rapides sans Grafana (temp CPU, disk, RAM, net).

## Pipeline 5 — Edge / Access

**Acteurs** : DuckDNS → NPM → services

```
DuckDNS (DNS dynamique, gratuit)
  → <user>.duckdns.org → IP host
NPM (Nginx Proxy Manager, port 80/81/443)
  → reverse proxy + TLS Let's Encrypt
  → routes vers services internes (jellyfin:8096, jellyseerr:5055, ...)
```

Guacamole (port 8081) : accès distant RDP/SSH via navigateur.

Filebrowser : navigation /downloads et /media via web.

Homarr : dashboard tuilage de tous les services (port 7575).

Portainer : UI Docker (port 9000).

## Hardlinks — pas de double stockage

Critique pour le seeding : `/downloads` et `/media` sont sur le **même filesystem** → Sonarr/Radarr utilisent `copyUsingHardlinks: true`. Un fichier physique = 1 inode = 2 paths visibles :
- `/downloads/<release>.mkv` → qBit voit ça pour seeder
- `/media/movies/<Title (Year)>/<Title> WEBDL-2160p.mkv` → Jellyfin voit ça

Coût stockage : compté **une seule fois** sur disque.

Si le disk-pressure handler delete le fichier dans `/downloads` (avec `deleteFiles=true` sur qBit), le hardlink dans `/media` survit (link count → 1, mais data préservée). Seeding stoppé pour ce torrent, mais lecture Jellyfin OK.

## Onboarding utilisateur — "Jellyfin = source unique"

Avant : créer un user demandait 2 actions (Jellyseerr + Jellyfin) → 2 mots de passe → désynchronisation possible.

Maintenant : `homelab-onboard-user.sh` crée :
1. User Jellyfin avec un mdp aléatoire (sole source of truth)
2. Import dans Jellyseerr en `userType=2` (Jellyfin-imported, **aucun mdp local**)
3. Mail unifié envoyé via SMTP

L'utilisateur a un seul couple username/password pour streaming + requêtes. Si l'admin clique "Add User" via Jellyseerr UI par habitude → `jellyseerr-user-poller` (timer 60s) détecte → cleanup → relance le script unifié → l'utilisateur reçoit le bon mail.

## Filesystem layout

```
/opt/homelab/
├── docker-compose.yml
├── .env                          (secrets, jamais committé)
├── scripts/                      (helpers + systemd units)
├── downloads/                    (qBit + pyload write here)
├── media/
│   ├── movies/                   (Radarr import dest)
│   └── tvshows/                  (Sonarr import dest)
├── logs/                         (scripts logs)
├── jellyfin/{config,cache}/
├── jellyseerr/config/
├── radarr/config/
├── sonarr/config/
├── prowlarr/config/
├── jackett/config/
├── qbittorrent/config/
├── pyload/config/
├── filebrowser/{database,config}/
├── homarr/                       (dashboard data)
├── grafana/                      (Grafana data)
├── influxdb/{data,config}/
├── npm/{data,letsencrypt}/
├── portainer/
├── gluetun/                      (VPN state, forwarded port)
├── telegraf/etc/telegraf.conf
└── guacamole/mysql/              (Guacamole DB)
```

État services = bind-mounts. **Pas de named volumes Docker**.
