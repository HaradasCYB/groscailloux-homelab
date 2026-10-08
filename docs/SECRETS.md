# Secrets et configuration d'hôte (`.env`)

`.env` (mode 600, propriétaire `deploy`) est lu par `docker compose` (interpolation) et par
`homelabd`/`homelabctl` (`EnvironmentFile` + dotenv). Jamais commité. Modèle : `.env.example`.
Une valeur avec espace se met entre guillemets doubles ; pas d'expression shell (`${X:-y}`).

**Valeurs d'hôte citées par `homelab.toml`** (2026-10-08) : ce qui désigne la seedbox (adresse publique, compte, dossier
personnel) n'est plus écrit dans le dépôt public. `homelab.toml` cite la variable (`"${SEEDBOX_HOME}/media/Anime"`),
remplacée au chargement par homelabd et homelabctl (`.env` lu avant `homelab.toml`, y compris pour `homelabctl list`)
et par les outils Python (`tools/lib/hlconf.py` : `tools/lib/hl.py`, `scripts/move-to-seedbox.py`,
`scripts/seedbox-cleanup.py`). Variable absente ou vide = refus de démarrer, avec le nom de la variable et des clés
qui la citent (jamais une valeur) ; `$${` = « ${ » littéral. Pour le montage, `scripts/seedbox-rclone-conf.sh`
(lancé par `homelab-seedbox-mount.service` à chaque démarrage) lit l'hôte SFTP et le compte dans `.env` et les écrit
dans la config effective `/run/homelab-seedbox-mount/rclone.conf` (600) : jamais en options `--sftp-*`, qui
changeraient le dossier du cache VFS ; rclone ne reçoit rien d'autre de `.env`. Ces variables s'écrivent sans
commentaire en fin de ligne (les lecteurs Python de `.env` le garderaient dans la valeur). Changer d'hôte de seedbox :
`.env`, puis `sudo systemctl restart homelabd` et, hors pic (coupe les lectures seedbox), le montage.

| Variable | Où l'obtenir | Utilisé par |
|---|---|---|
| `TZ` | `Europe/Paris` | tous les conteneurs |
| `HOST_IP` | IP publique du VPS | **plus lue nulle part** (compose, homelabd, scripts) : `PublishedServerUrl` vient de `JELLYFIN_PUBLIC_URL` ; à retirer de `.env` et de `.env.example` |
| `JELLYFIN_PUBLIC_URL`, `JELLYSEERR_PUBLIC_URL` | URLs NPM | mails, guide, `PublishedServerUrl` de Jellyfin (compose), `cert_watch` |
| `CHAT_ADMIN_EMAIL` (facultatif, repli `GUIDE_CONTACT_EMAIL`) | adresse de l'admin | récapitulatifs du tchat |
| `ONBOARD_PUBLIC_URL`, `GUIDE_CONTACT_EMAIL`, `GUIDE_CONTACT_DISCORD` | URL NPM de l'onboarder, contact de l'admin | page `/guide`, lien du mail de bienvenue |
| `SECRET_ENCRYPTION_KEY` | `openssl rand -hex 32` | Homarr |
| `DUCKDNS_SUBDOMAIN`, `DUCKDNS_TOKEN` | duckdns.org | duckdns |
| `VPN_SERVICE_PROVIDER`, `VPN_TYPE`, `WIREGUARD_PRIVATE_KEY`, `WIREGUARD_ADDRESSES`, `SERVER_COUNTRIES` | account.proton.me → WireGuard, cocher NAT-PMP | gluetun |
| `INFLUXDB_INIT_*` | choisis au premier boot (mode setup) | influxdb |
| `INFLUX_TOKEN` | InfluxDB UI → API Tokens (ou token opérateur initial) | telegraf.conf (via setup.sh), grafana |
| `GRAFANA_ADMIN_USER`, `GRAFANA_ADMIN_PASSWORD` | choisis | grafana |
| `MYSQL_ROOT_PASSWORD`, `GUACAMOLE_DB_*`, `MYSQL_USER_PASSWORD` | choisis au premier boot | guacdb, guacamole |
| `SONARR_API_KEY`, `RADARR_API_KEY`, `PROWLARR_API_KEY` | Settings → General | homelabd |
| `JELLYFIN_API_KEY` | Dashboard → API Keys (clé « Jellyseerr », la seule : « claude-setup » révoquée le 07/10) | homelabd |
| `JELLYSEERR_API_KEY` | Settings → General | homelabd |
| `JELLYFIN_LIB_FILMS`, `JELLYFIN_LIB_SERIES` | GUID dans l'URL de chaque bibliothèque | onboarding (policy) |
| `QUALITY_PROFILE_ID` | Sonarr/Radarr → Profiles (id dans l'URL) | auto_import |
| `SMTP_HOST`, `SMTP_PORT`, `SMTP_USER`, `SMTP_PASS`, `SMTP_FROM`, `SMTP_FROM_NAME` | Gmail : mot de passe d'application (2FA requis) | onboarding, diun |
| `ADMIN_EMAIL` | boîte lue par l'admin | diun |
| `HOMELABD_ONBOARD_TOKEN` | `openssl rand -hex 32` | `POST /onboard` |
| `COMPOSE_PROFILES` | `vpn` ou `novpn`, géré par `homelabctl vpn` | docker compose |
| `JELLYFIN_LIB_EXTRA` | ids de bibliothèques supplémentaires (virgules), données à chaque nouveau compte avec Films et Séries : Anime, Films d'animation et Collections (jamais les bibliothèques russes) | onboarding (policy) |
| `SEEDBOX_PUBLIC_URL` | adresse HTTPS des applis de la seedbox (proxy de l'hébergeur), sans `/` final | `homelab.toml` : `[seedbox] radarr_url`, `sonarr_url`, `qbit_url`, `bazarr_url`, `[tasks.russian_search] jackett_url` (`${SEEDBOX_PUBLIC_URL}/<appli>`) |
| `SEEDBOX_HOME` | dossier personnel du compte sur la seedbox (`/home/<compte>`) | `homelab.toml` : `[seedbox] media_root`, `sonarr_downloads`, `radarr_root`, `sonarr_root`, dossiers `seedbox_*` de `[tasks.anime_library]`, `[tasks.indexer_unblock] seedbox_apps_dir` |
| `SEEDBOX_USER` | nom du compte de la seedbox | `homelab.toml` `[seedbox] qbit_user` ; `user` de la config effective du montage (`scripts/seedbox-rclone-conf.sh`) |
| `SEEDBOX_SFTP_HOST` | nom d'hôte SFTP de la seedbox (celui de `ssh seedbox`, sans le compte) | `host` de la config effective du montage (`scripts/seedbox-rclone-conf.sh`, lancé par `homelab-seedbox-mount.service`) |
| `SEEDBOX_RADARR_API_KEY`, `SEEDBOX_SONARR_API_KEY` | `~/.apps/<app>/config.xml` sur la seedbox | homelabd, Jellyseerr |
| `SEEDBOX_JACKETT_API_KEY` | `~/.apps/jackett/Jackett/ServerConfig.json` | indexeur RuTracker des Arrs seedbox, `russian_search` (voie russe) |
| `SEEDBOX_QBIT_PASSWORD` | mot de passe WebUI qBittorrent seedbox (installeur hébergeur) | client des Arrs seedbox, `torrent_import` |
| `SEEDBOX_*_PASSWORD` (jackett, radarr, sonarr, bazarr, autobrr) | générés, passés à `app-<x> install -p` | UIs web seedbox |
| `SEEDBOX_BAZARR_API_KEY` | réglages de Bazarr (seedbox) | `[seedbox] bazarr_url` : client `clients::bazarr` (page d'état, `seedbox_health`) |
| `C411_RSS_API_KEY` | compte C411 (seconde clé) | **copie de référence**, lue par aucun fichier versionné : clé saisie dans l'indexeur C411 (RSS) des 4 Arrs et dans « C411 (2) » de Prowlarr |
| `RUTRACKER_USERNAME`, `RUTRACKER_PASSWORD` | compte RuTracker | **copie de référence** : compte saisi dans le Jackett de la seedbox (voie russe) ; mot de passe collé dans une conversation, à changer |
| `HOMELABD_RUSSIAN_USERS` | pseudos des comptes autorisés (virgules) ; hors dépôt | bouton « Chercher en russe » de Mon compte |
| `TMDB_API_KEY` | compte TMDB | **lue par aucun fichier versionné** (réserve) |
| `DISCORD_WEBHOOK_MEMBERS`, `DISCORD_WEBHOOK_ADMIN` | Discord : Modifier le salon → Intégrations → Webhooks | homelabd (`homelab_core::discord`, alertes), `homelabctl discord apply` (Arrs, Jellyseerr), `scripts/homelab-alert.sh`, `homelabd-watchdog.sh` ; jamais dans le dépôt ni les journaux |
| `DISCORD_ROLE_MEMBERS` (facultatif) | identifiant du rôle à mentionner | annonces sur le salon des membres |
| `PAYPAL_ENV` | `live` (en service depuis le 20/09) ou `sandbox` | homelabd : choisit `PAYPAL_*` ou `PAYPAL_SANDBOX_*` |
| `PAYPAL_CLIENT_ID`, `PAYPAL_SECRET`, `PAYPAL_PLAN_ID`, `PAYPAL_WEBHOOK_ID` | application REST PayPal **live** (même application et même plan que le bouton `DONATION_*`) ; webhook créé par `homelabctl subs paypal --webhook <url>` | abonnements (`/premium`, `/paypal/webhook`, `subscription_reconcile`) |
| `PAYPAL_SANDBOX_CLIENT_ID`, `PAYPAL_SANDBOX_SECRET`, `PAYPAL_SANDBOX_PLAN_ID`, `PAYPAL_SANDBOX_WEBHOOK_ID` | application sandbox (identifiants collés dans une conversation le 20/09 : à régénérer) | essais seulement (`/premium?test=1` si `PAYPAL_ENV=sandbox`) |
| `PREMIUM_PUBLIC_URL` | URL NPM de l'hôte public de `/premium` | liens des mails d'abonnement, webhook |
| `DONATION_NOTIFY_EMAIL` | adresse de l'admin | notification d'une demande d'activation premium |

La clé SSH du montage rclone, `~/.ssh/seedbox_sftp_ro` (hors dépôt ; nom historique), est autorisée côté seedbox avec
le commentaire `homelab-sftp-rd` et `restrict,command="…sftp-server -P write,mkdir,rename,…"` : **lecture + suppression,
aucune écriture** (le bouton « Supprimer » de Jellyfin doit pouvoir effacer). La clé d'administration est
`~/.ssh/seedbox_ed25519` (`ssh seedbox`).

Clés C411 : la clé de recherche n'est que dans Prowlarr (indexeur « C411 ») ; la seconde (`C411_RSS_API_KEY`) sert au RSS
des 4 Arrs et à « C411 (2) » de Prowlarr ; une **troisième clé**, distincte, est dans la base d'autobrr seulement (flux
désactivé le 08/10, comptée dans aucun budget : à révoquer chez C411 si le flux n'est pas rallumé).

## Rotation

- Clé API d'un Arr / Jellyfin / Jellyseerr : régénérer dans l'UI, mettre à jour `.env`,
  `sudo systemctl restart homelabd` (les conteneurs n'en dépendent pas).
- Clé API **Jellyseerr** : elle a d'autres consommateurs (Jellyfin Enhanced, Home Screen Sections, intégration Homarr) :
  toujours `sudo scripts/jellyseerr-rotate-key.py` (tout en une fois, ~12 s de coupure, aucune clé affichée).
- Webhook Discord : un webhook collé dans une conversation est compromis ; le recréer dans Discord, mettre à jour `.env`,
  restart homelabd, puis `homelabctl discord apply` (Arrs et Jellyseerr).
- Un secret collé dans une conversation (mot de passe RuTracker, identifiants PayPal sandbox…) est à régénérer.
- `INFLUX_TOKEN` : mettre à jour `.env`, `sudo ./setup.sh` (régénère `telegraf.conf`),
  `docker compose restart telegraf`, datasource Grafana.
- Secrets de conteneurs (`WIREGUARD_*`, `DUCKDNS_TOKEN`, `MYSQL_*`, `GRAFANA_*`) : `.env` puis
  `docker compose up -d <service>` (MySQL : le mot de passe root n'est lu qu'à l'initialisation ;
  changer via `ALTER USER` puis `.env`).
- `HOMELABD_ONBOARD_TOKEN` / `HOMELABD_STATUS_TOKEN` : `.env`, restart homelabd — toutes les sessions `/connexion`
  tombent (cookie signé avec le jeton) ; se reconnecter avec le nouveau. Aucun lien à changer (plus de `?token=`).
  Le jeton d'onboarding n'apparaît plus dans les pages (jeton de formulaire HMAC) ; sans lui, `POST /onboard` est
  fermé. Il signe aussi la clé `k` des liens `/premium` des mails : le renouveler rend ces clés caduques, sans risque.
- `HOMELABD_ADMIN_TRUSTED_IPS` (une IP, donc hors dépôt) : IP de la maison, pages d'admin sans connexion, lue
  seulement dans un `X-Forwarded-For` posé par NPM confirmé par Docker (jamais une connexion locale ni un autre
  conteneur). Si la box change d'IP : mettre la nouvelle, restart homelabd (le cookie d'un an couvre l'entre-deux).
- `ONBOARD_PUBLIC_URL` : son hôte est le seul qui sert les pages d'administration de homelabd (404 ailleurs).

## Où sont les autres secrets

- Certificats Let's Encrypt : `npm/letsencrypt/` (root) — inclus dans `homelabctl backup`.
- Mots de passe utilisateurs : uniquement dans Jellyfin (hashés). Aucun mot de passe ne transite par mail : le membre le
  choisit sur `/bienvenue/<jeton>` (lien à usage unique, jeton haché dans l'état) ; jamais journalisé.
- L'archive `backups/homelab-state-*.tar.zst` contient `.env` et les configs des services :
  la traiter comme un secret (mode 600).

## Accès d'administration (2026-09-12)

| Variable | Rôle |
|---|---|
| `NPM_ADMIN_TOOLS_PASSWORD` | mot de passe HTTP (utilisateur `groscailloux`) de la liste d'accès NPM « admin-outils » (id 2) devant Sonarr, Radarr, qBittorrent, Prowlarr, Grafana, Portainer, pyLoad, Guacamole, `/accounts` et `/recherche`. NPM le stocke aussi en clair dans sa base (conception NPM) et en apr1 dans `npm/data/access/2`. |
| `HOMARR_ADMIN_PASSWORD` | compte Homarr `groscailloux` (groupe admin) pour le tableau privé « Operations ». Le compte propriétaire existe aussi. |
| `HOMELABD_STATUS_TOKEN` | jeton de `/status` et `/status.html` (homelabd), ouvre une session limitée à ces deux pages (`/connexion`) ; l'iFrame du tableau Operations n'a plus de jeton. |
| `DONATION_PAYPAL_CLIENT_ID`, `DONATION_PAYPAL_PLAN_ID` | bouton PayPal de la page de don `/don`, et bouton de repli de `/premium` si `PAYPAL_ENV` repassait en sandbox. Publics une fois la page affichée, mais gardés hors du dépôt pour ne pas y lier le compte PayPal. |
| `GRAFANA_ADMIN_PASSWORD` | changé le 2026-09-12 (`grafana cli admin reset-admin-password`) : l'ancien avait fuité. Grafana l'enregistre dans sa base, la variable ne sert qu'au premier démarrage. |

Changer l'un d'eux : mettre à jour `.env` **et** l'endroit qui l'utilise (NPM : base + fichier `access/2` puis `nginx -s reload` ; Homarr : hash bcrypt en base ; homelabd : `systemctl restart homelabd`, plus aucune URL à changer dans Homarr : l'iFrame n'a plus de jeton).
