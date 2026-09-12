# CLAUDE.md

Guide pour Claude Code dans ce dépôt. Lire aussi ARCHITECTURE.md et AUTOMATION.md.

## Ce qu'est ce dépôt

`/opt/homelab` est **à la fois** le dépôt git (branche `main`, remote `HaradasCYB/groscailloux-homelab`)
et le répertoire de production : compose, config et code Rust sont versionnés ; l'état des
services (`<service>/`), `library/`, `backups/`, `state/`, `logs/` et `.env` sont ignorés par
git. « Déployer » = `docker compose up -d` pour les conteneurs, `cargo build` + `install` +
`systemctl restart homelabd` pour l'automatisation (`sudo ./setup.sh` fait tout).

## Commandes

```bash
docker compose config --quiet             # TOUJOURS avant un up -d
docker compose up -d [svc]                # recrée seulement ce qui a changé
docker compose ps ; docker compose logs -f <svc>

cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test
cargo build --release --target x86_64-unknown-linux-musl -j4      # laisser 2 vCPU à Jellyfin
sudo install target/x86_64-unknown-linux-musl/release/homelab{d,ctl} /usr/local/bin/ && sudo systemctl restart homelabd

homelabctl check | list | status | run <task> --dry-run | onboard | vpn | backup
journalctl -u homelabd -f
```

## Règles

- **Secrets** : uniquement dans `.env`. Ne jamais mettre une valeur en dur dans compose, TOML,
  code, scripts ou docs ; ne jamais coller `.env`, `backups/` ou une config de service dans un
  outil externe. Les scripts `scripts/*.sh` restants sourcent `.env`.
- **Images** : pinnées `tag@sha256`. Pour mettre à jour : nouveau tag + digest (`docker pull`
  puis `docker image inspect --format '{{index .RepoDigests 0}}'`), `up -d <svc>`, mettre
  `diun/images.yml` en cohérence. Pas de `:latest` nu.
- **Pas de `chown -R /opt/homelab`** : npm/, homarr/ (root), grafana/ (472), guacamole/mysql (999).
- **qBittorrent.conf** : arrêter le conteneur avant d'éditer, sinon il écrase le fichier.
- **Jamais de purge globale** de queue ou de torrents : toute suppression est ciblée et
  plafonnée (`max_actions_per_run`), c'est un invariant des tâches `stuck_handler`/`disk_pressure`.
- **Sonarr** : profil 6 `minFormatScore=-9999` (FR d'abord, VOSTFR toléré) ; C411 en
  interactif seulement. **Jellyfin** : pas de GPU, préférer x264 à HEVC.
- **Arrêter un service volontairement** : l'ajouter à `tasks.stack_health.ignore` dans
  `homelab.toml` (+ restart homelabd) ou désactiver la tâche, sinon `stack_health` le relance
  dans les 5 min. `guacamole` est en `restart: "no"` exprès (course au boot avec guacdb).
- **Profils compose** : `COMPOSE_PROFILES=vpn|novpn` dans `.env`, changé uniquement par
  `homelabctl vpn`. `gluetun`+`qbittorrent` et `qbittorrent-direct` ne coexistent jamais.
- **Nouvelle tâche** : un module dans `crates/homelab-core/src/tasks/`, `impl Task`, ajout dans
  `registry()`, section `[tasks.<nom>]` dans `config.rs` + `homelab.toml`, dry-run respecté,
  tests unitaires de la décision, paragraphe dans AUTOMATION.md.
- **Changement de comportement** = changement de `homelab.toml` (seuils, intervalles) avant
  changement de code. Les valeurs par défaut du code doivent rester égales à celles du TOML.
- **Reboot** : `homelab-stack.service` relance compose ; vérifier `docker compose ps` et
  `systemctl status homelabd` après.
- Les anciens scripts bash de `scripts/` ne sont plus planifiés ; ils restent comme référence
  jusqu'à suppression et ne doivent pas être relancés en parallèle de homelabd hors dry-run.

## Seedbox

- Accès admin : `ssh seedbox` (clé `~/.ssh/seedbox_ed25519`) ; apps via `app-<x> …`, en conteneurs
  Docker sur la seedbox (Radarr 16127, Sonarr 16126, Jackett 16129, FlareSolverr 16111 sur
  `172.17.0.1`), qBittorrent natif `127.0.0.1:16141`, autobrr natif `127.0.0.1:16123`. API
  publiques : `https://kakaouette.tofino.usbx.me/<app>`.
- Montage : rclone dans **`/mnt/seedbox/media`**, Jellyfin lie le **parent** `/mnt/seedbox`
  (rslave). Lier le point de montage FUSE lui-même casse la reprise après coupure.
- Nouvelles demandes Jellyseerr → Arrs seedbox (id 1). Ne rien importer côté seedbox qui existe
  déjà sur le VPS (doublons dans Jellyfin).
- Jellyseerr : ne jamais appeler `settings/jellyfin/library?sync=true` sans renvoyer `?enable=`
  avec la liste complète des bibliothèques.

## Pièges connus

- `.env` est lu par bash (`.` ), compose et dotenvy : pas d'expression shell, guillemets seulement
  autour des valeurs avec espaces.
- Le hook `hooks/qbit-update-port.sh` s'exécute dans l'image gluetun (busybox) : POSIX sh,
  `wget` uniquement.
- `GET /api/v3/manualimport` de Sonarr dure ~25 s sur un gros `/downloads` (timeout 5 min).
- Prowlarr n'a aucune application configurée : les indexers vivent dans Sonarr/Radarr et les
  publics passent par **Jackett** (+ FlareSolverr pour Cloudflare). Avant de retirer un service,
  vérifier qui l'appelle : `grep -r <nom>:<port>` dans les configs et les champs `baseUrl` des
  indexers Arr (`GET /api/v3/indexer`) — le retrait de Jackett/FlareSolverr le 2026-09-10 a coupé
  les indexers publics pendant deux jours.
- L'UI d'onboarding est sur l'hôte (8766) ; NPM doit cibler `172.18.0.1:8766`, pas un conteneur.
