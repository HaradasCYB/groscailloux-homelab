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
  Avec ufw actif, autoriser ce flux conteneur → hôte :
  `sudo ufw allow from 172.18.0.0/16 to any port 8766 proto tcp comment 'homelabd via NPM'`.
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

## Seedbox (optionnelle)

Mise en place (déjà faite sur la prod, à refaire sur une nouvelle seedbox) :
1. Sur la seedbox : `app-jackett|radarr|sonarr|bazarr|autobrr install -p <mdp>`, `app-flaresolverr install`,
   `app-unpackerr install` ; catégories qBittorrent `radarr`/`sonarr` ; clés API et mots de passe
   dans le `.env` du VPS (`SEEDBOX_*`).
2. Réglages des Arrs seedbox clonés depuis ceux du VPS (formats personnalisés, profils, indexers
   via le Jackett seedbox, C411 en automatique complet, client qBittorrent via le proxy HTTPS).
3. Clé `~/.ssh/seedbox_sftp_ro` ajoutée dans `~/.ssh/authorized_keys` de la seedbox avec
   `restrict,command="/usr/lib/openssh/sftp-server -R"` ; rclone ≥ 1.68 dans `/usr/local/bin` ;
   `user_allow_other` dans `/etc/fuse.conf` ; `mkdir -p /mnt/seedbox/media`.
4. `[seedbox] enabled = true` dans `homelab.toml`, `sudo homelabctl install` (active
   `homelab-seedbox-mount.service`), bibliothèques Jellyfin sur `/seedbox/media/Movies` et
   `/seedbox/media/TV Shows` (surveillance temps réel off), leurs ids dans `JELLYFIN_LIB_EXTRA`.
5. Jellyseerr : Radarr/Sonarr seedbox en serveurs par défaut ; bibliothèques activées via
   `…/settings/jellyfin/library?enable=<ids>` (**jamais** `sync=true` seul : il désactive tout).

Vérifier : `homelabctl check` (Arrs seedbox + montage), `systemctl status homelab-seedbox-mount`.

### Couper la seedbox (~10 min, sans impact sur le reste)

1. Jellyseerr → Settings → Services : remettre Radarr/Sonarr du VPS **par défaut**, supprimer ceux
   de la seedbox (les demandes en cours restent visibles).
2. `homelab.toml` : `[seedbox] enabled = false` → `sudo systemctl restart homelabd`.
3. `sudo systemctl disable --now homelab-seedbox-mount`.
4. Jellyfin : supprimer « Films (Seedbox) » et « Séries (Seedbox) », retirer leurs ids de
   `JELLYFIN_LIB_EXTRA` ; optionnel : retirer la ligne `/mnt/seedbox:/seedbox` du service jellyfin.
Les bibliothèques et le pipeline du VPS ne sont jamais touchés par ces étapes.

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

Au boot, le daemon Docker relance lui-même les conteneurs `restart: unless-stopped`, dans le
désordre et sans tenir compte des `depends_on`. Deux mécanismes compensent :
- `homelab-stack.service` lance `docker compose up -d --remove-orphans` après Docker : démarre ce
  que le daemon n'a pas lancé, en respectant les conditions (`guacamole` est en `restart: "no"`
  précisément pour n'être lancé que par compose, après `guacdb` healthy — l'image ne vérifie
  jamais sa base et resterait « Up » avec un login cassé).
- `homelabd.service` démarre ensuite et sa tâche `stack_health` fait une passe immédiate, puis
  toutes les 5 min : relance l'arrêté, redémarre l'`unhealthy` et ce dont la sonde échoue
  (voir AUTOMATION.md).

Après un reboot : `docker compose ps`, `homelabctl status` (stack_health `last_ok`),
`journalctl -u homelabd | grep stack_health`.

Pas de reboot planifié : aucun gain constaté (mémoire stable), une coupure de 2–3 min pour les
lectures et téléchargements, et le boot est le moment fragile. Rebooter à la main pour les
mises à jour du noyau (`apt` le signale), puis vérifier comme ci-dessus.
