#!/bin/bash
# Auto-cleanup homelab — vise à être lancé quotidiennement.
# Pour activer:
#   sudo ln -s /opt/homelab/scripts/homelab-cleanup.sh /etc/cron.daily/homelab-cleanup
#   sudo chmod +x /opt/homelab/scripts/homelab-cleanup.sh
LOG=/opt/homelab/logs/cron-cleanup.log
mkdir -p /opt/homelab/logs
exec >> "$LOG" 2>&1
echo "=== $(date -u) ==="

# Jellyfin transcode cache (segments > 24h — filet de sécurité au-delà de la suppression Jellyfin)
find /opt/homelab/jellyfin/cache/transcodes -maxdepth 1 -type f -mtime +1 -delete 2>/dev/null

# Dossiers downloads vides post-import (>2 jours)
find /opt/homelab/downloads -mindepth 1 -maxdepth 3 -type d -empty -mtime +2 -delete 2>/dev/null

# Recycle bins Sonarr/Radarr (>14 jours)
find /opt/homelab/media/movies/.recycle -type f -mtime +14 -delete 2>/dev/null
find /opt/homelab/media/tvshows/.recycle -type f -mtime +14 -delete 2>/dev/null

# Logs auto-import obsolètes
find /opt/homelab/logs -type f -name '*.log' -size +50M -mtime +7 -delete 2>/dev/null

# Anciens audit-backups (>30 jours)
find /opt/homelab/.audit-backup-* -maxdepth 0 -mtime +30 -exec rm -rf {} \; 2>/dev/null

echo "done"
