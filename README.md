# groscailloux-homelab

Stack média auto-hébergée sur un seul VPS : Jellyfin + Jellyseerr + Sonarr/Radarr/Prowlarr +
qBittorrent derrière un VPN, observabilité Telegraf/InfluxDB/Grafana, accès distant Guacamole,
reverse proxy Nginx Proxy Manager. L'automatisation (import, garde-fous qBittorrent, onboarding
utilisateurs) est un daemon Rust unique, `homelabd`, piloté par `homelabctl`.

```
docker-compose.yml   21 services, images pinnées tag@digest, healthchecks, limites mémoire
homelab.toml         configuration de homelabd (intervalles, seuils, chemins)
.env                 secrets et valeurs d'hôte (jamais commité) — modèle : .env.example
crates/              homelab-core (logique), homelabd (daemon), homelabctl (CLI)
systemd/             homelabd.service, homelab-stack.service, homelab-backup.{service,timer}
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

Détails dans [DEPLOY.md](DEPLOY.md). Architecture et flux dans [ARCHITECTURE.md](ARCHITECTURE.md).
Chaque tâche automatisée, ses endpoints et ses garde-fous dans [AUTOMATION.md](AUTOMATION.md).

## Exploitation au quotidien

```bash
docker compose ps                         # état + healthchecks
docker compose config --quiet             # valider avant tout up -d
docker compose up -d <service>            # appliquer une modification du compose

journalctl -u homelabd -f                 # logs des tâches (run_done task=… summary=…)
homelabctl status                         # dernier passage de chaque tâche
homelabctl run <tâche> [--dry-run]        # exécution manuelle (voir `homelabctl list`)
homelabctl onboard <user> <email>         # compte Jellyfin + Jellyseerr + mail
homelabctl vpn status|on|off              # qBittorrent via gluetun ou en direct
sudo homelabctl backup                    # état → backups/ (aussi chaque dimanche 04:30)
```

Mettre à jour une image : diun envoie un mail quand un nouveau tag existe. Changer `tag@sha256`
dans `docker-compose.yml`, `docker compose pull <svc> && docker compose up -d <svc>`, commit.

Mettre à jour `homelabd` : `cargo build --release --target x86_64-unknown-linux-musl`,
`sudo install target/x86_64-unknown-linux-musl/release/homelab{d,ctl} /usr/local/bin/`,
`sudo systemctl restart homelabd`. Ou un tag `v2.x.y` → release GitHub → `sudo ./setup.sh`.

## Ports

| Service | Port hôte | Service | Port hôte |
|---|---|---|---|
| Jellyfin | 8096 | Jellyseerr | 5055 |
| Sonarr | 8989 | Radarr | 7878 |
| Prowlarr | 9696 | qBittorrent (via gluetun) | 8080 |
| pyLoad | 8000 | Homarr | 7575 |
| Grafana | 3000 | InfluxDB | 8086 |
| Portainer | 9000 | Glances | 61208 |
| Guacamole | 8081 | NPM | 80 / 81 / 443 |
| homelabd (UI onboarding, /health, /status) | 8766 | | |

Licence MIT.
