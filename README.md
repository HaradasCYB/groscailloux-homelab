# groscailloux-homelab

Stack self-hosted complet pour serveur perso : streaming media (Jellyfin), demande de contenu (Jellyseerr), pipeline d'acquisition automatisée (Radarr / Sonarr / Prowlarr / qBittorrent derrière VPN ProtonVPN avec port forwarding), observabilité (Grafana / InfluxDB / Telegraf), accès distant (Guacamole), bureautique (Nginx Proxy Manager, DuckDNS, Portainer, Filebrowser, Homarr).

23 services Docker dans un `docker-compose.yml` monolithique + une dizaine de scripts bash automatisés via timers systemd.

## Points clés

- **Acquisition automatique** : drop d'un fichier dans `/downloads` → auto-détecté → ajouté à Radarr/Sonarr → importé via hardlink vers `/media` → visible dans Jellyfin
- **Onboarding utilisateur unifié** : 1 commande crée Jellyfin + Jellyseerr (Jellyfin-imported) + envoie un mail custom avec creds — pas de désynchronisation possible
- **VPN port forwarding** ProtonVPN/NAT-PMP pour seeding effectif derrière VPN
- **Per-tracker ratio policy** : C411 illimité, publics 2.0, autres 1.0 (auto-applique aux nouveaux torrents)
- **Disk pressure handler** : pause / delete les torrents anciens si disque > 90% (hardlinks `/media` survivent)
- **Stuck handler** : remplace automatiquement les torrents Sonarr/Radarr stalled depuis 8h via blocklist + re-search
- **UI hijack Jellyseerr** : "Add User" via UI → poller détecte → re-route vers le script unifié

## Quickstart (3 commandes)

```bash
git clone https://github.com/HaradasCYB/groscailloux-homelab.git /opt/homelab
cd /opt/homelab && cp .env.example .env && nano .env   # remplir tous les CHANGE_ME
sudo ./setup.sh
```

Détails complets dans **[DEPLOY.md](DEPLOY.md)**.

## Architecture

3 pipelines sur un réseau Docker partagé `homelab` :
1. **Acquisition** : Jellyseerr → Radarr/Sonarr → Prowlarr/Jackett → qBittorrent (gluetun VPN) → import vers `/media`
2. **Streaming** : Jellyfin lit `/media` en read-only
3. **Observabilité** : Telegraf → InfluxDB → Grafana

Schéma détaillé dans **[ARCHITECTURE.md](ARCHITECTURE.md)**.

## Documentation

- **[DEPLOY.md](DEPLOY.md)** — guide pas-à-pas de déploiement (achat domaine, ProtonVPN, Gmail, NPM)
- **[SECRETS.md](SECRETS.md)** — chaque variable `.env` expliquée + où l'obtenir
- **[SCRIPTS.md](SCRIPTS.md)** — description de chaque helper bash + cas d'usage
- **[ARCHITECTURE.md](ARCHITECTURE.md)** — diagramme et explications des pipelines

## Stack

| Service | Rôle | Port |
|---|---|---|
| Jellyfin | Serveur de streaming | 8096 |
| Jellyseerr | Demandes de contenu | 5055 |
| Radarr | Films | 7878 |
| Sonarr | Séries | 8989 |
| Prowlarr | Manager d'indexers | 9696 |
| Jackett | Manager d'indexers (legacy) | 9117 |
| qBittorrent | Client torrent (derrière VPN) | 8080 |
| pyLoad | Direct downloader | 8000 |
| FlareSolverr | Bypass Cloudflare | 8191 |
| gluetun | VPN container (ProtonVPN) | (n/a) |
| NPM | Reverse proxy + TLS | 80/81/443 |
| DuckDNS | DNS dynamique | (n/a) |
| Filebrowser | Navigateur fichiers | (random) |
| Homarr | Dashboard | 7575 |
| Dashdot / Glances | Monitoring sys | 3001 / 61208 |
| InfluxDB / Grafana | Métriques | 8086 / 3000 |
| Telegraf | Collecteur | (n/a) |
| Portainer | UI Docker | 9000 |
| Guacamole | RDP/SSH web | 8081 |

## Prérequis

- Linux (Debian / Ubuntu testé)
- Docker + Docker Compose v2
- systemd (pour les timers)
- Outils host : `jq`, `curl`, `openssl`, `inotify-tools`, `unrar`, `unzip`, `7z`
- Un domaine (gratuit via DuckDNS suffit)
- Un compte ProtonVPN Plus (~5€/mois, requis pour seed effectif derrière VPN)
- Un compte Gmail avec 2FA + App Password (pour les mails d'onboarding)

## License

MIT — voir [LICENSE](LICENSE).
