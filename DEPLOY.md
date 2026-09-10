# Déploiement

## Prérequis hôte

- Ubuntu/Debian x86_64, Docker Engine ≥ 24 + plugin compose v2, ~6 vCPU / 16 Go conseillés
  (Jellyfin transcode en logiciel, pas de GPU).
- Utilisateur `deploy` uid/gid 1000, membre du groupe `docker`, sudo.
- Paquets : `curl zstd tar unzip unrar p7zip-full`. Pour compiler : `rustup` + `musl-tools`.
- Un disque unique ext4 : les données vivent en bind mounts sous `/opt/homelab/<service>/`.

## Hôte neuf

```bash
sudo git clone https://github.com/HaradasCYB/groscailloux-homelab.git /opt/homelab
sudo chown -R deploy:deploy /opt/homelab
cd /opt/homelab && cp .env.example .env && chmod 600 .env
nano .env               # tout ce qui peut l'être avant le premier boot (voir SECRETS.md)
sudo ./setup.sh --no-start   # prérequis, dossiers, binaires, unités systemd
docker compose up -d influxdb jellyfin jellyseerr sonarr radarr prowlarr gluetun qbittorrent npm
```

Puis, dans chaque UI, terminer l'installation et récupérer les clés API ; compléter `.env`
(`*_API_KEY`, `JELLYFIN_LIB_*`, `INFLUX_TOKEN`) ; `sudo ./setup.sh` (idempotent : régénère
telegraf.conf, `up -d` du reste, démarre homelabd) ; `homelabctl check`.

Réglages à faire une fois dans les UIs :
- Sonarr/Radarr : root folders `/data/media/tvshows` et `/data/media/movies`, download client
  qBittorrent host `gluetun` port 8080, catégories, profil qualité FR (voir ARCHITECTURE.md).
- Prowlarr : indexers, puis sync vers Sonarr/Radarr (ou indexers directement dans les Arrs).
- qBittorrent : whitelist WebUI `127.0.0.1/8,172.18.0.0/16` (homelabd et les Arrs appellent
  l'API sans auth), chemin `/downloads`, `Session\IPv6Enabled=false` sous VPN.
- Jellyfin : bibliothèques Films/Séries sur `/media/movies` et `/media/tvshows` ; leurs GUID
  vont dans `.env`. Créer une clé API.
- Jellyseerr : lier Jellyfin, Sonarr, Radarr ; clé API.
- NPM : proxy hosts `<svc>.<domaine>` → `<container>:<port>` ; pour l'onboarder :
  `172.18.0.1:8766` (homelabd tourne sur l'hôte, 172.18.0.1 = passerelle du réseau `homelab`).
- Grafana : datasource InfluxDB (org/bucket de `.env`, token `INFLUX_TOKEN`).

## Mise à jour d'une installation existante

```bash
cd /opt/homelab && git pull
docker compose config --quiet && docker compose up -d      # ne recrée que ce qui a changé
sudo ./setup.sh            # si homelabd/homelabctl ou les unités systemd ont changé
```

## Profils VPN

`COMPOSE_PROFILES=vpn` (défaut) lance `gluetun` + `qbittorrent` (netns partagé, port
forwarding ProtonVPN poussé dans qBittorrent par `hooks/qbit-update-port.sh`).
`homelabctl vpn off` passe en `novpn` : `qbittorrent-direct` expose 8080/6881 lui-même,
les download clients Sonarr/Radarr et le proxy NPM sont repointés, IPv6 réactivé.
`homelabctl vpn on` fait l'inverse. Ne jamais lancer les deux profils en même temps.

## Sauvegarde et restauration

`sudo homelabctl backup` (et le timer `homelab-backup.timer`, dimanche 04:30) produit dans
`backups/` : `homelab-state-<ts>.tar.zst` (tout `/opt/homelab` hors `library/`, `influxdb/`,
caches et logs), `.sha256`, `.list.gz` (manifeste), `guacdb-<ts>.sql.gz`, `systemd-<ts>.tar.gz`
(unités, crontab, compose rendu) et `images-<ts>.txt`. Les 4 dernières archives sont gardées.
`library/` (médias, téléchargements) n'est pas sauvegardé : trop gros, re-téléchargeable.

Restaurer un service :
```bash
docker compose stop sonarr
sudo tar -xpf backups/homelab-state-<ts>.tar.zst -C /opt --numeric-owner homelab/sonarr
docker compose start sonarr
```
Guacamole : `zcat backups/guacdb-<ts>.sql.gz | docker exec -i guacdb mysql -uroot -p"$MYSQL_ROOT_PASSWORD"`.

## Retour arrière

- Compose : `git log` → `git checkout <commit> -- docker-compose.yml` → `docker compose up -d`.
  Les images restent en cache local ; les digests garantissent l'identité.
- homelabd : réinstaller un binaire précédent (release GitHub) et `systemctl restart homelabd`,
  ou `systemctl stop homelabd` — les services Docker n'en dépendent pas.
- État : extraction ciblée depuis `backups/` comme ci-dessus.

## Démarrage machine

`homelab-stack.service` lance `docker compose up -d --remove-orphans` après Docker ; les
`depends_on` conditionnels (guacamole → guacdb healthy, qbittorrent → gluetun healthy,
grafana/telegraf → influxdb healthy) remplacent l'ancien script de priorité de boot.
`homelabd.service` démarre ensuite.
