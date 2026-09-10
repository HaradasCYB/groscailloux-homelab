# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this repo is

A single-host self-hosted "homelab" stack defined entirely by `docker-compose.yml` plus a couple of helper shell scripts. Not a git repo — changes are made in place on the host (`/opt/homelab`) and applied with `docker compose`. There is no build step, test suite, or CI; "deploy" means restarting containers.

## Common commands

```bash
# Apply changes after editing docker-compose.yml
docker compose up -d                      # recreate only changed services
docker compose up -d --force-recreate <svc>

# Inspect
docker compose ps
docker compose logs -f <service>          # e.g. radarr, jellyfin, npm
docker exec -it <service> sh

# Pull newer images for the :latest tags
docker compose pull && docker compose up -d

# Auto-import watcher (runs continuously, intended to be backgrounded / under systemd)
./auto-import.sh                          # tails its log at /opt/homelab/logs/auto-import.log

# Force a Jellyfin library rescan
./refresh-jellyfin.sh
```

There are no lint/test commands — config is YAML and bash. When editing `docker-compose.yml`, validate with `docker compose config` before `up -d`.

## Architecture — how the services fit together

All ~20 services share one user-defined bridge network named `homelab`, so services address each other by container name on their internal port (e.g. `http://radarr:7878`, `http://influxdb:8086`). Host port mappings exist for the UIs but **inter-service URLs in configs must use the container name**, not `localhost` or the host IP.

Three pipelines run on top of this network:

**1. Media acquisition → playback.** Jellyseerr (5055) takes user requests and hands them to Radarr (7878, movies) / Sonarr (8989, TV). Those query Prowlarr (9696) — the unified indexer manager that also feeds Jackett (9117) — and dispatch downloads to qBittorrent (8080) or pyLoad (8000). Flaresolverr (8191) sits behind Prowlarr to bypass Cloudflare on indexer scrapes. All downloaders write to a single shared volume `/opt/homelab/downloads` (mounted into qbittorrent, pyload, radarr, sonarr at `/downloads`). Radarr/Sonarr move completed files into `/opt/homelab/media/{movies,tvshows}`, which Jellyfin (8096) serves read-only from `/media`.

**2. Auto-import bridge.** `auto-import.sh` is the glue between *direct* downloads (pyLoad, manual drops) and the Arr stack. It `inotifywait`s `/opt/homelab/downloads`, pattern-matches filenames against `S\d+E\d+` to classify movie vs. series, and POSTs `DownloadedMoviesScan` / `DownloadedEpisodesScan` to Radarr/Sonarr's `/api/v3/command` endpoints. **API keys are hardcoded in the script** — if you rotate them in the Arr UIs you must update `auto-import.sh` too. The script runs on the host (not in a container) and reaches the Arrs via the container names `radarr`/`sonarr`, which only resolve from inside the `homelab` network — so it's expected to be run via `docker compose run` or with the host's resolver pointed at Docker's embedded DNS, **or** the URLs need to change to `http://localhost:7878` / `:8989`. Check how it's actually launched before assuming it works as-is.

**3. Observability.** Telegraf collects host + Docker metrics (it bind-mounts `/`, `/var/run/docker.sock`, and `HOST_PROC=/hostfs/proc`) and writes to InfluxDB v2 (8086, org `homelab`, bucket `metrics`). Grafana (3000) reads from InfluxDB. The Telegraf → InfluxDB token is in `telegraf/etc/telegraf.conf`; rotating the InfluxDB token requires updating that file and restarting Telegraf.

**Edge / access.** Nginx Proxy Manager (`npm`, ports 80/81/443) is the public reverse proxy and TLS terminator (Let's Encrypt data lives in `npm/letsencrypt`). DuckDNS keeps `groscaillouxmovie.duckdns.org` pointed at the host. A `cloudflared` directory exists but is currently empty — Cloudflare Tunnel is not active. Guacamole stack (guacamole + guacd + guacdb MySQL) on 8081 provides browser-based remote desktop. Homarr (7575) is the dashboard; Portainer (9000) is the Docker UI.

## State and persistence layout

Each service's mutable state lives in `/opt/homelab/<service>/` (usually `config/`) and is bind-mounted, **not** in named Docker volumes. This means:
- Backups = `tar` of `/opt/homelab/<service>/`. There's a `jellyseerr-backup-20260501/` showing the convention.
- A `docker compose down -v` will *not* delete service state (no named volumes), but `rm -rf` of a service dir will.
- File ownership matters: most LinuxServer.io images run as `PUID=1000:PGID=1000` (the `deploy` user); Grafana's dir is owned by uid `472`; Jellyfin runs as `1000:1000` directly. Don't `chown -R` blindly across `/opt/homelab`.

## Conventions and gotchas

- **`SECRET_ENCRYPTION_KEY` is set on every service** with the same value. It's only meaningful to Jellyseerr and Homarr; for the others it's harmless noise. Don't treat its presence as evidence a service uses it.
- **`TZ=Europe/Paris` appears twice** in many service blocks — duplicate env keys, the second wins, both are the same value, so it's cosmetic. Safe to dedupe when touching a block.
- **Secrets are committed in plaintext** in `docker-compose.yml` and `auto-import.sh` (InfluxDB password, Grafana admin, MySQL passwords, DuckDNS token, Arr API keys). Treat the whole working tree as sensitive; don't paste it into external tools.
- **No `version:` key** in the compose file — relies on Compose v2 defaults. Don't add one back; modern Compose warns about it.
- The `jellyseerr` image `ghcr.io/seerr-team/seerr:latest` is unusual (the canonical image is `fallenbagel/jellyseerr`). If it fails to pull, that's likely why — verify before swapping.
