# CLAUDE.md

Guide pour Claude Code dans ce dépôt. Lire aussi ARCHITECTURE.md et AUTOMATION.md.

## Ce qu'est ce dépôt

`/opt/homelab` est **à la fois** le dépôt git (branche `v2`, remote `HaradasCYB/groscailloux-homelab`)
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

## Pièges connus

- `.env` est lu par bash (`.` ), compose et dotenvy : pas d'expression shell, guillemets seulement
  autour des valeurs avec espaces.
- Le hook `hooks/qbit-update-port.sh` s'exécute dans l'image gluetun (busybox) : POSIX sh,
  `wget` uniquement.
- `GET /api/v3/manualimport` de Sonarr dure ~25 s sur un gros `/downloads` (timeout 5 min).
- Prowlarr n'a aucune application configurée : les indexers vivent dans Sonarr/Radarr.
- L'UI d'onboarding est sur l'hôte (8766) ; NPM doit cibler `172.18.0.1:8766`, pas un conteneur.
