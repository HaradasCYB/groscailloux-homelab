# Homarr

À lire avant de modifier Homarr (tableaux, widgets, intégrations, base). Rôle des deux tableaux :
[ARCHITECTURE.md](../ARCHITECTURE.md#services).

## Règles

- **Modifier la base Homarr arrêté, après sauvegarde.** `homarr/db` appartient à root (d'où `sudo`) ; enchaîner arrêt,
  sauvegarde, modification et démarrage **en une commande** : `stack_health` relance Homarr sous 5 min. Ne jamais
  afficher les colonnes `password`.
- **Titres de section ≤ 20 caractères** (sinon le tableau ne se charge plus).
- Secrets d'intégration chiffrés AES-256-CBC avec `SECRET_ENCRYPTION_KEY` (ne sert qu'à Homarr). Rotation de la clé
  Jellyseerr : `scripts/jellyseerr-rotate-key.py` s'en charge (Homarr arrêté, base sauvegardée).
- Pings des outils protégés par NPM en URL interne (`http://sonarr:8989/ping`…).
- **Tableau public** (`isPublic`) : Homarr sert aux anonymes les données de **tout** widget lié à une intégration (tRPC,
  sans cookie). **Aucun widget de demandes, d'utilisateurs ou de sessions sur le tableau public** : « Demandes récentes »
  y exposait les 20 dernières demandes avec le pseudo des demandeurs (déplacé le 07/10 sur le tableau privé
  « Operations »). Contrôle : `GET` anonyme de `widget.mediaRequests.getLatestRequests` → 403. Calendriers et Nouveautés
  y restent (contenu non vérifié).
- Le widget « Releases » affiche la dernière version publiée d'un logiciel, pas celle qui tourne.

## Réglages notables

- **Widget « Téléchargements »** : `limitPerIntegration` est appliqué **côté serveur, avant** le filtre « masquer les
  terminés » du client ; à 10, les 10 torrents envoyés étaient tous terminés et le widget restait vide. Passé à 500
  (options de l'item `83gkiwxp5m1hwbu9iymgj53g`, sauvegarde `backups/homarr-db-20260919-165303-downloads-limit.sqlite`).
- **Cadence des tâches de fond** (Homarr 1.59) : table `cron_job_configuration`, lue au démarrage ; « downloads » passée de
  5 s à `* * * * *` le 07/10 (une session WebAPI qBittorrent par passage, ~17 000 par jour) ; retour = supprimer la ligne
  et redémarrer Homarr.
- **Session de test** : `HOMARR_ADMIN_PASSWORD` (compte admin dédié) par `/api/auth/callback/credentials`, puis
  `/api/auth/signout`. Le compte propriétaire existe aussi.

## Lot 3 (en cours jusqu'au 12/10)

Le lot 3 prépare Homarr 1.77.2 (`backups/lot3-20261008/homarr/`) : plus de tâches de fond par widget, télémétrie coupée,
cookie de session renommé (l'admin devra se reconnecter), retour arrière = base **et** `redis/dump.rdb`. Reporter ici ce
que dit `backups/lot3-20261008/NOTES-DOC.txt` après son exécution (la ligne `cron_job_configuration` « downloads »
deviendra sans effet ; restreindre le motif diun de Homarr à `^v1\.\d+\.\d+$`).
