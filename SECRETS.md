# SECRETS.md — Variables d'environnement

Toutes les variables sensibles vont dans `/opt/homelab/.env` (jamais commit). Copier `.env.example` → `.env` et remplir chaque `CHANGE_ME`.

## Variables globales

| Variable | Comment l'obtenir |
|---|---|
| `SECRET_ENCRYPTION_KEY` | Générer : `openssl rand -hex 32` |
| `TZ` | Format IANA (ex: `Europe/Paris`, `America/New_York`) |
| `HOST_IP` | IP publique du host : `curl https://ipinfo.io/ip` |

## VPN ProtonVPN (Wireguard + port forwarding)

| Variable | Comment l'obtenir |
|---|---|
| `WIREGUARD_PRIVATE_KEY` | account.proton.me → WireGuard → Create config → cocher **NAT-PMP (port forwarding)** + décocher **NAT modéré** + cocher **VPN Accelerator** → copier la `PrivateKey` du `.conf` |
| `WIREGUARD_ADDRESSES` | Donné par ProtonVPN (typique : `10.2.0.2/32`) |
| `SERVER_COUNTRIES` | Pays préféré (ex: `France`, `Switzerland`) |

⚠️ Plan **Plus / Visionary requis** pour le port forwarding (~5€/mois). Sans PF, le seeding derrière VPN est inefficace.

Alternatives : AirVPN (PF statique, ~50€/an), PIA, Mullvad (PF retiré 2023).

## DuckDNS (DNS dynamique gratuit)

| Variable | Comment l'obtenir |
|---|---|
| `DUCKDNS_SUBDOMAIN` | Inscription sur https://www.duckdns.org → créer sous-domaine `monhomelab.duckdns.org` → mettre juste `monhomelab` ici |
| `DUCKDNS_TOKEN` | Visible sur duckdns.org après login (token unique compte) |

## Bases de données

| Variable | Comment l'obtenir |
|---|---|
| `INFLUXDB_INIT_PASSWORD` | Générer : `openssl rand -base64 24` |
| `GRAFANA_ADMIN_PASSWORD` | Générer : `openssl rand -base64 24` |
| `MYSQL_ROOT_PASSWORD` | Générer : `openssl rand -base64 24` |
| `MYSQL_USER_PASSWORD` | Générer : `openssl rand -base64 24` |

## API keys des services *arr et media

À récupérer **après** le premier `setup.sh` (les services doivent être bootés). Pour chaque :

| Service | Où trouver l'API key |
|---|---|
| Sonarr | UI → Settings → General → API Key |
| Radarr | UI → Settings → General → API Key |
| Prowlarr | UI → Settings → General → API Key |
| Jackett | UI (top-right) → API Key |
| Jellyfin | UI → Dashboard → API Keys → New |
| Jellyseerr | UI → Settings → General → API Key |

## Jellyfin libraries

Après création des libraries Films/Séries dans Jellyfin Dashboard :

| Variable | Comment l'obtenir |
|---|---|
| `JELLYFIN_LIB_FILMS` | Dashboard → Libraries → cliquer la lib "Films" → l'URL contient `?id=<GUID>` |
| `JELLYFIN_LIB_SERIES` | Idem pour la lib "Séries" |

## SMTP Gmail (mails d'onboarding utilisateur)

| Variable | Comment l'obtenir |
|---|---|
| `SMTP_USER` | Adresse Gmail dédiée (compte séparé recommandé) |
| `SMTP_PASS` | App Password 16 chars : myaccount.google.com/apppasswords (nécessite 2FA active sur le compte) |
| `SMTP_FROM` | = `SMTP_USER` (par défaut) |
| `SMTP_FROM_NAME` | Nom affiché aux destinataires (ex: "Mon Homelab") |
| `ADMIN_EMAIL` | Ton email perso (admin Jellyseerr, reçoit les notifications) |

## URLs publiques (après config NPM + DuckDNS)

| Variable | Format |
|---|---|
| `JELLYFIN_PUBLIC_URL` | `https://jellyfin.<DUCKDNS_SUBDOMAIN>.duckdns.org` |
| `JELLYSEERR_PUBLIC_URL` | `https://jellyseerr.<DUCKDNS_SUBDOMAIN>.duckdns.org` |

## Optionnels

| Variable | Pour quoi |
|---|---|
| `C411_API_KEY` | Tracker privé FR. Si tu n'as pas de compte, laisser vide (pas critique) |

## Sécurité

- **Ne jamais committer `.env`** : `.gitignore` l'exclut déjà
- **Régénérer toutes les valeurs** si une fuite a lieu
- Les secrets utilisés une fois (`.env` lu par les containers) restent dans la mémoire des containers — `docker compose down && up -d` après changement
- Pour les API keys *arr, le service écrit la key dans son fichier de config — la rotation nécessite update dans les 2 endroits (l'UI + `.env`)
