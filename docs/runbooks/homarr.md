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
  « Operations »). **L'autorisation est par intégration, pas par widget** : le 14/10, le widget « Nouveautés » du tableau
  public, relié à l'intégration Jellyfin, ouvrait aux anonymes `widget.mediaServer.getCurrentStreams` (sessions en cours,
  pseudos, appareils, titres). Lien retiré (`integration_item`, sauvegarde `backups/domaine-20261014/homarr-db-avant-r1.sqlite`) :
  le widget reste, vide. **Aucune intégration Jellyfin, Seerr ou de téléchargement reliée à un widget du tableau public** ;
  seul le calendrier (Sonarr, Radarr) y reste. Contrôles anonymes → 403 : `widget.mediaRequests.getLatestRequests` et
  `widget.mediaServer.getCurrentStreams` (entrée `{"json":{"integrationIds":["<id Jellyfin>"],"showOnlyPlaying":false}}`).
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

## Tuile « Liste rouge » (tableau privé « Operations »)

Tuile vers la page d'admin de la liste rouge, **jamais sur le tableau public**. Procédure (règles ci-dessus : Homarr
arrêté, base sauvegardée, tout en une commande) :

1. Arrêt de Homarr, sauvegarde `backups/homarr-db-<date>-liste-rouge.sqlite` (`chmod o-rwx`).
2. Script modelé sur `backups/lot1-20261007/homarr/apply.py` : dans la section « Accès & docs », copier l'app
   « Comptes » (même hôte, même `ping_url`) en une app « Liste rouge » dont le `href` finit par `/accounts/liste-rouge`,
   puis créer l'item et ses 3 `item_layout` (un par taille d'écran) à côté de « Comptes ». Garder un `rollback.py` qui
   retire l'app, l'item et ses positions.
3. Démarrage de Homarr ; contrôler la tuile connecté (elle ouvre la page, l'auth NPM « admin-outils » est demandée comme
   pour « Comptes ») et l'absence de la tuile sur le tableau public.

## Lot 3 (en cours jusqu'au 12/10)

Le lot 3 prépare Homarr 1.77.2 (`backups/lot3-20261008/homarr/`) : plus de tâches de fond par widget, télémétrie coupée,
cookie de session renommé (l'admin devra se reconnecter), retour arrière = base **et** `redis/dump.rdb`. Reporter ici ce
que dit `backups/lot3-20261008/NOTES-DOC.txt` après son exécution (la ligne `cron_job_configuration` « downloads »
deviendra sans effet ; restreindre le motif diun de Homarr à `^v1\.\d+\.\d+$`).
