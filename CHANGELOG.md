# Journal des versions de Groscailloux

Groscailloux est un serveur de streaming privé pour une petite communauté : Jellyfin pour regarder, Jellyseerr
pour demander, et une chaîne d'acquisition automatique derrière. Ce journal retrace les grandes étapes, de la
première pile Docker à la version 1.0.0 qui clôt la bêta.

Les dates antérieures au dépôt git (avril-août 2026) sont **reconstituées** à partir des configurations et des
notes d'exploitation ; à partir du 10/09/2026, chaque ligne renvoie aux commits. Depuis le 21/09/2026, les versions
publiées le même jour sont regroupées sous la dernière d'entre elles (les tags git intermédiaires restent).

Le 08/10/2026, ce journal a été remis en ordre : l'ancienne section « 1.19.1 » (treize jours de nouveautés, du 23/09
au 05/10) est découpée en 1.19.1, 1.20.0 et 1.21.0, et l'ancienne « 1.20.0 » (revue Kaizen) est devenue 1.22.0.

## En bref

| Version | Date | Thème |
| --- | --- | --- |
| **[1.24.0](#1240--09102026--seedbox-hors-du-dépôt-mode-vo-natif-préparé-et-releases-publiques)** | 09/10/2026 | **Seedbox hors du dépôt, mode VO natif préparé et releases publiques** |
| [1.23.0](#1230--08102026--abonnés-à-la-main--aide-et-annonces--catalogue-et-langue-dorigine) | 08/10/2026 | Abonnés à la main, « Aide et annonces », catalogue, langue d'origine et outils |
| [1.22.0](#1220--07--08102026--revue-kaizen--alertes-sobriété-et-portes-dadministration) | 07 → 08/10/2026 | Revue Kaizen : alertes, sobriété et portes d'administration |
| [1.21.0](#1210--02--05102026--résilience-jellyfin-121-et-nouvelle-interface) | 02 → 05/10/2026 | Résilience, Jellyfin 12.1 et nouvelle interface |
| [1.20.0](#1200--2509--01102026--codec-x265-voie-russe-chromecast-et-site-de-secours) | 25/09 → 01/10/2026 | Codec x265, voie russe, Chromecast et site de secours |
| [1.19.1](#1191--23--25092026--audit--fiabilité-et-sécurité) | 23 → 25/09/2026 | Audit : fiabilité et sécurité |
| [1.19.0](#1190--21--22092026--suivi-des-demandes-langue-et-canari) | 21 → 22/09/2026 | Suivi des demandes, langue et canari |
| [1.18.0](#1180--20092026--abonnement-automatique--mon-compte--discord-et-bienvenue) | 20/09/2026 | Abonnement automatique, « Mon compte », Discord et bienvenue |
| [1.15.1](#1151--19092026--télé-français-partout-airplay-et--lire-sur-) | 19/09/2026 | Télé, français partout, AirPlay et « Lire sur » |
| [1.14.0](#1140--18092026--animés-complets-tout-par-la-seedbox-lecture-qui-tient-la-nuit) | 18/09/2026 | Animés complets, tout par la seedbox, lecture qui tient la nuit |
| [1.8.0](#180--17092026--recherche-par-identifiant-un-seul-indexeur-menus-anime) | 17/09/2026 | Recherche par identifiant, un seul indexeur, menus Anime |
| [1.1.1](#111--16092026--la-lecture-saide-elle-même-adaptée-aux-téléviseurs) | 16/09/2026 | La lecture s'aide elle-même, adaptée aux téléviseurs |
| [1.0.0](#100--15092026--fin-de-la-bêta--une-seule-plateforme-vps--seedbox) | 12 → 15/09/2026 | Fin de la bêta : une seule plateforme VPS + seedbox |
| [0.9.0](#090--10--11092026--la-refonte--homelabd) | 10 → 11/09/2026 | La refonte : l'automatisation réécrite en Rust (homelabd) |
| [0.3.0](#030--août-2026-reconstitué--jellyfin-enrichi) | août 2026 | Jellyfin enrichi par les plugins |
| [0.2.0](#020--fin-mai-2026-reconstitué--stabilisation) | fin mai 2026 | Stabilisation |
| [0.1.0](#010--30042026--06052026--les-fondations) | 30/04 → 06/05/2026 | Les fondations : demander un film depuis Jellyfin, tout arrive seul |

---

## 1.24.0 — 09/10/2026 — Seedbox hors du dépôt, mode VO natif préparé et releases publiques

Lot 4 de la revue Kaizen, troisième vague (décisions du propriétaire du 08/10 au soir), installée dans la nuit du 08
au 09/10. Rien ne change pour les membres tant que le mode VO « Langue d'origine » reste coupé.

### Pour les membres

- **« Mon compte », mode « Toujours en VO »** : le filet qui relance un titre dans sa langue d'origine ne se déclenche
  plus quand la lecture part déjà dans la bonne langue (un seul démarrage), et lit d'abord la langue d'origine sur la
  fiche Jellyfin (remplie par `original_language`) avant de la demander à TMDB.

### Pour l'administrateur

- **Seedbox et domaines hors du dépôt public** : l'adresse des applis de la seedbox, son compte et son dossier personnel
  ne sont plus écrits dans `homelab.toml` ni dans le code. `homelab.toml` cite `${SEEDBOX_PUBLIC_URL}`,
  `${SEEDBOX_HOME}` et `${SEEDBOX_USER}`, lus dans `.env` par homelabd, homelabctl et les outils Python ; une variable
  manquante fait refuser le démarrage en la nommant, jamais en montrant sa valeur.
  - Le montage seedbox écrit l'hôte SFTP et le compte (`SEEDBOX_SFTP_HOST`, `SEEDBOX_USER`) dans une configuration
    effective générée à chaque démarrage (`/run/homelab-seedbox-mount/rclone.conf`, 600), jamais en option : le cache
    VFS de 120 Go garde son dossier. Le modèle `rclone/rclone.conf` ne contient qu'un hôte réservé qui ne se résout
    pas.
  - Le lien du mail d'activation Premium est construit depuis `ONBOARD_PUBLIC_URL`.
  - Les valeurs restent dans l'historique git d'avant le 09/10 : non réécrit (il faudrait réécrire et repousser en
    force `main` et les 35 tags).
- **Mode VO « Langue d'origine » préparé, coupé par défaut** (`[accounts] vo_native = false`). Une fois activé, le mode
  VO pose la préférence native de Jellyfin 12 « Langue d'origine » au lieu du japonais, ce qui sert aussi les applis
  télé.
  - Un compte vu sur l'appli Android TV / Fire TV ou un autre client hors de `vo_native_clients` (lectures, sessions
    même sans capacités déclarées, appareils enregistrés) reste en japonais : ces applis perdent la piste d'origine
    quand elle n'est pas la piste par défaut (bogue de Jellyfin 12.1).
  - Nouvelle tâche `vo_native_guard` (30 s, inactive tant que `vo_native = false`) ; commandes
    `homelabctl accounts vo-native|vo-classic [--dry-run]`, avec sauvegarde des réglages avant chaque bascule.
  - Nouvelles clés : `[accounts] vo_native`, `vo_native_clients`, `vo_native_ignored_clients`, `vo_native_days`,
    `[tasks.vo_native_guard]`. Documentation : [AUTOMATION.md](docs/AUTOMATION.md) et
    [lecture-et-transcodage.md](docs/runbooks/lecture-et-transcodage.md).
- **Langue d'origine des séries** : `[tasks.original_language] series = true` est programmé le samedi 10/10 à 07:05
  (accord du propriétaire du 08/10) par un minuteur transitoire ; si homelabd ne relit pas sa configuration, il revient
  seul à `false`.
- **Releases publiques** : chaque tag `v1.x.y` publie sur GitHub les binaires musl de homelabd et homelabctl avec
  `SHA256SUMS` ; `setup.sh` vérifie l'empreinte avant d'installer et n'installe rien si elle ne correspond pas. La
  première est la 1.24.0 : les binaires d'avant portaient en dur l'adresse et le compte de la seedbox.

### Outillage

- `scripts/test_seedbox-rclone-conf.sh` (18 contrôles de la configuration rclone générée) ; `tools/lib/hlconf.py`
  développe `${NOM}` comme homelabd ; banc `tools/tests/compte-vo/compte-vo.js` (mode VO de Mon compte, par
  `tools/bench/bench.sh`).
- Commits : [`20fea60`][20fea60] (releases), [`9c9075f`][9c9075f] [`74dd39e`][74dd39e] [`8a22c77`][8a22c77] (seedbox hors du dépôt),
  [`f974848`][f974848] [`d9a0710`][d9a0710] [`2c971b9`][2c971b9] [`b04946b`][b04946b] [`76d0de6`][76d0de6] (mode VO natif) ; fusions [`4b3426f`][4b3426f]
  [`3ee13d6`][3ee13d6].

## 1.23.0 — 08/10/2026 — Abonnés à la main, « Aide et annonces », catalogue et langue d'origine

Lot 4 de la revue Kaizen, première vague : cinq chantiers fusionnés et installés le 08/10 en fin d'après-midi
(contrôles du démarrage sans erreur), plus une intervention sur la seedbox le même jour. La seconde vague ne touche
pas au service : documentation et numérotation des versions.

### Pour les membres

- **« Aide et annonces »** : la bulle du tchat et son panneau changent de nom, comme le guide public (captures
  refaites, plus étirées sur les petites largeurs), la page de premiers pas et les mails de l'administrateur. Les
  salons Entraide et Discussion sont fusionnés en un seul, « Entraide », sans message perdu ; trois onglets (Annonces,
  Entraide, Écrire à l'admin) tiennent toujours sur une ligne, téléphone compris.
  - À l'ouverture, Annonces arrive au début de la dernière annonce ; un compte neuf ne voit plus toutes les vieilles
    annonces comme « non lues » (celles de plus de 14 jours comptent comme lues, clé `[chat] new_member_read_days`).
  - Le focus va à l'onglet actif à l'ouverture et la touche Échap ferme le panneau, comme dans « Mon compte ».
  - Le bandeau d'annonce reste affiché tant que l'annonce n'est pas lue, sur ordinateur et téléphone (il s'effaçait au
    bout de 12 s sur les appareils à deux cœurs) ; sur télé, la touche Retour (Retour arrière sur une télécommande
    sans pointeur) le ferme, ainsi que le panneau ouvert.
  - Moins de requêtes : la bulle ne redemande plus son état à l'ouverture ni après chaque lecture, ne demande rien
    pendant une lecture vidéo et ralentit son rythme quand personne n'est là, sans perdre le temps réel ; un
    modérateur sur la liste des fils voit le badge d'un nouveau message privé en quelques secondes.
  - À savoir : la fusion des salons peut afficher « 1 non lu » une fois sur la bulle, et une page Jellyfin déjà
    ouverte garde l'ancien tchat jusqu'au rechargement (Ctrl+R).
  - Les règles de l'API du tchat sont testées (annonces réservées aux modérateurs, fils privés, suppression, fusion
    des salons, compte neuf).

### Abonnements

- **Abonnés hors PayPal gérés à la main** (décision du 08/10).
  - Sont concernées les fiches qui ne sont ni un essai de l'inscription publique ni liées à un abonnement PayPal :
    actif ou offert posé par l'admin, essai posé par l'admin, import, exempté, à qualifier.
  - Le cycle ne les suspend plus jamais et le membre ne reçoit plus aucun rappel.
  - À l'échéance, l'admin reçoit une seule information (mail et Discord admin) et `/accounts` affiche « À gérer
    (échéance passée) ».
  - Les essais de l'inscription et les abonnements PayPal ne changent pas.
- **Correction** : un essai posé par l'admin (`subs set --status trial`) est géré à la main. Avant, il recevait les
  rappels « t'abonner » mais n'était jamais suspendu.

### Pour l'administrateur

- **Rapport « catalogue jamais regardé » sur la page d'état** : la tâche `catalogue_report` (une fois par jour,
  lecture seule, aucune suppression) relève les titres qu'aucun compte n'a commencés 60 jours après leur arrivée, par
  sorte (film, série, animé), côté (VPS ou seedbox), taille et sorte de demandeur (membre, admin, aucune) ; elle met à
  part l'arriéré des séries déjà commencées et suit la voie russe (épisodes disponibles et lus). Affiché dans une
  section de `/status.html` ; l'administrateur décide. Une série demandée avant sa diffusion compte à partir de sa
  première diffusion, et une fiche sans identifiant TMDB est « inconnu », jamais « aucune demande ».
- **Langue d'origine des fiches** : une nouvelle tâche (`original_language`) remplit chaque matin, quelques fiches à
  la fois, la langue d'origine TMDB des films. Jellyfin peut alors choisir de lui-même la piste en version originale
  dans toutes les applis, télés comprises (rien ne change encore pour les membres : le mode « Toujours en VO » de
  « Mon compte » reste celui d'avant). Elle n'écrit jamais pendant une lecture, ne relit aucune vidéo et note chaque
  écriture pour pouvoir revenir en arrière.
  - Les séries restent à l'arrêt (`series = false`) en attendant la décision de l'admin : Jellyfin réécrirait la
    classification de chacune de leurs saisons et de leurs épisodes.
  - Retour arrière fiable : une fiche remise à vide n'est jamais réécrite le lendemain ; une série dont un épisode a
    sa propre classification n'est pas touchée ; les séries présentes à la fois sur le VPS et sur la seedbox sont
    traitées fiche par fiche.
- **autobrr (seedbox) : flux C411 désactivé** (08/10, intervention sur la seedbox). Son filtre unique poussait chaque
  release du flux (musique, livres et jeux compris) aux deux Arrs de la seedbox : environ 2 000 refus et 440
  avertissements « Unable to parse » par jour, pour 3 acceptations en six jours. Le RSS des Arrs (15 min) reste la
  voie d'entrée.
  - Pourquoi pas deux filtres par catégorie (films, séries) : autobrr 1.86.0, la dernière version, ne lit pas les
    catégories de C411 dans le flux et reçoit une catégorie vide ; un filtre par catégorie paraîtrait actif sans rien
    retenir, et trier sur le nom des releases est impossible (les packs de jeux et de musique portent des résolutions
    et des codecs comme les films).
  - Une troisième clé C411, propre au flux et comptée nulle part (ni dans le budget horaire ni dans Prowlarr), est
    identifiée ; elle n'est plus utilisée.
  - Alternative documentée, non montée : deux flux séparés par catégorie à la source. Retour arrière : réactiver le
    flux, ce qui remet aussitôt le bruit d'origine.

### Outillage

- **Un seul exécutant « hors pic »** : `tools/offpeak/offpeak.sh` remplace les huit scripts recopiés dans `backups/`.
  Il exécute une commande dans un créneau, seulement si personne ne regarde, et envoie un bilan sur le salon Discord
  admin ; sa programmation survit au redémarrage du serveur (unités `homelab-offpeak@`).
  - Deux travaux programmés au même moment (ou rattrapés ensemble au démarrage) ne se bloquent plus jusqu'à la fin de
    leur créneau : ils passent l'un après l'autre.
  - Un travail lancé en root (`sudo`) ne bloque plus les suivants : l'outil tourne toujours en `deploy`, seule la
    commande passe en root.
  - `homelabctl install` n'active plus aucune unité modèle (`@`) : aucun minuteur versionné n'agit seul à
    l'installation.
- **Un seul lanceur de bancs d'interface** : `tools/bench/bench.sh` remplace les lanceurs dispersés.
  - Les comptes de test `zz_` sont toujours supprimés, même si le banc est interrompu pendant leur création.
  - Les bancs sont refusés le soir (19:00–00:00) et pendant une lecture.
  - Le navigateur de banc ne voit plus les secrets du serveur.
- **Bancs de non-régression** : le banc de l'en-tête vérifie que le menu ☰ est libre sur toute sa largeur, sur
  téléphone et sous 900 px ; un banc couvre la voie russe de « Mon compte » (26 contrôles, avec contre-épreuve).

### Documentation et versions

- **Documentation restructurée** : `CLAUDE.md` (135 Ko, dont 12 passages périmés ou contradictoires : le budget C411
  écrit avec cinq valeurs différentes, PayPal décrit en « sandbox » alors que les abonnements tournent en production)
  ne garde plus que les règles courtes et les interdits ; le détail passe dans `docs/runbooks/`.
- **Numérotation remise en ordre** : l'ancienne section « 1.19.1 », qui mêlait treize jours de nouveautés (du 23/09 au
  05/10) dans le désordre, est découpée en 1.19.1, 1.20.0 et 1.21.0, chaque entrée rangée à sa date ; l'ancienne
  « 1.20.0 » (revue Kaizen) devient 1.22.0.
  - La version du programme (`Cargo.toml`, `homelabctl --version`) passe de 2.0.0 (valeur posée par la refonte de
    septembre et jamais incrémentée) à 1.23.0 : elle suit désormais ce journal. Le daemon affiche toujours `git
    describe`, qui repart du dernier tag.
  - Des tags annotés `v1.19.1`, `v1.20.0`, `v1.21.0` et `v1.22.0` marquent le dernier commit de chaque version (aucun
    tag n'avait été posé depuis `v1.19.0`).
- Commits : [`9fc590f`][9fc590f] [`2fc1cd6`][2fc1cd6] (abonnés), [`66984a9`][66984a9] [`4139ace`][4139ace]
  (catalogue), [`1f9c09b`][1f9c09b] [`e2937cf`][e2937cf] (langue d'origine), [`9846c52`][9846c52] [`0a46682`][0a46682]
  [`cc0b762`][cc0b762] [`d92da5e`][d92da5e] [`2f0e45b`][2f0e45b] (tchat), [`3c78191`][3c78191] [`d0f5bf6`][d0f5bf6]
  [`38d7cc2`][38d7cc2] [`0b8d8d9`][0b8d8d9] [`f60bcdb`][f60bcdb] [`02fd974`][02fd974] [`608ed9f`][608ed9f]
  [`798bbd3`][798bbd3] (outillage) ; fusions [`ce9615f`][ce9615f] [`cf7eaae`][cf7eaae] [`dc45ed5`][dc45ed5]
  [`36e06d0`][36e06d0] [`bb33d63`][bb33d63].

## 1.22.0 — 07 → 08/10/2026 — Revue Kaizen : alertes, sobriété et portes d'administration

Revue complète de la plateforme le 07/10. Lot 1 : réglages appliqués le jour même, sans code (sauf la veille des
images). Lot 2 : le moteur d'automatisation (homelabd), installé dans la nuit du 08/10 ; au lendemain, aucune erreur
de tâche ni alerte.

### Pour les membres

- **Abonnement PayPal sans risque de double prélèvement** : un abonnement qui se renouvelle tout seul ne reçoit plus
  « ton abonnement se termine, renouvelle ici » avec un lien de paiement (un clic créait un second abonnement). À la
  place, une seule information sans lien (« ton abonnement se renouvelle le JJ/MM »), et le délai de grâce ne commence
  que 36 h après la date de facturation (PayPal encaisse parfois plus tard). Un prélèvement raté donne un mail qui
  explique comment mettre à jour son moyen de paiement chez PayPal ; l'accès revient tout seul. Un second abonnement
  sur un compte déjà abonné est refusé (message clair, l'admin est prévenu). L'essai de 7 jours n'envoie plus de
  rappel « J-7 » dès l'inscription, et un parrainage ne met plus de date de fin à un accès offert sans limite.
- **Chromecast** : le serveur lui annonce le bon profil vidéo (High au lieu de Baseline) ; à confirmer en séance réelle
  sur les gels.
- **Démarrage et sauts plus rapides sur une connexion lente** : les listes de lecture des flux convertis sont
  compressées (1,4 Mo → 44 Ko pour un film).
- **Séries en cours de diffusion** : un pack de saison n'est plus pris tant que des épisodes suivis restent à
  diffuser (un pack de 51 épisodes avait été pris pour une saison dont un seul était sorti) ; les épisodes sortis
  arrivent un à un.

### Sécurité

- **Pages d'administration** : l'IP de la maison n'ouvre plus de session que vue par le proxy (avant, une requête
  locale avec un en-tête forgé ouvrait une session d'un an) ; pages d'administration invisibles par l'adresse publique
  de la page Premium ; commandes de la ligne de commande fermées depuis Internet ; jeton d'administration absent des
  pages ; noms de torrents et titres échappés sur la page d'état.
- **Page Premium** : elle n'indique plus « déjà abonné » qu'à partir d'un lien reçu par mail ; elle ne permet plus de
  savoir qui paie.
- **Tableau de bord public** : le widget « Demandes récentes », qui montrait les pseudos des demandeurs sans connexion,
  passe sur le tableau d'administration.
- **Proxy** : une adresse inconnue répond 404 tout de suite (fin d'une boucle interne déclenchée par les robots qui
  sondent le serveur, 13 épisodes en dix jours).
- **Ménage** : instance d'essai de Jellyfin supprimée (elle gardait une copie de la base), sauvegardes fermées aux
  autres comptes de la machine, clé d'API Jellyfin inutilisée révoquée ; les adresses qui portent un secret ne vont
  plus dans le journal.

### Alertes et surveillance

- **Une tâche en panne prévient l'admin** (6 échecs de suite et 30 min), une seule fois, puis à son retour ; trois
  alertes de ce type au plus par 10 min quand tout tombe ensemble. La dernière erreur, son heure et le nombre
  d'erreurs par jour restent visibles (`homelabctl status`, page d'état).
- **Preuve de livraison** : chaque alerte laisse une trace (journal et page d'état : livrée par mail, Discord, ou non) ;
  une alerte qui n'est pas partie est retentée au lieu d'être tenue pour signalée.
- **Seuils** : disque du serveur et quota de la seedbox à 85 % (une alerte par franchissement) ; relance de service
  ratée ; certificat TLS (à moins de 21 jours) et chaîne publique vers Jellyfin contrôlés chaque jour ; sauvegarde de
  plus de 8 jours ; veille des images.
- **Veille des images réparée** : elle était aveugle depuis le 18/09 (fichier abîmé lors d'un retrait de service) ;
  nouvelles versions de Jellyfin 12 et qBittorrent 5.2 reconnues, tri des versions corrigé, Glances déclaré sur son
  vrai tag.
  Le fichier est maintenant contrôlé chaque jour.
- **Minuteurs** : la sauvegarde et les trois chiens de garde préviennent l'admin s'ils échouent ; le moteur est relancé
  s'il ne fait plus tourner ses tâches (et plus seulement s'il ne répond plus) ; sa version exacte est affichée.
- **Lectures relancées en boucle** : l'alerte donne le titre au lieu d'un identifiant, une par rafale, sans répétition
  après un redémarrage.
- **Journal système** : 2 Go et 45 jours, rotation quotidienne (l'historique du moteur tombait à 1 à 3 jours).
- **Alertes sans répétition inutile** (08/10, suite de la revue) : certificat, sauvegarde et veille des images ne
  réalertent plus le même défaut à chaque redémarrage du moteur (au plus une fois par 20 h, un défaut différent ou une
  rechute alerte aussitôt) ; un site de téléchargement qui clignote ne refait plus une alerte « en panne » à chaque
  aller-retour (incident clos après 6 h sans panne) ; l'alerte « en panne » n'est tenue pour envoyée que si elle est
  partie ; le plafond de trois alertes d'échecs par 10 min tient aussi quand deux tâches tombent à quelques secondes
  d'écart.

### Sobriété

- **Moteur d'automatisation** : fichier d'état écrit en un appel au lieu d'environ 38 000 (4,2 Go écrits par jour) ;
  les passages sans rien de neuf ne remplissent plus le journal (4 344 lignes → 312 sur la même nuit) ; les
  sous-titres ne relisent plus toute la médiathèque toutes les 5 minutes (une fois par jour, à 5 h) ; le ménage des
  suppressions ne redemande plus les fichiers des 83 séries de la seedbox à chaque passage.
- **Services** : le tableau de bord ouvre une session qBittorrent par minute au lieu de toutes les 5 s ; synchronisation
  des téléchargements de Jellyseerr toutes les 5 min ; file d'attente de qBittorrent coupée sur le serveur (12
  torrents attendaient sans partager) ; synchronisation RSS du serveur arrêtée (il ne télécharge plus depuis le 18/09) ;
  Sonarr et Radarr de la seedbox en journal normal ; sous-titres automatiques sans le fournisseur qui refusait tout ni
  les séries russes.
- **Suites du lot 2** (08/10) : l'état différé part au plus 60 s après (avant : à la mutation suivante, des heures pour
  une tâche rare) ; `homelabctl status` ne dit plus « running » pour une tâche finie (`running? depuis` / `interrompu?`) ;
  le compteur « en relance différée » des sous-titres ne gonfle plus après des extractions ; le canari de lecture et les
  surveillances calmes n'écrivent plus de ligne d'information à chaque passage.

### Robustesse

- **Panne du site de téléchargement** : la pause de son indexeur n'est plus levée pendant la panne (34 arrêts inutiles
  des applis de la seedbox du 30/09 au 02/10), une seule alerte par incident ; copies de base de la seedbox purgées
  chaque jour.
- **Seedbox injoignable** : trois tâches sautent la seedbox au lieu d'échouer (~400 erreurs pour la panne de début
  octobre) ; l'alerte de santé de la seedbox reste la seule.
- **Coupures réseau** : une requête de lecture coupée par le serveur d'en face est rejouée une fois.
- **Abonnés** : le contrôle PayPal quotidien n'applique plus qu'un paiement réellement constaté, signale un
  prélèvement en attente (une fois par jour) et suit les résiliations ; les fiches d'abonné dont le compte disparaît
  ne sont plus retirées en masse (au-delà de 3 d'un coup, rien n'est retiré et l'admin est prévenu).
- **qBittorrent 5.2** : nouveau cookie de session accepté, en prévision de la mise à jour.
- **Packs numérotés « 01. Titre de l'épisode »** (08/10) : une saison d'animé publiée sans nom de série ni numéro de
  saison, que Sonarr ne lit pas du tout, est rangée dans la saison demandée, en dernier recours et seulement si le
  pack entier est sans ambiguïté ; un nom qui contient un autre numéro reste lu par Sonarr.

- Commits : [`9f62bb9`][9f62bb9] (veille des images), [`6cfa550`][6cfa550] [`cd3c901`][cd3c901]
  [`9d2a44e`][9d2a44e] [`f159451`][f159451] (portes d'administration), [`110ee1b`][110ee1b] [`d8cc62a`][d8cc62a]
  (abonnés), [`e9081bb`][e9081bb] [`4400b8b`][4400b8b] [`b582c2a`][b582c2a] (alertes), [`e08f59d`][e08f59d]
  [`e5605f3`][e5605f3] [`68a2a08`][68a2a08] [`37b3a2c`][37b3a2c] (sobriété), [`b6e7a4d`][b6e7a4d] [`b0812d1`][b0812d1]
  [`ae0df9d`][ae0df9d] [`9e76693`][9e76693] [`0c8c619`][0c8c619] (robustesse), [`e8a5d3f`][e8a5d3f] (chaîne des
  erreurs), [`3c98e54`][3c98e54] (alertes, suite), [`aa8bfd0`][aa8bfd0] (sobriété, suite), [`69afbef`][69afbef]
  [`30b3572`][30b3572] (packs numérotés) ; fusions [`39bd51c`][39bd51c] [`1ddf77f`][1ddf77f] [`2febb34`][2febb34]
  [`7d91fc2`][7d91fc2] [`33f720b`][33f720b] [`e8c8556`][e8c8556] [`e4e93b7`][e4e93b7] [`fe81d1e`][fe81d1e] ;
  documentation [`599db47`][599db47] [`ebbfc24`][ebbfc24].

## 1.21.0 — 02 → 05/10/2026 — Résilience, Jellyfin 12.1 et nouvelle interface

Du 02 au 05/10 : une seedbox et un moteur qui se relancent seuls, deux cas de séries que la recherche ne comprenait
pas, puis le passage à Jellyfin 12.1 (en service le 03/10 au soir) et tout ce qu'il a fallu adapter autour :
interface, extensions, SyncPlay, Intro Skipper, sauvegardes. Les entrées vont de la plus récente à la plus ancienne.

- **SyncPlay fiable de nouveau (05/10)** : depuis la mise à jour des extensions du matin, une extension ouvrait une
  seconde connexion en temps réel et le serveur y envoyait parfois les messages de la séance ; l'appli ne savait alors
  pas qu'elle était dans le groupe (impossible de l'arrêter) ou qu'elle l'avait quitté. Corrigé le soir même ; pour en
  profiter, recharger la page (Ctrl+R dans Jellyfin Desktop). Le changement de langue VO et « Lire sur » en profitent aussi.
- **Menu SyncPlay plus clair (05/10)** : chaque groupe tient sur une ligne (participants, nom du groupe, icône
  « rejoindre ») posée sur un fond qui montre ce qui se clique ; toucher n'importe où sur la ligne rejoint le groupe (avant,
  seule la petite icône le faisait). Un double appui ne rejoint qu'une fois. Le groupe où l'on se trouve déjà garde son
  bouton « Quitter » à part, jamais déclenché par erreur.
- **Extensions remises à jour (05/10)** : deux extensions inutilisées retirées (minuterie de mise en veille, galerie
  d'avatars, qui ralentissait l'ouverture de chaque page), deux autres passées sur leur version prévue pour Jellyfin 12
  (scripts maison, notifications), et une seconde source d'images pour les logos manquants : 5 titres en ont gagné un.
  Coupure de 52 secondes un matin sans lecture. Dans Jellyfin Desktop, un Ctrl+R suffit pour en profiter.
- **« Passer l'intro » plus sobre (05/10)** : la mise à jour d'Intro Skipper voulait refaire l'analyse de toute la
  médiathèque (~700 Go relus sur la seedbox en 8 à 9 matinées) alors que la plupart des titres avaient déjà leurs
  boutons. Les segments existants sont conservés tels quels, seuls les génériques douteux seront refaits, et l'analyse des
  nouveautés lit moins (fenêtre plus courte, plus de récapitulatifs ni de génériques de films). Aucun bouton perdu.
- **Sécurité de l'accueil (05/10)** : une faille connue de l'extension des rangées d'accueil (signalée chez son auteur,
  pas encore corrigée) permettait à un compte de lire les rangées d'un autre ou d'en ajouter pour tout le monde. Elle
  est fermée en amont de Jellyfin, sans rien changer à l'accueil des membres.
- **Sauvegardes plus sûres (05/10)** : les bases de données des services (Jellyfin, historique de lecture, Intro Skipper,
  notifications, Jellyseerr, Sonarr/Radarr…) sont copiées proprement avant l'archive hebdomadaire au lieu d'être prises
  en pleine écriture : une restauration repart d'une base cohérente. Historique de lecture gardé sans limite (il
  s'effaçait au-delà de 3 mois) ; scripts et styles de l'interface envoyés compressés (pages plus légères).
- **Télés LG et aide à la qualité (05/10)** : sur télé, la taille des sous-titres choisie dans Mon compte donnait des
  lignes géantes depuis la veille (jusqu'à 141 px) ; elles font maintenant 52, 65 ou 84 px selon le réglage
  (« Grande » par défaut). Les rangées de l'accueil se chargent de nouveau jusqu'en bas sur télé, et le panneau Mon
  compte ne dépasse plus de l'écran. Dans le lecteur, un saut (barre, « Passer l'intro ») ne compte plus comme une
  coupure : l'aide « Ta connexion ne tient pas la qualité maximale » ne s'affiche plus à tort ; sur PC, elle passe en
  haut au lieu de cacher quatre boutons du lecteur.
- **Interface revue pour le téléphone (04/10)** : 28 défauts de la nouvelle interface relevés sur téléphone et
  tablette, tous corrigés et vérifiés sur iPhone, Android, iPad, PC, ancienne interface et TV.
  - **Accueil** : sur PC, les rangées apparaissent de nouveau au premier écran (elles étaient sous le bas de l'écran) et
    ne remontent plus sur la bannière après un défilement. Sur téléphone, la ligne d'infos de la bannière est lisible
    (« 1 saison », âge et genre entiers) et la roue des réglages en anglais a disparu.
  - **Barre du haut** : sur PC, les onglets Accueil… Calendrier montent dans la barre quand il y a la place (une ligne
    de moins) ; sur téléphone, le bouton ☰ (bibliothèques) n'est plus recouvert par d'autres icônes et les onglets
    défilent avec un fondu, l'onglet ouvert restant visible ; la barre devient opaque au défilement ; le logo revient sur tablette et
    dans le menu ☰.
  - **Sous-titres lisibles sur téléphone et télé** : la taille choisie dans Mon compte s'applique enfin (environ 5 px
    sur iPhone auparavant).
  - **Lecteur** : SyncPlay et « Lire sur » ne disparaissent plus derrière un titre long ; « Passer le générique » ne
    gêne plus les commandes ; l'aide à la qualité s'affiche en haut au lieu de couvrir les boutons.
  - **Fiches et bibliothèques** : plus de 140 px de vide sous la barre ; titres des affiches sur deux lignes ; la barre
    A–Z ne mord plus sur les affiches ; noms de pistes audio lisibles ; langues et tailles en français ; plus de
    collections techniques (« Tendances cette semaine ») dans les fiches ; le bloc « Également disponible » montre les
    services français (caché sur téléphone).
  - **Panneaux** : Mon compte tient dans l'écran avec sa croix toujours visible ; le tchat montre ses 4 onglets ; le
    bandeau d'annonce ne recouvre plus le menu ; boutons plus faciles à toucher ; calendrier à l'heure française (17:00).
- **Journaux gardés 14 jours (04/10)** : Jellyfin n'en gardait que 3, malgré le réglage du 03/10 (ce n'était pas le
  bon). Un souci signalé par un membre peut maintenant être retrouvé deux semaines après.
- **Surveillance des lectures relancées en boucle (04/10)** : l'admin est prévenu quand un lecteur fait relancer
  sa conversion plus de 30 fois en une heure pour le même titre (une télé et un Chromecast l'ont fait ces derniers
  jours sans être repérés). L'ancien repère avait disparu avec Jellyfin 12.1.
- **Accueil complet (04/10)** : les rangées Tendances cette semaine, Anime, Les mieux notés et Films français sont de
  retour. Leur extension n'avait pas de version pour Jellyfin 12 : elle a été recompilée en attendant que son auteur
  l'intègre à Home Screen Sections.
- **Première connexion plus rapide (04/10)** : sur un nouvel appareil, la page ne se recharge plus pour passer en
  français (une dizaine de secondes gagnées). Elle ne revient plus non plus, parfois, à l'écran de connexion juste
  après avoir saisi son mot de passe.
- **La nouvelle interface retrouve ses repères (04/10)** :
  - le logo « Groscailloux TV » ;
  - sur l'accueil, les onglets Accueil, Favoris, Découvrir, Demandes et Calendrier ;
  - la cloche des notifications ;
  - des libellés en français (« Favoris », « Rechercher »… restaient en anglais, comme « Play » sur la bannière,
    devenu « Lire »).

  Le titre du film ou de la série à la une ne passe plus sous la barre. Dans « Lire sur », l'iPhone retrouve
  l'entrée AirPlay. Le navigateur Android explique quoi installer pour diffuser, et ailleurs une liste vide dit
  pourquoi.
- **Jellyfin 12.1 (03/10)** : passage à la dernière version de Jellyfin, avec 2 minutes de coupure.
  - **Pour les membres** : SyncPlay corrigé côté serveur, sous-titres d'animés qui gardent leurs styles, pages
    plus rapides, boutons « Tout lire » et « Aléatoire » sur les séries.
  - **Nouvelle interface** : navigateurs, Jellyfin Desktop et téléphones passent d'office sur la nouvelle
    interface de Jellyfin. Les bibliothèques sont dans la barre du haut. L'ancienne interface reste au choix dans
    Réglages → Affichage → « Mode d'affichage » ; les télés ne changent pas.
  - **Accueil** : 4 rangées ont manqué une nuit (Tendances, Anime, Les mieux notés, Films français), revenues le
    04/10 (voir plus haut).
  - **Retour en arrière** possible pendant 7 jours.
- **Tchat et Mon compte dans la nouvelle interface (03/10)** : leurs boutons avaient disparu après le passage à
  Jellyfin 12.1. Ils sont de retour dans la barre du haut, dans toutes les mises en page.
- **SyncPlay sans roue de chargement (03/10)** : dans l'appli Jellyfin Desktop, avancer pendant une séance à plusieurs
  laissait le groupe bloqué ; il fallait faire pause puis lecture. Le groupe repart maintenant seul après un saut.
  Il faut redémarrer l'appli une fois pour en profiter. Le correctif définitif viendra avec Jellyfin 12.1, déjà testé
  sur une instance d'essai.
- **Serveur allégé (03/10)** : la page Téléchargements se rafraîchit toutes les 2 minutes au lieu de 30 secondes. Une
  page laissée ouverte faisait à elle seule près des deux tiers des requêtes du serveur. Journaux plus lisibles. Les mises à jour d'extensions se font désormais à la main, après vérification.
- **Collections (03/10)** : *Les Gardiens de la Galaxie Vol. 3*, remplacé par une version 1080p, est revenu dans
  ses collections (Univers Marvel, Tendances, Les mieux notés).
- **Épisodes numérotés « - 07 » (03/10)** : une série téléchargée dont les fichiers s'appellent « Titre - 07 » au lieu
  de « S01E07 » restait « téléchargée mais pas rangée » (*Angels of Death*, demandée par un membre, rangée à la main
  le jour même). L'import comprend maintenant ces numéros quand la recherche visait une saison précise, avec les
  mêmes garde-fous qu'ailleurs : rien n'est rangé en cas de doute, et aucun épisode déjà présent n'est remplacé.
- **Séries publiées en intégrale (03/10)** : une série disponible seulement en un seul pack de toutes ses saisons
  restait « introuvable ». La recherche automatique demandait les saisons une par une et ne voyait jamais ce pack
  (*Space Dandy*, demandé par un membre, trouvé et ajouté à la main le jour même). Elle le trouve maintenant toute
  seule, regarde ce qu'il contient avant de le prendre, et ne télécharge que les épisodes qui manquent.
- **Tableau de bord (03/10)** : nouvelle tuile « Lien d'inscription » sur la page de supervision, pour retrouver
  d'un clic le lien à envoyer aux futurs membres. La tuile « Procédures » mène de nouveau à la bonne page, et la tuile
  d'un ancien service retiré a disparu.
- **Résilience (02/10)** : si la seedbox redémarre ou qu'une de ses applis s'arrête, elle se relance toute seule en
  quelques minutes, et l'admin est prévenu si une panne dure plus de 10 minutes. Côté serveur, le moteur
  d'automatisation est surveillé et relancé s'il se fige, et le téléchargement direct du serveur se répare seul après
  une coupure du VPN.
- Commits : [`02948e0`][02948e0] (résilience), [`e287e2f`][e287e2f] [`ba2d7f5`][ba2d7f5] (séries : intégrales et
  numéros nus), [`443545e`][443545e] [`b0e3548`][b0e3548] [`3decb9c`][3decb9c] [`2fd0099`][2fd0099]
  [`a62ede2`][a62ede2] [`82c1f32`][82c1f32] [`f3e3183`][f3e3183] (Jellyfin 12.1 et interface), [`eeb0523`][eeb0523]
  (surveillance des relances), [`194ad67`][194ad67] (télés et aide à la qualité), [`073e5e2`][073e5e2] (sauvegardes),
  [`693b748`][693b748] (Intro Skipper et accueil), [`1d24b0e`][1d24b0e] (extensions), [`1a25345`][1a25345]
  [`a0269e8`][a0269e8] (SyncPlay), [`04c861e`][04c861e] (dépôt et journal).

## 1.20.0 — 25/09 → 01/10/2026 — Codec x265, voie russe, Chromecast et site de secours

Du 25/09 (après-midi) au 01/10 : le choix des releases passe au x265 puis à un son que les appareils lisent sans
réencoder, la voie russe ouvre, les Chromecast et les télés LG sont repris un par un, et un site public de secours
prend le relais quand C411 est en panne. Les entrées vont de la plus récente à la plus ancienne.

- **Site de secours (30/09 → 01/10)** : quand notre site de téléchargement principal est en panne, les films demandés sont
  cherchés automatiquement sur un site public de secours, en français. Depuis le 01/10, les séries et les animés aussi
  (pour les animés, d'abord un site spécialisé, en VOSTFR ou en VF seulement).
- **Demandes pendant une panne de C411 (30/09)** : quand le site de téléchargement était en panne, une demande passait
  pour « introuvable » et n'était recherchée à nouveau que le lendemain. La panne est maintenant reconnue : la recherche
  reprend toute seule dès que le site revient.
- **Mon compte à la télécommande (29/09)** : sur les télés, le panneau Mon compte s'ouvrait mais on ne pouvait pas s'y
  déplacer. Les flèches passent maintenant d'un réglage à l'autre, et la langue et la taille des sous-titres se
  choisissent par des boutons.
- **Sous-titres en AirPlay (29/09)** : diffusé en AirPlay depuis un iPhone, iPad ou Mac, un film arrivait sur la télé
  sans ses sous-titres. Ils font maintenant partie du flux envoyé à la télé.
- **Chromecast 1080p (29/09)** : sur les Chromecast non 4K, certains films restaient en chargement infini, et chaque
  avance rapide prenait 10 à 15 secondes. Ces modèles reçoivent maintenant une image en 720p qu'ils savent tous lire :
  reprise environ deux fois plus rapide. Les Chromecast 4K ne changent pas.
- **Fenêtre « Épisode suivant » (29/09)** : quand le titre d'un épisode était un nom de fichier, la fenêtre de fin
  d'épisode poussait ses boutons « Démarrer maintenant » et « Cacher » hors de l'écran. Ils restent maintenant visibles.
- **Télés LG (29/09)** : changer de langue en cours de film (VF → VO) ne faisait rien sur les télés LG ; la télé
  restait sur la piste par défaut. Le changement passe maintenant par le serveur (quelques secondes de chargement,
  sans perte de qualité), et le mode « Toujours en VO » de Mon compte marche aussi sur LG. Sur les télés, les
  sous-titres sont plus grands, avec un contour, et le panneau Mon compte est lisible depuis le canapé.
- **Google Cast bloqué sur « Ready to cast » (27/09)** : depuis une mise à jour de l'appli Chromecast de Jellyfin, la
  télé demandait le son AC3/E-AC3 tel quel, chargeait le premier morceau puis abandonnait. Le serveur lui envoie de
  nouveau un son AAC stéréo, comme avant ; les autres appareils ne changent pas.
- **Deuxième lot de fichiers allégés (27/09)** : 18 titres de plus passés en version optimisée (dont Matrix, Batman
  Begins, Loki, Bet, Blue Box, Stranger Things 1985), environ 120 Go libérés ; 22 titres gardés faute de meilleure
  version disponible.
- **Contrôle complet (27/09)** : *La Cuisine* avait reçu par erreur 48 épisodes incomplets des saisons 4 à 6 (retirés,
  la série est revenue à ses 60 épisodes) ; un fichier incomplet ne peut plus être ajouté à la médiathèque. Collections
  remises à jour après le remplacement des fichiers de la veille. Tout le reste vérifié : services, tâches, lecture,
  espace, et chaque série et film de la seedbox présent dans Jellyfin.
- **Nouveaux titres bloqués à « Ajout à la médiathèque » (27/09)** : un fichier remplacé la veille était resté
  « fantôme » dans le montage de la seedbox et Jellyfin s'y était bloqué pendant des heures ; plus rien de nouveau
  n'apparaissait. Débloqué (les 6 films attendus sont arrivés), et un chien de garde débloque désormais ce cas seul en
  quelques minutes.
- **Contenu russe (26/09)** : un membre peut demander des séries et films russes par une voie dédiée (tracker
  russe, en VO russe) en choisissant le dossier « Russian » dans la fenêtre de demande ; ils arrivent dans deux
  bibliothèques « Séries russes » et « Films russes » visibles de son compte. *La Cuisine* (Кухня) y a été rangée.
  Le choix se fait d'un bouton sur ses propres cartes de demande (« Chercher en russe »), et la recherche se fait par
  le titre russe d'origine : *Интерны* (60 épisodes) et les épisodes manquants de *La Cuisine* ont été trouvés ainsi.
- **Films d'animation et animés visibles tout de suite (26/09)** : un titre rangé dans « Anime » ou « Films
  d'animation » n'apparaissait qu'à l'analyse du lendemain matin (un film attendu 45 min alors qu'il était déjà
  téléchargé). Il est maintenant rangé dans les 5 minutes et visible juste après.
- **VO, correctif (26/09)** : le mode « Toujours en VO » laissait Jellyfin lire la piste marquée par défaut dans le
  fichier, souvent la VF d'un MULTi (un animé partait en VF). La langue choisie passe maintenant avant ; les trois
  comptes déjà en VO ont été corrigés.
- **Moins de place pour la même image (26/09)** : à langue égale, la recherche automatique prend maintenant la
  version **x265** d'un épisode ou d'un film plutôt que la x264, environ 2,5 à 3 fois plus légère, et l'AV1 en dernier
  (moins bien lu par les vieux appareils). Une version française en x264 reste préférée à une x265 sans français.
  À égalité, le son en AAC, E-AC3 ou AC3 passe devant le DTS, que Chromecast et iPhone ne lisent pas : il fallait le
  réencoder pendant la lecture.
- **Arrivée des nouveaux membres (25/09)** : après le choix du mot de passe, une page « Premiers pas » guide pas à pas (quelle
  appli installer sur chaque écran, connexion à la télé par un code Quick Connect, langue, première demande, où poser
  une question), aussi disponible à tout moment et depuis « Mon compte ». Les mails de bienvenue disent quoi
  installer, et le guide commence par « Démarrer en 5 minutes », avec la connexion présentée appareil par appareil.
- **« Toujours en VO » (Mon compte) fonctionne vraiment (25/09)** : les animés démarrent en japonais sur tous les appareils,
  et sur le web, Jellyfin Desktop et l'iPhone, les films et séries basculent sur leur langue d'origine (anglais,
  coréen…) au lancement, sous-titres français toujours affichés. Un film français reste en français. Avant, le
  réglage suivait la piste « par défaut » du fichier, souvent la VF.
- **Films français pas encore sortis en VOD (25/09)** : l'onglet Demandes l'explique (« au cinéma depuis le …, VOD vers
  le … ») au lieu d'une recherche qui tourne dans le vide, et le serveur ne cherche plus avant la sortie probable.
- Commits : [`87bd90e`][87bd90e] [`0992a8d`][0992a8d] [`4a3ceb7`][4a3ceb7] [`5e9f4f3`][5e9f4f3] [`e527ac3`][e527ac3]
  (panne de C411 et site de secours), [`c07ea65`][c07ea65] [`d21e865`][d21e865] [`43c9367`][43c9367]
  [`6c882d7`][6c882d7] (télés, AirPlay et fenêtre « Épisode suivant »), [`e2a246f`][e2a246f] [`84575e9`][84575e9]
  [`8e9c762`][8e9c762] (Chromecast), [`7d99d01`][7d99d01] [`3f52c0d`][3f52c0d] [`5de524f`][5de524f]
  [`5d38f4b`][5d38f4b] [`eda19e5`][eda19e5] [`2c9477d`][2c9477d] (voie russe), [`d4927ae`][d4927ae] (montage seedbox),
  [`847767a`][847767a] (animés visibles tout de suite), [`240de11`][240de11] [`14a4ad7`][14a4ad7] (codec et son),
  [`cb56e3e`][cb56e3e] [`a3f6b56`][a3f6b56] [`6eb4876`][6eb4876] [`0d63999`][0d63999] (mode VO de « Mon compte »),
  [`4ff21bf`][4ff21bf] (premiers pas), [`6677d00`][6677d00] [`5f90e30`][5f90e30] (films français et VOD),
  [`2b6d928`][2b6d928] [`9475b91`][9475b91] [`35d097c`][35d097c] [`21053ec`][21053ec] [`1df4e82`][1df4e82]
  (documentation).

## 1.19.1 — 23 → 25/09/2026 — Audit : fiabilité et sécurité

Audit de fiabilité et de sécurité lancé le 23/09 et clos le 25/09 au matin ; les nouveautés de l'après-midi du 25/09
sont en 1.20.0. Les entrées vont de la plus récente à la plus ancienne.

- **Qualité réduite, seulement le temps d'une lecture (25/09)** : quand l'aide à la lecture a baissé la qualité sur une
  connexion faible, la lecture suivante repart en « Auto » (avant, l'appareil restait bloqué en basse qualité,
  image dégradée même sur une bonne connexion).
- **Pages studio et réseau (25/09)** de l'onglet Découvrir (Netflix, Ghibli…) et « Où le voir ailleurs » : elles
  fonctionnent (il manquait une clé TMDB, ~7 000 erreurs par jour).
- **Serveur (25/09)** : mémoire de secours (swap), vignettes de la barre de lecture plus générées pour les titres de la
  seedbox (la tâche du dimanche ne finissait jamais), fichier d'état plus robuste, commandes d'administration qui
  ne l'écrasent plus.
- **Séries françaises (23/09)** : un épisode que la base TheTVDB n'a pas encore daté (fréquent pour les séries françaises)
  est maintenant cherché lui aussi, dès que sa saison a commencé ; et les releases « VOF » (version originale
  française) sont enfin reconnues comme françaises, au lieu d'être écartées.
- **Pages d'administration (23/09)** : le jeton ne passe plus dans l'adresse des pages Comptes, Recherche, Créer un compte
  et État (il était recopié en clair dans les journaux du proxy à chaque chargement). On se connecte une fois
  (`/connexion`), la session tient un an ; un ancien lien avec jeton ouvre la session puis l'efface de l'adresse.
  Essais limités, jetons renouvelés, anciens journaux nettoyés, liens du tableau Homarr mis à jour. Depuis la
  maison, aucune connexion n'est demandée.
- **Sous-titres, suite (23/09)** : l'extraction a repris ; les 61 sous-titres complets trop lourds extraits avant le
  correctif (Bleach, Blue Box, Erased) ne sont plus choisis d'office.
- **Réseau (23/09)** : les ports d'administration des services (Sonarr, Radarr, Prowlarr, Jellyseerr, qBittorrent, Homarr,
  Grafana…) ne sont plus joignables directement depuis Internet ; tout passe par le proxy, comme avant.
- **Bureau à distance (Guacamole) (23/09)** : après ce redémarrage, le clavier ne répondait presque plus dans le bureau.
  Le service du bureau croyait chaque démarrage raté (il cherchait son fichier de suivi sous un autre nom), s'est
  relancé 144 fois, et chaque essai a laissé une session ouverte : 138 sessions se disputaient le clavier. Les
  sessions en trop sont fermées, le service démarre maintenant du premier coup, et une seule session peut
  s'ouvrir par écran.
- **Sauvegardes (23/09)** : l'archive est relue en entier avant qu'on supprime les anciennes (une compression ratée
  passait pour une réussite), les bases des abonnés et du tchat sont copiées proprement même en service, le mot
  de passe MySQL n'apparaît plus dans la liste des processus, et les vignettes et photos d'acteurs, régénérables,
  ne sont plus sauvegardées : l'archive redescend d'environ 8 Go à 3.
- **Sous-titres extraits (23/09)** : un épisode déjà traité n'est revu qu'au bout de 6 h, et une vidéo sans piste
  extractible une fois par semaine, au lieu de toutes les 5 minutes. Un sous-titre ASS de plus de 8 Mo n'est plus
  choisi par défaut : le SRT l'est, les lecteurs web ne peinent plus.
- **Fiabilité (23/09)** : une tâche qui rencontre une erreur imprévue ne s'arrête plus pour de bon, elle repasse à
  l'intervalle suivant ; un paiement PayPal dont le traitement échoue est rejoué à la relance de PayPal au lieu
  d'attendre le contrôle du lendemain ; une simple coupure réseau sur un film dont l'identifiant contient « 429 »
  ne met plus une clé C411 au repos pour rien ; la file des téléchargements est lue en entier, plus seulement ses
  200 premiers éléments.
- **Noms des tâches (23/09)** : le panneau Automatisation de Homarr et la page d'état n'affichent plus « Tâche » pour les six
  tâches les plus récentes (lectures simultanées, test de lecture, sous-titres extraits, cycle des abonnements,
  contrôle PayPal, lectures qui bouclent). Chaque tâche porte désormais son nom dans son propre code, et une tâche
  sans nom ne compile plus. Leurs résumés sont écrits en français lisible.
- **Maintenance de nuit (23/09)** : Jellyfin était bloqué depuis la veille sur la lecture d'un sous-titre de 43,8 Mo à
  travers le montage seedbox, ce qui empêchait l'analyse de la médiathèque. Redémarrage du serveur programmé hors
  lecture, avec contrôles automatiques au retour ; l'extraction des sous-titres est en pause en attendant son
  correctif.
- Commits : [`edf0a4f`][edf0a4f] [`79cdce4`][79cdce4] [`e760707`][e760707] [`f78c598`][f78c598] (tâches et fiabilité),
  [`373e5a3`][373e5a3] [`1282b76`][1282b76] (sous-titres), [`b6af815`][b6af815] (sauvegardes), [`e5cff85`][e5cff85]
  (bureau à distance), [`4c8d890`][4c8d890] [`8805c19`][8805c19] (réseau), [`74864b9`][74864b9] [`fd0eba1`][fd0eba1]
  (pages d'administration), [`c2e4bbe`][c2e4bbe] (séries françaises), [`05d7510`][05d7510] [`8aba45b`][8aba45b]
  [`3d7698f`][3d7698f] [`7894bd4`][7894bd4] (réglages, état, qualité et port 8096 du 25/09), [`483e2be`][483e2be]
  [`23c70b0`][23c70b0] (documentation).

## 1.19.0 — 21 → 22/09/2026 — Suivi des demandes, langue et canari

- **Où en est ma demande ?** Dans l'onglet Demandes de Jellyfin, chaque demande en cours affiche une barre
  d'avancement et une estimation : recherche (prochaine tentative), téléchargement (pourcentage, temps restant),
  ajout à la médiathèque, disponible.
- **Langue de lecture** : la piste française est choisie d'office quand elle existe, sous-titres français seulement
  quand l'audio n'est pas en français. Dans « Mon compte », chacun peut passer en « Toujours en VO, sous-titres
  français ».
- **Sous-titres automatiques** : Bazarr (seedbox) complète les sous-titres français manquants des nouveautés.
- Côté serveur : un canari de lecture vérifie toutes les 15 minutes qu'un transcodage démarre vraiment et prévient
  l'administrateur avant les membres.
- Journal des versions compacté : les versions publiées le même jour sont regroupées sous la dernière d'entre elles
  (les tags git intermédiaires existent toujours), et une annonce peut être publiée sur le tchat depuis la ligne de
  commande (`homelabctl chat announce`).
- Correctif du jour : la barre d'avancement ignorait tout téléchargement lancé par la plateforme sur la seedbox (le
  chemin normal de toute nouveauté) et affichait « recherche, prochaine tentative dans 7 jours » pendant que le titre
  se téléchargeait (*Black Clover* à 46 %). Elle lit maintenant aussi les téléchargements de qBittorrent. Sur
  téléphone, la barre chevauchait la ligne « membre • date » : elle est passée en dessous, sur toute la largeur.
  L'icône « Mon compte » de l'en-tête restait invisible après un rechargement tant qu'on n'avait pas cliqué dessus.
- **Les sous-titres arrivent tout de suite sur les titres de la seedbox.** Jusqu'ici, à la première lecture d'un épisode
  en VOSTFR, Jellyfin devait extraire les sous-titres en relisant tout le fichier à distance (2 à 12 minutes) : ils
  arrivaient en retard, ou jamais si l'on changeait de piste entre-temps. Ils sont désormais extraits une fois pour
  toutes sur la seedbox, à côté de la vidéo, **dans leur format d'origine** (l'ASS des animés garde ses couleurs et ses
  panneaux ; la piste malentendants reste à part), et Jellyfin les lit instantanément. Les titres récents passent en
  premier, les nouveautés sont traitées à l'arrivée.
- **Diffuser vers la télé depuis un téléphone Android** : le menu « Lire sur » disait seulement « aucun autre
  appareil connecté », ce qui laissait croire à un blocage. Il explique maintenant la vraie raison : le navigateur
  Android ne sait pas diffuser, l'appli Jellyfin du Play Store si.
- **Les films retrouvent leur vrai titre.** Trente films s'affichaient sous le nom du fichier téléchargé
  (« Matrix.Reloaded.2003.MULTi.VFF.1080p… »), parce que Jellyfin préférait le titre inscrit dans le fichier par celui
  qui l'a publié. C'est corrigé pour ceux-là et pour les suivants. Très visible sur une télé, où il n'y a que les
  titres et les affiches à l'écran.
- **Trois collections sans affiche** (Anime, Tendances, Univers Marvel) en ont une, et la rangée « Récemment ajouté »
  ne répète plus les mêmes titres deux fois.
- **Guide des membres à jour** : connexion détaillée appareil par appareil (dont la connexion rapide par code sur
  téléviseur), deux nouvelles étapes « Sous-titres et langues » et « Mon compte », suivi des demandes avec barre
  d'avancement, et mot de passe en libre-service.
- **Taille des sous-titres** : dans « Mon compte », choix Normale / Grande / Très grande pour l'appareil en cours,
  appliqué **aussitôt, même en pleine lecture** (sous-titres SRT ; les sous-titres stylés des animés gardent leur
  taille propre). « Grande » par défaut.

## 1.18.0 — 20/09/2026 — Abonnement automatique, « Mon compte », Discord et bienvenue

Regroupe 1.16.0, 1.17.0, 1.17.1, 1.17.2 et 1.18.0.

### Abonnement et « Mon compte »

- **Abonnement activé tout seul** : sur la page Premium, tu indiques ton nom de compte, tu payes, et ton compte est
  actif dans la seconde ; chaque mensualité le prolonge automatiquement. Tu as payé sans passer par la page ? Un
  formulaire rattache ton abonnement avec son identifiant PayPal.
- **Fin d'abonnement en douceur** : rappels par mail 7 jours et 1 jour avant l'échéance, 3 jours de grâce, puis
  l'accès se met en pause (rien n'est supprimé) et repart dès le paiement.
- **« Mon compte » dans Jellyfin** (icône en haut à droite) : état de l'abonnement et échéance, bouton d'abonnement,
  appareils connectés avec déconnexion, changement de mot de passe, code de parrainage, historique.
- **Essai gratuit de 7 jours** à l'inscription, et **parrainage** : 15 jours offerts au parrain et au filleul au premier
  paiement du filleul.
- Côté administrateur : colonne Abonnement sur la page Comptes (offrir pour N jours ou sans limite, exempter,
  prolonger, suspendre), commande `homelabctl subs`, cycle en mode observation la première semaine.

### Les nouveautés sur Discord

- **Un salon Discord pour les membres** : chaque film ou série qui arrive (avec l'affiche, un seul message par
  téléchargement), les demandes (nouvelle, validée, refusée, disponible — avec le pseudo du demandeur) et les
  annonces de l'administrateur y sont publiés. Plus besoin d'attendre un mail.
- **Un salon privé pour l'administrateur** : alertes de lecture, indexeur bloqué ou débloqué, nouveau compte à
  activer, mail de bienvenue non parti, service relancé, santé des outils d'acquisition, récapitulatif du tchat.
  Réglé par deux webhooks dans `.env` ; `homelabctl discord apply|test|remove` configure ou retire tout.

### Bienvenue comme chez les grands

- **Un vrai mail de bienvenue, qui arrive.** Fini le mail brut avec identifiant et mot de passe en clair (que Gmail
  rangeait en spam) : un mail sobre au nom de Groscailloux, avec **un bouton** vers une page où le membre **choisit
  son mot de passe**. Le lien vaut une heure et ne sert qu'une fois ; expiré, la page en renvoie un nouveau sur
  demande — ce qui sert aussi de « mot de passe oublié ».
- **Une page d'inscription** (`/inscription`) : pseudo + adresse, et le compte est créé en attente de validation ;
  l'administrateur est prévenu, active depuis la page Comptes, et le membre reçoit « ton compte est actif ».
- Aucun mot de passe ne circule plus par mail ni ne s'affiche dans les outils d'admin.

### De la place sur les deux disques, et deux correctifs de lecture

- **Déménagement VPS → seedbox** (`scripts/move-to-seedbox.py`) : 30 titres (≈ 450 Go) copiés sur la seedbox,
  vérifiés fichier par fichier, reconnus par Sonarr/Radarr là-bas, puis seulement retirés du serveur ; Jellyfin les
  affiche depuis la seedbox sans rien perdre (historique compris). Le transfert lancé l'après-midi a gêné la lecture
  d'un membre : il ne tourne plus que le matin (08 h 30 – 12 h 30), sur un seul flux plafonné, et un titre en cours
  de lecture attend.
- **Ménage de la seedbox** (`scripts/seedbox-cleanup.py`) : 62 téléchargements jamais entrés dans la médiathèque
  (ISO, logiciels, musique, journaux, sport…) et 18 entrées de corbeille retirés, ≈ 800 Go libérés ; les torrents
  trop récents pour le tracker sont retirés automatiquement au 7ᵉ jour.
- **Sauvegarde corrigée** : la sauvegarde de nuit embarquait le cache de lecture de la seedbox (149 Go d'archive au
  lieu de 3) et avait rempli le disque à 92 %. Archive supprimée, cache exclu.
- **« Mes médias » en haut de l'accueil**, sur tous les appareils et tous les comptes (sur téléphone, il fallait tout
  faire défiler pour voir la rangée des bibliothèques) ; les deux entrées « Découvrir » fantômes ont disparu.
- **La lecture restait sur « chargement »** pour tout ce qui devait être converti à la volée : l'espace temporaire de
  conversion était plein de restes de la veille. Vidé, purge automatique toutes les minutes (alerte à
  l'administrateur si ça sature), espace doublé à 4 Go.

## 1.15.1 — 19/09/2026 — Télé, français partout, AirPlay et « Lire sur »

Regroupe 1.14.1, 1.14.2, 1.14.3, 1.14.4, 1.15.0 et 1.15.1.

### Une interface pensée pour la télé

Sur un téléviseur (appli LG, Samsung… qui affichent le site), l'interface est désormais différente de celle du PC,
du téléphone et de la tablette — qui, eux, ne changent pas d'un pixel.

- **Accueil allégé** : plus de grand bandeau animé ni de bandes-annonces YouTube (ce qui pesait le plus sur une
  télé) ; les rangées se limitent à l'essentiel — Reprendre, À suivre, Films et Séries ajoutés, Anime, Collections,
  Ma médiathèque. Les rangées Tendances, Genres, Mieux notés, Découvrir, Mes demandes… restent sur les autres appareils.
- **Télécommande** : en-tête fixé en haut de l'écran (il défilait hors de l'écran dès que l'on parcourait les
  rangées) avec seulement Notifications, Rechercher et Profil ; onglets resserrés, focus plus visible.
- **Lecture d'abord** : sur la fiche d'un titre, « Lire » est plus grand et les blocs secondaires (Plus comme ça,
  genres, tags, studios, services de streaming, langues) sont cachés ; la distribution reste.
- **Pas de tchat sur télé** : sans clavier il était inutilisable ; il n'est plus chargé du tout.

### En français sur tous les appareils

- **L'interface est en français même sur un appareil réglé en anglais.** Jellyfin prenait la langue de l'appareil
  tant que le membre n'avait pas choisi la sienne : un PC en anglais affichait « Home », « Favorites », « Ends at
  11:09 PM ». Le français est posé d'office à la première ouverture (la page se recharge une fois). Un membre qui a
  choisi lui-même une autre langue la garde.

### « Lire sur » et AirPlay

- **Le menu « Lire sur » propose de nouveau tous vos appareils connectés**, quel que soit le réseau. Depuis le
  16/09, il n'affichait que ceux partageant l'adresse de votre box ; un iPhone protégé par le Relais privé iCloud
  (ou un VPN) ne la partage pas toujours, et sa propre TV disparaissait. Les appareils des autres membres restent
  invisibles ; quand aucun autre appareil de votre compte n'est connecté, le menu le dit au lieu de rester vide.
- **Sur iPhone, iPad et Mac, ce menu propose AirPlay**, et l'entrée fait tout le travail : elle ferme le menu, lance
  le titre affiché (depuis sa fiche ou depuis le bandeau de l'accueil) et ouvre le sélecteur d'écran d'Apple dès que
  la vidéo est prête ; si une lecture est déjà en cours, le sélecteur s'ouvre directement. La mention « Google Cast
  non pris en charge » (Cast n'existe que dans Chrome et sur Android) disparaît partout où Cast n'est pas disponible.

### Films demandés sous 5 minutes, et correctifs

- **Un film demandé est cherché dans les 5 minutes**, comme une série : Radarr ne cherche plus lui-même et le flux
  RSS ne ramène que les nouveautés, un film ancien (*Matrix*) attendait le rattrapage du lendemain.
- **qBittorrent du VPS de nouveau joignable** : la mise à jour de nuit l'avait laissé attaché à l'ancien conteneur
  VPN (imports côté VPS en pause pendant 11 h). Recréé, port VPN reposé, procédure documentée.
- **Homarr affiche enfin les téléchargements de la seedbox** (le widget ne recevait que 10 torrents, tous déjà
  terminés donc masqués) ; sur la page d'état, une saison notée « sans release » disparaît une fois complétée.

## 1.14.0 — 18/09/2026 — Animés complets, tout par la seedbox, lecture qui tient la nuit

Regroupe 1.9.0, 1.10.0, 1.11.0, 1.12.0, 1.13.0, 1.13.1 et 1.14.0. Journée marquée par un audit des quatre
applications de téléchargement (croisé avec le guide du tracker), un test en conditions réelles d'une demande
d'animé à 50 épisodes (18 minutes entre la demande et la disponibilité) et un audit complet de la chaîne de lecture
(14 jours de mesures).

### Lecture

- **L'avance rapide saute 10 secondes au lieu de 30**, par le bouton du lecteur comme par les flèches du clavier,
  dans les deux sens. Appliqué à tous les comptes et posé d'office sur les nouveaux (recharger l'application si elle
  était déjà ouverte).
- **Ce qui a été regardé la veille reste prêt le lendemain.** Une tâche de fond (les vignettes de défilement)
  tournait six heures chaque matin sans jamais finir et rapatriait plus d'un téraoctet, ce qui vidait la mémoire
  tampon des médias distants. Elle ne tourne plus que le dimanche, et la mémoire tampon passe de 20 à 120 Go.
- **Démarrage et sauts plus vifs sur les médias distants** (blocs de lecture plus petits), **le retour arrière ne
  relance plus l'encodage** avant cinq minutes, le serveur garde trois minutes d'avance quand une connexion faiblit,
  et **deux encodages simultanés tiennent le temps réel** (plafond de cœurs levé).
- **Plafond à l'acquisition** : les prochaines prises restent sous ~15 Mbit/s en 1080p, sans toucher à l'existant ni
  imposer de limite par membre. Alerte automatique quand un lecteur tourne en boucle sur un flux.
- Les changements qui coupent brièvement la lecture s'appliquent automatiquement à 04:30, hors des heures de
  visionnage.

### Animés

- **Pour les animés, la priorité change** : d'abord les versions **MULTi** (piste japonaise et française), puis la
  **VOSTFR**, le doublage seul ensuite. Séries et films ne changent pas : le français reste prioritaire.
- **Les épisodes qui « n'existaient pas » arrivent enfin.** Beaucoup d'animés sont diffusés en plusieurs parties,
  publiées chacune sous son propre nom : une saison restait incomplète pour toujours, sans que rien ne le signale
  (Bleach s'arrêtait à 34 épisodes sur 48). La plateforme reconnaît ces parties et les range au bon endroit, sans
  jamais prendre de risque : le contenu doit combler exactement les épisodes manquants, aucun épisode présent n'est
  remplacé, et au moindre doute rien n'est fait. Bleach est passée à **48 épisodes sur 48**.
- **Les épisodes sans titre définitif s'importent**, la **numérotation japonaise** (« 367 » plutôt que « saison 18,
  épisode 7 ») est comprise (23 séries corrigées, sans toucher aux fichiers), **les sous-titres livrés à côté de la
  vidéo ne sont plus perdus** à l'import, et un nommage de fansub qui bloquait un lot entier est enfin lu (*Erased*,
  disponible en entier).
- **Plus rien ne se perd en silence** : les téléchargements terminés que rien n'a pu ranger, et les saisons
  qu'aucune version ne couvre, apparaissent dans la page d'état avec la raison.

### Acquisition

- **Une saison entière arrive en un seul passage** : tous les épisodes manquants sont pris d'un coup à partir de la
  même recherche (avant : un épisode toutes les deux heures, près d'une journée pour une saison). *BLACK TORCH* est
  passé de 1 à 11 épisodes en quelques minutes.
- **Quatre fois plus de recherches possibles** : deux accès distincts à la source, chacun avec son compteur, une
  part réservée aux recherches lancées à la main, et **plus de coupure quand un accès sature** (tout continue sur
  l'autre). Films : rattrapage toutes les 15 minutes au lieu d'une heure.
- **Le serveur principal ne télécharge plus rien tout seul** : tout ce qui est neuf est récupéré par la seedbox.
  Ses fiches, qui avaient été mises par erreur sur un profil préférant la version japonaise, sont revenues sur le
  profil français (24 séries, 46 films, rien retéléchargé) ; celles qui n'avaient aucune règle de langue suivent
  désormais les mêmes (français d'abord, 1080p maximum).
- **Le format vidéo ne bloque plus une recherche** : les versions HEVC (x265) étaient écartées au profit du H.264.
  Mesuré sur le serveur, les deux demandent le même travail et les appareils des membres lisent le HEVC
  directement ; les écarter revenait à refuser la seule version française disponible.
- **Recherche élargie** quand l'identifiant ne couvre qu'une partie d'une saison (titres de la série essayés en
  plus), **dossiers de saison rétablis** pour les séries créées par une demande, et **redemander un titre supprimé
  fonctionne** : ses fichiers de partage, conservés pour le tracker, sont réutilisés tels quels.
- **Nouveautés repérées plus vite** (flux relu toutes les 15 minutes, rangement 2 minutes après la fin du
  téléchargement) et **corbeille de 14 jours** sur le serveur principal.

## 1.8.0 — 17/09/2026 — Recherche par identifiant, un seul indexeur, menus Anime

Regroupe 1.2.0, 1.3.0, 1.4.0, 1.5.0, 1.6.0, 1.7.0 et 1.8.0. Point de départ : les saisons 2, 3 et 4 d'un animé
demandé ne partaient jamais. Sonarr cherchait la série sous ses noms anglais et japonais alors que l'indexeur la
classe sous son nom français, envoyait des dizaines de requêtes d'un coup jusqu'à se faire bloquer 24 h, et un
échec immobilisait la saison trois jours.

### Recherche et indexeur

- **Les séries se cherchent par identifiant**, plus par leur nom : une demande part dans les dix minutes, avec une
  seule requête par saison, que la série s'appelle *Shingeki no Kyojin*, *Attack on Titan* ou *L'Attaque des Titans*.
  Les noms français et d'origine servent de repli. Films : un film encore manquant est recherché une seconde fois.
- **Fini les « aucun indexeur disponible »** : les applications de téléchargement ne lancent plus de recherche
  elles-mêmes (elles gardent le flux d'annonces), toutes les recherches passent par la plateforme avec un budget
  horaire et une réserve pour les recherches manuelles, et deux clés distinctes séparent le flux des recherches.
- **Le ménage dans les indexeurs** : 34 indexeurs publics inutilisés retirés, il n'en reste qu'un ; les recherches
  ne dépassent plus le délai d'attente, le journal d'erreurs s'est vidé, deux services inutiles arrêtés.
- **Indexeur toujours disponible** : mis en pause par Sonarr ou Radarr, il est remis en service une heure après son
  dernier échec, l'admin prévenu ; une recherche en échec est retentée dans l'heure au lieu du lendemain.
- **Plus de souplesse sur la langue** : sans version française, la version originale est acceptée en dernier
  recours (VF, MULTi, FRENCH, VOSTFR, puis VO). **Choix plus sûr** : versions démesurées écartées (plus de 6 Go par
  épisode, 25 Go par film), une seule source passe derrière une version bien partagée, et une œuvre dérivée
  (mini-épisodes, spéciaux, OVA, parodie…) n'est plus choisie à la place de la série (*Smoking Behind the
  Supermarket with You* avait été pris en mini-épisodes de 12 minutes).
- **Recherche manuelle** (administration) : une page dédiée cherche une saison ou un film par identifiant, chez
  l'indexeur principal et chez un indexeur d'animés, et affiche toutes les releases avec leurs écarts. Un clic lance
  le téléchargement, une seule requête est envoyée.
- **Demande supprimée = titre supprimé** : retirer une demande efface la fiche et ses fichiers, libère le torrent et
  rend le titre à nouveau demandable.

### Menus Anime et Films d'animation

- **Deux nouveaux menus** dans Jellyfin : **Anime** (séries) et **Films d'animation**, pour l'animation japonaise.
  Les autres dessins animés et les séries japonaises en prises de vues réelles restent dans Séries et Films.
- **Rangement automatique** d'après la fiche TMDB de chaque titre (genre Animation et origine japonaise) : un animé
  demandé arrive au bon endroit, ce qui passe à travers est rangé dans la demi-heure, jamais pendant qu'on le
  regarde ; l'admin peut forcer avec un tag. **27 titres déjà présents rangés** sans perte (historique conservé).

### Tout le monde voit tout

- **Demandes et Calendrier pour tout le monde** : chaque membre voit les demandes de tous (avec leur auteur) et le
  calendrier complet des sorties, comme l'administrateur, qui reste le seul à valider ou refuser.

### Identification et imports

- **Les séries ne partent plus sur leur spin-off** : Jellyfin confondait *The Walking Dead* avec *Dead City*,
  *L'Attaque des Titans* avec un dérivé, *Tomb Raider* avec *Lara Croft*. Chaque fiche est comparée à la référence
  des applications de téléchargement et corrigée seule ; les nouveaux dossiers portent l'identifiant du titre.
- **Imports plus sûrs** : un import ne remplace plus jamais un épisode déjà présent, le rattachement suit celui de
  Sonarr, un torrent au titre japonais ou anglais retrouve la fiche existante, et une série tout juste téléchargée
  s'affiche complète (le dossier est relu en entier avant de prévenir Jellyfin).

## 1.1.1 — 16/09/2026 — La lecture s'aide elle-même, adaptée aux téléviseurs

Regroupe 1.1.0 et 1.1.1. Un membre a vu sa série saccader alors que le serveur ne peinait pas : sa connexion était
descendue sous le débit du film. Jellyfin ne choisit sa qualité qu'au démarrage et relance le flux en boucle quand
le tampon se vide.

- **Bandeau d'aide dans le lecteur** : quand l'image se fige trois fois en trois minutes, un bandeau propose de
  réduire la qualité. Un clic et le film repart au même endroit ; le choix est retenu pour cet appareil. Mesuré sur
  une connexion bridée à 2 Mbit/s : 57 s d'image par minute au lieu de 15.
- **Guide complété** : une section « Ça saccade ? » explique « Auto », les 5 Mbit/s d'une lecture directe et comment
  fixer la qualité à la main. Aucun plafond de débit n'est imposé côté serveur.
- **Téléviseurs** : les bandeaux (annonce, message de l'admin, aide à la qualité) se ferment avec la touche
  **Retour** et s'effacent seuls au bout de douze secondes ; le tchat tourne au ralenti sur ces appareils. Mesuré :
  nos scripts coûtent environ deux points de processeur sur l'accueil, rien pendant la lecture.
- Administration : diagnostic reconstitué à partir des journaux du proxy, de ffmpeg et du cache rclone ; métrique
  réseau de Telegraf corrigée (elle mesurait son propre conteneur) ; banc d'essai reproductible sans écriture sur la
  production.

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

- **1.x.y** (y > 0) : corrections ;
- **1.x.0** : nouveautés ;
- **2.0.0** : changement majeur d'architecture.

Chaque version ajoute sa section en haut de ce fichier, avec ses commits. La version du programme
(`workspace.package.version` de `Cargo.toml`, affichée par `homelabctl --version`) est celle de la section du haut ;
un tag annoté `vX.Y.Z` marque le dernier commit de chaque version.

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
[9f62bb9]: https://github.com/HaradasCYB/groscailloux-homelab/commit/9f62bb9
[6cfa550]: https://github.com/HaradasCYB/groscailloux-homelab/commit/6cfa550
[cd3c901]: https://github.com/HaradasCYB/groscailloux-homelab/commit/cd3c901
[9d2a44e]: https://github.com/HaradasCYB/groscailloux-homelab/commit/9d2a44e
[f159451]: https://github.com/HaradasCYB/groscailloux-homelab/commit/f159451
[110ee1b]: https://github.com/HaradasCYB/groscailloux-homelab/commit/110ee1b
[d8cc62a]: https://github.com/HaradasCYB/groscailloux-homelab/commit/d8cc62a
[e9081bb]: https://github.com/HaradasCYB/groscailloux-homelab/commit/e9081bb
[4400b8b]: https://github.com/HaradasCYB/groscailloux-homelab/commit/4400b8b
[b582c2a]: https://github.com/HaradasCYB/groscailloux-homelab/commit/b582c2a
[e08f59d]: https://github.com/HaradasCYB/groscailloux-homelab/commit/e08f59d
[e5605f3]: https://github.com/HaradasCYB/groscailloux-homelab/commit/e5605f3
[68a2a08]: https://github.com/HaradasCYB/groscailloux-homelab/commit/68a2a08
[37b3a2c]: https://github.com/HaradasCYB/groscailloux-homelab/commit/37b3a2c
[b6e7a4d]: https://github.com/HaradasCYB/groscailloux-homelab/commit/b6e7a4d
[b0812d1]: https://github.com/HaradasCYB/groscailloux-homelab/commit/b0812d1
[ae0df9d]: https://github.com/HaradasCYB/groscailloux-homelab/commit/ae0df9d
[9e76693]: https://github.com/HaradasCYB/groscailloux-homelab/commit/9e76693
[0c8c619]: https://github.com/HaradasCYB/groscailloux-homelab/commit/0c8c619
[e8a5d3f]: https://github.com/HaradasCYB/groscailloux-homelab/commit/e8a5d3f
[39bd51c]: https://github.com/HaradasCYB/groscailloux-homelab/commit/39bd51c
[1ddf77f]: https://github.com/HaradasCYB/groscailloux-homelab/commit/1ddf77f
[2febb34]: https://github.com/HaradasCYB/groscailloux-homelab/commit/2febb34
[7d91fc2]: https://github.com/HaradasCYB/groscailloux-homelab/commit/7d91fc2
[33f720b]: https://github.com/HaradasCYB/groscailloux-homelab/commit/33f720b
[3c98e54]: https://github.com/HaradasCYB/groscailloux-homelab/commit/3c98e54
[aa8bfd0]: https://github.com/HaradasCYB/groscailloux-homelab/commit/aa8bfd0
[69afbef]: https://github.com/HaradasCYB/groscailloux-homelab/commit/69afbef
[30b3572]: https://github.com/HaradasCYB/groscailloux-homelab/commit/30b3572
[e8c8556]: https://github.com/HaradasCYB/groscailloux-homelab/commit/e8c8556
[e4e93b7]: https://github.com/HaradasCYB/groscailloux-homelab/commit/e4e93b7
[fe81d1e]: https://github.com/HaradasCYB/groscailloux-homelab/commit/fe81d1e
[9fc590f]: https://github.com/HaradasCYB/groscailloux-homelab/commit/9fc590f
[2fc1cd6]: https://github.com/HaradasCYB/groscailloux-homelab/commit/2fc1cd6
[66984a9]: https://github.com/HaradasCYB/groscailloux-homelab/commit/66984a9
[4139ace]: https://github.com/HaradasCYB/groscailloux-homelab/commit/4139ace
[1f9c09b]: https://github.com/HaradasCYB/groscailloux-homelab/commit/1f9c09b
[e2937cf]: https://github.com/HaradasCYB/groscailloux-homelab/commit/e2937cf
[9846c52]: https://github.com/HaradasCYB/groscailloux-homelab/commit/9846c52
[0a46682]: https://github.com/HaradasCYB/groscailloux-homelab/commit/0a46682
[cc0b762]: https://github.com/HaradasCYB/groscailloux-homelab/commit/cc0b762
[d92da5e]: https://github.com/HaradasCYB/groscailloux-homelab/commit/d92da5e
[2f0e45b]: https://github.com/HaradasCYB/groscailloux-homelab/commit/2f0e45b
[3c78191]: https://github.com/HaradasCYB/groscailloux-homelab/commit/3c78191
[d0f5bf6]: https://github.com/HaradasCYB/groscailloux-homelab/commit/d0f5bf6
[38d7cc2]: https://github.com/HaradasCYB/groscailloux-homelab/commit/38d7cc2
[0b8d8d9]: https://github.com/HaradasCYB/groscailloux-homelab/commit/0b8d8d9
[f60bcdb]: https://github.com/HaradasCYB/groscailloux-homelab/commit/f60bcdb
[02fd974]: https://github.com/HaradasCYB/groscailloux-homelab/commit/02fd974
[608ed9f]: https://github.com/HaradasCYB/groscailloux-homelab/commit/608ed9f
[798bbd3]: https://github.com/HaradasCYB/groscailloux-homelab/commit/798bbd3
[ce9615f]: https://github.com/HaradasCYB/groscailloux-homelab/commit/ce9615f
[cf7eaae]: https://github.com/HaradasCYB/groscailloux-homelab/commit/cf7eaae
[dc45ed5]: https://github.com/HaradasCYB/groscailloux-homelab/commit/dc45ed5
[36e06d0]: https://github.com/HaradasCYB/groscailloux-homelab/commit/36e06d0
[bb33d63]: https://github.com/HaradasCYB/groscailloux-homelab/commit/bb33d63
[599db47]: https://github.com/HaradasCYB/groscailloux-homelab/commit/599db47
[ebbfc24]: https://github.com/HaradasCYB/groscailloux-homelab/commit/ebbfc24
[02948e0]: https://github.com/HaradasCYB/groscailloux-homelab/commit/02948e0
[e287e2f]: https://github.com/HaradasCYB/groscailloux-homelab/commit/e287e2f
[ba2d7f5]: https://github.com/HaradasCYB/groscailloux-homelab/commit/ba2d7f5
[443545e]: https://github.com/HaradasCYB/groscailloux-homelab/commit/443545e
[b0e3548]: https://github.com/HaradasCYB/groscailloux-homelab/commit/b0e3548
[3decb9c]: https://github.com/HaradasCYB/groscailloux-homelab/commit/3decb9c
[2fd0099]: https://github.com/HaradasCYB/groscailloux-homelab/commit/2fd0099
[a62ede2]: https://github.com/HaradasCYB/groscailloux-homelab/commit/a62ede2
[82c1f32]: https://github.com/HaradasCYB/groscailloux-homelab/commit/82c1f32
[f3e3183]: https://github.com/HaradasCYB/groscailloux-homelab/commit/f3e3183
[eeb0523]: https://github.com/HaradasCYB/groscailloux-homelab/commit/eeb0523
[194ad67]: https://github.com/HaradasCYB/groscailloux-homelab/commit/194ad67
[073e5e2]: https://github.com/HaradasCYB/groscailloux-homelab/commit/073e5e2
[693b748]: https://github.com/HaradasCYB/groscailloux-homelab/commit/693b748
[1d24b0e]: https://github.com/HaradasCYB/groscailloux-homelab/commit/1d24b0e
[1a25345]: https://github.com/HaradasCYB/groscailloux-homelab/commit/1a25345
[a0269e8]: https://github.com/HaradasCYB/groscailloux-homelab/commit/a0269e8
[04c861e]: https://github.com/HaradasCYB/groscailloux-homelab/commit/04c861e
[87bd90e]: https://github.com/HaradasCYB/groscailloux-homelab/commit/87bd90e
[0992a8d]: https://github.com/HaradasCYB/groscailloux-homelab/commit/0992a8d
[4a3ceb7]: https://github.com/HaradasCYB/groscailloux-homelab/commit/4a3ceb7
[5e9f4f3]: https://github.com/HaradasCYB/groscailloux-homelab/commit/5e9f4f3
[e527ac3]: https://github.com/HaradasCYB/groscailloux-homelab/commit/e527ac3
[c07ea65]: https://github.com/HaradasCYB/groscailloux-homelab/commit/c07ea65
[d21e865]: https://github.com/HaradasCYB/groscailloux-homelab/commit/d21e865
[43c9367]: https://github.com/HaradasCYB/groscailloux-homelab/commit/43c9367
[6c882d7]: https://github.com/HaradasCYB/groscailloux-homelab/commit/6c882d7
[e2a246f]: https://github.com/HaradasCYB/groscailloux-homelab/commit/e2a246f
[84575e9]: https://github.com/HaradasCYB/groscailloux-homelab/commit/84575e9
[8e9c762]: https://github.com/HaradasCYB/groscailloux-homelab/commit/8e9c762
[7d99d01]: https://github.com/HaradasCYB/groscailloux-homelab/commit/7d99d01
[3f52c0d]: https://github.com/HaradasCYB/groscailloux-homelab/commit/3f52c0d
[5de524f]: https://github.com/HaradasCYB/groscailloux-homelab/commit/5de524f
[5d38f4b]: https://github.com/HaradasCYB/groscailloux-homelab/commit/5d38f4b
[eda19e5]: https://github.com/HaradasCYB/groscailloux-homelab/commit/eda19e5
[2c9477d]: https://github.com/HaradasCYB/groscailloux-homelab/commit/2c9477d
[d4927ae]: https://github.com/HaradasCYB/groscailloux-homelab/commit/d4927ae
[847767a]: https://github.com/HaradasCYB/groscailloux-homelab/commit/847767a
[240de11]: https://github.com/HaradasCYB/groscailloux-homelab/commit/240de11
[14a4ad7]: https://github.com/HaradasCYB/groscailloux-homelab/commit/14a4ad7
[cb56e3e]: https://github.com/HaradasCYB/groscailloux-homelab/commit/cb56e3e
[a3f6b56]: https://github.com/HaradasCYB/groscailloux-homelab/commit/a3f6b56
[6eb4876]: https://github.com/HaradasCYB/groscailloux-homelab/commit/6eb4876
[0d63999]: https://github.com/HaradasCYB/groscailloux-homelab/commit/0d63999
[4ff21bf]: https://github.com/HaradasCYB/groscailloux-homelab/commit/4ff21bf
[6677d00]: https://github.com/HaradasCYB/groscailloux-homelab/commit/6677d00
[5f90e30]: https://github.com/HaradasCYB/groscailloux-homelab/commit/5f90e30
[2b6d928]: https://github.com/HaradasCYB/groscailloux-homelab/commit/2b6d928
[9475b91]: https://github.com/HaradasCYB/groscailloux-homelab/commit/9475b91
[35d097c]: https://github.com/HaradasCYB/groscailloux-homelab/commit/35d097c
[21053ec]: https://github.com/HaradasCYB/groscailloux-homelab/commit/21053ec
[1df4e82]: https://github.com/HaradasCYB/groscailloux-homelab/commit/1df4e82
[edf0a4f]: https://github.com/HaradasCYB/groscailloux-homelab/commit/edf0a4f
[79cdce4]: https://github.com/HaradasCYB/groscailloux-homelab/commit/79cdce4
[e760707]: https://github.com/HaradasCYB/groscailloux-homelab/commit/e760707
[f78c598]: https://github.com/HaradasCYB/groscailloux-homelab/commit/f78c598
[373e5a3]: https://github.com/HaradasCYB/groscailloux-homelab/commit/373e5a3
[1282b76]: https://github.com/HaradasCYB/groscailloux-homelab/commit/1282b76
[b6af815]: https://github.com/HaradasCYB/groscailloux-homelab/commit/b6af815
[e5cff85]: https://github.com/HaradasCYB/groscailloux-homelab/commit/e5cff85
[4c8d890]: https://github.com/HaradasCYB/groscailloux-homelab/commit/4c8d890
[8805c19]: https://github.com/HaradasCYB/groscailloux-homelab/commit/8805c19
[74864b9]: https://github.com/HaradasCYB/groscailloux-homelab/commit/74864b9
[fd0eba1]: https://github.com/HaradasCYB/groscailloux-homelab/commit/fd0eba1
[c2e4bbe]: https://github.com/HaradasCYB/groscailloux-homelab/commit/c2e4bbe
[05d7510]: https://github.com/HaradasCYB/groscailloux-homelab/commit/05d7510
[8aba45b]: https://github.com/HaradasCYB/groscailloux-homelab/commit/8aba45b
[3d7698f]: https://github.com/HaradasCYB/groscailloux-homelab/commit/3d7698f
[7894bd4]: https://github.com/HaradasCYB/groscailloux-homelab/commit/7894bd4
[483e2be]: https://github.com/HaradasCYB/groscailloux-homelab/commit/483e2be
[23c70b0]: https://github.com/HaradasCYB/groscailloux-homelab/commit/23c70b0
[20fea60]: https://github.com/HaradasCYB/groscailloux-homelab/commit/20fea60
[9c9075f]: https://github.com/HaradasCYB/groscailloux-homelab/commit/9c9075f
[74dd39e]: https://github.com/HaradasCYB/groscailloux-homelab/commit/74dd39e
[8a22c77]: https://github.com/HaradasCYB/groscailloux-homelab/commit/8a22c77
[f974848]: https://github.com/HaradasCYB/groscailloux-homelab/commit/f974848
[d9a0710]: https://github.com/HaradasCYB/groscailloux-homelab/commit/d9a0710
[2c971b9]: https://github.com/HaradasCYB/groscailloux-homelab/commit/2c971b9
[b04946b]: https://github.com/HaradasCYB/groscailloux-homelab/commit/b04946b
[76d0de6]: https://github.com/HaradasCYB/groscailloux-homelab/commit/76d0de6
[4b3426f]: https://github.com/HaradasCYB/groscailloux-homelab/commit/4b3426f
[3ee13d6]: https://github.com/HaradasCYB/groscailloux-homelab/commit/3ee13d6
