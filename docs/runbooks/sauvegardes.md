# Sauvegardes

À lire avant d'ajouter un dossier volumineux ou une base SQLite sous `/opt/homelab`, de toucher à `backups/` ou de
restaurer. Commandes de sauvegarde et de restauration : [DEPLOY.md, Sauvegarde et restauration](../DEPLOY.md#sauvegarde-et-restauration).

## Règles

- **Sauvegarde d'état** = `sudo homelabctl backup`, lancée par `homelab-backup.service` (root, minuteur le dimanche à
  04:30 + jusqu'à 15 min, `systemd/homelab-backup.timer`), pas par homelabd ; `backup_watch` alerte si la dernière archive
  a plus de 8 jours ; `[backup] keep_last` = 4 archives.
- **Tout dossier volumineux sous `/opt/homelab` doit être dans `[backup] excludes`** : le 20/09, `cache/rclone` (fichiers
  creux de plusieurs centaines de Go) a produit une archive de 149 Go au lieu de 3 et poussé le disque à 92 %. Après un
  nouveau dossier de cache ou de données massives : l'ajouter, puis contrôler la taille de l'archive suivante et son
  manifeste (`.list.gz`).
- **Bases SQLite des services** (`[backup] sqlite`) : chacune est copiée par l'API de sauvegarde SQLite (rusqlite
  `Backup`, lecture seule, `quick_check`) dans `state/backup-snapshots/<chemin>` ; la base vivante (+ `-wal`, `-shm`,
  `-journal`) sort de l'archive. **Nouvelle base d'un service = l'ajouter à la liste.** À la restauration, les bases sont
  sous `state/backup-snapshots/`, pas à leur place.
- **`backups/` : rien de lisible par « autres »** (07/10 : 48 480 entrées corrigées). root y crée en 644, donc après un
  lot : `sudo find /opt/homelab/backups -perm -o+r ! -type l -exec chmod o-rwx {} +`. Un dossier qui contient des secrets
  (copies de bases, configurations) : 700/600.
- `backups/` et l'archive `homelab-state-*.tar.zst` contiennent `.env` et les configurations des services : les traiter
  comme des secrets, **ne jamais les coller dans un outil externe**.
- **Une instance d'essai (Jellyfin…) ne reste jamais en marche après son banc, et sa copie de base part avec elle**
  (`docker rm -f <nom> && sudo rm -rf /var/tmp/<nom>`, ou 700 si elle doit rester) : une instance d'essai a gardé jusqu'au
  07/10 une copie de la base de production (49 jetons d'appareil valides) en 0644.
- Les sauvegardes restent sur le même disque ; une copie hors du VPS est au plan du propriétaire (phase 3 de l'audit de
  résilience), à ne pas lancer sans lui.
- `state/chat.db` est migrée à l'ouverture (fusion Discussion → Entraide, idempotente) ; `state/subscriptions.db` a gagné
  la colonne `due_noted` le 08/10 (un ancien binaire la lit sans erreur).
- Historique de lecture : Playback Reporting en conservation illimitée (`MaxDataAge = -1`, voir
  [jellyfin-serveur-et-extensions.md](jellyfin-serveur-et-extensions.md#3-bibliothèques-et-analyses)).
