# DEPLOY.md — Guide de déploiement

Estimation : 30-60 min pour la première installation (dépend des comptes externes à créer).

## Phase 0 — Prérequis externes (à faire avant de cloner)

À créer / acheter :
- [ ] **Serveur Linux** (VPS Hetzner / Contabo / OVH ou home server) — Debian 12 ou Ubuntu 22.04+ recommandé. Min 4 vCPU, 8 GB RAM, **1 TB** disque pour démarrer
- [ ] **Domaine** : compte gratuit sur https://www.duckdns.org (sous-domaine `*.duckdns.org`)
- [ ] **VPN** : compte ProtonVPN Plus (~5€/mois). Le port forwarding requiert ce plan ou supérieur
- [ ] **Compte Gmail dédié** : créer un compte pour les emails noreply, activer la 2FA, générer un App Password sur myaccount.google.com/apppasswords

## Phase 1 — Préparation host

```bash
sudo apt update
sudo apt install -y docker.io docker-compose-plugin jq curl openssl \
                    inotify-tools unzip unrar p7zip-full git

sudo useradd -m -s /bin/bash -G docker,sudo deploy
sudo passwd deploy
sudo su - deploy
```

## Phase 2 — Clone + config

```bash
sudo mkdir -p /opt/homelab
sudo chown -R deploy:deploy /opt/homelab
cd /opt
git clone https://github.com/HaradasCYB/groscailloux-homelab.git homelab
cd /opt/homelab

cp .env.example .env
nano .env
```

Remplir **toutes** les `CHANGE_ME` selon `SECRETS.md`. Les API keys *arr peuvent rester en CHANGE_ME pour le moment, on les récupère après le 1er boot.

## Phase 3 — Bootstrap

```bash
sudo ./setup.sh
```

Ce script :
- Vérifie les prérequis
- Vérifie que `.env` ne contient pas de `CHANGE_ME` critique
- Crée l'arborescence `/opt/homelab/{downloads,media,logs,...}`
- Lance `docker compose up -d`
- Installe les unit files systemd dans `/etc/systemd/system/`
- Active les timers

À ce stade, **tous les services sont up** mais pas configurés au-delà des defaults.

## Phase 4 — Configuration initiale par service (~30 min)

### 4.1 — Jellyfin (port 8096)
1. Aller sur `http://<HOST_IP>:8096`
2. Suivre l'assistant : créer admin, ajouter libraries `Films` (path `/media/movies`) et `Séries` (`/media/tvshows`)
3. Récupérer l'**API Key** : Dashboard → API Keys → New
4. Récupérer les **library GUID** : Settings → Libraries → cliquer chaque lib → URL contient `?id=<GUID>`
5. Updater `.env` : `JELLYFIN_API_KEY`, `JELLYFIN_LIB_FILMS`, `JELLYFIN_LIB_SERIES`

### 4.2 — Sonarr (port 8989) + Radarr (port 7878)
1. Aller sur les UIs
2. Settings → General → API Key → copier
3. Settings → Media Management → cocher "Use Hardlinks Instead of Copy"
4. Settings → Download Clients → Add → qBittorrent :
   - Host: `gluetun` (qBit utilise `network_mode: service:gluetun`)
   - Port: `8080`
   - Categories : `sonarr` / `radarr`
5. Updater `.env` : `SONARR_API_KEY`, `RADARR_API_KEY`

### 4.3 — Prowlarr (port 9696)
1. Settings → General → API Key → copier
2. Settings → Apps → Add Sonarr / Radarr (URL `http://sonarr:8989` / `http://radarr:7878` + leur API key)
3. Indexers → Add : Torrent9, NorTorrent, World-torrent, 1337x, etc.
4. Updater `.env` : `PROWLARR_API_KEY`

### 4.4 — qBittorrent (port 8080)
1. Premier login : `admin` / mdp temporaire (`docker logs qbittorrent`)
2. Tools → Options → Web UI → changer le password
3. Activer la **whitelist localhost** : Tools → Options → Web UI → Bypass authentication for clients in whitelist subnets : `127.0.0.1/8, 172.18.0.0/16`
4. Désactiver UPnP : Tools → Options → Connection → décocher "Use UPnP / NAT-PMP" (le PF est géré par gluetun, voir `qbit-update-port.sh`)
5. Préférences → BitTorrent → "Add to new torrents these trackers" : coller la liste de trackers publics depuis [ngosang/trackerslist](https://raw.githubusercontent.com/ngosang/trackerslist/master/trackers_best.txt)
6. Settings → Categories → ajouter `sonarr` et `radarr`

### 4.5 — Jellyseerr (port 5055)
1. Suivre l'assistant : Jellyfin (URL `http://jellyfin:8096`, API key Jellyfin)
2. Settings → Notifications → Email :
   - SMTP Host: `smtp.gmail.com`
   - Port: `465`
   - Sender: ton `SMTP_FROM`
   - Username: `SMTP_USER`
   - Password: `SMTP_PASS` (App Password Gmail)
3. Settings → General → API Key → copier dans `.env`

### 4.6 — Nginx Proxy Manager (port 81)
1. Login : `admin@example.com` / `changeme` (à changer immédiatement)
2. Hosts → Proxy Hosts → Add :
   - Domain: `jellyfin.<DUCKDNS_SUBDOMAIN>.duckdns.org`
   - Forward Hostname/IP: `jellyfin`, Forward Port: `8096`
   - SSL → Let's Encrypt → Force SSL + HTTP/2
3. Répéter pour Jellyseerr, Sonarr, Radarr, etc.

### 4.7 — DuckDNS
Le container `duckdns` met à jour automatiquement (cron interne 5 min). Vérifier `docker logs duckdns` pour les `OK` réguliers.

### 4.8 — InfluxDB + Grafana
- InfluxDB (port 8086) : setup auto via env vars. Récupérer le token : `docker exec influxdb influx auth list`
- Telegraf : éditer `/opt/homelab/telegraf/etc/telegraf.conf` pour utiliser le token
- Grafana (port 3000) : login `admin/<.env GRAFANA_ADMIN_PASSWORD>`. Add Data Source → InfluxDB v2

## Phase 5 — Lancer les init scripts

```bash
cd /opt/homelab
./init/01-sonarr-init.sh         # Custom Formats VFF/MULTi/FRENCH/Season Pack + Quality Profile 6
./init/02-radarr-init.sh         # idem côté Radarr
./init/03-jellyseerr-init.sh     # SMTP + permissions par défaut
./init/04-jellyfin-init.sh       # libraries + (optionnel) admin user
```

## Phase 6 — Premier test

```bash
# Surveiller les logs
tail -f /opt/homelab/logs/auto-import.log
journalctl -u qbit-stuck-handler.timer -f
docker logs -f sonarr
```

## Phase 7 — Onboarding ton premier utilisateur

```bash
./scripts/homelab-onboard-user.sh testuser ton-email@example.com
```

Tu reçois un mail (vérifier spam). Si OK, partage avec un vrai user.

## Troubleshooting

| Symptôme | Diagnostic |
|---|---|
| qBit dit "firewalled" | PF gluetun pas actif. `docker exec gluetun cat /tmp/gluetun/forwarded_port` doit retourner un nombre |
| Sonarr/Radarr ne grabe rien | Vérifier les indexers (Settings → Indexers → Test) |
| Drop pyload pas importé | `tail /opt/homelab/logs/auto-import.log` — si `Detected` mais pas `auto_added`, problème lookup TMDB |
| Jellyfin ne voit pas un nouvel import | Force scan via Dashboard → Libraries → Scan All ou attendre le scan auto (15 min) |
| Email d'onboarding pas reçu | Test SMTP : `swaks --to test@example.com --from $SMTP_FROM --server smtp.gmail.com:465 --auth-user $SMTP_USER --auth-password $SMTP_PASS --tls` |

## Backup conseillé

```bash
# Hebdomadaire : tar les configs services (pas /downloads ni /media)
tar -czf homelab-backup-$(date +%F).tar.gz \
  --exclude='/opt/homelab/downloads' \
  --exclude='/opt/homelab/media' \
  --exclude='/opt/homelab/logs' \
  /opt/homelab
```
