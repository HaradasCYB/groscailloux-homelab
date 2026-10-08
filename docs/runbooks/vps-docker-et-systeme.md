# VPS : Docker Compose, images, qBittorrent, système

À lire avant de modifier `docker-compose.yml`, `diun/images.yml`, un conteneur (recréation, mise à jour d'image), le
VPN ou qBittorrent, ou avant un redémarrage du VPS. Déploiement et restauration : [DEPLOY.md](../DEPLOY.md).

## 1. Compose et images

- `docker compose config --quiet` **avant tout** `up -d` ; `docker compose up -d <svc>` ne recrée que ce qui a changé.
- **Images épinglées `tag@sha256`**, jamais de `:latest` nu (gluetun est encore en `latest@sha256:…`, épinglé par son
  digest ; le lot 3 le passe en `v3.41.3`). Mettre à jour : nouveau tag + digest (`docker pull` puis
  `docker image inspect --format '{{index .RepoDigests 0}}'`), `up -d <svc>`, puis `diun/images.yml` en cohérence.
- **`diun/images.yml`** : un bloc par image, jamais de clé sans `- name:` (des clés orphelines de Jackett/FlareSolverr ont
  rendu diun aveugle du 18/09 au 07/10). `homelabctl check` et la tâche `diun_watch` (24 h, alerte) refusent les clés en
  double et toute image du compose sans entrée `repo:tag`. `sort_tags: semver` sur les tags semver purs seulement (le tri
  par défaut est alphabétique : 9.5.9 passe devant 13.2.3), pas sur les tags linuxserver. Formats amont (07/10) :
  Jellyfin 12 en `x.y` ; qBittorrent `5.2.4_v2.0.15-lsN` ; Glances sans `v` (`4.5.4` ; le digest d'un tag est celui de
  son index, pas celui de `latest`). Un nouveau motif se teste avec un diun jetable (`DIUN_WATCH_RUNONSTARTUP=true`, sans
  notification).
- **Ports publiés sur `127.0.0.1` seulement** (Docker contourne ufw) : tout passe par NPM (noms de conteneurs) ou par
  `localhost` (homelabd, telegraf en `network_mode: host`). Restent ouverts à Internet : 80/443 (NPM), 6881 (BitTorrent),
  81 (admin NPM, **on n'y touche pas**). Le port 8096 de Jellyfin est fermé à Internet depuis le 25/09 (0 paquet direct en
  7 jours ; `127.0.0.1:8096:8096`, `PublishedServerUrl` = `${JELLYFIN_PUBLIC_URL}` ; Jellyfin recréé hors pic le 26/09,
  journal `backups/jellyfin-port-20260925/recreate.log`). Nouveau service : `"127.0.0.1:<port>:<port>"`.
- `cpu_shares` : 512 sur tout service de fond, 2048 pour Jellyfin et NPM (voir
  [lecture-et-transcodage.md](lecture-et-transcodage.md#2-fenêtre-des-tâches-lourdes-et-options-interdites)).
- **Pas de `chown -R /opt/homelab`** : `npm/`, `homarr/` (root), `grafana/` (472), `guacamole/mysql` (999).
- **Arrêter un service volontairement** : l'ajouter à `[tasks.stack_health] ignore` (+ restart homelabd) ou désactiver la
  tâche, sinon `stack_health` le relance dans les 5 min. `guacamole` est en `restart: "no"` exprès (course au démarrage
  avec guacdb : seul compose le lance, après guacdb healthy). Toute opération qui arrête Guacamole ou guacdb doit tolérer
  ces relances.
- `.env` est lu par bash (`.`), compose et dotenvy : pas d'expression shell, guillemets seulement autour des valeurs avec
  espaces.

## 2. VPN, gluetun et qBittorrent

- **Profils compose** : `COMPOSE_PROFILES=vpn|novpn` dans `.env`, changé uniquement par `homelabctl vpn`.
  `gluetun`+`qbittorrent` et `qbittorrent-direct` ne coexistent jamais.
- **Recréer `gluetun` = recréer `qbittorrent`** : qBittorrent est en `network_mode: service:gluetun` ; quand gluetun est
  recréé, compose laisse qbittorrent « Up » **sur l'espace réseau de l'ancien conteneur**, injoignable de partout alors
  que son healthcheck reste vert (11 h de coupure le 19/09). Toujours `docker compose up -d --force-recreate --no-deps
  qbittorrent` après un `up -d` qui a recréé gluetun, puis reposer le port transféré (`/tmp/gluetun/forwarded_port` →
  `setPreferences listen_port` ; le hook ne rejoue pas seul). La sonde qBittorrent de `stack_health` vue de l'hôte le fait
  aussi (`action = "recreate"` + `post_exec`). Après une mise à jour de gluetun : refaire le contrôle de fuite d'adresse.
- Le hook `hooks/qbit-update-port.sh` s'exécute dans l'image gluetun (busybox) : **POSIX sh, `wget` uniquement**.
- **`qBittorrent.conf` : arrêter le conteneur avant d'éditer**, sinon il écrase le fichier à l'arrêt.
- **Ne jamais remettre `172.18.0.0/16` dans `bypass_auth_subnet_whitelist`** (NPM y est : qBittorrent serait public sans
  mot de passe, comme jusqu'au 12/09) : seulement `127.0.0.0/8` et `172.18.0.1/32`. Garder
  `web_ui_reverse_proxy_enabled` (proxies de confiance `172.18.0.0/16`) : sans lui, qBittorrent voit toutes les
  connexions venir de NPM et un ban (5 échecs, 1 h) bloque tout le monde. Lever un ban : redémarrer qbittorrent.
- **File d'attente coupée** sur le VPS et la seedbox (`queueing_enabled = false`) : avec `max_active_uploads` 10, des
  torrents `stalledUP` gardaient les places et des torrents C411 restaient `queuedUP` sans partager.
- **qBittorrent 5.2** nomme son cookie `QBT_SID_<port>` (homelabd l'accepte) ; avant de monter un qBittorrent en 5.2,
  Sonarr ≥ 4.0.18 et Radarr ≥ 6.2.1 (ceux du VPS d'abord ; la seedbox est compatible).
- Règles de ratio par tracker : `[tasks.tracker_ratio]` (C411 illimité sur ses deux domaines).

## 3. Disque

- `disk_pressure` : alerte à `alert_pct` (85), suppression des torrents arrêtés les plus anciens à `hard_pct` (95, les
  liens physiques de `library/media` survivent), journal seulement à `crit_pct` (98).
- **Jamais de purge globale** de file ou de torrents : toute suppression est ciblée et plafonnée
  (`max_actions_per_run`), invariant de `stuck_handler` et `disk_pressure`.
- Tout dossier volumineux sous `/opt/homelab` va dans `[backup] excludes` ([sauvegardes.md](sauvegardes.md)).

## 4. Redémarrage et résilience

- **Redémarrer (VPS ou conteneurs qui coupent la lecture) seulement hors pic, sans lecture en cours** : programmer avec
  `tools/offpeak/offpeak.sh` ([outils-bancs-et-hors-pic.md](outils-bancs-et-hors-pic.md)).
- Au démarrage : `homelab-stack.service` relance compose (`up -d --remove-orphans`), puis homelabd et `stack_health` (passe
  immédiate). Vérifier ensuite `docker compose ps`, `systemctl status homelabd`, `homelabctl status`, et
  `pgrep -cx xfce4-session` = 1 (bureau VNC).
- Audit de résilience du 02/10 : redémarrage propre ou forcé → tout repart (Docker activé, conteneurs
  `unless-stopped`, Guacamole relancé par `stack_health`, montage, homelabd et minuteurs activés) ; panique noyau →
  redémarrage en 10 s (`kernel.panic = 10`, posé par sysctl : une panique **avant** sysctl fige le VPS, il n'y a pas de
  `panic=` sur la ligne de commande). Sonde qBittorrent vue de l'hôte, `seedbox_health`, chien de garde
  `homelabd-watchdog.timer`, homelabd en `Restart=always`. Reste à faire (phase 3, plan du propriétaire) : sauvegarde
  hors du VPS.
- Un redémarrage du VPS est prévu par le lot 3 le lundi 12/10 à 04:10 (`lot3-REBOOT`).

## 5. Bureau VNC (Guacamole, noVNC)

- Unité `systemd/vncserver@.service` (`-fg`, **sans PIDFile** : TigerVNC nomme son PID d'après `hostname -f`) et session
  `systemd/vnc-xstartup` → `~/.config/tigervnc/xstartup` (verrou `flock` par écran, boucle XFCE qui s'arrête avec le
  serveur X). Installer un script par **fichier neuf + `mv`** (bash relit un script en cours d'exécution).
- Le 23/09, l'ancienne unité avait relancé 144 fois au démarrage et laissé 138 sessions XFCE (sauvegarde
  `backups/vnc-20260923/`). Contrôle : `pgrep -cx xfce4-session` = 1.

## 6. Pièges du système

- **coreutils uutils** (Rust, 0.8.0, Ubuntu 26.04) : la forme courte `tail -25 a b` (plusieurs fichiers) échoue
  (« unexpected argument '-2' »). Dans les scripts, toujours `head -n N` et `tail -n N`.
- Un processus lancé depuis un script qui tient un verrou `flock` (fd 9) ou une sortie `tee` hérite du verrou et le garde
  à vie (vu dans une répétition, pas avec systemd ni dockerd).
- **Le SSH du VPS (sshd, mots de passe, clés, fail2ban) est le domaine du propriétaire** : ne jamais le modifier, signaler
  seulement.
