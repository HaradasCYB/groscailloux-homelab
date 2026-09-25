# groscailloux-homelab

**Version 1.19** (septembre 2026) — historique des versions dans [UPDATE.md](UPDATE.md).

Service de streaming privé pour une petite communauté, sur deux machines : un **VPS** (Jellyfin, Jellyseerr,
Nginx Proxy Manager, observabilité Telegraf/InfluxDB/Grafana, bureau distant Guacamole, bibliothèque historique) et
une **seedbox** (qBittorrent, Radarr, Sonarr, Bazarr : tous les téléchargements), montée sur le VPS par rclone.
L'automatisation est un daemon Rust unique, `homelabd`, piloté par `homelabctl` : recherche des releases par TMDB
(C411 seul), imports, sous-titres, garde-fous qBittorrent, onboarding et comptes des membres, abonnements PayPal,
pages « Mon compte », tchat et suivi des demandes intégrés à Jellyfin.

```
docker-compose.yml   21 services, images pinnées tag@digest, healthchecks, ports sur 127.0.0.1
homelab.toml         configuration de homelabd (intervalles, seuils, chemins)
.env                 secrets et valeurs d'hôte (jamais commité) — modèle : .env.example
crates/              homelab-core (logique), homelabd (daemon), homelabctl (CLI)
systemd/             homelabd, homelab-stack, homelab-backup, homelab-seedbox-mount, jellyfin-transcodes-purge
branding/jellyfin/   thème (calque CSS), scripts injectés (qualité, AirPlay, langue, TV), logo
docs/                INFRA.md (vue d'ensemble), ONBOARDING.md (arrivée d'un membre)
hooks/               qbit-update-port.sh, exécuté par gluetun à chaque port forwardé
setup.sh             bootstrap idempotent d'un hôte neuf
diun/images.yml      images surveillées pour notification de nouvelles versions
```

## Démarrage rapide

```bash
git clone https://github.com/HaradasCYB/groscailloux-homelab.git /opt/homelab
cd /opt/homelab && cp .env.example .env && chmod 600 .env && nano .env   # voir SECRETS.md
sudo ./setup.sh                 # ou --from-source pour compiler homelabd localement
homelabctl check                # chaque service répond avec les clés fournies
```

Vue d'ensemble avec schémas (physique, parcours d'une demande, stockage) : [docs/INFRA.md](docs/INFRA.md).
Arrivée d'un nouveau membre, côté admin : [docs/ONBOARDING.md](docs/ONBOARDING.md).
Détails dans [DEPLOY.md](DEPLOY.md). Architecture et flux dans [ARCHITECTURE.md](ARCHITECTURE.md).
Chaque tâche automatisée, ses endpoints et ses garde-fous dans [AUTOMATION.md](AUTOMATION.md).

## Exploitation au quotidien

```bash
docker compose ps                         # état + healthchecks
docker compose config --quiet             # valider avant tout up -d
docker compose up -d <service>            # appliquer une modification du compose

journalctl -u homelabd -f                 # logs des tâches (run_done task=… summary=…)
homelabctl status                         # dernier passage de chaque tâche
homelabctl run <tâche> [--dry-run]        # un passage exécuté par le démon (--dry-run : local, sans écriture)
homelabctl onboard <user> <email>         # compte Jellyfin + Jellyseerr + mail avec lien de bienvenue
homelabctl accounts list|on|off|link      # comptes des membres (on/off/delete passent par le démon)
homelabctl vpn status|on|off              # qBittorrent via gluetun ou en direct
sudo homelabctl backup                    # état → backups/ (aussi chaque dimanche 04:40, archive testée)
```

Mettre à jour une image : diun envoie un mail quand un nouveau tag existe. Changer `tag@sha256`
dans `docker-compose.yml`, `docker compose pull <svc> && docker compose up -d <svc>`, commit.

Mettre à jour `homelabd` : `cargo build --release --target x86_64-unknown-linux-musl`,
`sudo install target/x86_64-unknown-linux-musl/release/homelab{d,ctl} /usr/local/bin/`,
`sudo systemctl restart homelabd`. Ou un tag `v2.x.y` → release GitHub → `sudo ./setup.sh`.
Une clé ajoutée à `homelab.toml` est refusée par l'ancien binaire (`deny_unknown_fields`) : installer le nouveau
avant tout redémarrage. Un test vérifie que les valeurs par défaut du code sont celles de `homelab.toml`.

## Ports

Tous les services passent par Nginx Proxy Manager (80/443). Les ports des conteneurs ne sont publiés que sur
`127.0.0.1` : Docker contourne ufw, un port publié sur toutes les interfaces serait joignable depuis Internet.

| Service | Port (127.0.0.1) | Service | Port (127.0.0.1) |
|---|---|---|---|
| Jellyfin | 8096 | Jellyseerr | 5055 |
| Sonarr | 8989 | Radarr | 7878 |
| Prowlarr | 9696 | qBittorrent (via gluetun) | 8080 |
| pyLoad | 8000 | Homarr | 7575 |
| Grafana | 3000 | InfluxDB | 8086 |
| Portainer | 9000 | Glances | 61208 |
| Guacamole | 8081 | NPM (public) | 80 / 443, admin 81 |
| BitTorrent (public) | 6881 | homelabd (hôte : pages, API, /health) | 8766 |

Licence MIT.
