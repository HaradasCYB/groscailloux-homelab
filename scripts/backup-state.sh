#!/bin/bash
# backup-state.sh — snapshot à chaud de l'état homelab (hors médias, hors métriques)
# Usage: sudo ./scripts/backup-state.sh
set -euo pipefail

BASE=/opt/homelab
BACKUP_DIR=$BASE/backups
TS=$(date +%Y%m%d-%H%M%S)
OUT=$BACKUP_DIR/homelab-state-$TS.tar.zst

[ "$EUID" -eq 0 ] || { echo "run with sudo (root-owned dirs: npm/letsencrypt, homarr, grafana)" >&2; exit 1; }

mkdir -p "$BACKUP_DIR"
chown 1000:1000 "$BACKUP_DIR"

echo "==> guacdb mysqldump"
MYSQL_ROOT_PASSWORD=$(docker exec guacdb printenv MYSQL_ROOT_PASSWORD)
docker exec -e MYSQL_PWD="$MYSQL_ROOT_PASSWORD" guacdb \
  mysqldump --all-databases --single-transaction --routines --triggers \
  | gzip -6 > "$BACKUP_DIR/guacdb-$TS.sql.gz"

echo "==> tar state → $OUT"
# tar exit 1 = "file changed as we read it" on a live system; only exit 2 is fatal
set +o pipefail
tar --numeric-owner --warning=no-file-changed -cpf - -C /opt \
  --exclude=homelab/library \
  --exclude=homelab/backups \
  --exclude=homelab/influxdb \
  --exclude=homelab/jellyfin/cache \
  --exclude=homelab/jellyfin/config/log \
  --exclude='homelab/jellyfin-backup-*' \
  --exclude='homelab/jellyseerr-backup-*' \
  --exclude='homelab/*.bak-*' \
  --exclude=homelab/target \
  homelab \
  | zstd -T4 -3 -q -f -o "$OUT"
TAR_RC=${PIPESTATUS[0]}
set -o pipefail
[ "$TAR_RC" -le 1 ] || { echo "tar failed (rc=$TAR_RC)" >&2; exit "$TAR_RC"; }

echo "==> checksum + manifest"
( cd "$BACKUP_DIR" && sha256sum "$(basename "$OUT")" > "$OUT.sha256" )
zstd -dc "$OUT" | tar -tvf - | gzip -6 > "$OUT.list.gz"

echo "==> systemd units, crontab, rendered compose"
SYS_TMP=$(mktemp -d)
trap 'rm -rf "$SYS_TMP"' EXIT
mkdir -p "$SYS_TMP/systemd" "$SYS_TMP/cron"
cp /etc/systemd/system/{auto-import,homelab-boot-priority,jellyseerr-*,qbit-*,tba-*}.* "$SYS_TMP/systemd/" 2>/dev/null || true
cp -L /etc/cron.daily/homelab-cleanup "$SYS_TMP/cron/homelab-cleanup" 2>/dev/null || true
crontab -u deploy -l > "$SYS_TMP/cron/deploy.crontab" 2>/dev/null || true
docker compose -f "$BASE/docker-compose.yml" config > "$SYS_TMP/compose-rendered.yml"
tar -czf "$BACKUP_DIR/systemd-$TS.tar.gz" -C "$SYS_TMP" .

echo "==> docker images/containers reference"
docker images --digests --no-trunc > "$BACKUP_DIR/images-$TS.txt"
docker inspect $(docker ps -aq) > "$BACKUP_DIR/containers-$TS.json"

chmod 600 "$BACKUP_DIR"/*-"$TS"*
chown 1000:1000 "$BACKUP_DIR"/*-"$TS"*

echo "==> done"
ls -lh "$BACKUP_DIR"/*-"$TS"*
