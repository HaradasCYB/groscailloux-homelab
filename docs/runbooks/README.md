# Runbooks

Un runbook par domaine : les règles et leur pourquoi, les procédures, les pièges et les décisions en vigueur. Les
**interdits** tiennent en une ligne chacun dans [CLAUDE.md](../../CLAUDE.md#3-interdits-et-règles-dures) (chargé à chaque
session) ; ce dossier donne le détail, à lire **avant de toucher le domaine**. Le fonctionnement de chaque tâche de
homelabd reste dans [AUTOMATION.md](../AUTOMATION.md) ; l'architecture dans [ARCHITECTURE.md](../ARCHITECTURE.md) et
[INFRA.md](../INFRA.md) ; le déploiement et la restauration dans [DEPLOY.md](../DEPLOY.md) ; les variables de `.env`
dans [SECRETS.md](../SECRETS.md).

| Runbook | Domaine |
| --- | --- |
| [arrs-et-indexeurs.md](arrs-et-indexeurs.md) | Sonarr, Radarr, Prowlarr, C411 et budget, secours, profils, choix des releases, packs et numérotations, imports, Jellyseerr, `/recherche`, autobrr |
| [voie-russe.md](voie-russe.md) | RuTracker, tag `russe`, `russian_search`, bibliothèques russes, choix dans Mon compte |
| [seedbox-et-rclone.md](seedbox-et-rclone.md) | accès, applis et relance, quota, montage rclone, déménagement, suppression, ménage, remplacement de fichiers |
| [lecture-et-transcodage.md](lecture-et-transcodage.md) | limites mesurées, fenêtre des tâches lourdes, tmpfs, diagnostic, Chromecast, AirPlay, télés, langues, sous-titres, `original_language` |
| [jellyfin-serveur-et-extensions.md](jellyfin-serveur-et-extensions.md) | version 12.1, réseau, bibliothèques et analyses, identification, extensions, Intro Skipper, journaux |
| [jellyfin-interface.md](jellyfin-interface.md) | calque, scripts injectés, interface 12.1, mobile, télé, langue d'affichage, « Lire sur », SyncPlay, websocket |
| [comptes-et-abonnements.md](comptes-et-abonnements.md) | comptes premium, onboarding et mails, PayPal (live), abonnés gérés à la main, Mon compte |
| [tchat.md](tchat.md) | « Aide et annonces » : chemin, salons, non-lus, sondages, tests |
| [npm.md](npm.md) | règles NPM, configuration avancée de l'hôte 1 (SyncPlay, tchat, Chromecast, garde HSS, gzip), site par défaut |
| [homarr.md](homarr.md) | base, tableau public, widgets |
| [homelabd.md](homelabd.md) | code et configuration, construction et installation, état et CLI, alertes, Discord, pages d'admin |
| [vps-docker-et-systeme.md](vps-docker-et-systeme.md) | compose, images et diun, VPN et qBittorrent, disque, redémarrage et résilience, bureau VNC |
| [sauvegardes.md](sauvegardes.md) | sauvegarde d'état, bases SQLite, droits de `backups/`, instances d'essai |
| [outils-bancs-et-hors-pic.md](outils-bancs-et-hors-pic.md) | `tools/` (hors pic, bancs, tests), inventaire de `scripts/`, fenêtre de homelabd, exécutant du lot 3 |
| [incidents.md](incidents.md) | historique daté des incidents et la règle née de chacun |

**Règle d'écriture** : une valeur réglable ne s'écrit que dans `homelab.toml` (ou la source citée au § 4 de CLAUDE.md) ;
la doc cite la clé, et la valeur entre parenthèses seulement à titre indicatif. Un récit daté va dans
[incidents.md](incidents.md) ou dans [CHANGELOG.md](../../CHANGELOG.md), pas dans CLAUDE.md.

## Correspondance : ancien CLAUDE.md → nouvel emplacement

Ancien CLAUDE.md = version de `main` à `bb33d63` (1 421 lignes, 153 066 octets). Numéros de ligne de cette version.
« CLAUDE.md § n » = le nouveau CLAUDE.md.

### Sections d'en-tête

| Ancienne section | Nouvel emplacement |
| --- | --- |
| Ce qu'est ce dépôt (l. 5) | CLAUDE.md § 1 (+ carte des docs) |
| Commandes (l. 13) | CLAUDE.md § 2 (+ `/health`, `offpeak.sh`, `bench.sh`) |

### Section « Règles » (l. 28–1110)

| Ancienne puce (ligne) | Nouvel emplacement |
| --- | --- |
| Secrets (30) | CLAUDE.md § 3 « Secrets » ; droits de `backups/` → [sauvegardes.md](sauvegardes.md) ; clé API Jellyfin unique → [SECRETS.md](../SECRETS.md) |
| Images (37) | CLAUDE.md § 3 « Docker » ; diun → [vps-docker-et-systeme.md § 1](vps-docker-et-systeme.md#1-compose-et-images) |
| Pas de `chown -R` (46), qBittorrent.conf (47), Jamais de purge globale (48) | CLAUDE.md § 3 ; [vps-docker-et-systeme.md](vps-docker-et-systeme.md) |
| Une saison entière en une requête (50) | [arrs-et-indexeurs.md § 5](arrs-et-indexeurs.md#5-séries--saisons-packs-et-numérotations) ; plafond corrigé (20 → 60) et cité dans CLAUDE.md § 4 |
| Deux clés C411 (57) | [arrs-et-indexeurs.md § 2](arrs-et-indexeurs.md#2-indexeurs-et-clés-c411) ; CLAUDE.md § 4 |
| Réglages des 4 Arrs (61), Type « anime » (69) | [arrs-et-indexeurs.md § 3](arrs-et-indexeurs.md#3-profils-et-réglages-des-arrs) |
| Tout ce qui est neuf passe par la seedbox (72) | [arrs-et-indexeurs.md § 1](arrs-et-indexeurs.md#1-qui-cherche-qui-télécharge) ; CLAUDE.md § 3 |
| Numéros de profil (80) | [arrs-et-indexeurs.md § 3](arrs-et-indexeurs.md#3-profils-et-réglages-des-arrs) (noms relus par l'API) ; CLAUDE.md § 3-4 ; récit → [incidents.md](incidents.md) 18/09 |
| Couverture partielle (86), Cours récupérés seuls (93), Cours publiés sous leur titre (110), Intégrales (115) | [arrs-et-indexeurs.md § 5](arrs-et-indexeurs.md#5-séries--saisons-packs-et-numérotations) ; récits → [incidents.md](incidents.md) |
| Aucune recherche depuis Sonarr/Radarr (126) | [arrs-et-indexeurs.md § 1](arrs-et-indexeurs.md#1-qui-cherche-qui-télécharge) ; CLAUDE.md § 3 |
| Œuvres dérivées (133) | [arrs-et-indexeurs.md § 4](arrs-et-indexeurs.md#4-choix-dune-release-choose-best_movie_release-recherche) ; récit → incidents 17/09 |
| Voie russe (138) | [voie-russe.md](voie-russe.md) |
| Indexers (178) | [arrs-et-indexeurs.md § 2](arrs-et-indexeurs.md#2-indexeurs-et-clés-c411) — **corrigé** : Prowlarr a 4 indexeurs |
| Le codec source n'est pas un critère de charge (185) | [lecture-et-transcodage.md § 1](lecture-et-transcodage.md#1-ce-qui-limite-vraiment-mesures) — **périmé retiré** : « garder h264 comme départage » (remplacé par x265 d'abord le 26/09) |
| Un seul transcodage 1080p (193) | [lecture-et-transcodage.md § 1](lecture-et-transcodage.md#1-ce-qui-limite-vraiment-mesures) |
| C411 sur deux domaines (200) | [arrs-et-indexeurs.md § 2](arrs-et-indexeurs.md#c411-en-panne-indexeur-en-pause) ; CLAUDE.md § 3 ; récit → incidents 14/09 |
| Recherche des séries = homelabd (204) | [arrs-et-indexeurs.md § 1-2](arrs-et-indexeurs.md#1-qui-cherche-qui-télécharge) — **périmé retiré** : « 25 requêtes/h, 12/h séries, 1/h films » |
| Indexeur en pause (215) | [arrs-et-indexeurs.md § 2](arrs-et-indexeurs.md#c411-en-panne-indexeur-en-pause) |
| Import non demandé par l'Arr (222) | [arrs-et-indexeurs.md § 6](arrs-et-indexeurs.md#6-imports) ; CLAUDE.md § 3 |
| Arrêter un service volontairement (225) | [vps-docker-et-systeme.md § 1](vps-docker-et-systeme.md#1-compose-et-images) ; CLAUDE.md § 3 |
| Alertes admin (228) | [homelabd.md § 4](homelabd.md#4-alertes-admin-et-discord) ; [AUTOMATION.md](../AUTOMATION.md#alertes-admin-2026-10-07) |
| Audit lecture du 18/09 (250) | [lecture-et-transcodage.md § 2-3](lecture-et-transcodage.md#2-fenêtre-des-tâches-lourdes-et-options-interdites), rclone → [seedbox-et-rclone.md § 4](seedbox-et-rclone.md#4-montage-rclone), récit → incidents 18/09 et 20/09 — **corrigé** : tmpfs 4 Go et `mem_limit` 6g (pas 2 Go / 4g), purge chaque minute par unités versionnées (pas « horaire, minuteur transitoire »), port 8096 fermé le 25/09, lien seedbox ~8–10 Mo/s par connexion (les « > 500 Mbit/s » étaient un agrégat) |
| Lecture Jellyfin (293), Aucune option qui lit la vidéo (296), Médias en écriture (299) | [lecture-et-transcodage.md § 2](lecture-et-transcodage.md#2-fenêtre-des-tâches-lourdes-et-options-interdites) ; CLAUDE.md § 3 |
| Clé rclone de la seedbox (304) | [seedbox-et-rclone.md § 4](seedbox-et-rclone.md#4-montage-rclone) |
| Comptes (309) | [comptes-et-abonnements.md § 1](comptes-et-abonnements.md#1-comptes) |
| Langue d'affichage (318), Saut du lecteur (334) | [jellyfin-interface.md § 6](jellyfin-interface.md#6-langue-daffichage-et-saut) |
| Onboarding et mails (343) | [comptes-et-abonnements.md § 2](comptes-et-abonnements.md#2-onboarding-et-mails) |
| Discord (357) | [homelabd.md § 4](homelabd.md#4-alertes-admin-et-discord) |
| Abonnés (369) | [comptes-et-abonnements.md § 3](comptes-et-abonnements.md#3-abonnements) (+ gestion à la main du lot 4 ; PayPal live depuis le 20/09 23:30) |
| Lot « lecture et suivi » v1.19 (399) | langue et sous-titres → [lecture-et-transcodage.md § 7-8](lecture-et-transcodage.md#7-langue-audio-et-sous-titres-des-comptes) ; avancement des demandes → [jellyfin-interface.md § 10](jellyfin-interface.md#10-vue-densemble-des-membres) ; canari → [lecture-et-transcodage.md § 4](lecture-et-transcodage.md#4-diagnostiquer-une-lecture-qui-saccade) |
| Demandes Jellyseerr (438) | [arrs-et-indexeurs.md § 7](arrs-et-indexeurs.md#7-jellyseerr) |
| Tchat des membres (445) | [tchat.md](tchat.md) ; télé → [jellyfin-interface.md § 5](jellyfin-interface.md#5-téléviseurs) |
| Saccades en cours de lecture (488) | [lecture-et-transcodage.md § 4](lecture-et-transcodage.md#4-diagnostiquer-une-lecture-qui-saccade) |
| « Lire sur » (509) | [jellyfin-interface.md § 7](jellyfin-interface.md#7--lire-sur--cast-et-airplay) ; Chromecast, AirPlay, LG → [lecture-et-transcodage.md § 5-6](lecture-et-transcodage.md#5-chromecast) ; blocs NPM → [npm.md](npm.md#chromecast) — **périmé retiré** : sous-titres TV « ×1,4 » (remplacés par `4.8vh` le 05/10) |
| Applis TV natives (595) | [lecture-et-transcodage.md § 6](lecture-et-transcodage.md#6-airplay-télés-lg-applis-tv-natives) |
| EnableEmbeddedTitles (603), Images manquantes (611) | [jellyfin-serveur-et-extensions.md § 3](jellyfin-serveur-et-extensions.md#3-bibliothèques-et-analyses) |
| Adresses des clients (614) | [jellyfin-serveur-et-extensions.md § 2](jellyfin-serveur-et-extensions.md#2-réseau) |
| NPM hôte 1 (618), Faille HSS (621) | [npm.md § 2](npm.md#2-hôte-1-jellyfin--configuration-avancée) |
| SyncPlay bloqué après un saut (635) | [jellyfin-interface.md § 8](jellyfin-interface.md#8-syncplay) |
| Jellyfin 12.1 (657) | instantané, authentification, Collection Sections, Intro Skipper → [jellyfin-serveur-et-extensions.md § 1 et 6](jellyfin-serveur-et-extensions.md#1-version-et-migration-121) ; interfaces, `gc-header.js`, Media Bar 3.0, interface mobile → [jellyfin-interface.md § 4](jellyfin-interface.md#4-nouvelle-interface-121-gardée-et-adaptée) ; migration → [JELLYFIN-12.md](../JELLYFIN-12.md) |
| Jellyfin Enhanced, audit du 03/10 (805) | [jellyfin-serveur-et-extensions.md § 6](jellyfin-serveur-et-extensions.md#6-extensions) |
| Une seule connexion temps réel par page (814) | [jellyfin-interface.md § 9](jellyfin-interface.md#9-une-seule-connexion-temps-réel-par-page) |
| Lot d'extensions du 05/10 (828) | [jellyfin-serveur-et-extensions.md § 6](jellyfin-serveur-et-extensions.md#6-extensions) ; taille des sous-titres → [lecture-et-transcodage.md § 8](lecture-et-transcodage.md#8-sous-titres) |
| Journaux Jellyfin (848) | [jellyfin-serveur-et-extensions.md § 7](jellyfin-serveur-et-extensions.md#7-journaux) |
| Throttling (858) | [lecture-et-transcodage.md § 3](lecture-et-transcodage.md#3-conteneur-jellyfin-et-tmpfs-de-transcodage) |
| Collections après remplacement (864) | [jellyfin-serveur-et-extensions.md § 5](jellyfin-serveur-et-extensions.md#5-collections-après-un-remplacement-de-fichier) |
| Identifications surveillées (869), Titre mal identifié (875) | [jellyfin-serveur-et-extensions.md § 4](jellyfin-serveur-et-extensions.md#4-identification-des-titres) |
| Animés (883) | [jellyfin-serveur-et-extensions.md § 3](jellyfin-serveur-et-extensions.md#3-bibliothèques-et-analyses) — **corrigé** : « 2 à 3 min » faux, une analyse à la fois |
| Codec x265 d'abord (904), Langue des animés (919) | [arrs-et-indexeurs.md § 3-4](arrs-et-indexeurs.md#4-choix-dune-release-choose-best_movie_release-recherche) ; mesures → [lecture-et-transcodage.md § 1](lecture-et-transcodage.md#1-ce-qui-limite-vraiment-mesures) |
| Fansubs (931), Numéro nu (939), Numéro en tête (946) | [arrs-et-indexeurs.md § 5-6](arrs-et-indexeurs.md#5-séries--saisons-packs-et-numérotations) |
| Épisodes sans date / VOF (953) | [arrs-et-indexeurs.md § 3 et 5](arrs-et-indexeurs.md#3-profils-et-réglages-des-arrs) |
| Langue, panne de C411, secours (960) | [arrs-et-indexeurs.md § 2 et 4](arrs-et-indexeurs.md#c411-en-panne-indexeur-en-pause) |
| Titre supprimé puis redemandé (989) | [arrs-et-indexeurs.md § 6](arrs-et-indexeurs.md#6-imports) |
| Suppression d'une demande (996) | [arrs-et-indexeurs.md § 7](arrs-et-indexeurs.md#7-jellyseerr) ; CLAUDE.md § 3 |
| Recherche manuelle (1001) | [arrs-et-indexeurs.md § 8](arrs-et-indexeurs.md#8-recherche-manuelle-recherche) — **périmé retiré** : clé `[manual_search] max_queries_per_hour` (n'existe plus) |
| Dossiers de saison (1008) | [arrs-et-indexeurs.md § 3](arrs-et-indexeurs.md#3-profils-et-réglages-des-arrs) |
| Vue d'ensemble des membres (1012) | [jellyfin-interface.md § 10](jellyfin-interface.md#10-vue-densemble-des-membres) ; [arrs-et-indexeurs.md § 7](arrs-et-indexeurs.md#7-jellyseerr) |
| Historique Arr (1018) | [arrs-et-indexeurs.md § 3](arrs-et-indexeurs.md#3-profils-et-réglages-des-arrs) ; CLAUDE.md § 3 |
| Ports sur 127.0.0.1 (1021) | [vps-docker-et-systeme.md § 1](vps-docker-et-systeme.md#1-compose-et-images) ; CLAUDE.md § 3-4 |
| Pages d'administration (1027) | [homelabd.md § 5](homelabd.md#5-pages-dadministration-crateshomelabdsrcadmin_authrs-client_addrrs) ; [AUTOMATION.md](../AUTOMATION.md#pages-dadministration-2026-09-23-portes-du-2026-10-07) |
| Profils compose (1054) | [vps-docker-et-systeme.md § 2](vps-docker-et-systeme.md#2-vpn-gluetun-et-qbittorrent) ; CLAUDE.md § 3 |
| Journal des versions (1056) | CLAUDE.md § 3 « Git et versions » |
| Nouvelle tâche (1059), Changement de comportement (1062) | [homelabd.md § 1](homelabd.md#1-changer-le-comportement-ou-le-code) ; CLAUDE.md § 3 |
| Recréer gluetun = recréer qbittorrent (1067) | [vps-docker-et-systeme.md § 2](vps-docker-et-systeme.md#2-vpn-gluetun-et-qbittorrent) ; CLAUDE.md § 3 |
| Bases SQLite dans la sauvegarde (1074), Sauvegarde d'état (1095) | [sauvegardes.md](sauvegardes.md) ; [DEPLOY.md](../DEPLOY.md#sauvegarde-et-restauration) |
| Historique de lecture (1079) | [jellyfin-serveur-et-extensions.md § 3](jellyfin-serveur-et-extensions.md#3-bibliothèques-et-analyses) ; CLAUDE.md § 3 |
| Compression NPM (1082), Site par défaut NPM (1088) | [npm.md § 2 et 4](npm.md#4-site-par-défaut) |
| Reboot (1099), Bureau VNC (1101) | [vps-docker-et-systeme.md § 4-5](vps-docker-et-systeme.md#4-redémarrage-et-résilience) |
| `scripts/` (1106) | [outils-bancs-et-hors-pic.md § 3](outils-bancs-et-hors-pic.md#3-scripts-inventaire-au-0810) — **corrigé** : 10 scripts (4 lancés par systemd) + 3 dans `scripts/seedbox/` |

### Section « Interface Jellyfin » (l. 1112–1149)

| Ancienne puce (ligne) | Nouvel emplacement |
| --- | --- |
| Thème (1114), Écran de connexion (1119), Logo (1122), Retour arrière (1125), Accueil (1139) | [jellyfin-interface.md § 1 et 3](jellyfin-interface.md#3-thème-logo-accueil) |
| Extensions d'IAmParadox27 (1127) | [jellyfin-serveur-et-extensions.md § 6](jellyfin-serveur-et-extensions.md#6-extensions) |
| Bibliothèque « Collections » (1130) | [jellyfin-serveur-et-extensions.md § 3](jellyfin-serveur-et-extensions.md#3-bibliothèques-et-analyses) |
| Jellyfin Enhanced, onglets (1132) | [jellyfin-interface.md § 3](jellyfin-interface.md#3-thème-logo-accueil) |
| Après un redémarrage de Jellyfin (1136) | [jellyfin-serveur-et-extensions.md § 1](jellyfin-serveur-et-extensions.md#1-version-et-migration-121) |
| Tester l'interface (1144) | [jellyfin-interface.md § 1](jellyfin-interface.md#1-règles-de-linterface) ; [outils-bancs-et-hors-pic.md](outils-bancs-et-hors-pic.md) ; CLAUDE.md § 3 |

### Section « Seedbox » (l. 1151–1231)

| Ancienne puce (ligne) | Nouvel emplacement |
| --- | --- |
| Accès admin (1153) | [seedbox-et-rclone.md § 1](seedbox-et-rclone.md#1-accès-et-applis) — adresse publique retirée (voir `[seedbox]` de homelab.toml) |
| Redémarrage de l'hôte seedbox (1157) | [seedbox-et-rclone.md § 2](seedbox-et-rclone.md#2-redémarrage-de-lhôte-de-la-seedbox) |
| Arrs et Bazarr de la seedbox (1170) | [seedbox-et-rclone.md § 1](seedbox-et-rclone.md#1-accès-et-applis) |
| Résilience (1176) | [vps-docker-et-systeme.md § 4](vps-docker-et-systeme.md#4-redémarrage-et-résilience) ; montage absent → [seedbox-et-rclone.md § 4](seedbox-et-rclone.md#4-montage-rclone) |
| Espace = quota (1183) | [seedbox-et-rclone.md § 3](seedbox-et-rclone.md#3-espace) |
| Montage (1185), Jellyfin et doubles dossiers (1189), Nouvelles demandes (1187) | [seedbox-et-rclone.md § 4-5](seedbox-et-rclone.md#5-jellyfin-et-la-seedbox) |
| Jellyseerr et la synchro des bibliothèques (1191) | [arrs-et-indexeurs.md § 7](arrs-et-indexeurs.md#7-jellyseerr) ; CLAUDE.md § 3 |
| Déménager un titre (1196) | [seedbox-et-rclone.md § 6](seedbox-et-rclone.md#6-déménager-un-titre-du-vps-vers-la-seedbox) — **périmé retiré** : minuteur `move-to-seedbox-offpeak` (n'existe plus) |
| Supprimer des titres (1216), Ménage (1224) | [seedbox-et-rclone.md § 7](seedbox-et-rclone.md#7-supprimer-faire-le-ménage-remplacer) — **périmé retiré** : minuteur `seedbox-cleanup-deferred` (n'existe plus, `deferred.json` vide depuis le 27/09) |

### Section « Pièges connus » (l. 1233–1421)

| Ancienne puce (ligne) | Nouvel emplacement |
| --- | --- |
| Tests (1235) | [outils-bancs-et-hors-pic.md § 1](outils-bancs-et-hors-pic.md#1-principes), [tchat.md § 5](tchat.md#5-tests), [sauvegardes.md](sauvegardes.md) — **périmé retiré** : « une session ouverte par l'API compte dans la limite de 2 appareils » (`MaxActiveSessions` = 0) |
| Fichier fantôme dans rclone (1245) | [seedbox-et-rclone.md § 4](seedbox-et-rclone.md#4-montage-rclone) |
| `GET /Items` sans `UserId` (1254) | [jellyfin-serveur-et-extensions.md § 3](jellyfin-serveur-et-extensions.md#3-bibliothèques-et-analyses) ; CLAUDE.md § 3 |
| `GET /Devices?userId=` (1257) | [comptes-et-abonnements.md § 1](comptes-et-abonnements.md#1-comptes) ; CLAUDE.md § 3 |
| JavaScript Injector ne purge pas (1260) | [jellyfin-serveur-et-extensions.md § 6](jellyfin-serveur-et-extensions.md#6-extensions) |
| Rotation de la clé Jellyseerr (1262) | [arrs-et-indexeurs.md § 7](arrs-et-indexeurs.md#7-jellyseerr) |
| `.env` (1269), hook gluetun (1271) | [vps-docker-et-systeme.md](vps-docker-et-systeme.md) ; CLAUDE.md § 3 |
| `manualimport` de Sonarr (1273) | [arrs-et-indexeurs.md § 6](arrs-et-indexeurs.md#6-imports) |
| Prowlarr sans application (1277) | [arrs-et-indexeurs.md § 2](arrs-et-indexeurs.md#2-indexeurs-et-clés-c411) — **périmé retiré** : « les publics passent par Jackett », « 25 requêtes/heure » |
| Indexeur bloqué par un Arr (1285) | [arrs-et-indexeurs.md § 2](arrs-et-indexeurs.md#c411-en-panne-indexeur-en-pause) |
| Remplacer par une version plus légère (1290), Série en vidéo (1309) | [seedbox-et-rclone.md § 7](seedbox-et-rclone.md#remplacer-un-titre-par-une-version-plus-légère) |
| Jamais de fichier incomplet (1313), `torrent_import` ne remplace jamais (1325) | [arrs-et-indexeurs.md § 6](arrs-et-indexeurs.md#6-imports) ; CLAUDE.md § 3 |
| L'hébergeur arrête les torrents publics (1319) | [seedbox-et-rclone.md § 7](seedbox-et-rclone.md#7-supprimer-faire-le-ménage-remplacer) ; [voie-russe.md](voie-russe.md) |
| `homelabctl` n'écrit jamais l'état (1330) | [homelabd.md § 3](homelabd.md#3-état-cli-et-journal) ; CLAUDE.md § 3 |
| Deux sessions dans le dépôt (1348) | [homelabd.md § 2](homelabd.md#2-construire-et-installer) ; CLAUDE.md § 3 |
| Journaliser une erreur (1357), Clients HTTP (1361), homelabd cloisonné (1367) | [homelabd.md § 1](homelabd.md#1-changer-le-comportement-ou-le-code) ; CLAUDE.md § 3 |
| Bibliothèque supprimée en 10.11 (1369), Comptes « toutes les bibliothèques » (1375) | [jellyfin-serveur-et-extensions.md § 3](jellyfin-serveur-et-extensions.md#3-bibliothèques-et-analyses) |
| Rangée « Mes médias » (1379) | [jellyfin-serveur-et-extensions.md § 6](jellyfin-serveur-et-extensions.md#6-extensions) ; `DELETE /Items` → CLAUDE.md § 3 |
| qBittorrent (1389) | [vps-docker-et-systeme.md § 2](vps-docker-et-systeme.md#2-vpn-gluetun-et-qbittorrent) ; CLAUDE.md § 3 |
| NPM (1398), UI d'onboarding sur l'hôte (1421) | [npm.md § 1](npm.md#1-règles) |
| Compter les ffmpeg (1403) | [lecture-et-transcodage.md § 3](lecture-et-transcodage.md#3-conteneur-jellyfin-et-tmpfs-de-transcodage) |
| Homarr (1405) | [homarr.md](homarr.md) |

### Ajouts de cette refonte (hors ancien CLAUDE.md)

- Notes de la vague 1 du lot 4 (`backups/lot4-20261008/notes-doc-vague1.json`) : abonnés hors PayPal gérés à la main
  ([comptes-et-abonnements.md](comptes-et-abonnements.md)), « Aide et annonces » ([tchat.md](tchat.md)), outillage
  `tools/` ([outils-bancs-et-hors-pic.md](outils-bancs-et-hors-pic.md)), rapport `catalogue_report`
  ([AUTOMATION.md](../AUTOMATION.md)), tâche `original_language` et essai de la préférence audio native
  ([lecture-et-transcodage.md § 7](lecture-et-transcodage.md#7-langue-audio-et-sous-titres-des-comptes)), autobrr
  ([arrs-et-indexeurs.md § 9](arrs-et-indexeurs.md#9-autobrr-seedbox--flux-c411-désactivé-le-0810)).
- Règles durables du lot 3 (`backups/lot3-20261008/NOTES-DOC.txt`) : coreutils uutils (`head -n`/`tail -n`), fenêtre de
  homelabd lue dans l'état, exécutant du lot 3 jusqu'au 12/10, dérive base ↔ fichiers des hôtes NPM 5 et 14, panique
  noyau avant sysctl. Ce qui ne sera vrai qu'après l'exécution du lot 3 (nouvelles versions) n'est pas reporté.
