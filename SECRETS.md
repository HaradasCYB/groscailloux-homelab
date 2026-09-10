# Secrets et configuration d'hôte (`.env`)

`.env` (mode 600, propriétaire `deploy`) est lu par `docker compose` (interpolation) et par
`homelabd`/`homelabctl` (`EnvironmentFile` + dotenv). Jamais commité. Modèle : `.env.example`.
Une valeur avec espace se met entre guillemets doubles ; pas d'expression shell (`${X:-y}`).

| Variable | Où l'obtenir | Utilisé par |
|---|---|---|
| `TZ` | `Europe/Paris` | tous les conteneurs |
| `HOST_IP` | IP publique du VPS | Jellyfin `PublishedServerUrl` |
| `JELLYFIN_PUBLIC_URL`, `JELLYSEERR_PUBLIC_URL` | URLs NPM | mail d'onboarding |
| `SECRET_ENCRYPTION_KEY` | `openssl rand -hex 32` | Homarr |
| `DUCKDNS_SUBDOMAIN`, `DUCKDNS_TOKEN` | duckdns.org | duckdns |
| `VPN_SERVICE_PROVIDER`, `VPN_TYPE`, `WIREGUARD_PRIVATE_KEY`, `WIREGUARD_ADDRESSES`, `SERVER_COUNTRIES` | account.proton.me → WireGuard, cocher NAT-PMP | gluetun |
| `INFLUXDB_INIT_*` | choisis au premier boot (mode setup) | influxdb |
| `INFLUX_TOKEN` | InfluxDB UI → API Tokens (ou token opérateur initial) | telegraf.conf (via setup.sh), grafana |
| `GRAFANA_ADMIN_USER`, `GRAFANA_ADMIN_PASSWORD` | choisis | grafana |
| `MYSQL_ROOT_PASSWORD`, `GUACAMOLE_DB_*`, `MYSQL_USER_PASSWORD` | choisis au premier boot | guacdb, guacamole |
| `SONARR_API_KEY`, `RADARR_API_KEY`, `PROWLARR_API_KEY` | Settings → General | homelabd |
| `JELLYFIN_API_KEY` | Dashboard → API Keys | homelabd |
| `JELLYSEERR_API_KEY` | Settings → General | homelabd |
| `JELLYFIN_LIB_FILMS`, `JELLYFIN_LIB_SERIES` | GUID dans l'URL de chaque bibliothèque | onboarding (policy) |
| `QUALITY_PROFILE_ID` | Sonarr/Radarr → Profiles (id dans l'URL) | auto_import |
| `SMTP_HOST`, `SMTP_PORT`, `SMTP_USER`, `SMTP_PASS`, `SMTP_FROM`, `SMTP_FROM_NAME` | Gmail : mot de passe d'application (2FA requis) | onboarding, diun |
| `ADMIN_EMAIL` | boîte lue par l'admin | diun |
| `HOMELABD_ONBOARD_TOKEN` | `openssl rand -hex 32` | `POST /onboard` |
| `COMPOSE_PROFILES` | `vpn` ou `novpn`, géré par `homelabctl vpn` | docker compose |

## Rotation

- Clé API d'un Arr / Jellyfin / Jellyseerr : régénérer dans l'UI, mettre à jour `.env`,
  `sudo systemctl restart homelabd` (les conteneurs n'en dépendent pas).
- `INFLUX_TOKEN` : mettre à jour `.env`, `sudo ./setup.sh` (régénère `telegraf.conf`),
  `docker compose restart telegraf`, datasource Grafana.
- Secrets de conteneurs (`WIREGUARD_*`, `DUCKDNS_TOKEN`, `MYSQL_*`, `GRAFANA_*`) : `.env` puis
  `docker compose up -d <service>` (MySQL : le mot de passe root n'est lu qu'à l'initialisation ;
  changer via `ALTER USER` puis `.env`).
- `HOMELABD_ONBOARD_TOKEN` : `.env`, restart homelabd, nouveau lien `?token=` pour l'admin.

## Où sont les autres secrets

- Certificats Let's Encrypt : `npm/letsencrypt/` (root) — inclus dans `homelabctl backup`.
- Mots de passe utilisateurs : uniquement dans Jellyfin (hashés). Le mail d'onboarding est le
  seul endroit où un mot de passe transite en clair ; il n'est jamais loggé.
- L'archive `backups/homelab-state-*.tar.zst` contient `.env` et les configs des services :
  la traiter comme un secret (mode 600).
