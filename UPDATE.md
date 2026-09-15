# Journal des versions de Groscailloux

Groscailloux est un serveur de streaming privé pour une petite communauté : Jellyfin pour regarder, Jellyseerr
pour demander, et une chaîne d'acquisition automatique derrière. Ce journal retrace les grandes étapes, de la
première pile Docker à la version 1.0.0 qui clôt la bêta.

Les dates antérieures au dépôt git (avril-août 2026) sont **reconstituées** à partir des configurations et des
notes d'exploitation ; à partir du 10/09/2026, chaque ligne renvoie aux commits.

## En bref

| Version | Date | Thème |
| --- | --- | --- |
| **[1.0.0](#100--15092026--fin-de-la-bêta--une-seule-plateforme-vps--seedbox)** | 12 → 15/09/2026 | **Fin de la bêta : une seule plateforme VPS + seedbox** |
| [0.9.0](#090--10--11092026--la-refonte--homelabd) | 10 → 11/09/2026 | La refonte : l'automatisation réécrite en Rust (homelabd) |
| [0.3.0](#030--août-2026-reconstitué--jellyfin-enrichi) | août 2026 | Jellyfin enrichi par les plugins |
| [0.2.0](#020--fin-mai-2026-reconstitué--stabilisation) | fin mai 2026 | Stabilisation |
| [0.1.0](#010--30042026--06052026--les-fondations) | 30/04 → 06/05/2026 | Les fondations : demander un film depuis Jellyfin, tout arrive seul |

---

## 1.0.0 — 15/09/2026 — Fin de la bêta : une seule plateforme VPS + seedbox

La version 1.0.0 réunit le VPS et la seedbox en **une seule plateforme** : un seul Jellyfin, une seule
bibliothèque, des demandes qui partent seules et des membres qui ont enfin un guide, un tchat et des règles
claires. C'est la fin de la bêta.

### Plateforme unifiée VPS + seedbox (12 → 13/09)

- Les médias de la seedbox sont lus **par le Jellyfin du VPS**, via un montage rclone (cache limité, reprise
  après coupure) ([`882949b`][882949b], [`ae84d87`][ae84d87], [`26b4fad`][26b4fad]).
- Plus de bibliothèques « (Seedbox) » : **Films** et **Séries** ont chacune un dossier VPS et un dossier
  seedbox, invisibles pour les membres.
- Les nouvelles demandes partent vers la seedbox ; aucun titre n'est importé deux fois d'une machine à
  l'autre, ni aucune saison en double ([`27c39c8`][27c39c8]).
- Un nouvel import sur la seedbox apparaît dans Jellyfin en quelques minutes (tâche `seedbox_refresh`).
- Schémas de l'infrastructure dans `docs/INFRA.md` ([`be5b771`][be5b771]).

### Acquisition automatique (12 → 14/09)

- **C411 seul en automatique** sur les quatre Radarr/Sonarr ; les autres indexers en recherche manuelle.
  Profils français : 1080p maximum, VF d'abord, VOSTFR en dernier recours ([`a6760a4`][a6760a4]).
- Imports débloqués automatiquement :
  - titres reconnus seulement par leur identifiant (`id_match_import`) ([`cd245e8`][cd245e8]) ;
  - releases françaises rejetées en « Unknown Series » (`unknown_series_grab`) ([`a8e1493`][a8e1493]) ;
  - torrents ajoutés à la main dans qBittorrent (`torrent_import`) ([`a6760a4`][a6760a4]).
- Catégories anime de C411 et limite d'API documentées ([`04bc345`][04bc345]) ; C411 reconnu sur ses deux
  domaines d'annonce (53 torrents relancés) ([`6b6d071`][6b6d071]).
- **Supprimer un film dans Jellyfin nettoie tout** : fichiers, fiche Radarr/Sonarr, demande Jellyseerr et
  torrent (après son partage minimal) ([`748cd53`][748cd53], [`4d5d18a`][4d5d18a]).

### Lecture fluide (12/09, 15/09)

- **98 % de lecture directe** : tâches lourdes (trickplay, analyses) entre 5 h et 13 h seulement, aucune
  lecture de vidéo à l'ajout d'un titre, priorité CPU à Jellyfin ([`a6760a4`][a6760a4], [`a2a7a62`][a2a7a62]).
- **2 écrans par compte** : une 3e lecture simultanée est arrêtée avec un message à l'écran ; plus de limite
  d'appareils connectés, qui bloquait les iPhone ([`3eb05e9`][3eb05e9]).
- **« Lire sur »** ne propose que ses propres appareils, sur le même Wi-Fi ([`b060a08`][b060a08]).

### Comptes et accès (14 → 15/09)

- **Comptes premium** : un compte non premium est suspendu, jamais supprimé ; plafonds de capacité ; page
  « Comptes » pour activer, suspendre ou supprimer ([`a99b7e2`][a99b7e2], [`597415e`][597415e],
  [`748cd53`][748cd53]).
- **Demandes validées automatiquement**, avec un quota de 10 films et 10 saisons par semaine
  ([`4d5d18a`][4d5d18a]).
- Écran de connexion sans liste de comptes, titre en français ([`b8f239f`][b8f239f]) ; page d'inscription qui
  retient son jeton ([`3b646ad`][3b646ad]).
- Page de soutien facultative, sans aucun lien avec l'accès au service ([`7161370`][7161370],
  [`676ae96`][676ae96]).

### Interface « Groscailloux TV » (14/09)

- Thème ElegantFin épinglé + calque maison bleu, logo Groscailloux TV, bannière avec bande-annonce en fondu
  ([`4b167cb`][4b167cb], [`6b6d071`][6b6d071]).
- Accueil façon Netflix : 16 rangées, collections françaises, rangée **« Tendances cette semaine »** calculée
  chaque jour d'après ce que regardent les membres (`trending`) ([`4b167cb`][4b167cb], [`2e8ccbb`][2e8ccbb]).
- Onglets en français : Découvrir, Demandes, Calendrier ; plugins inutiles retirés.

### Communauté : guide et tchat (15/09)

- **Guide des nouveaux membres**, en ligne (`/guide`) et en PDF, lié depuis le mail de bienvenue
  ([`cccd582`][cccd582]).
- **Tchat intégré à Jellyfin** : bulle en haut à droite, salons Annonces, Entraide et Discussion, conversation
  privée avec l'admin, bannière des annonces, récapitulatifs par mail ([`a9bcf2a`][a9bcf2a]).
- **Messages ciblés** : l'admin écrit en privé à un ou plusieurs membres (« ta période d'essai se termine
  samedi »), avec mail facultatif ([`f9982de`][f9982de]).
- Suppression de message confirmée dans le message lui-même, pour marcher dans les applis mobiles
  ([`5208a40`][5208a40]).

### Sécurité et administration (12 → 15/09)

- Homarr en deux tableaux (public pour les membres, privé pour l'admin) ; outils d'admin derrière
  l'authentification de Nginx Proxy Manager ; page d'état protégée ; qBittorrent plus jamais ouvert sans mot
  de passe ([`4367c22`][4367c22], [`b8ab624`][b8ab624], [`4e1612b`][4e1612b], [`954308c`][954308c]).
- Jellyfin voit enfin l'adresse réelle des clients (proxy de confiance corrigé) ([`b060a08`][b060a08]).
- Rotation de la clé Jellyseerr en une commande ; clé de la seedbox limitée à la lecture et à la suppression
  ([`b8f239f`][b8f239f], [`4d5d18a`][4d5d18a]).
- Mémoire de Homarr relevée après un arrêt ([`5374920`][5374920]).
- Règle de travail : **tout changement se teste dans un environnement de test** (comptes temporaires, instance
  séparée), jamais sur la production.

### Incidents et leçons retenues

- **10 → 12/09** : le retrait de Jackett et FlareSolverr a coupé les indexers publics pendant deux jours.
  Rétablis ([`822946d`][822946d]) ; on vérifie désormais qui utilise un service avant de le retirer.
- **14/09** : un bannissement dans qBittorrent a bloqué l'accès web de tout le monde (NPM vu comme un seul
  client) ; corrigé par la reconnaissance du proxy ([`954308c`][954308c]).
- **15/09** : une suppression d'appareils mal filtrée a déconnecté tous les membres. Plus aucune suppression
  en masse sans vérifier chaque élément.
- **15/09** : un film au titre ambigu mal identifié par Jellyfin restait « en cours » dans Jellyseerr ; la
  correction est documentée.

### Commits

[`822946d`][822946d] [`882949b`][882949b] [`cd245e8`][cd245e8] [`be5b771`][be5b771] [`ae84d87`][ae84d87]
[`a6760a4`][a6760a4] [`a2a7a62`][a2a7a62] [`26b4fad`][26b4fad] [`a8e1493`][a8e1493] [`4e1612b`][4e1612b]
[`4367c22`][4367c22] [`b8ab624`][b8ab624] [`04bc345`][04bc345] [`27c39c8`][27c39c8] [`954308c`][954308c]
[`a99b7e2`][a99b7e2] [`597415e`][597415e] [`5374920`][5374920] [`7161370`][7161370] [`676ae96`][676ae96]
[`748cd53`][748cd53] [`4b167cb`][4b167cb] [`2e8ccbb`][2e8ccbb] [`6b6d071`][6b6d071] [`3b646ad`][3b646ad]
[`4d5d18a`][4d5d18a] [`cccd582`][cccd582] [`b8f239f`][b8f239f] [`a9bcf2a`][a9bcf2a] [`b060a08`][b060a08]
[`f9982de`][f9982de] [`3eb05e9`][3eb05e9] [`5208a40`][5208a40]

---

## 0.9.0 — 10 → 11/09/2026 — La refonte : homelabd

Le serveur tournait avec une dizaine de scripts bash et des réglages faits à la main. Tout est repris dans le
dépôt, proprement.

- **homelabd**, un démon en Rust, remplace tous les scripts ; `homelabctl` le pilote en ligne de commande
  (état, simulation, lancement d'une tâche) ([`36a2506`][36a2506], [`488e66a`][488e66a]).
- **Secrets sortis du code** vers un fichier `.env` ; configuration versionnée dans `homelab.toml`
  ([`e820f61`][e820f61]).
- **Docker durci** : images épinglées par empreinte, contrôles de santé, limites de ressources, alertes de
  nouvelles versions (Diun) ([`2795fce`][2795fce]).
- **Auto-réparation** : la tâche `stack_health` relance tout service arrêté ou malade ; démarrage de
  Guacamole fiabilisé ([`e666986`][e666986]).
- Page d'inscription servie par homelabd ; script d'installation, documentation et intégration continue
  ([`42143c3`][42143c3], [`b513331`][b513331], [`aca132d`][aca132d], [`fa3acc8`][fa3acc8],
  [`35c4829`][35c4829]).

## 0.3.0 — août 2026 (reconstitué) — Jellyfin enrichi

Configurations des plugins datées de mi-août 2026.

- Demandes Jellyseerr directement dans Jellyfin (SeerrFin, onglets Films / Séries / Demandes).
- Bannière vedette (Media Bar), aperçu des épisodes dans le lecteur, notifications de nouveautés (NotifySync).
- Mode cinéma, génériques d'animés, synchronisation avec les listes d'animés.
- « Reprendre » sans doublons, limite de flux, thème Jellyfish.

## 0.2.0 — fin mai 2026 (reconstitué) — Stabilisation

- **Passer l'intro** avec Intro Skipper.
- Dashdot ne consomme plus 200 % de CPU (test de débit en boucle coupé).
- Grand ménage des connexions mortes dans Guacamole.

## 0.1.0 — 30/04/2026 → 06/05/2026 — Les fondations

Tag git [`v0.1.0`][v0.1.0].

- **Pile Docker complète** :
  - streaming et demandes : Jellyfin, Jellyseerr ;
  - acquisition : Sonarr, Radarr, Prowlarr, qBittorrent, pyLoad ;
  - accès : Nginx Proxy Manager, DuckDNS, Homarr ;
  - supervision : Grafana, InfluxDB, Telegraf, Dashdot ;
  - bureau à distance : Guacamole.
- **Un bouton « Demander » dans Jellyfin** (Jellyfin Enhanced + Jellyseerr) : on cherche un film, on le
  demande, **il arrive tout seul** dans la bibliothèque.
- **Ajout automatique** : tout fichier déposé est reconnu, rangé et importé sans intervention (`auto-import`).
- **Création de compte en une fois** : Jellyfin + Jellyseerr + mail de bienvenue, avec sa page web ; les comptes
  créés depuis Jellyseerr sont redirigés vers ce parcours.
- **VPN ProtonVPN** avec redirection de port pour partager correctement (après NordVPN), bascule VPN / direct.
- **Torrents gérés seuls** :
  - torrents bloqués remplacés ;
  - disque protégé quand il sature ;
  - ratio de partage adapté à chaque tracker.
- **Animés et séries** :
  - épisodes tout juste sortis importés sans attendre leur titre (TBA) ;
  - seules les saisons demandées sont suivies.
- **Français d'abord** : profils VF, VOSTFR en dernier recours ; H.264 préféré au HEVC (serveur sans carte
  graphique) ; indexers publics relancés.
- Commits : [`f4bb55f`][f4bb55f] [`994892c`][994892c] [`2cd0f84`][2cd0f84] [`8399207`][8399207]
  [`c9d8fb9`][c9d8fb9].

---

## Numérotation pour la suite

- **1.0.x** : corrections ;
- **1.x.0** : nouveautés ;
- **2.0.0** : changement majeur d'architecture.

Chaque version ajoute sa section en haut de ce fichier, avec ses commits.

<!-- liens des commits -->
[v0.1.0]: https://github.com/HaradasCYB/groscailloux-homelab/tree/v0.1.0
[f4bb55f]: https://github.com/HaradasCYB/groscailloux-homelab/commit/f4bb55f
[994892c]: https://github.com/HaradasCYB/groscailloux-homelab/commit/994892c
[2cd0f84]: https://github.com/HaradasCYB/groscailloux-homelab/commit/2cd0f84
[8399207]: https://github.com/HaradasCYB/groscailloux-homelab/commit/8399207
[c9d8fb9]: https://github.com/HaradasCYB/groscailloux-homelab/commit/c9d8fb9
[e820f61]: https://github.com/HaradasCYB/groscailloux-homelab/commit/e820f61
[2795fce]: https://github.com/HaradasCYB/groscailloux-homelab/commit/2795fce
[36a2506]: https://github.com/HaradasCYB/groscailloux-homelab/commit/36a2506
[42143c3]: https://github.com/HaradasCYB/groscailloux-homelab/commit/42143c3
[b513331]: https://github.com/HaradasCYB/groscailloux-homelab/commit/b513331
[488e66a]: https://github.com/HaradasCYB/groscailloux-homelab/commit/488e66a
[aca132d]: https://github.com/HaradasCYB/groscailloux-homelab/commit/aca132d
[e666986]: https://github.com/HaradasCYB/groscailloux-homelab/commit/e666986
[fa3acc8]: https://github.com/HaradasCYB/groscailloux-homelab/commit/fa3acc8
[35c4829]: https://github.com/HaradasCYB/groscailloux-homelab/commit/35c4829
[822946d]: https://github.com/HaradasCYB/groscailloux-homelab/commit/822946d
[882949b]: https://github.com/HaradasCYB/groscailloux-homelab/commit/882949b
[cd245e8]: https://github.com/HaradasCYB/groscailloux-homelab/commit/cd245e8
[be5b771]: https://github.com/HaradasCYB/groscailloux-homelab/commit/be5b771
[ae84d87]: https://github.com/HaradasCYB/groscailloux-homelab/commit/ae84d87
[a6760a4]: https://github.com/HaradasCYB/groscailloux-homelab/commit/a6760a4
[a2a7a62]: https://github.com/HaradasCYB/groscailloux-homelab/commit/a2a7a62
[26b4fad]: https://github.com/HaradasCYB/groscailloux-homelab/commit/26b4fad
[a8e1493]: https://github.com/HaradasCYB/groscailloux-homelab/commit/a8e1493
[4e1612b]: https://github.com/HaradasCYB/groscailloux-homelab/commit/4e1612b
[4367c22]: https://github.com/HaradasCYB/groscailloux-homelab/commit/4367c22
[b8ab624]: https://github.com/HaradasCYB/groscailloux-homelab/commit/b8ab624
[04bc345]: https://github.com/HaradasCYB/groscailloux-homelab/commit/04bc345
[27c39c8]: https://github.com/HaradasCYB/groscailloux-homelab/commit/27c39c8
[954308c]: https://github.com/HaradasCYB/groscailloux-homelab/commit/954308c
[a99b7e2]: https://github.com/HaradasCYB/groscailloux-homelab/commit/a99b7e2
[597415e]: https://github.com/HaradasCYB/groscailloux-homelab/commit/597415e
[5374920]: https://github.com/HaradasCYB/groscailloux-homelab/commit/5374920
[7161370]: https://github.com/HaradasCYB/groscailloux-homelab/commit/7161370
[676ae96]: https://github.com/HaradasCYB/groscailloux-homelab/commit/676ae96
[748cd53]: https://github.com/HaradasCYB/groscailloux-homelab/commit/748cd53
[4b167cb]: https://github.com/HaradasCYB/groscailloux-homelab/commit/4b167cb
[2e8ccbb]: https://github.com/HaradasCYB/groscailloux-homelab/commit/2e8ccbb
[6b6d071]: https://github.com/HaradasCYB/groscailloux-homelab/commit/6b6d071
[3b646ad]: https://github.com/HaradasCYB/groscailloux-homelab/commit/3b646ad
[4d5d18a]: https://github.com/HaradasCYB/groscailloux-homelab/commit/4d5d18a
[cccd582]: https://github.com/HaradasCYB/groscailloux-homelab/commit/cccd582
[b8f239f]: https://github.com/HaradasCYB/groscailloux-homelab/commit/b8f239f
[a9bcf2a]: https://github.com/HaradasCYB/groscailloux-homelab/commit/a9bcf2a
[b060a08]: https://github.com/HaradasCYB/groscailloux-homelab/commit/b060a08
[f9982de]: https://github.com/HaradasCYB/groscailloux-homelab/commit/f9982de
[3eb05e9]: https://github.com/HaradasCYB/groscailloux-homelab/commit/3eb05e9
[5208a40]: https://github.com/HaradasCYB/groscailloux-homelab/commit/5208a40
