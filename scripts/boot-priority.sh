#!/usr/bin/env bash
# Au boot, démarre Guacamole stack en priorité, puis force tous les autres services
# Activé via /etc/systemd/system/homelab-boot-priority.service
set -e
LOG=/var/log/homelab-boot-priority.log
echo "[$(date -Iseconds)] start" >> "$LOG"

# Attendre que Docker soit prêt (max 30s)
for i in {1..30}; do docker info >/dev/null 2>&1 && break; sleep 1; done

# Démarrage prioritaire de la stack Guacamole : DB → daemon → web frontend
echo "[$(date -Iseconds)] starting guacdb..." >> "$LOG"
docker start guacdb 2>&1 | tee -a "$LOG" || true
sleep 4
echo "[$(date -Iseconds)] starting guacd..." >> "$LOG"
docker start guacd 2>&1 | tee -a "$LOG" || true
sleep 2
echo "[$(date -Iseconds)] starting guacamole..." >> "$LOG"
docker start guacamole 2>&1 | tee -a "$LOG" || true
sleep 5

# Filet de sécurité : compose up -d pour TOUS les services (idempotent, no-op si déjà up)
echo "[$(date -Iseconds)] compose up -d (all services)..." >> "$LOG"
docker compose -f /opt/homelab/docker-compose.yml up -d 2>&1 | tee -a "$LOG"

echo "[$(date -Iseconds)] done" >> "$LOG"
