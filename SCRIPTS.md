# SCRIPTS.md — Helpers bash

Tous les scripts sont dans `scripts/`. Chacun lit `/opt/homelab/.env` au démarrage pour ses secrets.

Les scripts en mode **timer systemd** sont installés par `setup.sh`. Les scripts **ad-hoc** sont à invoquer manuellement.

## Onboarding utilisateur unifié

### `homelab-onboard-user.sh` (ad-hoc)
Crée un compte unifié Jellyfin + Jellyseerr (Jellyfin-imported) + envoie un mail custom avec creds.

```bash
./scripts/homelab-onboard-user.sh <username> <email> [password]
```

- Génère un mdp aléatoire si absent
- Crée user Jellyfin (non-admin, accès Films + Séries)
- Importe dans Jellyseerr (`userType=2` Jellyfin-imported, pas de mdp local)
- Envoie 1 mail unifié via SMTP avec les 2 URLs et les creds

À utiliser **à la place** de "Add User" dans l'UI Jellyseerr.

### `jellyfin-create-user.sh` (interne)
Helper de bas niveau utilisé par `homelab-onboard-user.sh`. Crée juste un user Jellyfin avec policy Films+Séries non-admin.

### `jellyseerr-user-poller.{sh,service,timer}` (timer 60s)
Garde-fou : si l'admin clique "Add User" Jellyseerr UI au lieu d'utiliser le script, ce poller détecte le user local créé, le supprime et relance `homelab-onboard-user.sh`. L'utilisateur reçoit donc 2 mails (le standard Jellyseerr + le custom unifié).

## Auto-import (drop pyload / DDL manuel)

### `auto-import.{sh,service}` (daemon)
`inotifywait` sur `/opt/homelab/downloads`. À chaque nouveau `.mkv/.mp4/.avi` :

1. Parse le nom via Sonarr/Radarr `/api/v3/parse`
2. Lookup TMDB/TVDB via `/lookup`
3. **Auto-add** à Sonarr/Radarr avec `searchForXxx: false` si absent de la library
4. Trigger `DownloadedXxxScan` pour l'import (hardlink vers `/movies` ou `/tv`)

Gère aussi les archives `.zip`/`.rar` : extrait, prend le 1er video, trigger.

Supprime le fichier source (move) — ce qui veut dire que **les fichiers DDL ne sont pas seedables** (uniquement pour pyload, pas pour torrents).

### `tba-import-bypass.{sh,service,timer}` (timer 5min)
Bypass le garde-fou Sonarr "Episode has a TBA title and recently aired" pour les anime fraîchement sortis. Détecte les fichiers en `/downloads` rejetés uniquement pour cette raison et fait un ManualImport bypass.

### `jellyseerr-sonarr-monitor-sync.{sh,service,timer}` (timer 10min)
Réconcilie les `seasons[].monitored` Sonarr avec les saisons réellement demandées via Jellyseerr. Évite que Sonarr télécharge toutes les saisons d'une série quand l'utilisateur n'en voulait qu'une.

## qBittorrent automation

### `qbit-stuck-handler.{sh,service,timer}` (timer 5min)
Détecte les torrents Sonarr/Radarr en `stalledDL`/`metaDL` depuis ≥ 8h via `errorMessage` regex `(stalled|metadata|no connections)`. Pour chaque match : `DELETE /queue/{id}?removeFromClient=true&blocklist=true&skipRedownload=false` → blocklist la release, retire de qBit, déclenche un nouveau search.

State file persistant pour suivre la durée de stall. Limite 5 actions par run.

### `qbit-disk-pressure-handler.{sh,service,timer}` (timer 15min)
Garde-fou disque :
- < 90% : log only
- 90-95% : pause les torrents les plus inactifs (état uploading + upspeed < 10 KB/s, triés par `last_activity`)
- 95-98% : delete (avec `deleteFiles=true`) les torrents en `pausedUP` les plus anciens. Le `/downloads` est libéré mais **le hardlink dans `/media` survit**, donc le media reste lisible dans Jellyfin
- > 98% : alerte log, pas d'action automatique

### `qbit-tracker-ratio-policy.{sh,service,timer}` (timer 30min)
Per-tracker ratio limits via `setShareLimits` :
- C411 (`*c411.org*`) → ratio illimité, time illimité (priorité max)
- Privés similaires (`yggleak`, `u2p`, `ygg.gratis`) → ratio 2.0, time 14j
- Trackers publics connus (opentrackr, demonii, exodus...) → ratio 2.0, time 14j
- Default → ratio 1.0, time 7j

Auto-applique aux nouveaux torrents au prochain run.

### `qbit-update-port.sh` (hook gluetun)
Lancé par gluetun à chaque renouvellement du port forwardé NAT-PMP (toutes les 60s avec ProtonVPN). Patch le `listen_port` de qBit via API. Aucun cron — déclenché par `VPN_PORT_FORWARDING_UP_COMMAND` dans la conf gluetun.

## Misc

### `homelab-cleanup.sh` (ad-hoc)
Cleanup divers : torrents pyload échoués, .torrent orphelins, etc.

### `vpn-toggle.sh` (ad-hoc, legacy)
Bascule qBittorrent entre VPN ON/OFF. Datable de l'époque NordVPN — moins utile aujourd'hui qu'on est sur ProtonVPN avec port forwarding (pas besoin de toggle).

```bash
sudo ./scripts/vpn-toggle.sh {on|off|status}
```

## Logs

Tous les scripts écrivent dans `/opt/homelab/logs/<nom>.log` (TSV format `timestamp\tevent\tdetails`).

Surveiller en live :
```bash
tail -f /opt/homelab/logs/qbit-stuck-handler.log
tail -f /opt/homelab/logs/auto-import.log
journalctl -u qbit-stuck-handler.timer -f
```

## Désactivation temporaire

```bash
sudo systemctl stop qbit-stuck-handler.timer
sudo systemctl stop qbit-disk-pressure-handler.timer
# etc.
```
