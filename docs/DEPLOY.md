# Déploiement

## Prérequis hôte

- Ubuntu/Debian x86_64, Docker Engine ≥ 24 + plugin compose v2, ~6 vCPU / 16 Go conseillés
  (Jellyfin transcode en logiciel, pas de GPU).
- Utilisateur `deploy` uid/gid 1000, membre du groupe `docker`, sudo.
- Paquets : `curl zstd tar unzip unrar p7zip-full`. Pour compiler : `rustup` + `musl-tools`.
- Un swap de 4 Go en filet (`/swapfile`, `vm.swappiness=10` dans `/etc/sysctl.d/`) : la somme des limites mémoire
  des conteneurs dépasse la RAM.
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
- Prowlarr : sans application liée (les indexers vivent dans les Arrs) ; « C411 » (clé de recherche) et « C411 (2) »
  (`C411_RSS_API_KEY`), `queryLimit` 45 chacun, plus « Nyaa.si » et « World-torrent » (secours pendant une panne de C411,
  `[indexers] fallback*`). Il ne sert qu'aux recherches de homelabd (`series_search`, `movie_search`, `/recherche`).
  Dans les Arrs, C411 est en RSS seulement, avec la seconde clé.
- qBittorrent (conteneur arrêté pour éditer `qBittorrent.conf`) : dispense d'authentification WebUI limitée à
  `127.0.0.0/8` et `172.18.0.1/32` (homelabd depuis l'hôte) — **jamais `172.18.0.0/16`** : NPM est dans ce réseau et
  qBittorrent serait public sans mot de passe ; les Arrs s'authentifient ; `web_ui_reverse_proxy_enabled` avec les
  proxies de confiance `172.18.0.0/16` ; chemin `/downloads`, `Session\IPv6Enabled=false` sous VPN.
- Jellyfin : bibliothèques Films/Séries sur `/media/movies` et `/media/tvshows` ; leurs GUID
  vont dans `.env`. Créer une clé API.
- Jellyseerr : lier Jellyfin, Sonarr, Radarr ; clé API.
- Nouveau service dans `docker-compose.yml` : port publié en `"127.0.0.1:<port>:<port>"`.
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

`homelab.toml` refuse toute clé inconnue : quand il en gagne une, installer homelabd **et** homelabctl dans la foulée
(l'ancien homelabctl échoue, l'ancien homelabd ne redémarrerait plus). `homelabctl install` (appelé par `setup.sh`) pose
aussi `systemd/journald-homelab.conf` ; s'il a changé, `sudo systemctl restart systemd-journald` (ne coupe aucun
service). Après le redémarrage de homelabd : `curl -s 127.0.0.1:8766/health` (version `git describe`), `homelabctl
check` (diun compris), et aucune ligne « docker inspect en échec » dans `journalctl -u homelabd`.

## Profils VPN

`COMPOSE_PROFILES=vpn` (défaut) lance `gluetun` + `qbittorrent` (netns partagé, port
forwarding ProtonVPN poussé dans qBittorrent par `hooks/qbit-update-port.sh`).
`homelabctl vpn off` passe en `novpn` : `qbittorrent-direct` expose 8080/6881 lui-même,
les download clients Sonarr/Radarr et le proxy NPM sont repointés, IPv6 réactivé.
`homelabctl vpn on` fait l'inverse. Ne jamais lancer les deux profils en même temps.

## Déposer un fichier depuis un PC

Filebrowser (`https://filebrowser.<domaine>`, envoi jusqu'à 50 Go sans tampon côté NPM) → glisser
la vidéo dans `downloads/`. Le watcher attend la fin de l'envoi (taille stable), puis Sonarr/Radarr
importent et renomment ; Jellyfin l'affiche aussitôt. Nommer les épisodes `Série - S17E48.ext` :
en numérotation absolue (« … - 48 »), Sonarr peut viser le mauvais épisode (Bleach : absolu 48 =
S03E07). Ne pas déposer directement dans `media/` : le fichier y reste inconnu de Sonarr.

## Seedbox (optionnelle)

Mise en place (déjà faite sur la prod, à refaire sur une nouvelle seedbox) :
1. Sur la seedbox : `app-radarr|sonarr|bazarr|autobrr install -p <mdp>`, `app-unpackerr install` ; catégories qBittorrent `radarr`/`sonarr` ; clés API et mots de passe
   dans le `.env` du VPS (`SEEDBOX_*`).
2. Réglages des Arrs seedbox clonés depuis ceux du VPS (formats personnalisés, profils — vérifier le **nom** du
   profil, les numéros diffèrent d'une machine à l'autre —, C411 déclaré directement en RSS seulement, client
   qBittorrent via le proxy HTTPS). Jackett et FlareSolverr de la seedbox ne servent qu'à la voie russe (RuTracker,
   voir [runbooks/voie-russe.md](runbooks/voie-russe.md)).
3. Clé rclone ajoutée dans `~/.ssh/authorized_keys` de la seedbox avec
   `restrict,command="/usr/lib/openssh/sftp-server -P write,mkdir,rename,…"` (lecture + suppression, aucune écriture :
   le bouton « Supprimer » de Jellyfin doit pouvoir effacer) ; rclone ≥ 1.68 dans `/usr/local/bin` ;
   `user_allow_other` dans `/etc/fuse.conf` ; `mkdir -p /mnt/seedbox/media`.
4. `[seedbox] enabled = true` dans `homelab.toml`, `sudo homelabctl install` (active
   `homelab-seedbox-mount.service`) ; Jellyfin : ajouter `/seedbox/media/Movies` comme second dossier
   de « Films » et `/seedbox/media/TV Shows` à « Séries » (Tableau de bord → Bibliothèques → Gérer
   les dossiers) ; `JELLYFIN_LIB_EXTRA` reste vide ; `[seedbox] qbit_url`/`qbit_user` +
   `SEEDBOX_QBIT_PASSWORD` pour `torrent_import`.
5. Jellyseerr : Radarr/Sonarr seedbox en serveurs par défaut, en `preventSearch` ; bibliothèques activées via
   `…/settings/jellyfin/library?enable=<ids de toutes les bibliothèques>` (Seerr 3.2 : **jamais** `sync=true` seul, et le
   `GET` sans paramètre désactive lui aussi tout ; lire l'état par `GET /api/v1/settings/jellyfin`). Le lot 3 prépare
   Seerr 3.5.0, qui change cette API (`backups/lot3-20261008/NOTES-DOC.txt`).

Vérifier : `homelabctl check` (Arrs seedbox + montage), `systemctl status homelab-seedbox-mount`.

### Couper la seedbox (~10 min, sans impact sur le reste)

1. Jellyseerr → Settings → Services : remettre Radarr/Sonarr du VPS **par défaut**, supprimer ceux
   de la seedbox (les demandes en cours restent visibles).
2. `homelab.toml` : `[seedbox] enabled = false` → `sudo systemctl restart homelabd`.
3. `sudo systemctl disable --now homelab-seedbox-mount`.
4. Jellyfin : retirer les dossiers `/seedbox/media/Movies` de « Films » et `/seedbox/media/TV Shows`
   de « Séries », puis scanner (les titres venant de la seedbox disparaissent) ; optionnel : retirer
   la ligne `/mnt/seedbox:/seedbox` du service jellyfin.
Les bibliothèques et le pipeline du VPS ne sont jamais touchés par ces étapes.

## Sauvegarde et restauration

`sudo homelabctl backup` (et le timer `homelab-backup.timer`, dimanche 04:30 + jusqu'à 15 min) produit dans
`backups/` (700) : `homelab-state-<ts>.tar.zst` (tout `/opt/homelab` hors `[backup] excludes` : `library/`,
`influxdb/`, caches, journaux, vignettes de défilement et photos d'acteurs de Jellyfin ; ~3 Go), `.sha256`,
`.list.gz` (manifeste), `guacdb-<ts>.sql.gz`, `systemd-<ts>.tar.gz` (unités, crontab, compose rendu) et
`images-<ts>.txt`. Les bases SQLite des services en marche (`[backup] sqlite` : homelabd, Jellyfin et ses
extensions, Jellyseerr, Arrs, Prowlarr, Homarr, Grafana, NPM, pyLoad) sont d'abord copiées par l'API de sauvegarde
SQLite (lecture seule, ~2 s, ~120 Mo, `quick_check` de chaque copie, propriétaire et droits gardés) dans
`state/backup-snapshots/<chemin d'origine>` ; la base vivante et ses `-wal`/`-shm`/`-journal` sortent de l'archive (un
tar les lisait à des instants différents). Une base qui ne se copie pas reste dans l'archive telle quelle et
`homelabctl backup` l'écrit en avertissement. L'archive est relue (`zstd -t`) avant qu'on supprime les anciennes ; les
4 dernières sont gardées. Tout nouveau dossier volumineux sous `/opt/homelab` doit rejoindre `[backup] excludes`, toute
nouvelle base SQLite d'un service `[backup] sqlite`. Les sauvegardes restent sur le même disque (copie hors site à l'étude).
`library/` (médias, téléchargements) n'est pas sauvegardé : trop gros, re-téléchargeable.

Restaurer un service (les bases SQLite sont dans l'archive sous `homelab/state/backup-snapshots/`, pas à leur place :
les remettre service arrêté, après avoir retiré les `-wal`/`-shm` de la base actuelle, sinon SQLite les rejouerait sur
la copie) :
```bash
docker compose stop sonarr
sudo tar -xpf backups/homelab-state-<ts>.tar.zst -C /opt --numeric-owner homelab/sonarr
sudo tar -xpf backups/homelab-state-<ts>.tar.zst -C /tmp --numeric-owner homelab/state/backup-snapshots/sonarr
for db in sonarr.db logs.db; do
  sudo rm -f "sonarr/config/$db-wal" "sonarr/config/$db-shm"
  sudo cp -p "/tmp/homelab/state/backup-snapshots/sonarr/config/$db" "sonarr/config/$db"
done
docker compose start sonarr
sudo rm -rf /tmp/homelab
```
Restauration complète (tous les services arrêtés, archive entière extraite dans `/opt`) : aucune base n'est alors à sa
place, toutes sont sous `state/backup-snapshots/`. Les remettre puis retirer le dossier (et `homelab/state` seul = la
même boucle limitée à `state/backup-snapshots/state`) :
```bash
cd /opt/homelab && sudo find state/backup-snapshots -type f | while read -r f; do
  d=${f#state/backup-snapshots/}; sudo rm -f "$d-wal" "$d-shm" "$d-journal"; sudo cp -p "$f" "$d"
done && sudo rm -rf state/backup-snapshots
```
Guacamole : `zcat backups/guacdb-<ts>.sql.gz | docker exec -i guacdb mysql -uroot -p"$MYSQL_ROOT_PASSWORD"`.

Bases de homelabd restaurées d'une date antérieure :
- `state/chat.db` est migrée à l'ouverture (salon Discussion fusionné dans Entraide, 08/10) : une copie d'avant est
  rejouée par la même migration, idempotente.
- `state/subscriptions.db` a gagné la colonne `due_noted` le 08/10 : un ancien binaire la lit sans erreur (liste de
  colonnes explicite) mais reprendrait les rappels et les suspensions des abonnés gérés à la main.
- Un état `state/homelabd.json` ancien est relu par le nouveau binaire (champs `serde(default)`).

## Retour arrière

- Compose : `git log` → `git checkout <commit> -- docker-compose.yml` → `docker compose up -d`.
  Les images restent en cache local ; les digests garantissent l'identité.
- homelabd : réinstaller le binaire précédent (garder une copie de `/usr/local/bin/homelab{d,ctl}` avant chaque
  installation ; aucune release GitHub n'est publiée à ce jour) et `systemctl restart homelabd`, ou
  `systemctl stop homelabd` — les services Docker n'en dépendent pas. Un ancien binaire refuse un `homelab.toml` qui a
  gagné des clés (`deny_unknown_fields`) : retirer d'abord ces clés.
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
mises à jour du noyau (`apt` le signale), **hors pic et sans lecture en cours** (programmer par
`tools/offpeak/offpeak.sh --as root`), puis vérifier comme ci-dessus et `pgrep -cx xfce4-session` = 1. Le lot 3 prévoit un
redémarrage le 12/10 à 04:10 (`lot3-REBOOT`), avec sa liste de contrôle `backups/lot3-20261008/reboot/check.sh`.
